import {
  FlaskConical,
  ListChecks,
  Package,
  ScanSearch,
  ShieldCheck,
  Tag,
  Wrench,
  type LucideIcon,
} from "lucide-react"
import type {
  AgentType,
  AutomationDraft,
  AutomationIsolation,
  AutomationTriggerKind,
} from "@/lib/types"

/** i18n keys live in the `Automations` namespace; the unions keep `t(...)`
 *  type-checked against the typed message catalog. */
type TemplateTitleKey =
  | "tplCodeReviewTitle"
  | "tplDependencyUpdatesTitle"
  | "tplTestCoverageTitle"
  | "tplTodoSweepTitle"
  | "tplCiTriageTitle"
  | "tplReleaseNotesTitle"
  | "tplSecurityAuditTitle"

type TemplateDescKey =
  | "tplCodeReviewDesc"
  | "tplDependencyUpdatesDesc"
  | "tplTestCoverageDesc"
  | "tplTodoSweepDesc"
  | "tplCiTriageDesc"
  | "tplReleaseNotesDesc"
  | "tplSecurityAuditDesc"

export interface AutomationTemplate {
  id: string
  icon: LucideIcon
  /** Icon-chip classes: a text color + a matching low-alpha background tint. */
  accent: string
  titleKey: TemplateTitleKey
  descKey: TemplateDescKey
  /** Canonical English starting prompt; the user edits it before saving. Kept
   *  out of the i18n catalog deliberately — agent prompts are conventionally
   *  English and this is editable seed content, not chrome. */
  prompt: string
  trigger_kind: AutomationTriggerKind
  /** Suggested cadence. Carried even for manual templates so flipping the
   *  trigger to "schedule" in the editor keeps a sensible default. */
  cron: string
  isolation: AutomationIsolation
}

export const AUTOMATION_TEMPLATES: AutomationTemplate[] = [
  {
    id: "code-review",
    icon: ScanSearch,
    accent: "text-blue-500 bg-blue-500/10",
    titleKey: "tplCodeReviewTitle",
    descKey: "tplCodeReviewDesc",
    prompt:
      "Review the latest changes in this repository — the uncommitted diff if there is one, otherwise the most recent commits. Look for bugs, regressions, security issues, and code-quality problems. For each finding, cite the file and line, explain the risk, and propose a concrete fix. Group findings by severity (critical / high / medium / low). Produce a written review; do not change any files.",
    trigger_kind: "schedule",
    cron: "0 9 * * 1-5",
    isolation: "worktree_per_run",
  },
  {
    id: "dependency-updates",
    icon: Package,
    accent: "text-amber-500 bg-amber-500/10",
    titleKey: "tplDependencyUpdatesTitle",
    descKey: "tplDependencyUpdatesDesc",
    prompt:
      "Audit this project's dependencies for outdated or vulnerable packages. Identify safe, non-breaking upgrades (patch and minor versions, plus majors with a clear migration path). Apply the safe upgrades, update the lockfile, and run the test suite to confirm nothing breaks. Summarize what you changed, what you skipped, and why.",
    trigger_kind: "schedule",
    cron: "0 9 * * 1",
    isolation: "worktree_per_run",
  },
  {
    id: "test-coverage",
    icon: FlaskConical,
    accent: "text-emerald-500 bg-emerald-500/10",
    titleKey: "tplTestCoverageTitle",
    descKey: "tplTestCoverageDesc",
    prompt:
      "Find important code paths in this project that lack test coverage — prioritize core logic, error handling, and recently changed code. Write focused, meaningful tests for the highest-value gaps and make sure they pass. Avoid trivial or redundant tests. Summarize what you covered and what still needs attention.",
    trigger_kind: "schedule",
    cron: "0 9 * * 1",
    isolation: "worktree_per_run",
  },
  {
    id: "todo-sweep",
    icon: ListChecks,
    accent: "text-violet-500 bg-violet-500/10",
    titleKey: "tplTodoSweepTitle",
    descKey: "tplTodoSweepDesc",
    prompt:
      "Search the codebase for TODO, FIXME, HACK, and XXX comments. Collect them into a single list grouped by area, with the file and line for each. Assess each one's priority and effort, flag anything that looks stale or risky, and recommend which to tackle first. Produce a written summary; do not change any files.",
    trigger_kind: "manual",
    cron: "0 9 * * 1",
    isolation: "worktree_per_run",
  },
  {
    id: "ci-triage",
    icon: Wrench,
    accent: "text-orange-500 bg-orange-500/10",
    titleKey: "tplCiTriageTitle",
    descKey: "tplCiTriageDesc",
    prompt:
      "Investigate the most recent failing checks or build errors in this project. Reproduce the failures locally where possible, identify the root cause of each, and propose a specific fix. Distinguish real failures from flaky or environmental ones. Summarize each failure, its cause, and the recommended fix.",
    trigger_kind: "manual",
    cron: "0 * * * *",
    isolation: "worktree_per_run",
  },
  {
    id: "release-notes",
    icon: Tag,
    accent: "text-sky-500 bg-sky-500/10",
    titleKey: "tplReleaseNotesTitle",
    descKey: "tplReleaseNotesDesc",
    prompt:
      "Summarize everything that changed since the last release tag into clear, user-facing release notes. Group entries by type (features, fixes, performance, breaking changes) and write each in plain language. Call out anything that requires action from users. Output the notes in Markdown; do not change any project files.",
    trigger_kind: "manual",
    cron: "0 9 * * 1",
    isolation: "worktree_per_run",
  },
  {
    id: "security-audit",
    icon: ShieldCheck,
    accent: "text-rose-500 bg-rose-500/10",
    titleKey: "tplSecurityAuditTitle",
    descKey: "tplSecurityAuditDesc",
    prompt:
      "Audit this codebase for security issues — injection, authentication and authorization gaps, unsafe handling of secrets, vulnerable dependencies, path traversal, and SSRF risks. For each finding, cite the file and line, rate the severity, and describe a concrete remediation. Produce a written report; do not change any files.",
    trigger_kind: "schedule",
    cron: "0 9 * * 1",
    isolation: "worktree_per_run",
  },
]

function detectTimezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC"
  } catch {
    return "UTC"
  }
}

/** Build an editor seed draft from a template. The localized `name` is resolved
 *  by the caller (it lives in the i18n catalog); agent + folder come from the
 *  workspace defaults. */
export function templateToDraft(
  template: AutomationTemplate,
  opts: { name: string; agentType: AgentType; folderId: number | null }
): AutomationDraft {
  return {
    name: opts.name,
    enabled: true,
    trigger_kind: template.trigger_kind,
    cron: template.cron,
    timezone: detectTimezone(),
    agent_type: opts.agentType,
    root_folder_id: opts.folderId,
    isolation: template.isolation,
    branch: null,
    is_remote_branch: false,
    config: {
      prompt_blocks: [{ type: "text", text: template.prompt }],
      display_text: template.prompt,
      mode_id: null,
      config_values: {},
    },
  }
}
