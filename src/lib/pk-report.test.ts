import { describe, expect, it } from "vitest"
import { JSDOM } from "jsdom"
import {
  buildPkReportHtml,
  pickRunnableHtmlPath,
  reportableArtifactPaths,
} from "@/lib/pk-report"
import {
  decodePkReportArtifact,
  encodePkReportArtifact,
  preparePkReportArtifactHtml,
} from "@/lib/pk-report-artifact"
import type { PkRound } from "@/stores/pk-arena-store"

const round = {
  id: "7",
  task: "实现一个中文贪吃蛇",
  createdAt: Date.parse("2026-08-20T08:00:00Z"),
  status: "finished",
  judgeResult: {
    scores: [
      {
        slot: 0,
        agentType: "qoder",
        score: 88,
        rank: 1,
        comment: "结构清晰",
      },
    ],
    summary: "Qoder 获胜",
    rawText: "",
  },
  contestants: [
    {
      slot: 0,
      agentType: "qoder",
      label: null,
      status: "done",
      durationMs: 1200,
      usage: {
        inputTokens: 0,
        outputTokens: 0,
        turnCount: 3,
        tokensReported: false,
      },
      diff: "+const snake = true",
    },
  ],
} as PkRound

describe("buildPkReportHtml", () => {
  it("ignores the worktree .git control file when selecting a runnable HTML artifact", () => {
    expect(pickRunnableHtmlPath([".git", "index.html"])).toBe("index.html")
    expect(pickRunnableHtmlPath([".git\\config", "index.html"])).toBe(
      "index.html"
    )
    expect(reportableArtifactPaths([".git", "index.html"])).toEqual([
      "index.html",
    ])
  })

  it("recognizes a conventional HTML entrypoint in a multi-file web project", () => {
    expect(
      pickRunnableHtmlPath([
        ".git",
        "README.md",
        "index.html",
        "assets/game.js",
        "assets/game.css",
      ])
    ).toBe("index.html")
    expect(
      pickRunnableHtmlPath([
        "package.json",
        "src/main.ts",
        "dist/index.html",
        "dist/assets/app.js",
      ])
    ).toBe("dist/index.html")
  })

  it("builds a self-contained localized battle report without inventing tokens", () => {
    const html = buildPkReportHtml(
      round,
      { "0": [{ path: "index.html" }] },
      "zh-CN"
    )

    expect(html).toContain("智能体 PK 战报")
    expect(html).toContain("Qoder 获胜")
    expect(html).toContain("index.html")
    expect(html).toContain("未提供")
    expect(html).not.toContain(">0</td>")
    expect(html).not.toContain("https://")
  })

  it("localizes reports for every supported UI language", () => {
    const expectations = [
      ["zh-TW", "智能體 PK 戰報"],
      ["ja", "エージェントPK戦報"],
      ["ko", "에이전트 PK 결과 보고서"],
      ["es", "Informe de batalla PK de agentes"],
      ["de", "Agenten-PK-Kampfbericht"],
      ["fr", "Rapport de bataille PK des agents"],
      ["pt", "Relatório de batalha PK de agentes"],
      ["ar", "تقرير منافسة الوكلاء"],
    ] as const

    for (const [locale, title] of expectations) {
      expect(buildPkReportHtml(round, {}, locale)).toContain(title)
    }
    expect(buildPkReportHtml(round, {}, "ar")).toContain(
      '<html lang="ar" dir="rtl">'
    )
  })

  it("embeds a single HTML artifact as a sandboxed runnable preview", () => {
    const contentBase64 = btoa("<h1>Playable</h1><script>game()</script>")
    const html = buildPkReportHtml(
      round,
      { "0": [{ path: "index.html", contentBase64 }] },
      "en"
    )

    expect(html).toContain("data-showcase")
    expect(html).toContain("data-artifact-trigger")
    expect(html).toContain('sandbox="allow-scripts allow-pointer-lock"')
    expect(html).toContain("data-open-artifact")
    expect(html).toContain("Open and run")
    expect(html.indexOf("data-showcase")).toBeLessThan(html.indexOf("Results"))
    expect(html).not.toContain("+const snake = true")
    expect(html).not.toContain("<h1>Playable</h1>")

    const dom = new JSDOM(html)
    const embedded = dom.window.document
      .querySelector<HTMLElement>("[data-artifact-html]")
      ?.getAttribute("data-artifact-html")
    expect(decodePkReportArtifact(embedded ?? "")).toContain(
      "data-codeg-storage-compat"
    )
  })

  it("injects compatibility after the doctype and only once", () => {
    const artifact = "<!doctype html><script>game()</script>"
    const prepared = preparePkReportArtifactHtml(artifact)

    expect(prepared).toMatch(
      /^<!doctype html><script data-codeg-storage-compat>/
    )
    expect(preparePkReportArtifactHtml(prepared)).toBe(prepared)
  })

  it("round-trips UTF-8 artifact content through report base64", () => {
    const artifact = "<!doctype html><title>中文作品 🐍</title>"
    expect(decodePkReportArtifact(encodePkReportArtifact(artifact))).toBe(
      artifact
    )
  })

  it("keeps scores attached to repeated-agent contestant slots", () => {
    const repeated = {
      ...round,
      judgeResult: {
        scores: [
          {
            slot: 0,
            agentType: "qoder",
            score: 91,
            rank: 1,
            comment: "first",
          },
          {
            slot: 1,
            agentType: "qoder",
            score: 42,
            rank: 2,
            comment: "second",
          },
        ],
        summary: "first wins",
        rawText: "",
      },
      contestants: [
        { ...round.contestants[0], slot: 0, label: "Model A" },
        { ...round.contestants[0], slot: 1, label: "Model B" },
      ],
    } as PkRound

    const html = buildPkReportHtml(repeated, {}, "en")

    expect(html).toMatch(
      /Model A<\/small><\/td>\s*<td><strong class="score">91/
    )
    expect(html).toMatch(
      /Model B<\/small><\/td>\s*<td><strong class="score">42/
    )
  })

  it("runs the first embedded entry immediately and switches entries on click", () => {
    const twoEntries = {
      ...round,
      contestants: [
        { ...round.contestants[0], slot: 0, label: "First" },
        { ...round.contestants[0], slot: 1, label: "Second" },
      ],
    } as PkRound
    const html = buildPkReportHtml(
      twoEntries,
      {
        "0": [
          {
            path: "index.html",
            contentBase64: btoa("<h1>First entry</h1>"),
          },
        ],
        "1": [
          {
            path: "game.html",
            contentBase64: btoa("<h1>Second entry</h1>"),
          },
        ],
      },
      "en"
    )
    const dom = new JSDOM(html, {
      runScripts: "dangerously",
      beforeParse(window) {
        Object.defineProperty(window, "TextDecoder", { value: TextDecoder })
      },
    })
    const frame = dom.window.document.querySelector("iframe")
    const triggers = dom.window.document.querySelectorAll<HTMLElement>(
      "[data-artifact-trigger]"
    )

    expect(frame?.srcdoc).toContain("First entry")
    triggers[1].click()
    expect(frame?.srcdoc).toContain("Second entry")
    expect(triggers[0].getAttribute("aria-pressed")).toBe("false")
    expect(triggers[1].getAttribute("aria-pressed")).toBe("true")
  })

  it("keeps Web Storage-dependent entries runnable inside the opaque-origin sandbox", () => {
    const artifact = `<!doctype html><html><body><p id="status">booting</p><script>
      localStorage.setItem("high-score", "7")
      document.querySelector("#status").textContent = localStorage.getItem("high-score")
    </script></body></html>`
    const report = buildPkReportHtml(
      round,
      {
        "0": [
          {
            path: "index.html",
            contentBase64: btoa(artifact),
          },
        ],
      },
      "en"
    )
    const reportDom = new JSDOM(report, {
      runScripts: "dangerously",
      beforeParse(window) {
        Object.defineProperty(window, "TextDecoder", { value: TextDecoder })
      },
    })
    const srcdoc = reportDom.window.document.querySelector("iframe")?.srcdoc

    expect(srcdoc).toBeTruthy()
    const artifactDom = new JSDOM(srcdoc, { runScripts: "dangerously" })
    expect(
      artifactDom.window.document.querySelector("#status")?.textContent
    ).toBe("7")
  })
})
