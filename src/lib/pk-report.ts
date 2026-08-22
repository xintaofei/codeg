/** Build a self-contained, shareable PK battle report. */
import { getAgentLabel } from "@/lib/custom-agents"
import { assignJudgeScoreSlots, contestantForJudgeScore } from "@/lib/pk-judge"
import {
  decodePkReportArtifact,
  encodePkReportArtifact,
  preparePkReportArtifactHtml,
} from "@/lib/pk-report-artifact"
import type { PkRound } from "@/stores/pk-arena-store"

export interface PkReportArtifact {
  path: string
  /** UTF-8 HTML encoded as base64. Present only for a runnable single-file
   * artifact; keeping it encoded prevents contestant markup from escaping the
   * report document while it is being generated. */
  contentBase64?: string
}

/** Select a genuinely standalone HTML artifact from a contestant worktree. */
export function reportableArtifactPaths(paths: readonly string[]): string[] {
  return paths.filter((path) => {
    const normalized = path.replace(/\\/g, "/").replace(/^\.\//, "")
    return normalized !== ".git" && !normalized.startsWith(".git/")
  })
}

export function pickRunnableHtmlPath(paths: readonly string[]): string | null {
  const reportable = reportableArtifactPaths(paths)
  const htmlPaths = reportable.filter((path) => /\.html?$/i.test(path))
  if (htmlPaths.length === 1) return htmlPaths[0]

  // Multi-file web entries are the normal case: index.html plus scripts,
  // styles and assets. Prefer the shallowest conventional entrypoint, while
  // refusing to guess between several arbitrarily named HTML documents.
  const indexPaths = htmlPaths
    .filter((path) => /(^|\/)index\.html?$/i.test(path))
    .sort((a, b) => {
      const depth = (path: string) => path.replace(/\\/g, "/").split("/").length
      return depth(a) - depth(b) || a.localeCompare(b)
    })
  return indexPaths[0] ?? null
}

function esc(raw: string): string {
  return raw
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
}

function formatMs(ms: number): string {
  const seconds = Math.round(ms / 1000)
  if (seconds < 60) return `${seconds}s`
  return `${Math.floor(seconds / 60)}m ${String(seconds % 60).padStart(2, "0")}s`
}

function diffStats(diff: string | null) {
  let added = 0
  let removed = 0
  for (const line of diff?.split("\n") ?? []) {
    if (line.startsWith("+") && !line.startsWith("+++")) added += 1
    if (line.startsWith("-") && !line.startsWith("---")) removed += 1
  }
  return { added, removed }
}

function renderDiff(diff: string | null, empty: string): string {
  if (!diff?.trim()) return `<p class="empty">${esc(empty)}</p>`
  return `<pre>${diff
    .split("\n")
    .map((line) => {
      const cls = line.startsWith("+")
        ? "add"
        : line.startsWith("-")
          ? "del"
          : line.startsWith("@@")
            ? "hunk"
            : ""
      return `<span class="line ${cls}">${esc(line || " ")}</span>`
    })
    .join("")}</pre>`
}

function runnableHtmlArtifact(
  artifacts: readonly PkReportArtifact[]
): PkReportArtifact | null {
  return (
    artifacts.find(
      (artifact) =>
        artifact.contentBase64 != null && /\.html?$/i.test(artifact.path)
    ) ?? null
  )
}

export function buildPkReportHtml(
  round: PkRound,
  artifactsBySlot: Record<string, PkReportArtifact[]>,
  locale = "zh-CN"
): string {
  const zh = locale.toLowerCase().startsWith("zh")
  const l = zh
    ? {
        title: "智能体 PK 战报",
        status: "状态",
        finished: "已结束",
        running: "进行中",
        ready: "就绪",
        contestants: "参赛智能体",
        duration: "总用时",
        generated: "生成时间",
        ranking: "比赛结果",
        agent: "智能体",
        time: "用时",
        tokens: "输出 Token",
        turns: "轮次",
        changes: "代码变更",
        files: "文件",
        unavailable: "未提供",
        verdict: "裁判结论",
        details: "选手产出",
        outputFiles: "产出文件",
        diff: "代码 Diff",
        noFiles: "没有产出文件",
        noDiff: "没有可比较的代码变更",
        showcase: "作品试玩",
        showcaseHint: "选择选手，直接运行其最终 HTML 作品",
        shareHint: "报告已完整内嵌作品，直接发送这个 HTML 文件即可分享",
        preview: "可运行预览",
        openRun: "新窗口运行",
        runHere: "在上方运行",
        viewSource: "查看 HTML 源码",
        sandboxHint: "预览在隔离环境中运行",
        generatedBy: "由 Codeg 智能体 PK 竞技场生成",
      }
    : {
        title: "Agent PK Battle Report",
        status: "Status",
        finished: "Finished",
        running: "Running",
        ready: "Ready",
        contestants: "Contestants",
        duration: "Duration",
        generated: "Generated",
        ranking: "Results",
        agent: "Agent",
        time: "Time",
        tokens: "Output tokens",
        turns: "Turns",
        changes: "Code changes",
        files: "Files",
        unavailable: "Not reported",
        verdict: "Judge verdict",
        details: "Contestant output",
        outputFiles: "Output files",
        diff: "Code diff",
        noFiles: "No output files",
        noDiff: "No comparable code changes",
        showcase: "Play the entries",
        showcaseHint: "Choose a contestant to run the final HTML entry",
        shareHint:
          "Every entry is embedded in this report—share this HTML file as-is",
        preview: "Live preview",
        openRun: "Open and run",
        runHere: "Run above",
        viewSource: "View HTML source",
        sandboxHint: "Preview runs in an isolated sandbox",
        generatedBy: "Generated by Codeg Agent PK Arena",
      }

  const finished = round.contestants.every((c) =>
    ["done", "error", "canceled"].includes(c.status)
  )
  const statusText = finished
    ? l.finished
    : round.status === "ready"
      ? l.ready
      : l.running
  const duration = round.contestants.reduce<number | null>(
    (max, c) => (c.durationMs == null ? max : Math.max(max ?? 0, c.durationMs)),
    null
  )
  const scores = assignJudgeScoreSlots(
    round.judgeResult?.scores ?? [],
    round.contestants.filter((contestant) => contestant.status === "done")
  ).sort((a, b) => a.rank - b.rank)
  const scoreBySlot = new Map(
    scores.flatMap((score) =>
      score.slot == null ? [] : ([[score.slot, score]] as const)
    )
  )

  const runnableEntries = round.contestants
    .flatMap((contestant) => {
      const artifact = runnableHtmlArtifact(
        artifactsBySlot[String(contestant.slot)] ?? []
      )
      if (!artifact?.contentBase64) return []
      const score = scoreBySlot.get(contestant.slot)
      return [
        {
          contestant,
          artifact: {
            ...artifact,
            contentBase64: encodePkReportArtifact(
              preparePkReportArtifactHtml(
                decodePkReportArtifact(artifact.contentBase64)
              )
            ),
          },
          score,
          agentLabel: `${getAgentLabel(contestant.agentType)}${
            contestant.label ? ` · ${contestant.label}` : ""
          }`,
        },
      ]
    })
    .sort(
      (a, b) =>
        (a.score?.rank ?? Number.MAX_SAFE_INTEGER) -
          (b.score?.rank ?? Number.MAX_SAFE_INTEGER) ||
        a.contestant.slot - b.contestant.slot
    )

  const showcase = runnableEntries.length
    ? `<section class="showcase" data-showcase>
        <div class="section-head showcase-heading"><div><h2>${l.showcase}</h2><p>${l.showcaseHint}</p></div><span>${l.shareHint}</span></div>
        <div class="entry-switcher" role="list" aria-label="${esc(l.showcase)}">${runnableEntries
          .map(
            ({ contestant, artifact, score, agentLabel }, index) =>
              `<button type="button" class="entry-trigger${index === 0 ? " is-active" : ""}" data-artifact-trigger data-slot="${contestant.slot}" data-agent-label="${esc(agentLabel)}" data-artifact-path="${esc(artifact.path)}" data-artifact-html="${artifact.contentBase64}" aria-pressed="${index === 0 ? "true" : "false"}"><span class="entry-rank">${score ? `#${score.rank}` : `0${index + 1}`}</span><span class="entry-name"><b>${esc(agentLabel)}</b><small>${score ? `${score.score} pts` : artifact.path}</small></span><span class="entry-play">▶</span></button>`
          )
          .join("")}</div>
        <div class="play-stage">
          <div class="play-toolbar"><div><b data-active-agent></b><small data-active-path></small></div><button type="button" data-open-artifact>${l.openRun} ↗</button></div>
          <iframe title="${esc(l.preview)}" sandbox="allow-scripts allow-pointer-lock" referrerpolicy="no-referrer"></iframe>
        </div>
      </section>`
    : ""

  const rows = round.contestants
    .map((c, index) => {
      const stat = diffStats(c.diff)
      const score = scoreBySlot.get(c.slot)
      const artifacts = artifactsBySlot[String(c.slot)] ?? []
      const tokenText = c.usage?.tokensReported
        ? c.usage.outputTokens.toLocaleString(locale)
        : l.unavailable
      return `<tr>
        <td class="rank">${score ? `#${score.rank}` : String(index + 1).padStart(2, "0")}</td>
        <td><b>${esc(getAgentLabel(c.agentType))}</b>${c.label ? `<small>${esc(c.label)}</small>` : ""}</td>
        <td>${score ? `<strong class="score">${score.score}</strong>` : "—"}</td>
        <td>${c.durationMs == null ? "—" : formatMs(c.durationMs)}</td>
        <td>${tokenText}</td>
        <td>${c.usage?.turnCount ?? "—"}</td>
        <td><span class="plus">+${stat.added}</span> <span class="minus">−${stat.removed}</span></td>
        <td>${artifacts.length}</td>
      </tr>`
    })
    .join("")

  const judge = round.judgeResult
    ? `<section class="verdict">
        <div class="eyebrow">${l.verdict}</div>
        <h2>${esc(round.judgeResult.summary)}</h2>
        <div class="comments">${scores
          .map((score) => {
            const contestant = contestantForJudgeScore(score, round.contestants)
            return `<article><span>#${score.rank}</span><div><b>${esc(
              getAgentLabel(score.agentType)
            )}${contestant?.label ? ` · ${esc(contestant.label)}` : ""} · ${score.score}</b><p>${esc(score.comment)}</p></div></article>`
          })
          .join("")}</div>
      </section>`
    : ""

  const details = round.contestants
    .map((c) => {
      const artifacts = artifactsBySlot[String(c.slot)] ?? []
      const runnable = runnableHtmlArtifact(artifacts)
      const fileList = artifacts.length
        ? `<ul>${artifacts
            .map((artifact) => `<li>${esc(artifact.path)}</li>`)
            .join("")}</ul>`
        : `<p class="empty">${l.noFiles}</p>`
      const body = runnable
        ? `<div class="artifact-layout">
          <div class="artifact-files"><h3>${l.outputFiles}</h3>${fileList}</div>
          <div class="artifact-note">
            <h3>${l.preview}</h3><p>${esc(runnable.path)} · ${l.sandboxHint}</p>
            <button type="button" data-run-slot="${c.slot}">${l.runHere} ↑</button>
            <details class="source-disclosure"><summary>${l.viewSource}</summary><pre class="artifact-source" data-source-slot="${c.slot}"></pre></details>
          </div>
        </div>`
        : `<div class="detail-grid">
          <div><h3>${l.outputFiles}</h3>${
            artifacts.length ? fileList : `<p class="empty">${l.noFiles}</p>`
          }</div>
          <div><h3>${l.diff}</h3>${renderDiff(c.diff, l.noDiff)}</div>
        </div>`
      return `<details class="contestant-detail">
        <summary><span>${esc(getAgentLabel(c.agentType))}${c.label ? ` · ${esc(c.label)}` : ""}</span><span>${l.outputFiles} ${artifacts.length} · ${runnable ? l.preview : l.diff}</span></summary>
        ${body}
      </details>`
    })
    .join("")

  return `<!doctype html>
<html lang="${esc(locale)}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>${esc(l.title)} · ${esc(round.task)}</title>
<style>
:root{--ink:#11110f;--paper:#f3f0e8;--card:#fffdf7;--line:#d8d2c5;--accent:#ffd84d;--muted:#6b675e;--green:#087b45;--red:#b42318}*{box-sizing:border-box}body{margin:0;background:#d9d4c8;color:var(--ink);font:14px/1.55 "Avenir Next","PingFang SC","Microsoft YaHei",sans-serif}.sheet{width:min(1280px,100%);margin:16px auto;background:var(--paper);box-shadow:0 24px 80px #302b2050}.masthead{position:relative;overflow:hidden;background:var(--ink);color:#fff;padding:28px 40px 24px}.masthead:after{content:"PK";position:absolute;right:22px;bottom:-96px;font:900 220px/1 Georgia,serif;color:#ffffff0b;letter-spacing:-20px}.brand{display:flex;align-items:center;gap:12px;font-size:11px;font-weight:700;letter-spacing:.16em;text-transform:uppercase}.brand i{display:block;width:28px;height:5px;background:var(--accent)}h1{position:relative;max-width:980px;margin:24px 0 20px;font:700 clamp(30px,4.2vw,52px)/1.08 Georgia,"Songti SC",serif;letter-spacing:-.035em;text-wrap:balance}.meta{position:relative;display:grid;grid-template-columns:repeat(4,1fr);gap:24px;border-top:1px solid #ffffff28;padding-top:15px}.meta span{display:block;color:#aaa79f;font-size:10px;text-transform:uppercase;letter-spacing:.08em}.meta b{display:block;margin-top:2px;color:#fff;font-size:14px;letter-spacing:0}.content{padding:28px 40px 56px}.section-head{display:flex;align-items:end;justify-content:space-between;margin:0 0 14px}.section-head h2{margin:0;font:700 24px/1.2 Georgia,"Songti SC",serif;letter-spacing:-.02em}.section-head span,.eyebrow{font-size:10px;font-weight:800;letter-spacing:.12em;text-transform:uppercase;color:var(--muted)}.showcase{margin-bottom:34px}.showcase-heading{align-items:start}.showcase-heading p{margin:4px 0 0;color:var(--muted)}.showcase-heading>span{max-width:380px;text-align:right;line-height:1.45}.entry-switcher{display:flex;gap:0;overflow-x:auto;border:1px solid var(--ink);border-bottom:0;background:var(--card)}.entry-trigger{display:grid;grid-template-columns:auto minmax(120px,1fr) auto;align-items:center;gap:10px;min-width:210px;min-height:58px;border:0;border-right:1px solid var(--line);background:transparent;padding:9px 12px;color:var(--ink);cursor:pointer;text-align:left}.entry-trigger:hover{background:#ebe6da}.entry-trigger.is-active{background:var(--accent)}.entry-rank{font:800 16px ui-monospace,monospace}.entry-name{min-width:0}.entry-name b,.entry-name small{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.entry-name small{color:var(--muted);font-size:10px}.entry-play{font-size:10px}.play-stage{border:1px solid var(--ink);background:#fff}.play-toolbar{display:flex;align-items:center;justify-content:space-between;gap:16px;min-height:52px;padding:9px 12px;border-bottom:1px solid var(--line);background:#ebe6da}.play-toolbar div{min-width:0}.play-toolbar b,.play-toolbar small{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.play-toolbar small{color:var(--muted);font-size:11px}.play-toolbar button,.artifact-note>button{min-height:36px;border:1px solid var(--ink);background:var(--ink);color:#fff;padding:7px 12px;cursor:pointer;font:700 12px/1 inherit}.play-stage iframe{display:block;width:100%;height:min(680px,70vh);border:0;background:#fff}.table-wrap{overflow:auto;border:1px solid var(--line);background:var(--card)}table{width:100%;border-collapse:collapse;min-width:820px}th,td{padding:15px 14px;border-bottom:1px solid var(--line);text-align:left;white-space:nowrap}th{background:#ebe6da;color:var(--muted);font-size:10px;letter-spacing:.08em;text-transform:uppercase}tr:last-child td{border-bottom:0}.rank{font:800 18px ui-monospace,monospace}.score{display:inline-grid;place-items:center;min-width:42px;padding:3px 8px;background:var(--accent);font-size:17px}td small{display:block;color:var(--muted)}.plus{color:var(--green)}.minus{color:var(--red)}.verdict{margin:34px 0;padding:30px;border:1px solid var(--ink);background:var(--card)}.verdict h2{max-width:900px;margin:10px 0 26px;font:700 24px/1.4 Georgia,"Songti SC",serif;text-wrap:pretty}.comments{display:grid;grid-template-columns:repeat(auto-fit,minmax(240px,1fr));gap:12px}.comments article{display:flex;gap:12px;border-top:1px solid var(--line);padding-top:14px}.comments article>span{font:800 18px ui-monospace,monospace}.comments p{margin:5px 0 0;color:var(--muted);text-wrap:pretty}.details{margin-top:34px}details{border-top:1px solid var(--line);background:var(--card)}details:last-child{border-bottom:1px solid var(--line)}summary{display:flex;justify-content:space-between;gap:20px;padding:18px 20px;cursor:pointer;font-weight:700}summary span:last-child{color:var(--muted);font-size:12px;font-weight:500}.detail-grid{display:grid;grid-template-columns:minmax(220px,.35fr) minmax(0,1fr);gap:24px;border-top:1px solid var(--line);padding:20px}.detail-grid h3,.artifact-layout h3{margin:0 0 10px;font-size:12px;text-transform:uppercase;letter-spacing:.08em}.detail-grid ul,.artifact-files ul{margin:0;padding-left:18px;word-break:break-all}.artifact-layout{display:grid;grid-template-columns:minmax(180px,.28fr) minmax(0,1fr);gap:20px;border-top:1px solid var(--line);padding:20px}.artifact-note p{margin:0 0 12px;color:var(--muted)}.source-disclosure{margin-top:12px;border:1px solid var(--line)}.source-disclosure summary{padding:10px 12px;font-size:12px}.artifact-source{max-height:420px}.empty{color:var(--muted)}pre{max-height:560px;margin:0;overflow:auto;background:#171715;color:#d7d3ca;padding:16px;font:11px/1.55 ui-monospace,SFMono-Regular,Menlo,monospace}.line{display:block;white-space:pre}.line.add{background:#0c7b4530;color:#a9e6c5}.line.del{background:#b4231833;color:#ffb4ab}.line.hunk{color:#ffd84d}.footer{display:flex;justify-content:space-between;border-top:1px solid var(--line);margin-top:42px;padding-top:18px;color:var(--muted);font-size:12px}@media(max-width:720px){.sheet{margin:0}.masthead,.content{padding-left:18px;padding-right:18px}.masthead{padding-top:22px}.meta{grid-template-columns:1fr 1fr}.showcase-heading{display:block}.showcase-heading>span{display:block;margin-top:8px;text-align:left}.entry-trigger{min-width:190px}.play-stage iframe{height:62vh}.detail-grid,.artifact-layout{grid-template-columns:1fr}.footer{display:block}}@media print{body{background:#fff}.sheet{width:100%;margin:0;box-shadow:none}.showcase{display:none}details{break-inside:avoid}.masthead{-webkit-print-color-adjust:exact;print-color-adjust:exact}}
</style></head><body><main class="sheet">
<header class="masthead"><div class="brand"><i></i>Codeg · ${esc(l.title)}</div><h1>${esc(round.task)}</h1><div class="meta"><div><span>${l.status}</span><b>${statusText}</b></div><div><span>${l.contestants}</span><b>${round.contestants.length}</b></div><div><span>${l.duration}</span><b>${duration == null ? "—" : formatMs(duration)}</b></div><div><span>${l.generated}</span><b>${new Date().toLocaleString(locale)}</b></div></div></header>
<div class="content">${showcase}<section><div class="section-head"><h2>${l.ranking}</h2><span>ROUND ${round.id}</span></div><div class="table-wrap"><table><thead><tr><th>#</th><th>${l.agent}</th><th>Score</th><th>${l.time}</th><th>${l.tokens}</th><th>${l.turns}</th><th>${l.changes}</th><th>${l.files}</th></tr></thead><tbody>${rows}</tbody></table></div></section>${judge}<section class="details"><div class="section-head"><h2>${l.details}</h2></div>${details}</section><footer class="footer"><span>${esc(l.generatedBy)}</span><span>${new Date(round.createdAt).toLocaleString(locale)}</span></footer></div>
</main><script>
(()=>{const showcase=document.querySelector("[data-showcase]");if(!showcase)return;const decode=(value)=>{const binary=atob(value);const bytes=Uint8Array.from(binary,(char)=>char.charCodeAt(0));return new TextDecoder().decode(bytes)};const triggers=[...showcase.querySelectorAll("[data-artifact-trigger]")];const frame=showcase.querySelector("iframe");const agent=showcase.querySelector("[data-active-agent]");const path=showcase.querySelector("[data-active-path]");const openButton=showcase.querySelector("[data-open-artifact]");let active=null;const activate=(button)=>{if(!button||!frame)return;active=button;for(const item of triggers){const selected=item===button;item.classList.toggle("is-active",selected);item.setAttribute("aria-pressed",String(selected))}if(agent)agent.textContent=button.dataset.agentLabel||"";if(path)path.textContent=(button.dataset.artifactPath||"")+" · ${l.sandboxHint}";frame.title=(button.dataset.agentLabel||"")+" · ${l.preview}";frame.srcdoc=decode(button.dataset.artifactHtml||"")};for(const button of triggers)button.addEventListener("click",()=>activate(button));if(openButton)openButton.addEventListener("click",()=>{if(!active)return;const html=decode(active.dataset.artifactHtml||"");const url=URL.createObjectURL(new Blob([html],{type:"text/html;charset=utf-8"}));window.open(url,"_blank","noopener");setTimeout(()=>URL.revokeObjectURL(url),60000)});document.addEventListener("click",(event)=>{const target=event.target;if(!(target instanceof Element))return;const runButton=target.closest("[data-run-slot]");if(!runButton)return;const slot=runButton.getAttribute("data-run-slot");const trigger=triggers.find((item)=>item.dataset.slot===slot);if(!trigger)return;activate(trigger);window.scrollTo({top:Math.max(0,showcase.getBoundingClientRect().top+window.scrollY-12),behavior:"smooth"})});document.addEventListener("toggle",(event)=>{const detail=event.target;if(!(detail instanceof HTMLDetailsElement)||!detail.open||!detail.classList.contains("source-disclosure"))return;const source=detail.querySelector("[data-source-slot]");if(!source||source.textContent)return;const slot=source.getAttribute("data-source-slot");const trigger=triggers.find((item)=>item.dataset.slot===slot);if(trigger)source.textContent=decode(trigger.dataset.artifactHtml||"")},true);activate(triggers[0])})();
</script></body></html>`
}
