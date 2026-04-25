import { spawnSync } from "node:child_process"

function run(cmd, args, options = {}) {
  const result = spawnSync(cmd, args, {
    stdio: "inherit",
    shell: process.platform === "win32",
    ...options,
  })
  if (result.status !== 0) {
    process.exit(result.status ?? 1)
  }
}

run("pnpm", ["build"])
run(
  "cargo",
  ["build", "--release", "--bin", "codeg-server", "--no-default-features"],
  {
    cwd: "src-tauri",
  }
)
