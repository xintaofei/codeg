import type { CdpCallOptions } from "./cdp.js"
import { MAX_SCREENSHOT_BYTES } from "./contracts.js"
import { BrowserError } from "./errors.js"
import type { BrowserTab, PageSnapshot, SemanticNode } from "./runtime-types.js"

export interface CdpCallTransport {
  call<T = unknown>(
    method: string,
    params?: Record<string, unknown>,
    options?: CdpCallOptions
  ): Promise<T>
}

interface AxValue {
  value?: unknown
}

interface AxNode {
  backendDOMNodeId?: number
  ignored?: boolean
  role?: AxValue
  name?: AxValue
  value?: AxValue
  description?: AxValue
  properties?: Array<{ name: string; value?: AxValue }>
}

const INTERACTIVE_ROLES = new Set([
  "button",
  "checkbox",
  "combobox",
  "gridcell",
  "link",
  "listbox",
  "menuitem",
  "option",
  "radio",
  "searchbox",
  "slider",
  "spinbutton",
  "switch",
  "tab",
  "textbox",
  "treeitem",
])

export class CdpPageAutomation {
  private readonly refs = new Map<string, number>()

  constructor(
    private readonly transport: () => Promise<CdpCallTransport>,
    private readonly info: () => Promise<BrowserTab>
  ) {}

  clear(): void {
    this.refs.clear()
  }

  async snapshot(options: {
    interactive: boolean
    limit: number
    timeoutMs: number
  }): Promise<PageSnapshot> {
    const cdp = await this.transport()
    const result = await cdp.call<{ nodes: AxNode[] }>(
      "Accessibility.getFullAXTree",
      {},
      { timeoutMs: options.timeoutMs }
    )
    this.refs.clear()
    const nodes: SemanticNode[] = []
    for (const node of result.nodes) {
      if (node.ignored || !node.backendDOMNodeId) continue
      const role = stringValue(node.role)
      if (!role || (options.interactive && !INTERACTIVE_ROLES.has(role)))
        continue
      const name = stringValue(node.name)
      const value = stringValue(node.value)
      if (!options.interactive && !name && !value && role === "generic")
        continue
      const ref = `e${nodes.length + 1}`
      this.refs.set(ref, node.backendDOMNodeId)
      const semantic: SemanticNode = {
        ref,
        role: bounded(role, 128),
        name: bounded(name, 4_096),
      }
      if (value) semantic.value = bounded(value, 4_096)
      const description = stringValue(node.description)
      if (description) semantic.description = bounded(description, 4_096)
      const disabled = propertyBoolean(node, "disabled")
      if (disabled !== undefined) semantic.disabled = disabled
      const focused = propertyBoolean(node, "focused")
      if (focused !== undefined) semantic.focused = focused
      nodes.push(semantic)
      if (nodes.length >= options.limit) break
    }
    const info = await this.info()
    return { nodes, documentUrl: info.url, title: info.title }
  }

  async screenshot(options: {
    format: "png" | "jpeg"
    fullPage: boolean
    quality?: number
    timeoutMs: number
  }): Promise<{ data: string; mimeType: "image/png" | "image/jpeg" }> {
    const cdp = await this.transport()
    let clip: Record<string, number> | undefined
    if (options.fullPage) {
      const metrics = await cdp.call<{
        contentSize: { width: number; height: number }
      }>("Page.getLayoutMetrics", {}, { timeoutMs: options.timeoutMs })
      clip = {
        x: 0,
        y: 0,
        width: Math.max(1, Math.ceil(metrics.contentSize.width)),
        height: Math.max(1, Math.ceil(metrics.contentSize.height)),
        scale: 1,
      }
    }
    const result = await cdp.call<{ data: string }>(
      "Page.captureScreenshot",
      {
        format: options.format,
        ...(options.format === "jpeg" && options.quality
          ? { quality: options.quality }
          : {}),
        captureBeyondViewport: options.fullPage,
        fromSurface: true,
        ...(clip ? { clip } : {}),
      },
      { timeoutMs: options.timeoutMs }
    )
    if (Buffer.byteLength(result.data, "base64") > MAX_SCREENSHOT_BYTES) {
      throw new BrowserError(
        "RESULT_TOO_LARGE",
        "The screenshot exceeds the five megabyte result limit"
      )
    }
    return {
      data: result.data,
      mimeType: options.format === "jpeg" ? "image/jpeg" : "image/png",
    }
  }

