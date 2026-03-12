import type { AgentType } from "@/lib/types"
import { AGENT_COLORS } from "@/lib/types"
import { cn } from "@/lib/utils"

import ClaudeColor from "@lobehub/icons/es/Claude/components/Color"
import ClineMono from "@lobehub/icons/es/Cline/components/Mono"
import GeminiColor from "@lobehub/icons/es/Gemini/components/Color"
import GithubCopilotMono from "@lobehub/icons/es/GithubCopilot/components/Mono"
import GooseMono from "@lobehub/icons/es/Goose/components/Mono"
import QwenColor from "@lobehub/icons/es/Qwen/components/Color"
import KimiColor from "@lobehub/icons/es/Kimi/components/Color"
import MistralColor from "@lobehub/icons/es/Mistral/components/Color"
import OpenClawColor from "@lobehub/icons/es/OpenClaw/components/Color"
import { OpenAI, OpenCode } from "@lobehub/icons"

interface AgentIconProps {
  agentType: AgentType
  className?: string
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type AnyIcon = React.ComponentType<any>

const COLOR_ICONS: Partial<Record<AgentType, AnyIcon>> = {
  claude_code: ClaudeColor,
  gemini: GeminiColor,
  qwen_code: QwenColor,
  kimi: KimiColor,
  mistral_vibe: MistralColor,
  open_claw: OpenClawColor,
}

const MONO_ICONS: Partial<Record<AgentType, AnyIcon>> = {
  codex: OpenAI,
  open_code: OpenCode,
  github_copilot: GithubCopilotMono,
  cline: ClineMono,
  goose: GooseMono,
}

// Text-color versions for Mono icons and SVG fallbacks
const AGENT_TEXT_COLORS: Partial<Record<AgentType, string>> = {
  open_code: "text-blue-500",
  auggie: "text-purple-500",
  autohand: "text-emerald-500",
  cline: "text-rose-500",
  codebuddy_code: "text-violet-500",
  corust_agent: "text-amber-500",
  github_copilot: "text-gray-700 dark:text-gray-300",
  goose: "text-lime-500",
  junie: "text-pink-500",
  minion_code: "text-fuchsia-500",
  qoder: "text-teal-500",
  factory_droid: "text-yellow-600",
  stakpak: "text-slate-500",
}

function FallbackIcon({
  agentType,
  className,
}: {
  agentType: AgentType
  className?: string
}) {
  const cls = cn("shrink-0", AGENT_TEXT_COLORS[agentType], className)

  switch (agentType) {
    case "auggie":
      return (
        <svg viewBox="0 0 24 24" fill="none" className={cls}>
          <path
            d="M13 2L3 14h9l-1 8 10-12h-9l1-8z"
            fill="currentColor"
            fillOpacity="0.15"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      )
    case "junie":
      return (
        <svg viewBox="0 0 24 24" fill="none" className={cls}>
          <path
            d="M12 2l8 10-8 10-8-10z"
            fill="currentColor"
            fillOpacity="0.15"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinejoin="round"
          />
        </svg>
      )
    case "qoder":
      return (
        <svg viewBox="0 0 24 24" fill="none" className={cls}>
          <path
            d="M8 3H6a2 2 0 00-2 2v14a2 2 0 002 2h2M16 3h2a2 2 0 012 2v14a2 2 0 01-2 2h-2"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
          />
        </svg>
      )
    case "factory_droid":
      return (
        <svg viewBox="0 0 24 24" fill="none" className={cls}>
          <circle
            cx="12"
            cy="12"
            r="3"
            stroke="currentColor"
            strokeWidth="1.5"
          />
          <path
            d="M12 1v3M12 20v3M4.22 4.22l2.12 2.12M17.66 17.66l2.12 2.12M1 12h3M20 12h3M4.22 19.78l2.12-2.12M17.66 6.34l2.12-2.12"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
          />
        </svg>
      )
    case "autohand":
      return (
        <svg viewBox="0 0 24 24" fill="none" className={cls}>
          <path
            d="M18 8h-1V6a5 5 0 00-10 0v2H6a2 2 0 00-2 2v10a2 2 0 002 2h12a2 2 0 002-2V10a2 2 0 00-2-2zm-6 9a2 2 0 110-4 2 2 0 010 4zm3.1-9H8.9V6a3.1 3.1 0 116.2 0v2z"
            fill="currentColor"
            fillOpacity="0.85"
          />
        </svg>
      )
    case "codebuddy_code":
      return (
        <svg viewBox="0 0 24 24" fill="none" className={cls}>
          <path
            d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-1 14H9V8h2v8zm4 0h-2V8h2v8z"
            fill="currentColor"
            fillOpacity="0.85"
          />
        </svg>
      )
    case "corust_agent":
      return (
        <svg viewBox="0 0 24 24" fill="none" className={cls}>
          <path
            d="M12 3L2 7.5v9L12 21l10-4.5v-9L12 3zm0 2.24L19.5 8.5 12 11.76 4.5 8.5 12 5.24zM4 10.26l7 3.15V19l-7-3.15v-5.59zm9 8.74v-5.59l7-3.15v5.59L13 19z"
            fill="currentColor"
            fillOpacity="0.85"
          />
        </svg>
      )
    case "minion_code":
      return (
        <svg viewBox="0 0 24 24" fill="none" className={cls}>
          <rect
            x="4"
            y="2"
            width="16"
            height="20"
            rx="4"
            stroke="currentColor"
            strokeWidth="1.5"
            fill="currentColor"
            fillOpacity="0.1"
          />
          <circle cx="9" cy="10" r="2" fill="currentColor" />
          <circle cx="15" cy="10" r="2" fill="currentColor" />
          <path
            d="M9 15h6"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
          />
        </svg>
      )
    case "stakpak":
      return (
        <svg viewBox="0 0 24 24" fill="none" className={cls}>
          <rect
            x="3"
            y="15"
            width="18"
            height="4"
            rx="1"
            fill="currentColor"
            fillOpacity="0.85"
          />
          <rect
            x="3"
            y="10"
            width="18"
            height="4"
            rx="1"
            fill="currentColor"
            fillOpacity="0.6"
          />
          <rect
            x="3"
            y="5"
            width="18"
            height="4"
            rx="1"
            fill="currentColor"
            fillOpacity="0.35"
          />
        </svg>
      )
    default:
      return (
        <span
          className={cn(
            "rounded-full shrink-0",
            AGENT_COLORS[agentType],
            className
          )}
        />
      )
  }
}

export function AgentIcon({ agentType, className }: AgentIconProps) {
  const ColorIcon = COLOR_ICONS[agentType]
  if (ColorIcon) {
    return (
      <span className={cn("inline-flex shrink-0", className)}>
        <ColorIcon size="100%" />
      </span>
    )
  }

  const MonoIcon = MONO_ICONS[agentType]
  if (MonoIcon) {
    return (
      <span
        className={cn(
          "inline-flex shrink-0",
          AGENT_TEXT_COLORS[agentType],
          className
        )}
      >
        <MonoIcon size="100%" />
      </span>
    )
  }

  return <FallbackIcon agentType={agentType} className={className} />
}
