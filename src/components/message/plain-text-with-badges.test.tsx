import { render } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { PlainTextWithBadges } from "./plain-text-with-badges"

const badge = (c: HTMLElement, kind: string) =>
  c.querySelector(`[data-reference-badge][data-ref-type='${kind}']`)

describe("PlainTextWithBadges", () => {
  it("renders plain prose as text with no badge", () => {
    const { container } = render(<PlainTextWithBadges text="just some text" />)
    expect(container.textContent).toBe("just some text")
    expect(container.querySelector("[data-reference-badge]")).toBeNull()
  })

  it("renders Markdown syntax verbatim (no formatting elements)", () => {
    const { container } = render(
      <PlainTextWithBadges text={"# Heading\n**bold**\n- item"} />
    )
    expect(container.querySelector("h1")).toBeNull()
    expect(container.querySelector("strong")).toBeNull()
    expect(container.querySelector("li")).toBeNull()
    expect(container.textContent).toContain("# Heading")
    expect(container.textContent).toContain("**bold**")
    expect(container.textContent).toContain("- item")
  })

  it("renders each reference kind as its badge, in place", () => {
    const { container: file } = render(
      <PlainTextWithBadges text="edit [app.ts](file:///repo/app.ts) here" />
    )
    expect(badge(file, "file")).not.toBeNull()
    expect(file.textContent).toContain("edit")
    expect(file.textContent).toContain("here")

    const { container: agent } = render(
      <PlainTextWithBadges text="[@Codex](codeg://agent/codex)" />
    )
    expect(badge(agent, "agent")).not.toBeNull()

    const { container: session } = render(
      <PlainTextWithBadges text="[#42](codeg://session/42)" />
    )
    expect(badge(session, "session")).not.toBeNull()

    const { container: commit } = render(
      <PlainTextWithBadges text="[a1b2c3d](codeg://commit/%2Frepo@a1b2c3ddeadbeef)" />
    )
    expect(badge(commit, "commit")).not.toBeNull()
  })

  it("badges a bare /command token but not a path", () => {
    const { container: cmd } = render(
      <PlainTextWithBadges text="run /review please" />
    )
    expect(badge(cmd, "skill")).not.toBeNull()
    // The badge shows the bare name (no `/` prefix), matching the composer.
    expect(cmd.textContent).toContain("review")
    expect(cmd.textContent).not.toContain("/review")

    const { container: path } = render(
      <PlainTextWithBadges text="see /usr/bin for it" />
    )
    expect(badge(path, "skill")).toBeNull()
    expect(path.textContent).toContain("/usr/bin")
  })

  it("does NOT badge a non-reference http link", () => {
    const { container } = render(
      <PlainTextWithBadges text="[docs](https://example.com)" />
    )
    expect(container.querySelector("[data-reference-badge]")).toBeNull()
    expect(container.textContent).toBe("[docs](https://example.com)")
  })

  it("preserves newlines via pre-wrap", () => {
    const { container } = render(<PlainTextWithBadges text={"a\nb"} />)
    const root = container.firstElementChild as HTMLElement
    expect(root.className).toContain("whitespace-pre-wrap")
    expect(container.textContent).toBe("a\nb")
  })
})