  async click(ref: string, timeoutMs: number): Promise<void> {
    const rect = await this.callOnRef<{ x: number; y: number }>(
      ref,
      `function () {
        this.scrollIntoView({ block: "center", inline: "center" });
        const rect = this.getBoundingClientRect();
        return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
      }`,
      [],
      timeoutMs
    )
    const cdp = await this.transport()
    await cdp.call(
      "Input.dispatchMouseEvent",
      {
        type: "mousePressed",
        x: rect.x,
        y: rect.y,
        button: "left",
        clickCount: 1,
      },
      { timeoutMs }
    )
    await cdp.call(
      "Input.dispatchMouseEvent",
      {
        type: "mouseReleased",
        x: rect.x,
        y: rect.y,
        button: "left",
        clickCount: 1,
      },
      { timeoutMs }
    )
  }

  async type(
    ref: string,
    text: string,
    clear: boolean,
    timeoutMs: number
  ): Promise<void> {
    await this.callOnRef(
      ref,
      `function (clear) {
        this.scrollIntoView({ block: "center", inline: "center" });
        this.focus();
        if (clear) {
          const prototype = this instanceof HTMLTextAreaElement
            ? HTMLTextAreaElement.prototype
            : HTMLInputElement.prototype;
          const setter = Object.getOwnPropertyDescriptor(prototype, "value")?.set;
          if (setter) setter.call(this, ""); else this.value = "";
          this.dispatchEvent(new Event("input", { bubbles: true }));
        }
        return true;
      }`,
      [{ value: clear }],
      timeoutMs
    )
    await (
      await this.transport()
    ).call("Input.insertText", { text }, { timeoutMs })
  }

  async press(key: string, timeoutMs: number): Promise<void> {
    const details = keyDetails(key)
    const cdp = await this.transport()
    await cdp.call(
      "Input.dispatchKeyEvent",
      { type: "keyDown", ...details },
      { timeoutMs }
    )
    await cdp.call(
      "Input.dispatchKeyEvent",
      { type: "keyUp", ...details, text: undefined },
      { timeoutMs }
    )
  }

  async scroll(
    ref: string | undefined,
    deltaX: number,
    deltaY: number,
    timeoutMs: number
  ): Promise<void> {
    if (ref) {
      await this.callOnRef(
        ref,
        "function (x, y) { this.scrollBy(x, y); return true; }",
        [{ value: deltaX }, { value: deltaY }],
        timeoutMs
      )
      return
    }
    await (
      await this.transport()
    ).call(
      "Runtime.evaluate",
      {
        expression: `window.scrollBy(${JSON.stringify(deltaX)}, ${JSON.stringify(deltaY)})`,
        returnByValue: true,
      },
      { timeoutMs }
    )
  }

  async wait(options: {
    milliseconds?: number
    text?: string
    urlIncludes?: string
    timeoutMs: number
  }): Promise<void> {
    if (options.milliseconds !== undefined) {
      await sleep(Math.min(options.milliseconds, options.timeoutMs))
      if (!options.text && !options.urlIncludes) return
    }
    const deadline = Date.now() + options.timeoutMs
    while (Date.now() <= deadline) {
      const info = await this.info()
      const urlMatches =
        options.urlIncludes === undefined ||
        info.url.includes(options.urlIncludes)
      let textMatches = options.text === undefined
      if (options.text !== undefined) {
        const snapshot = await this.snapshot({
          interactive: false,
          limit: 1_000,
          timeoutMs: Math.max(100, deadline - Date.now()),
        })
        textMatches = snapshot.nodes.some(
          (node) =>
            node.name.includes(options.text!) ||
            node.value?.includes(options.text!)
        )
      }
      if (urlMatches && textMatches) return
      await sleep(100)
    }
    throw new BrowserError("TIMEOUT", "Browser wait condition timed out", {
      retryable: true,
      recovery: "retry",
    })
  }

