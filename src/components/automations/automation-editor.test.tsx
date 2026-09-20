import { render, screen, waitFor, cleanup } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import enMessages from "@/i18n/messages/en.json"
import { AutomationEditor } from "./automation-editor"
import * as api from "@/lib/api"
import type { Automation, Pipeline } from "@/lib/types"
import {
  resetAppWorkspaceStore,
  useAppWorkspaceStore,
} from "@/stores/app-workspace-store"

vi.mock("@/lib/platform", () => ({
  isDesktop: () => false,
  openFileDialog: vi.fn(),
  openUrl: vi.fn(),
  subscribe: vi.fn().mockResolvedValue(() => {}),
  onTransportReconnect: vi.fn(() => () => {}),
}))

vi.mock("@/lib/transport", () => ({
  getActiveRemoteConnectionId: () => null,
  isDesktop: () => false,
  getTransport: () => ({
    call: vi.fn(),
    emit: vi.fn(),
    on: vi.fn(),
  }),
}))

vi.mock("@/lib/api", () => ({
  automationComputeNextRun: vi.fn().mockResolvedValue("2026-06-02T09:00:00Z"),
  describeAgentOptions: vi.fn().mockResolvedValue({
    modes: null,
    available_commands: [],
    config_options: [],
  }),
  pipelineList: vi.fn().mockResolvedValue([]),
  scanSkillsDir: vi.fn().mockResolvedValue([]),
}))

const MOCK_PIPELINES: Pipeline[] = [
  {
    id: 1,
    name: "Duet (Coder + Reviewer)",
    preset_key: "duet",
    folder_id: 1,
    graph: { steps: [], loops: [] },
    isolation: "worktree_per_run",
    created_at: "2026-09-20T00:00:00Z",
    updated_at: "2026-09-20T00:00:00Z",
  },
  {
    id: 2,
    name: "Team (Planner + Coder + Reviewer + Tests)",
    preset_key: "team",
    folder_id: null,
    graph: { steps: [], loops: [] },
    isolation: "worktree_per_run",
    created_at: "2026-09-20T00:00:00Z",
    updated_at: "2026-09-20T00:00:00Z",
  },
]

