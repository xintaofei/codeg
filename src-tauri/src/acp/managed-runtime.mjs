// Executed inside npm's exact-version package cache. No models, credentials,
// global installations, user settings or account directories are written here.
import fs from "node:fs"
import path from "node:path"
import { spawn, spawnSync } from "node:child_process"
import { pathToFileURL } from "node:url"

function packageEntry(
  packageName,
  command,
  version,
  searchPath = process.env.PATH || ""
) {
  for (const dir of searchPath.split(path.delimiter)) {
    if (path.basename(dir) !== ".bin") continue
    const root = path.resolve(dir, "..", packageName)
    const manifest = path.join(root, "package.json")
    if (!fs.existsSync(manifest)) continue
    const pkg = JSON.parse(fs.readFileSync(manifest, "utf8"))
    if (pkg.name !== packageName || pkg.version !== version) continue
    const relative = typeof pkg.bin === "string" ? pkg.bin : pkg.bin?.[command]
    if (typeof relative !== "string") continue
    const entry = path.resolve(root, relative)
    if (!entry.startsWith(root + path.sep) || !fs.existsSync(entry)) continue
    return { entry, binDir: dir }
  }
  throw new Error("Prepared package has no executable entry")
}

function packageBin(...args) {
  return packageEntry(...args).entry
}

function commandFor(entry, args) {
  return /\.[cm]?js$/i.test(entry)
    ? [process.execPath, [entry, ...args]]
    : [entry, args]
}

function main(argv) {
  const [
    adapterPackage,
    adapterCommand,
    adapterVersion,
    runtimePackage,
    runtimeCommand,
    runtimeKey,
    runtimeVersion,
    ...args
  ] = argv
  const adapter = packageBin(adapterPackage, adapterCommand, adapterVersion)
  const env = { ...process.env }
  let runtime
  if (runtimeKey && !env[runtimeKey]?.trim()) {
    const resolved = packageEntry(
      runtimePackage,
      runtimeCommand,
      runtimeVersion
    )
    runtime = resolved.entry
    // Codex's supported override is an executable command. On Windows the npm
    // shim runs its JS entry under Node; passing the .js path directly invokes
    // Windows Script Host instead. Claude's package exposes its native binary.
    if (
      runtimeKey !== "CLAUDE_CODE_EXECUTABLE" &&
      process.platform === "win32"
    ) {
      const shim = path.join(resolved.binDir, runtimeCommand + ".cmd")
      if (!fs.existsSync(shim))
        throw new Error("Prepared runtime command is missing")
      runtime = shim
    }
    env[runtimeKey] = runtime
    // Codeg owns this prepared copy. Keep its own updater from touching a
    // global/native installation; model discovery remains enabled.
    if (runtimeKey === "CLAUDE_CODE_EXECUTABLE") env.DISABLE_AUTOUPDATER = "1"
  }
  if (args[0] === "--codeg-check") {
    for (const entry of [adapter, runtime].filter(Boolean)) {
      // Version probes use direct executable/Node invocation, never a shell.
      const actual = entry.endsWith(".cmd")
        ? packageBin(runtimePackage, runtimeCommand, runtimeVersion)
        : entry
      const [program, probeArgs] = commandFor(actual, ["--version"])
      const result = spawnSync(program, probeArgs, {
        env,
        stdio: "ignore",
        windowsHide: true,
        timeout: 15000,
      })
      if (result.error || result.status !== 0)
        throw new Error("Prepared runtime failed its version probe")
    }
    return
  }
  const [program, childArgs] = commandFor(adapter, args)
  const child = spawn(program, childArgs, {
    env,
    stdio: "inherit",
    windowsHide: true,
  })
  child.on("error", () => {
    process.exitCode = 1
  })
  child.on("exit", (code) => {
    process.exitCode = code ?? 1
  })
  for (const signal of ["SIGTERM", "SIGINT"])
    process.on(signal, () => child.kill(signal))
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href
) {
  try {
    main(process.argv.slice(2))
  } catch {
    process.stderr.write("Could not start the prepared agent runtime.\n")
    process.exitCode = 1
  }
}
export { packageBin, commandFor, main }
