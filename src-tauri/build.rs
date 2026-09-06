fn main() {
    #[cfg(feature = "tauri-runtime")]
    {
        ensure_sidecar_placeholder();
        tauri_build::build();
        // tauri-build embeds the comctl32-v6 SxS manifest (resource.lib) only
        // into the binaries (`cargo:rustc-link-arg-bins`). Test binaries link
        // the same lib, whose tauri code imports TaskDialogIndirect & friends
        // — entry points that exist only in comctl32 v6. Without the manifest
        // a test exe loads comctl32 v5 and dies at load with
        // STATUS_ENTRYPOINT_NOT_FOUND (0xC0000139).
        //
        // Linking resource.lib into the tests cannot happen from here:
        // `cargo:rustc-link-arg-tests` skips the lib's unit-test harness
        // (it only reaches tests/*.rs), and the unspecific
        // `cargo:rustc-link-arg` reaches the bins too, where the duplicate
        // resources fail the link with CVT1100. The embedding therefore lives
        // in lib.rs as a `#[cfg(test)] #[link(...)]`, which is scoped to the
        // test compilations alone; this directive only makes resource.lib
        // findable on the library search path.
        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is always set");
        println!("cargo:rustc-link-search=native={out_dir}");
        // Integration tests (tests/*.rs) link the lib WITHOUT cfg(test), so
        // the #[cfg(test)] #[link] in lib.rs is inert for them — their exes
        // would load comctl32 v5 and die at load with
        // STATUS_ENTRYPOINT_NOT_FOUND on TaskDialogIndirect. This directive
        // reaches exactly those targets, complementing the lib-side
        // attribute without ever hitting the same compilation twice.
        let resource = std::path::Path::new(&out_dir).join("resource.lib");
        println!("cargo:rustc-link-arg-tests={}", resource.display());
    }
}

/// Tauri's bundler validates that every `bundle.externalBin` path resolves
/// to an existing file at build.rs time. The real `codeg-mcp` sidecar is
/// produced by `pnpm tauri:prepare-sidecars` (invoked from
/// `beforeBuildCommand` / `beforeDevCommand` and the CI release matrix) —
/// but plain `cargo check --features tauri-runtime` doesn't go through that
/// path, so without a backstop every contributor would hit
/// `resource path ... doesn't exist` on first compile.
///
/// We write a zero-byte placeholder when the sidecar is missing so
/// `cargo check` / clippy / rust-analyzer succeed. Production paths
/// overwrite the placeholder with the real binary before Tauri bundles it:
///   * `pnpm tauri build`  → `beforeBuildCommand` → `prepare-sidecars.mjs`
///   * release.yml         → explicit "Stage codeg-mcp sidecar" step
///   * `pnpm tauri dev`    → `beforeDevCommand` → `prepare-sidecars.mjs`
///
/// If you ever bypass those wrappers (e.g. invoking the Tauri CLI directly
/// without beforeBuildCommand) you'd ship the placeholder, so emit a
/// cargo:warning that surfaces in any compile log to make that loud.
#[cfg(feature = "tauri-runtime")]
fn ensure_sidecar_placeholder() {
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
    let path = dir.join(format!("codeg-mcp-{triple}{ext}"));

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
            "cargo:warning=codeg-mcp sidecar missing at {}; wrote 0-byte placeholder. \
             Run `pnpm tauri:prepare-sidecars` before `tauri build` to ship a working binary.",
            path.display()
        );
    }
}
