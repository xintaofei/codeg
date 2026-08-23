import { mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

import { afterEach, describe, expect, it } from "vitest"

import {
  parseWindowsListenerPid,
  parseWindowsProfileProcessRoots,
  waitForDevToolsPort,
} from "../chrome-process.js"

const tempRoots: string[] = []

afterEach(async () => {
  await Promise.all(
    tempRoots
      .splice(0)
      .map((path) => rm(path, { recursive: true, force: true }))
  )
})

describe("waitForDevToolsPort", () => {
  it("accepts a DevTools port written after the launcher hands off and exits", async () => {
    const root = await mkdtemp(join(tmpdir(), "codeg-chrome-process-"))
    tempRoots.push(root)
    const portFile = join(root, "DevToolsActivePort")
    const writePort = new Promise<void>((resolveWrite, rejectWrite) => {
      setTimeout(() => {
        writeFile(portFile, "59774\n/devtools/browser/test\n", "utf8").then(
          () => resolveWrite(),
          rejectWrite
        )
      }, 20)
    })

    await expect(
      waitForDevToolsPort(
        portFile,
        500,
        { exitCode: 0 },
        {
          handoffGraceMs: 200,
          pollIntervalMs: 5,
        }
      )
    ).resolves.toBe(59_774)
    await writePort
  })

  it("still rejects an exited launcher that never exposes DevTools", async () => {
    const root = await mkdtemp(join(tmpdir(), "codeg-chrome-process-"))
    tempRoots.push(root)

    await expect(
      waitForDevToolsPort(
        join(root, "missing"),
        500,
        { exitCode: 1 },
        {
          handoffGraceMs: 20,
          pollIntervalMs: 5,
        }
      )
    ).rejects.toMatchObject({ code: "RUNTIME_START_FAILED" })
  })
})

describe("parseWindowsListenerPid", () => {
  it("returns the PID owning the exact loopback DevTools port", () => {
    const output = [
      "  TCP    127.0.0.1:9774         0.0.0.0:0              LISTENING       111",
      "  TCP    127.0.0.1:59774        0.0.0.0:0              LISTENING       64288",
      "  TCP    127.0.0.1:597740       0.0.0.0:0              LISTENING       222",
    ].join("\r\n")

    expect(parseWindowsListenerPid(output, 59_774)).toBe(64_288)
    expect(parseWindowsListenerPid(output, 59_775)).toBeNull()
  })
})

describe("parseWindowsProfileProcessRoots", () => {
  it("returns only roots from the exact-profile process tree", () => {
    const output = [
      "64288,65864",
      "66200,64288",
      "35260,64288",
      "27492,64288",
      "70000,1234",
      "not,a-process",
    ].join("\r\n")

    expect(parseWindowsProfileProcessRoots(output)).toEqual([64_288, 70_000])
  })
})
