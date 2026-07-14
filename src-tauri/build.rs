fn main() {
    #[cfg(feature = "tauri-runtime")]
    {
        ensure_sidecar_placeholders();
        tauri_build::build();
    }
}

/// Tauri's bundler validates that every `bundle.externalBin` path resolves
/// to an existing file at build.rs time. Real sidecars (`codeg-mcp`,
/// `codeg-tsnet`) are produced by `pnpm tauri:prepare-sidecars` (invoked from
/// `beforeBuildCommand` / `beforeDevCommand` and the CI release matrix) —
/// but plain `cargo check --features tauri-runtime` doesn't go through that
/// path, so without a backstop every contributor would hit
/// `resource path ... doesn't exist` on first compile.
///
/// We write a zero-byte placeholder when a sidecar is missing so
/// `cargo check` / clippy / rust-analyzer succeed. Production paths
/// overwrite the placeholder with the real binary before Tauri bundles it:
///   * `pnpm tauri build`  → `beforeBuildCommand` → `prepare-sidecars.mjs`
///   * release.yml         → explicit "Stage sidecars for Tauri bundle" step
///   * `pnpm tauri dev`    → `beforeDevCommand` → `prepare-sidecars.mjs`
///
/// If you ever bypass those wrappers (e.g. invoking the Tauri CLI directly
/// without beforeBuildCommand) you'd ship the placeholder, so emit a
/// cargo:warning that surfaces in any compile log to make that loud.
#[cfg(feature = "tauri-runtime")]
fn ensure_sidecar_placeholders() {
    use std::fs;
    use std::path::PathBuf;

    let triple = std::env::var("TARGET").unwrap_or_default();
    if triple.is_empty() {
        return;
    }
    let ext = if triple.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let dir = PathBuf::from("binaries");

    for name in ["codeg-mcp", "codeg-tsnet"] {
        let path = dir.join(format!("{name}-{triple}{ext}"));
        println!("cargo:rerun-if-changed={}", path.display());

        let needs_placeholder = match fs::metadata(&path) {
            Ok(meta) => meta.len() == 0,
            Err(_) => true,
        };

        if needs_placeholder {
            if let Err(e) = fs::create_dir_all(&dir) {
                panic!("failed to create {}: {e}", dir.display());
            }
            if let Err(e) = fs::write(&path, b"") {
                panic!(
                    "failed to write sidecar placeholder {}: {e}",
                    path.display()
                );
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o755));
            }
            println!(
                "cargo:warning={name} sidecar missing at {}; wrote 0-byte placeholder. \
                 Run `pnpm tauri:prepare-sidecars` before `tauri build` to ship a working binary.",
                path.display()
            );
        }
    }
}
