// @vitest-environment node
import { afterEach, describe, expect, it } from "vitest"
import {
  packageBin,
  commandFor,
} from "../../src-tauri/src/acp/managed-runtime.mjs"
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs"
import { delimiter, dirname, join, resolve } from "node:path"
import { spawnSync } from "node:child_process"

const bootstrap = resolve("src-tauri/src/acp/managed-runtime.mjs")
const scratch: string[] = []
function fixture(
  name: string,
  version: string,
  command = "agent",
  script = "process.stdout.write(JSON.stringify({args:process.argv.slice(2),cwd:process.cwd(),runtime:process.env.CLAUDE_CODE_EXECUTABLE,account:process.env.CLAUDE_CONFIG_DIR}));"
) {
  const root = mkdtempSync(join(process.cwd(), ".tmp-managed-runtime-"))
  scratch.push(root)
  const modules = join(root, "node_modules")
  const packageRoot = join(modules, name)
  const bin = join(modules, ".bin")
  mkdirSync(packageRoot, { recursive: true })
  mkdirSync(bin)
  const entry = join(packageRoot, "entry.cjs")
  writeFileSync(
    join(packageRoot, "package.json"),
    JSON.stringify({ name, version, bin: { [command]: "entry.cjs" } })
  )
  writeFileSync(entry, script)
  return { root, packageRoot, entry, bin }
}

afterEach(() => {
  for (const dir of scratch.splice(0)) {
    if (dirname(dir) !== process.cwd())
      throw new Error("unexpected fixture location")
    rmSync(dir, { recursive: true, force: true })
  }
})

describe("managed runtime launcher", () => {
  it("selects the requested release even if another version appears first on PATH", () => {
    const old = fixture("@example/agent", "1.0.0")
    const current = fixture("@example/agent", "1.1.0")
    expect(
      packageBin(
        "@example/agent",
        "agent",
        "1.1.0",
        [old.bin, current.bin].join(delimiter)
      )
    ).toBe(current.entry)
    expect(() =>
      packageBin("@example/agent", "agent", "2.0.0", old.bin)
    ).toThrow()
  })

  it("refuses an executable that escapes the package directory", () => {
    const pkg = fixture("agent", "1.0.0")
    writeFileSync(join(pkg.root, "outside.cjs"), "")
    writeFileSync(
      join(pkg.packageRoot, "package.json"),
      JSON.stringify({
        name: "agent",
        version: "1.0.0",
        bin: "../../outside.cjs",
      })
    )
    expect(() => packageBin("agent", "agent", "1.0.0", pkg.bin)).toThrow()
  })

  it("runs JS under Node and keeps arguments as arguments", () => {
    expect(commandFor("/agent/cli.js", ["a & b", "two words"])).toEqual([
      process.execPath,
      ["/agent/cli.js", "a & b", "two words"],
    ])
    expect(commandFor("/agent/cli.exe", ["--acp"])).toEqual([
      "/agent/cli.exe",
      ["--acp"],
    ])
  })

  it("updates the runtime independently of the adapter and preserves account isolation and argument boundaries", () => {
    const adapter = fixture("@example/adapter", "1.0.0")
    const old = fixture("@example/runtime", "2.0.0", "runtime")
    const latest = fixture("@example/runtime", "2.1.0", "runtime")
    const result = spawnSync(
      process.execPath,
      [
        bootstrap,
        "@example/adapter",
        "agent",
        "1.0.0",
        "@example/runtime",
        "runtime",
        "CLAUDE_CODE_EXECUTABLE",
        "2.1.0",
        "a & b",
        "two words",
      ],
      {
        cwd: adapter.root,
        env: {
          ...process.env,
          PATH: [adapter.bin, old.bin, latest.bin, process.env.PATH].join(
            delimiter
          ),
          CLAUDE_CODE_EXECUTABLE: "",
          CLAUDE_CONFIG_DIR: "/profiles/second",
        },
        encoding: "utf8",
        windowsHide: true,
        timeout: 5000,
      }
    )
    expect(result.status, result.stderr).toBe(0)
    expect(JSON.parse(result.stdout)).toEqual({
      args: ["a & b", "two words"],
      cwd: adapter.root,
      runtime: latest.entry,
      account: "/profiles/second",
    })
  })

  it("honors an explicit runtime executable instead of replacing it", () => {
    const adapter = fixture("adapter", "1.0.0")
    const result = spawnSync(
      process.execPath,
      [
        bootstrap,
        "adapter",
        "agent",
        "1.0.0",
        "uninstalled-runtime",
        "runtime",
        "CLAUDE_CODE_EXECUTABLE",
        "2.0.0",
      ],
      {
        env: {
          ...process.env,
          PATH: adapter.bin + delimiter + process.env.PATH,
          CLAUDE_CODE_EXECUTABLE: "/custom/runtime",
          CLAUDE_CONFIG_DIR: "/profiles/first",
        },
        encoding: "utf8",
        windowsHide: true,
        timeout: 5000,
      }
    )
    expect(result.status, result.stderr).toBe(0)
    expect(JSON.parse(result.stdout).runtime).toBe("/custom/runtime")
  })

  it("rejects a failed runtime probe without exposing its output", () => {
    const adapter = fixture("adapter", "1.0.0", "agent", "process.exit(0)")
    const runtime = fixture(
      "runtime",
      "2.0.0",
      "runtime",
      "process.stdout.write('private probe output');process.exit(2)"
    )
    const result = spawnSync(
      process.execPath,
      [
        bootstrap,
        "adapter",
        "agent",
        "1.0.0",
        "runtime",
        "runtime",
        "CLAUDE_CODE_EXECUTABLE",
        "2.0.0",
        "--codeg-check",
      ],
      {
        env: {
          ...process.env,
          PATH: [adapter.bin, runtime.bin, process.env.PATH].join(delimiter),
          CLAUDE_CODE_EXECUTABLE: "",
        },
        encoding: "utf8",
        windowsHide: true,
        timeout: 5000,
      }
    )
    expect(result.status).toBe(1)
    expect(result.stdout).toBe("")
    expect(result.stderr).not.toContain("private probe output")
  })
})
