"use client"

import { useState } from "react"
import type { AcceptState, IntentEnvelope, Op } from "@/lib/types"
import { SemanticResultView } from "./SemanticResultView"

export function SemanticComposer({
  onSubmit,
  workingDir = "",
}: {
  onSubmit: (e: IntentEnvelope) => void
  workingDir?: string
}) {
  const [intent, setIntent] = useState("")
  const [why, setWhy] = useState("")
  const [opsText, setOpsText] = useState("")
  const [accept, setAccept] = useState<AcceptState>("pending")
  const [result, setResult] = useState<string | null>(null)
  const [raw, setRaw] = useState<string | null>(null)

  // MVP: each non-empty line becomes a shell op.
  function parseOps(text: string): Op[] {
    return text
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line.length > 0)
      .map((line) => ({ tool: "shell", params: { cmd: line } }))
  }

  async function run() {
    const ops = parseOps(opsText)
    // Build the envelope locally first so the caller always receives a fully
    // formed payload, even before/without the network round-trip.
    const env: IntentEnvelope = {
      intent,
      why,
      ops,
      accept: "pending",
      result: null,
      raw: null,
    }
    onSubmit(env)
    try {
      const res = await fetch("/semantic_submit", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          req: {
            intent,
            why,
            ops,
            working_dir: workingDir,
            agent_type: "ClaudeCode",
          },
        }),
      })
      const serverEnv: IntentEnvelope = await res.json()
      setAccept(serverEnv.accept)
      setResult(serverEnv.result)
      setRaw(serverEnv.raw)
    } catch {
      // Keep the locally-built envelope as the visible result on network failure.
    }
  }

  return (
    <div className="semantic-composer">
      <label>
        intent
        <input value={intent} onChange={(e) => setIntent(e.target.value)} />
      </label>
      <label>
        why
        <textarea value={why} onChange={(e) => setWhy(e.target.value)} />
      </label>
      <label>
        ops
        <textarea
          value={opsText}
          onChange={(e) => setOpsText(e.target.value)}
        />
      </label>
      <button type="button" onClick={() => void run()}>
        run
      </button>
      <SemanticResultView accept={accept} result={result} raw={raw} />
    </div>
  )
}
