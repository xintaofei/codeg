import remarkBreaks from "remark-breaks"
import { defaultRemarkPlugins } from "streamdown"

const standardRemarkPlugins = Object.values(defaultRemarkPlugins)
const preserveLineBreaksRemarkPlugins = [...standardRemarkPlugins, remarkBreaks]

export function getMarkdownPreviewRemarkPlugins(preserveLineBreaks: boolean) {
  return preserveLineBreaks
    ? preserveLineBreaksRemarkPlugins
    : standardRemarkPlugins
}
