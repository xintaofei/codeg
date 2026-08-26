import { render, screen, fireEvent } from "@testing-library/react"
import { SemanticComposer } from "./SemanticComposer"

test("submits intent/why and never includes raw in payload", () => {
  const onsubmit = vi.fn()
  render(<SemanticComposer onSubmit={onsubmit} />)
  fireEvent.change(screen.getByLabelText(/intent/i), {
    target: { value: "list files" },
  })
  fireEvent.change(screen.getByLabelText(/why/i), {
    target: { value: "see layout" },
  })
  fireEvent.click(screen.getByRole("button", { name: /run/i }))
  const sent = onsubmit.mock.calls[0][0]
  expect(sent.intent).toBe("list files")
  expect(sent.raw).toBeNull()
})
