// The ONE decision function behind every http(s) click in the app: transcript
// links, tool cards, the terminal, notifications. Synchronous and pure — the
// caller supplies a snapshot of preferences and of the surface it sits in, and
// gets back an action to execute INSIDE THE CLICK'S OWN CALL STACK (a system
// target must reach `window.open` / `openUrl` before the user gesture expires;
// WebKit's popup blocker swallows anything a microtask later, see issue #410).
//
// Order, and why it is fixed:
//   1. classify — identical to what `link-safety.tsx` has always done, so a
//      local path still opens the file panel, `mailto:`/`tel:` still go to the
//      OS, and an unknown scheme (`vscode:`, `javascript:` …) is still refused
//      rather than handed to the OS.
//   2. terminal rules — a `block` host rule, or a loopback/private address seen
//      from a window bound to a REMOTE codeg-server. These cannot be inverted
//      by the modifier key: flipping a remote `localhost:3000` to the system
//      browser would only ever hit the local machine's loopback.
//   3. base target — an explicit menu choice, else a host rule, else the
//      per-source preference; without a built-in browser it is always `system`.
//   4. modifier — ⌘ (macOS) / Ctrl (elsewhere) inverts an ordinary preference.
//   5. placement — the file column when it is on screen, else the transcript's
//      own viewer drawer (full-page routes), else the column anyway.

import { isLoopbackOrPrivateHost } from "@/lib/browser/browser-url"
import type {
  BrowserPrefsSnapshot,
  LinkSource,
  LinkTarget,
} from "@/lib/browser/browser-prefs"
import { classifyLinkTarget, type LocalFileTarget } from "@/lib/link-classify"

export interface HostRule {
  /** Hostname, `*.suffix`, or `*`; an optional `:port` pins the port. */
  pattern: string
  action: LinkTarget | "block"
}

export interface LinkSurface {
  /** `browser_capabilities().available` on desktop; false in web mode. */
  builtinAvailable: boolean
  /** The workspace file column is on screen (conversations route). */
  fileColumnVisible: boolean
  /** A session viewer host can render the file/browser drawer instead. */
  viewerHostAvailable: boolean
  /** The window is bound to a remote codeg-server (`isRemoteDesktopMode()`). */
  remoteDesktop: boolean
}

export interface ResolveLinkContext {
  source: LinkSource
  /** Primary modifier held during the gesture: ⌘ on macOS, Ctrl elsewhere. */
  modifier: boolean
  surface: LinkSurface
  prefs: Pick<BrowserPrefsSnapshot, "defaultTarget">
  hostRules?: readonly HostRule[]
  /** An explicit user choice (context menu). Replaces the preference and
   *  ignores the modifier, but still yields to the terminal rules. */
  forceTarget?: LinkTarget
}

export type LinkAction =
  | { kind: "file"; target: LocalFileTarget }
  | { kind: "os-handler"; url: string; protocol: string }
  | {
      kind: "reject"
      reason: "empty" | "unsupported-scheme" | "blocked-host"
      url: string
    }
  | { kind: "system"; url: string }
  | {
      kind: "builtin"
      url: string
      placement: "tab" | "drawer"
      /** Chosen by the remote-workspace terminal rule: the address is loopback
       *  or private and only reachable from the remote host. */
      remoteOverride: boolean
    }

function effectivePort(parsed: URL): string {
  if (parsed.port) return parsed.port
  return parsed.protocol === "https:" ? "443" : "80"
}

function splitPattern(pattern: string): { host: string; port: string | null } {
  const trimmed = pattern.trim().toLowerCase()
  // `[::1]:3000` — an IPv6 literal keeps its brackets; the port follows them.
  if (trimmed.startsWith("[")) {
    const close = trimmed.indexOf("]")
    if (close === -1) return { host: trimmed, port: null }
    const host = trimmed.slice(1, close)
    const rest = trimmed.slice(close + 1)
    return { host, port: rest.startsWith(":") ? rest.slice(1) : null }
  }
  const colon = trimmed.lastIndexOf(":")
  if (colon !== -1 && /^\d+$/.test(trimmed.slice(colon + 1))) {
    return { host: trimmed.slice(0, colon), port: trimmed.slice(colon + 1) }
  }
  return { host: trimmed, port: null }
}

function hostMatches(patternHost: string, hostname: string): boolean {
  if (patternHost === "*") return true
  if (patternHost.startsWith("*.")) {
    const suffix = patternHost.slice(1) // ".example.com"
    return hostname.endsWith(suffix) && hostname.length > suffix.length
  }
  return patternHost === hostname
}

/** First matching rule wins; hostnames compare case-insensitively. */
export function matchHostRule(
  rules: readonly HostRule[] | undefined,
  parsed: URL
): HostRule | null {
  if (!rules || rules.length === 0) return null
  const hostname = parsed.hostname.replace(/^\[|\]$/g, "").toLowerCase()
  const port = effectivePort(parsed)
  for (const rule of rules) {
    const { host, port: rulePort } = splitPattern(rule.pattern)
    if (!host) continue
    if (!hostMatches(host, hostname)) continue
    if (rulePort !== null && rulePort !== port) continue
    return rule
  }
  return null
}

function invert(target: LinkTarget): LinkTarget {
  return target === "builtin" ? "system" : "builtin"
}

export function resolveLinkAction(
  rawUrl: string,
  ctx: ResolveLinkContext
): LinkAction {
  // 1. classify
  const classified = classifyLinkTarget(rawUrl)
  switch (classified.kind) {
    case "empty":
      return { kind: "reject", reason: "empty", url: rawUrl }
    case "file":
      return { kind: "file", target: classified.target }
    case "os-handler":
      return {
        kind: "os-handler",
        url: classified.url,
        protocol: classified.protocol,
      }
    case "unsupported":
      return { kind: "reject", reason: "unsupported-scheme", url: rawUrl }
    case "http":
      break
  }
  const { url, parsed } = classified
  const { surface } = ctx

  // 2. terminal rules
  const rule = matchHostRule(ctx.hostRules, parsed)
  if (rule?.action === "block") {
    return { kind: "reject", reason: "blocked-host", url }
  }
  const hostname = parsed.hostname.replace(/^\[|\]$/g, "")
  if (
    surface.remoteDesktop &&
    surface.builtinAvailable &&
    isLoopbackOrPrivateHost(hostname)
  ) {
    return {
      kind: "builtin",
      url,
      placement: placementFor(surface),
      remoteOverride: true,
    }
  }

  // 3. base target
  let target: LinkTarget
  if (!surface.builtinAvailable) {
    target = "system"
  } else if (ctx.forceTarget) {
    target = ctx.forceTarget
  } else {
    // `block` returned above, so a surviving rule carries a plain target.
    target = rule?.action ?? ctx.prefs.defaultTarget[ctx.source] ?? "builtin"
    // 4. modifier
    if (ctx.modifier) target = invert(target)
  }

  // 5. placement
  if (target === "system") return { kind: "system", url }
  return {
    kind: "builtin",
    url,
    placement: placementFor(surface),
    remoteOverride: false,
  }
}

function placementFor(surface: LinkSurface): "tab" | "drawer" {
  if (surface.fileColumnVisible) return "tab"
  return surface.viewerHostAvailable ? "drawer" : "tab"
}
