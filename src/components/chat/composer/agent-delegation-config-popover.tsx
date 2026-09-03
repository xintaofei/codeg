"use client"

import { useMemo } from "react"
import { useTranslations } from "next-intl"
import { Loader2, RotateCcw } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Popover, PopoverAnchor, PopoverContent } from "@/components/ui/popover"
import { AgentConfigSection } from "@/components/automations/agent-config-section"
import { InlineSessionConfigToggle } from "@/components/chat/session-config-selector"
import { useAgentOptions } from "@/components/automations/use-agent-options"
import { getAgentLabel } from "@/lib/custom-agents"
import type {
  AgentDelegationDefaults,
  AgentType,
  SessionConfigOptionInfo,
} from "@/lib/types"

interface AgentDelegationConfigPopoverProps {
  /** The mentioned agent this popover configures. */
  agentType: AgentType
  /** Working dir for the ACP probe (the composer's folder path). */
  workingDir: string | null
  /**
   * DOM element the popover points at — the mention badge that opened it.
   * Null falls back to Radix's default positioning.
   */
  anchorEl: HTMLElement | null
  /**
   * The agent's persisted global delegation default — the baseline the
   * selectors start from when this mention carries no override yet. Read-only
   * here: nothing in this popover ever writes to global settings.
   */
  globalDefault: AgentDelegationDefaults | null
  /**
   * True while the global default is still being fetched. The rows stay
   * uneditable until it lands: an edit emitted against a not-yet-loaded
   * baseline would contain ONLY the touched row, and the backend applies a
   * per-call value INSTEAD of the global default (no merge), so the global's
   * other pins would be silently dropped.
   */
  globalDefaultLoading?: boolean
  /** Failure loading the global baseline; editing stays disabled until retry. */
  globalDefaultError?: string | null
  /** Retries the global baseline fetch after an error. */
  onRetryGlobalDefault?: () => void
  /** The mention's current draft-local override; null = following global. */
  value: AgentDelegationDefaults | null
  /**
   * Emits the next draft-local override. `null` resets the mention to the
   * global default (removes the entry) — it never touches global settings.
   */
  onChange: (next: AgentDelegationDefaults | null) => void
  onClose: () => void
}

/**
 * Per-mention delegation config: the popover an `@Agent` mention opens so the
 * user can pin a model / mode / permission for THIS one delegation.
 *
 * The option rows are the same `AgentConfigSection` surface the settings page
 * and the automation editor use, fed by the same transient ACP probe
 * (`describeAgentOptions` via `useAgentOptions`) — only options the agent
 * actually advertises are selectable, and nothing is written to the agent's
 * native config or environment. Boolean config options (e.g. Cline's
 * `auto_approve`) render as toggles beside the selects. Initial selections
 * come from the mention's override, falling back to the agent's persisted
 * global delegation default, so an untouched popover shows exactly what a
 * delegation would use today.
 *
 * Edits emit the FULL effective selection, not just the changed row: the
 * backend applies a per-call value INSTEAD of the global default (per-call >
 * global > agent native, no merge), so a partial override that dropped the
 * global's model pin would silently change rows the user never touched.
 */
