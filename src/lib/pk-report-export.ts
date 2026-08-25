import { saveTextFile, type ExportResult } from "@/lib/export-conversation"

export function savePkReportHtml(
  html: string,
  roundId: string
): Promise<ExportResult> {
  return saveTextFile({
    content: html,
    suggestedName: `codeg-pk-${roundId}.html`,
    mimeType: "text/html;charset=utf-8",
    filterName: "HTML battle report",
    ext: "html",
  })
}