  async waitForReadyState(timeoutMs: number): Promise<void> {
    const deadline = Date.now() + timeoutMs
    const cdp = await this.transport()
    while (Date.now() <= deadline) {
      const result = await cdp.call<{
        result?: { value?: string }
      }>(
        "Runtime.evaluate",
        { expression: "document.readyState", returnByValue: true },
        { timeoutMs: Math.max(100, deadline - Date.now()) }
      )
      if (result.result?.value === "complete") return
      await sleep(100)
    }
    throw new BrowserError("TIMEOUT", "Page navigation timed out", {
      retryable: true,
      recovery: "retry",
    })
  }

  private async callOnRef<T>(
    ref: string,
    functionDeclaration: string,
    args: Array<{ value: unknown }>,
    timeoutMs: number
  ): Promise<T> {
    const backendNodeId = this.refs.get(ref)
    if (!backendNodeId) {
      throw new BrowserError(
        "REF_NOT_FOUND",
        "The semantic ref is stale; take a new snapshot and retry",
        { retryable: true, recovery: "retry" }
      )
    }
    const cdp = await this.transport()
    const resolved = await cdp.call<{
      object?: { objectId?: string }
    }>("DOM.resolveNode", { backendNodeId }, { timeoutMs })
    const objectId = resolved.object?.objectId
    if (!objectId)
      throw new BrowserError("REF_NOT_FOUND", "The semantic ref is gone")
    try {
      const result = await cdp.call<{
        result?: { value?: T }
        exceptionDetails?: unknown
      }>(
        "Runtime.callFunctionOn",
        {
          objectId,
          functionDeclaration,
          arguments: args,
          returnByValue: true,
          awaitPromise: false,
        },
        { timeoutMs }
      )
      if (result.exceptionDetails || result.result?.value === undefined) {
        throw new BrowserError("INTERNAL_ERROR", "The browser action failed")
      }
      return result.result.value
    } finally {
      await cdp
        .call("Runtime.releaseObject", { objectId })
        .catch(() => undefined)
    }
  }
}

function stringValue(value: AxValue | undefined): string {
  return typeof value?.value === "string" ? value.value : ""
}

function propertyBoolean(node: AxNode, name: string): boolean | undefined {
  const value = node.properties?.find((property) => property.name === name)
    ?.value?.value
  return typeof value === "boolean" ? value : undefined
}

function bounded(value: string, maximum: number): string {
  return value.length <= maximum ? value : value.slice(0, maximum)
}

function keyDetails(key: string): Record<string, unknown> {
  const named: Record<string, { code: string; keyCode: number }> = {
    Control: { code: "ControlLeft", keyCode: 17 },
    Shift: { code: "ShiftLeft", keyCode: 16 },
    Alt: { code: "AltLeft", keyCode: 18 },
    Meta: { code: "MetaLeft", keyCode: 91 },
    Enter: { code: "Enter", keyCode: 13 },
    Tab: { code: "Tab", keyCode: 9 },
    Escape: { code: "Escape", keyCode: 27 },
    Backspace: { code: "Backspace", keyCode: 8 },
    Delete: { code: "Delete", keyCode: 46 },
    ArrowUp: { code: "ArrowUp", keyCode: 38 },
    ArrowDown: { code: "ArrowDown", keyCode: 40 },
    ArrowLeft: { code: "ArrowLeft", keyCode: 37 },
    ArrowRight: { code: "ArrowRight", keyCode: 39 },
    Home: { code: "Home", keyCode: 36 },
    End: { code: "End", keyCode: 35 },
    PageUp: { code: "PageUp", keyCode: 33 },
    PageDown: { code: "PageDown", keyCode: 34 },
  }
  const match = named[key]
  if (match) {
    return {
      key,
      code: match.code,
      windowsVirtualKeyCode: match.keyCode,
      nativeVirtualKeyCode: match.keyCode,
    }
  }
  const character = [...key][0] ?? ""
  return {
    key: character,
    code: character.length === 1 ? `Key${character.toUpperCase()}` : character,
    text: character,
    windowsVirtualKeyCode: character.toUpperCase().charCodeAt(0),
    nativeVirtualKeyCode: character.toUpperCase().charCodeAt(0),
  }
}

function sleep(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds))
}
