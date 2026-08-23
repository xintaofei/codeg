import { describe, expect, it } from "vitest"
import enMessages from "@/i18n/messages/en.json"
import {
  canSubmitFollowUp,
  DEFAULT_FOLLOW_UP_INTENT,
  followUpComposerTarget,
  followUpScenario,
  FOLLOW_UP_SCENARIOS,
} from "@/lib/task-follow-up"
import type { AgentType, WorkTask, WorkTaskConfig } from "@/lib/types"

describe("follow-up scenarios", () => {
  it("defaults to the scenario that reproduces the old behaviour", () => {
    // `revise` composes the prompt the action sent before scenarios existed,
    // so an untouched chip row means an unchanged workflow.
    expect(DEFAULT_FOLLOW_UP_INTENT).toBe("revise")
    expect(FOLLOW_UP_SCENARIOS[0].intent).toBe("revise")
  })

  it("mirrors the backend enum", () => {
    expect(FOLLOW_UP_SCENARIOS.map((s) => s.intent)).toEqual([
      "revise",
      "continue",
      "question",
      "verify",
    ])
  })

  it("has a translated label and placeholder for every scenario", () => {
    const tasks = enMessages.Tasks as Record<string, string>
    for (const scenario of FOLLOW_UP_SCENARIOS) {
      expect(tasks[scenario.labelKey]).toBeTruthy()
      expect(tasks[scenario.placeholderKey]).toBeTruthy()
    }
  })

  it("only lets the self-check be sent empty", () => {
    for (const scenario of FOLLOW_UP_SCENARIOS) {
      expect(canSubmitFollowUp(scenario.intent, "")).toBe(scenario.allowsEmpty)
      expect(canSubmitFollowUp(scenario.intent, "   ")).toBe(
        scenario.allowsEmpty
      )
      expect(canSubmitFollowUp(scenario.intent, "do the thing")).toBe(true)
    }
    expect(canSubmitFollowUp("verify", "")).toBe(true)
    expect(canSubmitFollowUp("question", "")).toBe(false)
  })

  it("resolves a scenario by intent", () => {
    expect(followUpScenario("question").labelKey).toBe("followUpIntentQuestion")
  })
})

describe("followUpComposerTarget", () => {
  const FOLDERS = [
    { id: 1, path: "/repo", default_agent_type: "codex" as AgentType },
    { id: 2, path: "/repo-task-7", default_agent_type: null },
  ]

  const config = (agent_type: AgentType): WorkTaskConfig => ({
    prompt_blocks: [],
    display_text: "",
    config_values: {},
    agent_type,
  })

  function task(overrides: Partial<WorkTask> = {}): WorkTask {
    return {
      folder_id: 1,
      worktree_folder_id: 2,
      config: null,
      ...overrides,
    } as WorkTask
  }

  it("points at the worktree the agent actually works in", () => {
    expect(
      followUpComposerTarget({
        task: task(),
        conversationAgentType: "claude_code",
        folders: FOLDERS,
      })
    ).toEqual({
      agentType: "claude_code",
      folderPath: "/repo-task-7",
      skillWorkspacePath: "/repo-task-7",
    })
  })

  it("falls back to the project folder before the task has a worktree", () => {
    const target = followUpComposerTarget({
      task: task({ worktree_folder_id: null }),
      conversationAgentType: null,
      folders: FOLDERS,
    })
    expect(target.folderPath).toBe("/repo")
    expect(target.skillWorkspacePath).toBeNull()
  })

  it("prefers the conversation's agent over the task's own override", () => {
    // The conversation is what ran; a task-level override only says what was
    // asked for, and a system substitution may have changed it.
    const target = followUpComposerTarget({
      task: task({ config: config("codex") }),
      conversationAgentType: "claude_code",
      folders: FOLDERS,
    })
    expect(target.agentType).toBe("claude_code")
  })

  it("falls back through the override and the folder default", () => {
    expect(
      followUpComposerTarget({
        task: task({ config: config("gemini") }),
        conversationAgentType: null,
        folders: FOLDERS,
      }).agentType
    ).toBe("gemini")
    expect(
      followUpComposerTarget({
        task: task(),
        conversationAgentType: null,
        folders: FOLDERS,
      }).agentType
    ).toBe("codex")
  })

  it("survives folders that haven't hydrated yet", () => {
    expect(
      followUpComposerTarget({
        task: task(),
        conversationAgentType: null,
        folders: [],
      })
    ).toEqual({
      agentType: "claude_code",
      folderPath: null,
      skillWorkspacePath: null,
    })
  })
})
