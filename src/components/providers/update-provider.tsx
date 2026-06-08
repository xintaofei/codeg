"use client"

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  type AppUpdateState,
  confirmRollbackVersion,
  getAppUpdateState,
  getRunningServerVersion,
  normalizeAppUpdateError,
  readServerVersionStrict,
  restartApp,
  rollbackServer,
  startAppUpdate,
  subscribeAppUpdateState,
  usesTauriUpdater,
  waitForServerHealthy,
} from "@/lib/updater"
import { getTransport } from "@/lib/transport"

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

const IDLE_STATE: AppUpdateState = { seq: 0, status: "idle" }

const LIFECYCLES = new Set([
  "idle",
  "downloading",
  "installing",
  "ready_to_restart",
  "restarting",
  "error",
])

/** Reject payloads that aren't a well-formed AppUpdateState — e.g. an older
 * remote server whose `perform_app_update` still returns the legacy
 * `{ version, needRestart, ... }` shape with no seq/status. Without this an
 * undefined `seq` would poison the monotonic guard. */
function isAppUpdateState(x: unknown): x is AppUpdateState {
  if (!x || typeof x !== "object") return false
  const c = x as Record<string, unknown>
  return typeof c.seq === "number" && LIFECYCLES.has(c.status as string)
}

export interface UpdateContextValue {
  /** Backend-owned lifecycle. The single source of truth, re-synced on mount
   * so it survives navigation and reloads. */
  state: AppUpdateState
  /** A download/install is actively in progress. */
  isUpdating: boolean
  /** Seconds remaining on the post-restart countdown (server mode), or null. */
  restartCountdown: number | null
  /** A rollback is being applied. */
  isRollingBack: boolean
  /** A relaunch has been requested and is in progress. */
  isRestarting: boolean
  /** True once the first authoritative state has landed. Before this, `state`
   * is the default `idle` placeholder and must not be treated as a real
   * backend status. */
  hydrated: boolean
  /** Any action (update / restart / rollback) is in flight — for disabling
   * controls without each consumer re-deriving it. */
  isBusy: boolean
  /** Begin (or attach to) a background download+install of the available
   * update. Progress arrives via {@link state}. */
  startUpdate: () => Promise<void>
  /** Relaunch into the staged update. Call when `state.status` is
   * `ready_to_restart`. Desktop relaunches the app; server drives the
   * countdown + health-poll + reload. */
  restart: () => Promise<void>
  /** Revert to the previously-installed server bundle (server mode only). */
  rollback: () => Promise<void>
}

const UpdateContext = createContext<UpdateContextValue | null>(null)

/** Drive the visible "restarting in N…" countdown over the relaunch delay, then
 * resolve so the caller can start polling /health. */
function countdown(
  delayMs: number,
  onTick: (seconds: number) => void
): Promise<void> {
  const totalWaitMs = delayMs + 1000
  const start = Date.now()
  return new Promise<void>((resolve) => {
    const tick = () => {
      const remaining = Math.max(0, totalWaitMs - (Date.now() - start))
      onTick(Math.ceil(remaining / 1000))
      if (remaining <= 0) resolve()
      else setTimeout(tick, 250)
    }
    tick()
  })
}