describe("AutomationEditor - Pipeline Action & Regression", () => {
  beforeEach(() => {
    resetAppWorkspaceStore()
    useAppWorkspaceStore.setState({
      folders: [
        {
          id: 1,
          name: "repo",
          path: "/path/to/repo",
          kind: "regular",
        } as never,
        {
          id: 2,
          name: "repo2",
          path: "/path/to/repo2",
          kind: "regular",
        } as never,
      ],
    })
    vi.clearAllMocks()
    vi.mocked(api.pipelineList).mockResolvedValue(MOCK_PIPELINES)
    vi.mocked(api.describeAgentOptions).mockResolvedValue({
      modes: null,
      available_commands: [],
      config_options: [],
    })
  })

  afterEach(() => {
    cleanup()
  })

  function renderEditor(
    automation: Automation | null = null,
    onSubmit = vi.fn().mockResolvedValue(undefined),
    onCancel = vi.fn()
  ) {
    return {
      onSubmit,
      onCancel,
      ...render(
        <NextIntlClientProvider locale="en" messages={enMessages}>
          <AutomationEditor
            automation={automation}
            onSubmit={onSubmit}
            onCancel={onCancel}
          />
        </NextIntlClientProvider>
      ),
    }
  }

  it("renders all three action buttons including 'Run pipeline'", () => {
    renderEditor()
    expect(
      screen.getByRole("button", {
        name: enMessages.Automations.actionLaunchSession,
      })
    ).toBeInTheDocument()
    expect(
      screen.getByRole("button", {
        name: enMessages.Automations.actionEnqueueTask,
      })
    ).toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: enMessages.Pipeline.automationAction })
    ).toBeInTheDocument()
  })

  it("defaults to launch_session and shows session-specific controls", () => {
    renderEditor()
    const sessionBtn = screen.getByRole("button", {
      name: enMessages.Automations.actionLaunchSession,
    })
    expect(sessionBtn).toHaveAttribute("aria-pressed", "true")
    expect(
      screen.getByLabelText(enMessages.Automations.isolationWorktree)
    ).toBeInTheDocument()
    expect(
      screen.queryByLabelText(enMessages.Pipeline.automationPipeline)
    ).not.toBeInTheDocument()
  })

  it("switches to 'Run pipeline' and displays the pipeline selector", async () => {
    const user = userEvent.setup()
    renderEditor()

    const pipelineActionBtn = screen.getByRole("button", {
      name: enMessages.Pipeline.automationAction,
    })
    await user.click(pipelineActionBtn)

    expect(pipelineActionBtn).toHaveAttribute("aria-pressed", "true")
    await waitFor(() => {
      expect(vi.mocked(api.pipelineList)).toHaveBeenCalled()
    })

    expect(
      screen.getByLabelText(enMessages.Pipeline.automationPipeline)
    ).toBeInTheDocument()
    // Session isolation checkbox is hidden for pipeline action
    expect(
      screen.queryByLabelText(enMessages.Automations.isolationWorktree)
    ).not.toBeInTheDocument()
  })

  it("submits 'run_pipeline' action with selected pipeline_id", async () => {
    const user = userEvent.setup()
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    renderEditor(null, onSubmit)

    // Fill name
    const nameInput = screen.getByLabelText(enMessages.Automations.name)
    await user.type(nameInput, "My Pipeline Automation")

    // Type prompt into rich composer
    const promptInput = screen.getByLabelText(enMessages.Automations.prompt)
    await user.type(promptInput, "Run daily audit pipeline")

    // Switch action to run_pipeline
    const pipelineActionBtn = screen.getByRole("button", {
      name: enMessages.Pipeline.automationAction,
    })
    await user.click(pipelineActionBtn)

    // Select pipeline from dropdown
    const pipelineTrigger = screen.getByLabelText(
      enMessages.Pipeline.automationPipeline
    )
    await user.click(pipelineTrigger)
    const option = await screen.findByText("Duet (Coder + Reviewer)")
    await user.click(option)

    // Submit
    const saveBtn = screen.getByRole("button", {
      name: enMessages.Automations.save,
    })
    await user.click(saveBtn)

    await waitFor(() => {
      expect(onSubmit).toHaveBeenCalledTimes(1)
    })

    const draft = onSubmit.mock.calls[0][0]
    expect(draft.name).toBe("My Pipeline Automation")
    expect(draft.config.action).toBe("run_pipeline")
    expect(draft.config.pipeline_id).toBe(1)
    expect(draft.isolation).toBe("worktree_per_run")
  })

  it("initializes existing automation with action='run_pipeline' and preselected pipeline_id", async () => {
    const user = userEvent.setup()
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    const existingAutomation: Automation = {
      id: 42,
      name: "Existing Pipeline Automation",
      enabled: true,
      trigger_kind: "manual",
      cron: null,
      timezone: "UTC",
      next_run_at: null,
      agent_type: "claude_code",
      root_folder_id: 1,
      isolation: "worktree_per_run",
      branch: null,
      is_remote_branch: false,
      config: {
        action: "run_pipeline",
        pipeline_id: 2,
        prompt_blocks: [{ type: "text", text: "Existing pipeline prompt" }],
        display_text: "Existing pipeline prompt",
        config_values: {},
      },
      last_run_at: null,
      last_run_status: null,
      last_run_conversation_id: null,
      unseen_failures: 0,
      created_at: "2026-06-01T00:00:00Z",
      updated_at: "2026-06-01T00:00:00Z",
    }

    renderEditor(existingAutomation, onSubmit)

    const pipelineActionBtn = screen.getByRole("button", {
      name: enMessages.Pipeline.automationAction,
    })
    expect(pipelineActionBtn).toHaveAttribute("aria-pressed", "true")

    // The selector should display the selected pipeline name
    await waitFor(() => {
      expect(
        screen.getByLabelText(enMessages.Pipeline.automationPipeline)
      ).toHaveTextContent("Team (Planner + Coder + Reviewer + Tests)")
    })

    // Submit and verify pipeline_id is preserved
    const saveBtn = screen.getByRole("button", {
      name: enMessages.Automations.save,
    })
    await user.click(saveBtn)

    await waitFor(() => {
      expect(onSubmit).toHaveBeenCalledTimes(1)
    })

    const draft = onSubmit.mock.calls[0][0]
    expect(draft.config.action).toBe("run_pipeline")
    expect(draft.config.pipeline_id).toBe(2)
  })

  it("regression: submits 'launch_session' action without pipeline_id", async () => {
    const user = userEvent.setup()
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    renderEditor(null, onSubmit)

    const nameInput = screen.getByLabelText(enMessages.Automations.name)
    await user.type(nameInput, "Session Automation")

    const promptInput = screen.getByLabelText(enMessages.Automations.prompt)
    await user.type(promptInput, "Run session prompt")

    const saveBtn = screen.getByRole("button", {
      name: enMessages.Automations.save,
    })
    await user.click(saveBtn)

    await waitFor(() => {
      expect(onSubmit).toHaveBeenCalledTimes(1)
    })

    const draft = onSubmit.mock.calls[0][0]
    expect(draft.config.action).toBe("launch_session")
    expect(draft.config.pipeline_id).toBeUndefined()
  })

  it("regression: submits 'enqueue_task' action without pipeline_id and forced worktree_per_run", async () => {
    const user = userEvent.setup()
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    renderEditor(null, onSubmit)

    const nameInput = screen.getByLabelText(enMessages.Automations.name)
    await user.type(nameInput, "Enqueue Automation")

    const promptInput = screen.getByLabelText(enMessages.Automations.prompt)
    await user.type(promptInput, "Run task prompt")

    const enqueueBtn = screen.getByRole("button", {
      name: enMessages.Automations.actionEnqueueTask,
    })
    await user.click(enqueueBtn)

    const saveBtn = screen.getByRole("button", {
      name: enMessages.Automations.save,
    })
    await user.click(saveBtn)

    await waitFor(() => {
      expect(onSubmit).toHaveBeenCalledTimes(1)
    })

    const draft = onSubmit.mock.calls[0][0]
    expect(draft.config.action).toBe("enqueue_task")
    expect(draft.config.pipeline_id).toBeUndefined()
    expect(draft.isolation).toBe("worktree_per_run")
  })

  it("updates pipeline options and selection when picking a different pipeline", async () => {
    const user = userEvent.setup()
    const onSubmit = vi.fn().mockResolvedValue(undefined)
    renderEditor(null, onSubmit)

    const nameInput = screen.getByLabelText(enMessages.Automations.name)
    await user.type(nameInput, "Pipeline Switch Automation")

    const promptInput = screen.getByLabelText(enMessages.Automations.prompt)
    await user.type(promptInput, "Switch pipeline prompt")

    const pipelineActionBtn = screen.getByRole("button", {
      name: enMessages.Pipeline.automationAction,
    })
    await user.click(pipelineActionBtn)

    // Open select, pick Team
    const pipelineTrigger = screen.getByLabelText(
      enMessages.Pipeline.automationPipeline
    )
    await user.click(pipelineTrigger)
    const teamOption = await screen.findByText(
      "Team (Planner + Coder + Reviewer + Tests)"
    )
    await user.click(teamOption)

    // Open select again, pick Duet
    await user.click(pipelineTrigger)
    const duetOption = await screen.findByText("Duet (Coder + Reviewer)")
    await user.click(duetOption)

    const saveBtn = screen.getByRole("button", {
      name: enMessages.Automations.save,
    })
    await user.click(saveBtn)

    await waitFor(() => {
      expect(onSubmit).toHaveBeenCalledTimes(1)
    })

    const draft = onSubmit.mock.calls[0][0]
    expect(draft.config.action).toBe("run_pipeline")
    expect(draft.config.pipeline_id).toBe(1)
  })
})
