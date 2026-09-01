import { render, screen, cleanup, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import { AgentConfigSection } from "./agent-config-section"
import type { AgentOptionsSnapshot } from "@/lib/types"

const SNAPSHOT: AgentOptionsSnapshot = {
  modes: null,
  available_commands: [],
  config_options: [
    {
      id: "model",
      name: "Model",
      description: "Which model answers",
      category: null,
      kind: {
        type: "select",
        current_value: "opus",
        options: [
          { value: "opus", name: "Opus", description: "The deep one" },
          { value: "haiku", name: "Haiku", description: "The quick one" },
        ],
        groups: [],
      },
    },
  ],
}

function renderSection(layout: "inline" | "stacked") {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <AgentConfigSection
        snapshot={SNAPSHOT}
        loading={false}
        error={null}
        onReload={vi.fn()}
        modeId={null}
        configValues={{}}
        onModeChange={vi.fn()}
        onConfigChange={vi.fn()}
        layout={layout}
      />
    </NextIntlClientProvider>
  )
}

// The automation editor's and task editor's composer chips carry the same
// subtitles the chat composer's selectors do: a blurb per option row, and the
// option's own blurb under the chip's hover hint.
describe("AgentConfigSection subtitles", () => {
  afterEach(() => cleanup())

  it("gives every option row its description", async () => {
    const user = userEvent.setup()
    renderSection("inline")

    await user.click(screen.getByRole("combobox"))
    const list = await screen.findByRole("listbox")
    expect(within(list).getByText("The deep one")).toBeInTheDocument()
    expect(within(list).getByText("The quick one")).toBeInTheDocument()
  })

  it("keeps the description off the closed chip", () => {
    // Radix mirrors an item's `ItemText` into the trigger to render
    // `SelectValue`. A description rendered inside it would print on the chip
    // too and double the composer bar's height — hence it lives outside.
    renderSection("inline")

    const trigger = screen.getByRole("combobox")
    expect(trigger).toHaveTextContent("Opus")
    expect(trigger).not.toHaveTextContent("The deep one")
  })

  it("hangs the option's own blurb under the inline chip's hover hint", async () => {
    const user = userEvent.setup()
    renderSection("inline")

    const trigger = screen.getByRole("combobox")
    expect(trigger).not.toHaveAttribute("title")

    await user.hover(trigger)
    const tip = await screen.findByRole("tooltip")
    expect(tip).toHaveTextContent("Model")
    expect(tip).toHaveTextContent("Which model answers")
  })

  it("shows no hover hint in the stacked layout, which labels itself", async () => {
    const user = userEvent.setup()
    renderSection("stacked")

    // The visible <label> carries the name here, so the chip-only hint would
    // just be a second copy of it.
    expect(screen.getByText("Model")).toBeInTheDocument()
    await user.hover(screen.getByRole("combobox"))
    expect(screen.queryByRole("tooltip")).toBeNull()
  })
})