export function UpdateProvider({ children }: { children: React.ReactNode }) {
  const t = useTranslations("SystemSettings")
  const [state, setState] = useState<AppUpdateState>(IDLE_STATE)
  const [restartCountdown, setRestartCountdown] = useState<number | null>(null)
  const [isRollingBack, setIsRollingBack] = useState(false)
  // True from the moment a relaunch is requested until it completes (or the app
  // is gone). Covers the desktop window between the click and the backend's
  // `restarting` event — where neither restartCountdown nor status would yet
  // mark us busy — so a second click can't re-trigger the relaunch.
  const [isRestarting, setIsRestarting] = useState(false)
  // False until the first authoritative state (snapshot or event) lands. Until
  // then `state` is the default `idle` placeholder, which consumers must not
  // treat as a real "idle" backend status (e.g. offering rollback).
  const [hydrated, setHydrated] = useState(false)

  // Mirror state into a ref so the action callbacks read the latest snapshot
  // without being re-created on every transition.
  const stateRef = useRef(state)
  useEffect(() => {
    stateRef.current = state
  }, [state])

  // Highest seq applied. Guards against a late snapshot clobbering a fresher
  // live event (or vice-versa). seq is per-process and resets to 0 in a new
  // process; on a reconnect the high-water is reset so the new process's
  // snapshot is accepted (see `resync`). The effective transport is fixed for
  // this provider's lifetime: a window is born remote (URL `remoteConnectionId`,
  // preserved across settings navigation) or local and stays so; switching to a
  // different backend goes through `RemoteConnectionGate`'s loading state, which
  // unmounts and remounts this provider. So arming once at mount is correct.
  const latestSeqRef = useRef(0)
  // Bumped on every reconnect reset, so a snapshot fetch started before the
  // reset cannot apply after it.
  const resyncEpochRef = useRef(0)
  const applyState = useCallback((next: unknown) => {
    if (!isAppUpdateState(next)) return
    if (next.seq < latestSeqRef.current) return
    latestSeqRef.current = next.seq
    setState(next)
    setHydrated(true)
  }, [])

  // Arm the subscription BEFORE fetching the snapshot so no transition is
  // missed in the gap. Re-sync on transport reconnect.
  useEffect(() => {
    let cancelled = false
    let unsub: (() => void) | null = null

    const resync = async (resetForReconnect = false) => {
      // A reconnect may mean the backend process restarted (server self-update,
      // crash, supervisor relaunch) with its seq back at 0. Reset the
      // high-water and bump the epoch so the authoritative post-reset snapshot
      // is accepted, and a fetch started before the reset is discarded when it
      // resolves.
      if (resetForReconnect) {
        resyncEpochRef.current += 1
        latestSeqRef.current = 0
      }
      const epoch = resyncEpochRef.current
      try {
        const snap = await getAppUpdateState()
        // Discard if we unmounted, or a newer reset superseded this fetch while
        // it was in flight.
        if (cancelled || epoch !== resyncEpochRef.current) return
        applyState(snap)
      } catch (err) {
        console.error("[Update] snapshot failed:", err)
      }
    }

    const arm = async () => {
      try {
        const u = await subscribeAppUpdateState((s) => {
          if (!cancelled) applyState(s)
        })
        // If we unmounted while subscribing, the cleanup below already ran with
        // a null `unsub` — tear the subscription down here so it doesn't leak.
        if (cancelled) {
          u()
          return
        }
        unsub = u
      } catch (err) {
        // Not fatal: the snapshot below still seeds current state, and a later
        // reconnect re-arms. The login screen (no auth yet) lands here.
        console.error("[Update] subscribe failed:", err)
      }
      if (!cancelled) await resync()
    }

    void arm()
    const offReconnect = getTransport().onReconnect?.(() => {
      void resync(true)
    })

    return () => {
      cancelled = true
      unsub?.()
      offReconnect?.()
    }
  }, [applyState])

  // ─── Actions ────────────────────────────────────────────────────────────

  const startUpdate = useCallback(async () => {
    try {
      const snap = await startAppUpdate()
      applyState(snap)
    } catch (err) {
      // The detached backend task reports its own failures via the state
      // event; this only fires if the kickoff call itself failed (e.g. the
      // server is unreachable).
      const { rawMessage } = normalizeAppUpdateError(err)
      toast.error(t("installFailed", { message: rawMessage }))
      console.error("[Update] start failed:", err)
    }
  }, [applyState, t])

  const restart = useCallback(async () => {
    setIsRestarting(true)
    // Desktop relaunches the whole app — nothing to verify, the new process
    // boots into the updated build.
    if (usesTauriUpdater()) {
      try {
        await restartApp()
        // Success: the app is relaunching; stay busy until it does.
      } catch (err) {
        setIsRestarting(false)
        const { rawMessage } = normalizeAppUpdateError(err)
        toast.error(t("installFailed", { message: rawMessage }))
        console.error("[Update] restart failed:", err)
      }
      return
    }

    // Server / remote: relaunch, then confirm the new version actually came up
    // (and wasn't auto-rolled-back) before declaring success. Ported from the
    // original in-page upgrade flow so the supervisor trial semantics survive.
    setRestartCountdown(0)
    try {
      // The version running now. The server stays on it through swap + staged
      // until it exits, so reading it here (rather than caching it at download
      // start) is reload-safe: a baseline equal to the post-restart version
      // means the supervisor rolled a failed boot back.
      let baseline: string | null = null
      let reachable = false
      for (let i = 0; i < 3; i++) {
        try {
          baseline = await readServerVersionStrict()
          reachable = true
          break
        } catch {
          await sleep(1000)
        }
      }
      if (!reachable) {
        toast.error(t("serverUnreachable"))
        return
      }
      const isRollback = (v: string | null): boolean =>
        !!v && !!baseline && v === baseline

      const snap = stateRef.current
      const target = snap.version
      const delayMs = snap.restartDelayMs ?? 2000
      const trialSeconds =
        snap.capability === "supervised" ? (snap.trialSeconds ?? 0) : 0

      await restartApp()
      await countdown(delayMs, setRestartCountdown)

      const healthy = await waitForServerHealthy({
        timeoutMs: 90_000,
        intervalMs: 1500,
      })
      if (!healthy) {
        toast.error(t("restartTimeout"))
        return
      }

      // Healthy — but the supervisor may have auto-rolled-back a version that
      // couldn't boot. Confirm the running version advanced.
      const running = await getRunningServerVersion()
      if (isRollback(running)) {
        toast.error(t("upgradeRolledBack"))
        return
      }

      // Supervised trial: keep watching across the probation window; a version
      // that boots but can't stay up is reverted within it.
      if (trialSeconds > 0 && target) {
        const trialDeadline = Date.now() + trialSeconds * 1000 + 3000
        let reverted = false
        while (Date.now() < trialDeadline) {
          setRestartCountdown(Math.ceil((trialDeadline - Date.now()) / 1000))
          await sleep(2000)
          const v = await getRunningServerVersion()
          if (isRollback(v)) {
            reverted = true
            break
          }
        }
        if (reverted) {
          toast.error(t("upgradeRolledBack"))
          return
        }
        // The loop skips transient nulls (briefly down during relaunch), so
        // require one definitive post-window reading before claiming success.
        let finalVersion: string | null = null
        for (let i = 0; i < 5; i++) {
          finalVersion = await getRunningServerVersion()
          if (finalVersion) break
          await sleep(1500)
        }
        if (isRollback(finalVersion)) {
          toast.error(t("upgradeRolledBack"))
          return
        }
        if (!finalVersion) {
          toast.error(t("restartTimeout"))
          return
        }
      }

      toast.success(t("upgradeSuccess"))
      window.location.reload()
    } catch (err) {
      const { rawMessage } = normalizeAppUpdateError(err)
      toast.error(t("installFailed", { message: rawMessage }))
      console.error("[Update] restart flow failed:", err)
    } finally {
      setRestartCountdown(null)
      setIsRestarting(false)
    }
  }, [t])

  const rollback = useCallback(async () => {
    setIsRollingBack(true)
    setRestartCountdown(null)
    try {
      // The version we are rolling back *from*. A successful rollback brings the
      // server back onto a different (previous) version.
      let fromVersion: string | null = null
      let reachable = false
      for (let i = 0; i < 3; i++) {
        try {
          fromVersion = await readServerVersionStrict()
          reachable = true
          break
        } catch {
          await sleep(1000)
        }
      }
      if (!reachable) {
        toast.error(t("serverUnreachable"))
        return
      }

      // Revert + relaunch is a single locked server op: it responds before it
      // exits/re-execs, so there is no separate restart call.
      const result = await rollbackServer()
      const delayMs =
        result.restartDelayMs || (stateRef.current.restartDelayMs ?? 2000)

      await countdown(delayMs, setRestartCountdown)

      const healthy = await waitForServerHealthy({
        timeoutMs: 90_000,
        intervalMs: 1500,
      })
      if (!healthy) {
        toast.error(t("restartTimeout"))
        return
      }

      const outcome = await confirmRollbackVersion(fromVersion)
      if (outcome === "unchanged") {
        toast.error(t("rollbackFailed"))
        return
      }
      if (outcome === "unreachable") {
        toast.error(t("restartTimeout"))
        return
      }

      toast.success(t("rollbackSuccess"))
      window.location.reload()
    } catch (err) {
      toast.error(t("rollbackFailed"))
      console.error("[Update] rollback failed:", err)
    } finally {
      setIsRollingBack(false)
      setRestartCountdown(null)
    }
  }, [t])

  const isUpdating =
    state.status === "downloading" || state.status === "installing"
  const isBusy =
    isUpdating ||
    isRollingBack ||
    isRestarting ||
    restartCountdown !== null ||
    state.status === "restarting"

  const value = useMemo<UpdateContextValue>(
    () => ({
      state,
      isUpdating,
      restartCountdown,
      isRollingBack,
      isRestarting,
      hydrated,
      isBusy,
      startUpdate,
      restart,
      rollback,
    }),
    [
      state,
      isUpdating,
      restartCountdown,
      isRollingBack,
      isRestarting,
      hydrated,
      isBusy,
      startUpdate,
      restart,
      rollback,
    ]
  )

  return (
    <UpdateContext.Provider value={value}>{children}</UpdateContext.Provider>
  )
}

/** Access the app-update controller. Returns null outside a provider, so the
 * global indicator can render nothing rather than throw on surfaces (login,
 * aux windows) that don't mount it. */
export function useAppUpdate(): UpdateContextValue | null {
  return useContext(UpdateContext)
}
