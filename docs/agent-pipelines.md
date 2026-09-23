# Agent pipelines

One agent in a chat handles a targeted edit well. A larger task usually wants a
shape: plan it, write it, review it, and send the review back to whoever wrote
the code. Until now that meant driving every hand-off by hand.

A pipeline runs that chain for you. Nothing here is on by default: Codeg still
opens in single-agent mode with memory off, and the chat you already know
behaves exactly as before.

## Picking a chain

The composer has four buttons above the message box.

| Mode | Chain | Fix rounds |
| --- | --- | --- |
| Single agent | no pipeline, the classic chat | n/a |
| Duet | coder, then reviewer | up to 3 |
| Team | planner, coder, reviewer, tests | up to 3 |
| Custom | whatever you build | 1 to 10 |

Pick anything but Single and a row of chips appears below, one per step, each
naming its role, its agent and its model. That row is the chain that will run.

The choice is remembered per folder.

## Editing the chain

Click a chip. Two lists open: the agent that runs the step, and the model it
runs on. The models come from asking that agent what it accepts, so the list is
whatever your installed adapter really offers rather than a table that goes
stale. There is no save button. The chip updates and the change is stored.

Editing Duet or Team stores your version of the built-in chain; a **Reset**
appears beside the chips and puts the shipped one back. A custom chain saves as
itself.

In Custom mode a **+** sits at the end of the row. It asks for the role first,
because the role decides where the step lands: a planner goes to the front,
since it exists to brief the steps after it, and everything else is appended.
Deleting a step lives inside the same popover, and the last step cannot go.

While a pipeline mode is active the composer hides its own model and reasoning
pickers. The model belongs to each step now, and two controls for one thing
point at a session the run does not use. Everything else stays, including the
edit-permission mode, which matters more once several agents are writing.

For the rest of a step (its prompt, timeout, read-only flag and loop target)
open the pipeline card on the Infinite Conversations canvas.

## What the roles do

- **planner** reads the task and the code and writes a plan. It must not edit.
- **coder** makes the change. This is the only step expected to write files.
- **reviewer** checks the result and reports a verdict. It must not edit.
- **tests** runs the suite and reports a verdict. It may write, because test
  runs leave artifacts.
- **custom** is whatever you need.

A step marked read-only is policed: Codeg hashes the worktree before and after
it, and a step that changed anything has its verdict turned into `inconclusive`
with the offending paths named. The step's prompt says so in plain words too,
so the agent is told the rule rather than only punished for breaking it.

Two things commonly trip that guard and are worth knowing about: a session-start
hook that writes a file into every new directory, and a build cache the project
does not ignore. Both show up in the note, so you can add them to `.gitignore`
and move on.

## Verdicts

A reviewer or test step ends by calling the `pipeline_verdict` tool once:

- `pass`: the work is good, the chain moves on.
- `changes_requested`: back to the coder with the notes, which are required.
- `inconclusive`: the step could not tell, and the run stops.

If the agent's tool calling is unreliable, a `VERDICT: PASS` line in its own
output is parsed as a fallback. Prefer an agent that calls the tool properly;
the fallback is a safety net, not a plan.

After the last fix round the run stops as `stopped_max_iterations` rather than
looping forever.

## Isolation and landing the work

Each run works in its own git worktree on a temporary branch, so your working
copy is untouched while agents write. When you are satisfied, **Apply** brings
the result over as either a squash commit or a merge commit, then removes the
worktree and the branch.

Apply refuses to run when the project has staged changes of its own, rather
than sweeping them into a commit you did not write.

A folder can only host one run at a time.

If Codeg is closed mid-run, the run is marked `interrupted` on the next start
rather than being left looking alive.

## Watching a run

The run card in the conversation shows each step, the round count and the
verdicts as they land.

The **chat + code** panel beside it shows what changed: a file tree with A/M/D/R
badges, the diff itself, and a place to leave notes on individual lines. From
there you can send those notes back for another round, stop and finish by hand,
or apply.

## Running one on a schedule

Automations take a **Run pipeline** action. Pick the chain, pick the folder, set
a cron expression or leave it manual. The run is headless and behaves exactly as
it would from the composer, worktree included.

## Memory

Off by default. Turn it on in Settings, Memory.

Three backends: off, a local SQLite graph with full-text search and two-hop edge
traversal, or your own MCP server.

What gets remembered is your choice. Four built-in kinds ship (decisions, fixed
bugs, task summaries, facts and preferences) and you can add your own with an
instruction in your own words. Each kind is set to automatic, on request, or
off, and the whole store is scoped either to the project or shared globally.

`memory_write`, `memory_search` and `memory_link` are exposed to agents only
while memory is on. Everything written passes a redactor that strips API keys,
tokens, passwords and private keys first. Injected memory is wrapped in a
`<memory untrusted="true">` block so an agent treats it as data.

Scope is worth a thought if you keep several clients' projects in one window:
project scope keeps each store separate, global shares one across all of them.

## Step reference

A step in the graph carries:

| Field | Meaning |
| --- | --- |
| `id` | unique, lowercase, up to 32 chars |
| `role` | `planner`, `coder`, `reviewer`, `tests`, `custom` |
| `label` | what the UI shows |
| `agent_type` | `claude_code`, `codex`, `gemini`, any installed ACP agent |
| `mode_id` | optional agent sub-mode |
| `config_values` | passed to the agent; `model` lives here |
| `prompt_template` | the instructions, with the variables below |
| `timeout_secs` | default 1800, range 1 to 86400 |
| `read_memory` | query memory before this step |
| `read_only` | the step must not touch files |

Prompt variables: `$task` (what you typed), `$plan` (the last planner's
summary), `$summary` (the previous step's), `$review` (notes from the last
`changes_requested`), `$memory` (retrieved context).

A graph is rejected before it runs if it is empty, has more than eight steps,
repeats a step id, names an unknown agent, has an empty prompt, has a timeout
out of range, or has a loop that points forward, starts anywhere but a reviewer
or tests step, or asks for fewer than 1 or more than 10 rounds.
