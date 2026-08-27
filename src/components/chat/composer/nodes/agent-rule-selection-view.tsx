import { ListChecks } from "lucide-react"
import { NodeViewWrapper, type ReactNodeViewProps } from "@tiptap/react"

import type { AgentRuleSelectionAttrs } from "../agent-rule-selection"

export function AgentRuleSelectionView({ node }: ReactNodeViewProps) {
  const attrs = node.attrs as AgentRuleSelectionAttrs
  const count = Array.isArray(attrs.ruleIds) ? attrs.ruleIds.length : 0
  return (
    <NodeViewWrapper
      as="span"
      className="mx-0.5 inline-flex max-w-full items-center gap-1 rounded-md border border-border bg-muted px-1.5 py-0.5 align-middle text-xs font-medium text-foreground"
      contentEditable={false}
      data-agent-rule-selection-badge=""
      title={attrs.sources?.join(", ")}
    >
      <ListChecks className="size-3 shrink-0" aria-hidden="true" />
      <span className="truncate">Agent Rules Picker · {count} rules</span>
    </NodeViewWrapper>
  )
}
