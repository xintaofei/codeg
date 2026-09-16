"use client"

import { useState } from "react"
import type { AcceptState } from "@/lib/types"

export function SemanticResultView({
  accept,
  result,
  raw,
}: {
  accept: AcceptState
  result: string | null
  raw: string | null
}) {
  const [showRaw, setShowRaw] = useState(false)
  return (
    <div className="semantic-result">
      <div className="accept">{accept}</div>
      {result != null && <div className="result">{result}</div>}
      {raw != null && (
        <button type="button" onClick={() => setShowRaw((s) => !s)}>
          {showRaw ? "hide raw" : "show raw"}
        </button>
      )}
      {showRaw && raw != null && <pre className="raw">{raw}</pre>}
    </div>
  )
}
