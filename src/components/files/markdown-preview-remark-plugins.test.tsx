import { render, waitFor } from "@testing-library/react"
import { Streamdown } from "streamdown"
import { describe, expect, it } from "vitest"
import { getMarkdownPreviewRemarkPlugins } from "./markdown-preview-remark-plugins"

function renderMarkdown(content: string, preserveLineBreaks: boolean) {
  return render(
    <Streamdown
      remarkPlugins={getMarkdownPreviewRemarkPlugins(preserveLineBreaks)}
    >
      {content}
    </Streamdown>
  )
}

describe("Markdown preview line break policy", () => {
  it("keeps CommonMark soft-break behavior when disabled", async () => {
    const { container } = renderMarkdown("first\nsecond", false)

    await waitFor(() => expect(container.querySelector("p")).not.toBeNull())
    expect(container.querySelector("br")).toBeNull()
    expect(container.querySelector("p")).toHaveTextContent("first second")
  })

  it("renders a soft break as a visible line break when enabled", async () => {
    const { container } = renderMarkdown("first\nsecond", true)

    await waitFor(() => expect(container.querySelector("br")).not.toBeNull())
    expect(container.querySelectorAll("br")).toHaveLength(1)
  })

  it("preserves existing Markdown block structures when enabled", async () => {
    const content = [
      "first  ",
      "second",
      "",
      "| A | B |",
      "| --- | --- |",
      "| 1 | 2 |",
      "",
      "```text",
      "code line 1",
      "code line 2",
      "```",
    ].join("\n")
    const { container } = renderMarkdown(content, true)

    await waitFor(() =>
      expect(container).toHaveTextContent("code line 1code line 2")
    )
    expect(container.querySelectorAll("p")).toHaveLength(1)
    expect(container.querySelectorAll("p br")).toHaveLength(1)
    expect(container.querySelectorAll("table")).toHaveLength(1)
    expect(container.querySelectorAll("code br")).toHaveLength(0)
  })
})