export function AgentDelegationConfigPopover({
  agentType,
  workingDir,
  anchorEl,
  globalDefault,
  globalDefaultLoading = false,
  globalDefaultError = null,
  onRetryGlobalDefault,
  value,
  onChange,
  onClose,
}: AgentDelegationConfigPopoverProps) {
  const t = useTranslations("Folder.chat.messageInput")
  const tAutomations = useTranslations("Automations")
  const tDelegation = useTranslations("AcpAgentSettings.multiAgent")
  // Same transient probe the settings page uses; the module cache dedups
  // rapid re-opens of the same (agent, folder) pair.
  const { snapshot, loading, error, reload } = useAgentOptions(
    agentType,
    workingDir
  )

  // Displayed selection: the mention's override first, then the global
  // default — the popover never shows an empty state that would suggest the
  // delegation runs with nothing pinned while the global default actually is.
  const effectiveModeId = value?.mode_id ?? globalDefault?.mode_id ?? null
  const effectiveConfigValues =
    value?.config_values ?? globalDefault?.config_values ?? {}

  // Any row edit emits the whole displayed picture (see doc comment), then the
  // popover stays open so the user can keep tweaking; dismissal is Radix's.
  // Rows left on "Agent default" are NOT pinned — they inherit the agent's
  // native default at spawn time, exactly what the row's sentinel displays.
  const emit = (
    modeId: string | null,
    configValues: Record<string, string>
  ) => {
    const modeEmpty = modeId == null || modeId.length === 0
    const config: Record<string, string> = {}
    for (const [id, v] of Object.entries(configValues)) {
      if (v.length > 0) config[id] = v
    }
    if (modeEmpty && Object.keys(config).length === 0) {
      onChange(null)
      return
    }
    // `mode_id` is omitted (not nulled) when unpinned, matching the shape the
    // Rust `AgentDelegationDefaults` treats as "no mode override".
    onChange({
      ...(modeEmpty ? {} : { mode_id: modeId as string }),
      config_values: config,
    })
  }

  const virtualRef = useMemo(
    () => ({
      current: {
        getBoundingClientRect: () =>
          anchorEl?.getBoundingClientRect() ?? new DOMRect(0, 0, 0, 0),
      },
    }),
    [anchorEl]
  )

  // Boolean config options don't fit a select — they render as toggles below
  // the selects (same chip the live composer uses), reading/writing the same
  // effective `config_values` map. Opaque strings end to end: the backend
  // encodes `"true"` as a real JSON boolean on the wire.
  const booleanOptions =
    snapshot?.config_options.filter(
      (option: SessionConfigOptionInfo) => option.kind.type === "boolean"
    ) ?? []

  return (
    <Popover
      open
      onOpenChange={(open) => {
        if (!open) onClose()
      }}
    >
      <PopoverAnchor virtualRef={virtualRef} />
      <PopoverContent
        side="top"
        align="start"
        className="w-[20rem] max-w-[calc(100vw-1rem)] gap-3 p-3"
        aria-label={t("mentionConfigTitle", {
          agent: getAgentLabel(agentType),
        })}
      >
        <div className="flex items-center justify-between gap-2">
          <p className="min-w-0 truncate text-sm font-medium">
            {t("mentionConfigTitle", { agent: getAgentLabel(agentType) })}
          </p>
          <Button
            size="xs"
            variant="ghost"
            className="shrink-0 gap-1 text-xs text-muted-foreground"
            disabled={value == null}
            onClick={() => onChange(null)}
            title={t("mentionConfigUseGlobal")}
          >
            <RotateCcw className="size-3" aria-hidden="true" />
            {t("mentionConfigUseGlobal")}
          </Button>
        </div>
        <p className="text-xs text-muted-foreground">
          {t("mentionConfigHint")}
        </p>
        {globalDefaultLoading ? (
          // Baseline still loading: hold the rows back entirely. Letting the
          // user edit against an empty baseline would emit a partial override
          // that the backend applies INSTEAD of the global default, dropping
          // pins the user never touched. The load is a cheap local IPC.
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
            {t("loadingSettings")}
          </div>
        ) : globalDefaultError ? (
          <div className="space-y-2 text-xs text-destructive">
            <p>{tDelegation("loadFailed", { detail: globalDefaultError })}</p>
            <Button
              size="xs"
              variant="outline"
              onClick={() => onRetryGlobalDefault?.()}
            >
              {tDelegation("retry")}
            </Button>
          </div>
        ) : (
          <>
            <AgentConfigSection
              snapshot={snapshot}
              loading={loading}
              error={error}
              onReload={reload}
              modeId={effectiveModeId}
              configValues={effectiveConfigValues}
              onModeChange={(modeId) => emit(modeId, effectiveConfigValues)}
              onConfigChange={(optionId, valueId) => {
                const nextConfig = { ...effectiveConfigValues }
                if (valueId === null) delete nextConfig[optionId]
                else nextConfig[optionId] = valueId
                emit(effectiveModeId, nextConfig)
              }}
            />
            {booleanOptions.length > 0 ? (
              <div className="flex flex-wrap items-center gap-1.5">
                {booleanOptions.map((option) => (
                  <InlineSessionConfigToggle
                    key={option.id}
                    option={option}
                    overrideValue={effectiveConfigValues[option.id] ?? null}
                    onSelect={(configId, next) =>
                      emit(effectiveModeId, {
                        ...effectiveConfigValues,
                        [configId]: next,
                      })
                    }
                    onLabel={t("toggleOn")}
                    offLabel={t("toggleOff")}
                  />
                ))}
              </div>
            ) : null}
          </>
        )}
        {!loading &&
        !error &&
        snapshot === null &&
        !globalDefaultLoading &&
        !globalDefaultError ? (
          <p className="text-xs text-muted-foreground">
            {tAutomations("configNone")}
          </p>
        ) : null}
      </PopoverContent>
    </Popover>
  )
}
