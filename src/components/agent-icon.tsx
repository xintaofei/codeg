import { memo, useId } from "react"

import type { AgentType } from "@/lib/types"
import { AGENT_COLORS } from "@/lib/types"
import { cn } from "@/lib/utils"

import {
  ClaudeCode,
  Cline,
  GeminiCLI,
  OpenCode,
} from "@lobehub/icons"

interface AgentIconProps {
  agentType: AgentType
  className?: string
}

// Codex.Color has a white background rect that shows as a white square on
// dark backgrounds. This component renders only the logo path with its
// gradient, without the opaque background.
const CodexColorIcon = memo(function CodexColorIcon({
  size = "1em",
}: {
  size?: string | number
}) {
  const id = useId()
  return (
    <svg
      height={size}
      style={{ flex: "none", lineHeight: 1 }}
      viewBox="2 2 20 20"
      width={size}
      xmlns="http://www.w3.org/2000/svg"
    >
      <title>Codex</title>
      <path
        d="M9.064 3.344a4.578 4.578 0 012.285-.312c1 .115 1.891.54 2.673 1.275.01.01.024.017.037.021a.09.09 0 00.043 0 4.55 4.55 0 013.046.275l.047.022.116.057a4.581 4.581 0 012.188 2.399c.209.51.313 1.041.315 1.595a4.24 4.24 0 01-.134 1.223.123.123 0 00.03.115c.594.607.988 1.33 1.183 2.17.289 1.425-.007 2.71-.887 3.854l-.136.166a4.548 4.548 0 01-2.201 1.388.123.123 0 00-.081.076c-.191.551-.383 1.023-.74 1.494-.9 1.187-2.222 1.846-3.711 1.838-1.187-.006-2.239-.44-3.157-1.302a.107.107 0 00-.105-.024c-.388.125-.78.143-1.204.138a4.441 4.441 0 01-1.945-.466 4.544 4.544 0 01-1.61-1.335c-.152-.202-.303-.392-.414-.617a5.81 5.81 0 01-.37-.961 4.582 4.582 0 01-.014-2.298.124.124 0 00.006-.056.085.085 0 00-.027-.048 4.467 4.467 0 01-1.034-1.651 3.896 3.896 0 01-.251-1.192 5.189 5.189 0 01.141-1.6c.337-1.112.982-1.985 1.933-2.618.212-.141.413-.251.601-.33.215-.089.43-.164.646-.227a.098.098 0 00.065-.066 4.51 4.51 0 01.829-1.615 4.535 4.535 0 011.837-1.388zm3.482 10.565a.637.637 0 000 1.272h3.636a.637.637 0 100-1.272h-3.636zM8.462 9.23a.637.637 0 00-1.106.631l1.272 2.224-1.266 2.136a.636.636 0 101.095.649l1.454-2.455a.636.636 0 00.005-.64L8.462 9.23z"
        fill={`url(#${id})`}
      />
      <defs>
        <linearGradient
          gradientUnits="userSpaceOnUse"
          id={id}
          x1="12"
          x2="12"
          y1="3"
          y2="21"
        >
          <stop stopColor="#B1A7FF" />
          <stop offset=".5" stopColor="#7A9DFF" />
          <stop offset="1" stopColor="#3941FF" />
        </linearGradient>
      </defs>
    </svg>
  )
})

const GenericColorIcon = memo(function GenericColorIcon({
  size = "1em",
}: {
  size?: string | number
}) {
  return (
    <svg
      height={size}
      style={{ flex: "none", lineHeight: 1 }}
      viewBox="0 0 24 24"
      width={size}
      xmlns="http://www.w3.org/2000/svg"
    >
      <title>Generic</title>
      <rect
        fill="#475569"
        height="24"
        rx="4"
        width="24"
      />
      <text
        dominantBaseline="central"
        fill="white"
        fontFamily="system-ui, sans-serif"
        fontSize="14"
        fontWeight="bold"
        textAnchor="middle"
        x="12"
        y="12.5"
      >
        G
      </text>
    </svg>
  )
})

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type AnyIcon2 = React.ComponentType<any>

const COLOR_ICONS: Partial<Record<AgentType, AnyIcon2>> = {
  claude_code: ClaudeCode.Color,
  codex: CodexColorIcon,
  gemini: GeminiCLI.Color,
  generic: GenericColorIcon,
}

const MONO_ICONS: Partial<Record<AgentType, AnyIcon2>> = {
  open_code: OpenCode,
  cline: Cline,
}

// Text-color versions for Mono icons
const AGENT_TEXT_COLORS: Partial<Record<AgentType, string>> = {}

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
