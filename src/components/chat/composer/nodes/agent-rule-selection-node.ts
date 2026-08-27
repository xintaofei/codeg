import { mergeAttributes, Node, type JSONContent } from "@tiptap/core"
import { ReactNodeViewRenderer } from "@tiptap/react"

import {
  AGENT_RULE_SELECTION_NODE,
  isAgentRuleSelectionAttrs,
  serializeAgentRuleSelection,
  type AgentRuleSelectionAttrs,
} from "../agent-rule-selection"
import { AgentRuleSelectionView } from "./agent-rule-selection-view"

function parseJsonArray(raw: string | null): string[] {
  if (!raw) return []
  try {
    const parsed = JSON.parse(raw) as unknown
    return Array.isArray(parsed) &&
      parsed.every((item) => typeof item === "string")
      ? parsed
      : []
  } catch {
    return []
  }
}

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    agentRuleSelection: {
      insertAgentRuleSelection: (attrs: AgentRuleSelectionAttrs) => ReturnType
    }
  }
}

export const AgentRuleSelection = Node.create({
  name: AGENT_RULE_SELECTION_NODE,
  group: "inline",
  inline: true,
  atom: true,
  selectable: true,
  draggable: false,

  addAttributes() {
    return {
      version: {
        default: 1,
        parseHTML: () => 1,
        renderHTML: () => ({ "data-version": "1" }),
      },
      ruleIds: {
        default: [],
        parseHTML: (element) =>
          parseJsonArray(element.getAttribute("data-rule-ids")),
        renderHTML: (attrs) => ({
          "data-rule-ids": JSON.stringify(attrs.ruleIds),
        }),
      },
      sourceHash: {
        default: "",
        parseHTML: (element) => element.getAttribute("data-source-hash") ?? "",
        renderHTML: (attrs) => ({ "data-source-hash": attrs.sourceHash }),
      },
      sources: {
        default: [],
        parseHTML: (element) =>
          parseJsonArray(element.getAttribute("data-sources")),
        renderHTML: (attrs) => ({
          "data-sources": JSON.stringify(attrs.sources),
        }),
      },
      exactText: {
        default: "",
        parseHTML: (element) => element.getAttribute("data-exact-text") ?? "",
        renderHTML: (attrs) => ({ "data-exact-text": attrs.exactText }),
      },
      envelopeNonce: {
        default: "",
        parseHTML: (element) =>
          element.getAttribute("data-envelope-nonce") ?? "",
        renderHTML: (attrs) => ({
          "data-envelope-nonce": attrs.envelopeNonce,
        }),
      },
    }
  },

  parseHTML() {
    return [{ tag: "span[data-agent-rule-selection]" }]
  },

  renderHTML({ node, HTMLAttributes }) {
    const attrs = node.attrs as AgentRuleSelectionAttrs
    return [
      "span",
      mergeAttributes(HTMLAttributes, {
        "data-agent-rule-selection": "",
      }),
      isAgentRuleSelectionAttrs(attrs)
        ? serializeAgentRuleSelection(attrs)
        : "Agent Rules Picker",
    ]
  },

  renderText({ node }) {
    const attrs = node.attrs as AgentRuleSelectionAttrs
    return isAgentRuleSelectionAttrs(attrs)
      ? serializeAgentRuleSelection(attrs)
      : ""
  },

  renderMarkdown(node: JSONContent) {
    const attrs = node.attrs as unknown
    return isAgentRuleSelectionAttrs(attrs)
      ? serializeAgentRuleSelection(attrs)
      : ""
  },

  addNodeView() {
    return ReactNodeViewRenderer(AgentRuleSelectionView)
  },

  addCommands() {
    return {
      insertAgentRuleSelection:
        (attrs: AgentRuleSelectionAttrs) =>
        ({ commands }) =>
          commands.insertContent({ type: AGENT_RULE_SELECTION_NODE, attrs }),
    }
  },
})
