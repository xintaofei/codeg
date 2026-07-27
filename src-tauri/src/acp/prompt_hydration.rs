//! Server-side re-hydration of uploaded image attachments.
//!
//! In web / remote-workspace mode the composer does NOT inline image bytes
//! into the `/acp_prompt` JSON body — a couple of screenshots of base64 would
//! blow past axum's default 2 MiB `DefaultBodyLimit` and the request would be
//! rejected with a plain-text 413 before the handler runs. Instead the client
//! uploads each image through `/upload_attachment` (streamed, size-capped,
//! with visible error toasts) and sends the prompt block with an **empty
//! payload** plus a `file://` uri pointing at the uploaded file. This module
//! reads those bytes back and fills the block in, so the agent still receives
//! a complete ACP image block (multimodal understanding is preserved — a bare
//! ResourceLink would leave it to the model to maybe-read the file as text).
//!
//! Two marker shapes exist, mirroring the two wire encodings the composer's
//! `imageAttachmentToPromptBlock` picks from the agent's prompt capabilities:
//!
//! - `Image { data: "", uri: Some("file://…") }` — agents that accept native
//!   image content.
//! - `Resource { blob: Some(""), mime_type: Some("image/*"), uri: "file://…" }`
//!   — agents that reject images but take embedded context (e.g. Grok). The
//!   client already chose the caps-correct shape, so hydration only fills
//!   bytes and never converts between shapes.
//!
//! Anything else — a block whose payload is already populated (the desktop
//! local path, which inlines over Tauri IPC where no body limit exists), a
//! non-`file://` uri (`clipboard://…` synth ids), a non-image resource — is
//! left untouched, so the desktop flow is byte-identical to before.
//!
//! Security: only files under `codeg_uploads_root()` are readable here. The
//! uri is percent-decoded, canonicalized, and prefix-checked against the
//! canonicalized root, so `acp_prompt` cannot be used as an arbitrary-file
//! read oracle (the viewer broadcast would otherwise ship the bytes to every
//! attached client). Hydration is read-only, so canonicalize + `starts_with`
//! is sufficient — the openat-based `upload_jail` machinery guards *writes*
//! against TOCTOU swaps and is not needed for a bounded read.

use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

use crate::acp::error::AcpError;
use crate::acp::types::PromptInputBlock;
use crate::web::handlers::files::UPLOAD_MAX_BYTES;

/// Aggregate ceiling on the raw bytes one prompt may hydrate, summed across
/// every marker occurrence — the SAME file referenced N times counts N times,
/// because each occurrence becomes its own base64 copy in the block list and
/// again in the viewer broadcast projection.
///
/// Without this, the 2 MiB HTTP body limit stops being a memory backstop: a
/// ~2 KiB body carrying many markers to one 20 MiB upload would expand to
/// hundreds of MiB server-side. The value allows a realistic prompt (a
/// handful of large screenshots) while bounding the worst case to
/// ~85 MiB of base64 (+ one clone for the viewer projection).
pub const HYDRATION_TOTAL_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Global bound on prompts hydrating CONTENT concurrently. The per-connection
/// prompt lock (held at the call site) already serializes hydration within a
/// connection, but the connection count itself is unbounded — one token can
/// open many sessions — so without a cross-connection bound N connections
/// could all be inside the read+encode phase at once. Two permits bound the
/// concurrent hydration WORK (≤ 2 × 85 MiB of base64 being produced at any
/// instant) while still letting two conversations attach images at the same
/// moment without queueing.
///
/// This is a concurrency bound, not a residency bound: once hydration
/// returns, the encoded payload lives on in the prompt blocks, the viewer
/// broadcast, and `pending_user_message` until that turn completes — one
/// bounded unit per live image-bearing turn, same as desktop inline sends
/// have always cost. A global residency budget would need turn-lifecycle
/// resource ownership (or connection caps) and is deliberately out of scope.
///
/// Scope: acquired only around pass 2 (the reads + base64), and only when at
/// least one marker resolved — marker-less prompts (the overwhelming
/// majority) never touch it. Ordering is strictly `prompt lock → this
/// semaphore`, never the reverse, so it cannot deadlock.
static HYDRATION_CONCURRENCY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// Fill empty image payloads whose uri points at an uploaded file.
///
/// Two passes: first resolve + validate every marker (jail, regular-file,
/// per-file cap, aggregate cap) WITHOUT reading any content, then read and
/// fill. An over-limit prompt is thus rejected before a single content byte
/// is buffered. Mutates `blocks` in place. Errors abort the whole prompt (the
/// caller rejects before any side effect — no conversation row is created and
/// no status flips), so a deleted / oversized / out-of-jail upload surfaces
/// as a structured send failure instead of a silently image-less prompt.
pub async fn hydrate_prompt_blocks(
    blocks: &mut [PromptInputBlock],
    uploads_root: &Path,
) -> Result<(), AcpError> {
    // Pass 1: resolve each marker to a validated canonical path.
    let mut resolved: Vec<(usize, std::path::PathBuf)> = Vec::new();
    let mut canonical_root: Option<std::path::PathBuf> = None;
    let mut total_bytes: u64 = 0;
    for (idx, block) in blocks.iter().enumerate() {
        let uri = match block {
            PromptInputBlock::Image {
                data,
                uri: Some(uri),
                ..
            } if data.is_empty() => uri,
            PromptInputBlock::Resource {
                uri,
                mime_type: Some(mime),
                blob: Some(blob),
                ..
            } if blob.is_empty() && mime.starts_with("image/") => uri,
            _ => continue,
        };
        let Some(path) = file_uri_to_path(uri) else {
            // Non-file uri (`clipboard://…` synth ids) — not a marker; the
            // block stays untouched.
            continue;
        };

        // Canonicalize the root once, lazily — only prompts that actually
        // carry markers pay for it. A missing root means the referenced
        // upload cannot exist either.
        if canonical_root.is_none() {
            canonical_root = Some(tokio::fs::canonicalize(uploads_root).await.map_err(|e| {
                AcpError::protocol(format!(
                    "image attachment unavailable: uploads directory not accessible ({e})"
                ))
            })?);
        }
        let root = canonical_root.as_ref().expect("set above");

        // Canonicalize the target so symlinks / `..` segments can't sneak a
        // read outside the jail.
        let canonical = tokio::fs::canonicalize(&path).await.map_err(|e| {
            AcpError::protocol(format!(
                "image attachment unavailable: {} ({e})",
                path.display()
            ))
        })?;
        if !canonical.starts_with(root) {
            return Err(AcpError::protocol(format!(
                "image attachment unavailable: {} is outside the uploads directory",
                canonical.display()
            )));
        }

        let meta = tokio::fs::metadata(&canonical).await.map_err(|e| {
            AcpError::protocol(format!(
                "image attachment unavailable: {} ({e})",
                canonical.display()
            ))
        })?;
        if !meta.is_file() {
            return Err(AcpError::protocol(format!(
                "image attachment unavailable: {} is not a regular file",
                canonical.display()
            )));
        }
        // Mirrors the upload-side per-file cap: nothing bigger can have been
        // uploaded through the endpoint, so anything bigger here is a
        // hand-placed file we refuse to base64 into memory.
        if meta.len() > UPLOAD_MAX_BYTES {
            return Err(AcpError::protocol(format!(
                "image attachment unavailable: {} exceeds the {} byte upload limit",
                canonical.display(),
                UPLOAD_MAX_BYTES
            )));
        }
        total_bytes = total_bytes.saturating_add(meta.len());
        if total_bytes > HYDRATION_TOTAL_MAX_BYTES {
            return Err(AcpError::protocol(format!(
                "image attachments exceed the {HYDRATION_TOTAL_MAX_BYTES} byte total limit for one prompt"
            )));
        }
        resolved.push((idx, canonical));
    }

    // Pass 2: read and fill. Every path was validated above; a read failure
    // here (file vanished in between) still aborts the prompt loudly.
    if resolved.is_empty() {
        return Ok(());
    }
    let _hydration_permit = HYDRATION_CONCURRENCY.acquire().await.map_err(|_| {
        // Only reachable if the semaphore were closed, which never happens —
        // defensive rather than a panic in the prompt path.
        AcpError::protocol("image attachment hydration unavailable".to_string())
    })?;
    for (idx, canonical) in resolved {
        let bytes = tokio::fs::read(&canonical).await.map_err(|e| {
            AcpError::protocol(format!(
                "image attachment unavailable: {} ({e})",
                canonical.display()
            ))
        })?;
        match &mut blocks[idx] {
            PromptInputBlock::Image { data, .. } => *data = BASE64.encode(&bytes),
            PromptInputBlock::Resource {
                blob: Some(blob), ..
            } => *blob = BASE64.encode(&bytes),
            _ => unreachable!("resolved indices only point at marker blocks"),
        }
    }
    Ok(())
}

/// Decode the two `file://` shapes the frontend's `buildFileUri` produces:
///
/// - Unix: `/a/b c` → `file:///a/b%20c`
/// - Windows drive: `C:\a\b` → `file:///C:/a/b`
///
/// A `#fragment` is stripped before decoding — `buildFileUri` percent-encodes
/// literal `#` in path segments, so a bare `#` can only be a line-range
/// fragment (never part of an uploaded filename). UNC uris (`file://server/…`)
/// are not produced for uploads and fall through to canonicalize failure.
fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.split('#').next().unwrap_or(rest);
    let decoded = urlencoding::decode(rest).ok()?;
    let decoded = decoded.as_ref();
    if decoded.is_empty() {
        return None;
    }
    // `file:///C:/x` decodes to `/C:/x`; on Windows the leading slash is the
    // uri separator, not part of the path. Unix never produces this shape
    // (upload paths live under the user's home), so gate on the platform to
    // avoid mangling a genuine `/C:/…` Unix directory.
    #[cfg(windows)]
    {
        let bytes = decoded.as_bytes();
        if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b':'
        {
            return Some(PathBuf::from(&decoded[1..]));
        }
    }
    Some(PathBuf::from(decoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_block(data: &str, uri: Option<&str>) -> PromptInputBlock {
        PromptInputBlock::Image {
            data: data.to_string(),
            mime_type: "image/png".to_string(),
            uri: uri.map(str::to_string),
        }
    }

    fn resource_blob_block(blob: &str, uri: &str, mime: &str) -> PromptInputBlock {
        PromptInputBlock::Resource {
            uri: uri.to_string(),
            mime_type: Some(mime.to_string()),
            text: None,
            blob: Some(blob.to_string()),
        }
    }

    /// buildFileUri-style encoding of an absolute path for tests. Percent-
    /// encodes each segment like the frontend does.
    fn to_file_uri(path: &Path) -> String {
        let normalized = path.to_string_lossy().replace('\\', "/");
        let encoded = normalized
            .split('/')
            .map(|seg| urlencoding::encode(seg).into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if normalized.starts_with('/') {
            format!("file://{encoded}")
        } else {
            format!("file:///{encoded}")
        }
    }

    #[tokio::test]
    async fn fills_empty_image_data_from_uploads_root() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("shot.png");
        tokio::fs::write(&file, b"png-bytes").await.unwrap();

        let mut blocks = vec![image_block("", Some(&to_file_uri(&file)))];
        hydrate_prompt_blocks(&mut blocks, root.path())
            .await
            .unwrap();

        match &blocks[0] {
            PromptInputBlock::Image { data, .. } => {
                assert_eq!(data, &BASE64.encode(b"png-bytes"));
            }
            other => panic!("unexpected block: {other:?}"),
        }
    }

    #[tokio::test]
    async fn fills_empty_image_resource_blob() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("pasted image.png");
        tokio::fs::write(&file, b"blob-bytes").await.unwrap();

        // Space in the filename exercises the percent-decode path.
        let mut blocks = vec![resource_blob_block("", &to_file_uri(&file), "image/png")];
        hydrate_prompt_blocks(&mut blocks, root.path())
            .await
            .unwrap();

        match &blocks[0] {
            PromptInputBlock::Resource { blob, .. } => {
                assert_eq!(blob.as_deref(), Some(BASE64.encode(b"blob-bytes").as_str()));
            }
            other => panic!("unexpected block: {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_a_path_outside_the_uploads_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("secret.png");
        tokio::fs::write(&file, b"secret").await.unwrap();

        let mut blocks = vec![image_block("", Some(&to_file_uri(&file)))];
        let err = hydrate_prompt_blocks(&mut blocks, root.path())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("image attachment unavailable"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn rejects_a_symlink_escaping_the_root() {
        #[cfg(unix)]
        {
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let target = outside.path().join("secret.png");
            tokio::fs::write(&target, b"secret").await.unwrap();
            let link = root.path().join("link.png");
            std::os::unix::fs::symlink(&target, &link).unwrap();

            let mut blocks = vec![image_block("", Some(&to_file_uri(&link)))];
            let err = hydrate_prompt_blocks(&mut blocks, root.path())
                .await
                .unwrap_err();
            assert!(
                err.to_string().contains("outside the uploads directory"),
                "unexpected error: {err}"
            );
        }
    }

    #[tokio::test]
    async fn errors_on_a_missing_upload() {
        let root = tempfile::tempdir().unwrap();
        let ghost = root.path().join("gone.png");

        let mut blocks = vec![image_block("", Some(&to_file_uri(&ghost)))];
        let err = hydrate_prompt_blocks(&mut blocks, root.path())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("image attachment unavailable"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn leaves_populated_and_non_marker_blocks_untouched() {
        let root = tempfile::tempdir().unwrap();

        let mut blocks = vec![
            // Data already inline (desktop path) — untouched even with a uri.
            image_block("aW5saW5l", Some("file:///Users/me/Pictures/x.png")),
            // Non-file uri marker shapes — untouched.
            image_block("", Some("clipboard://pasted-1")),
            image_block("", None),
            // Non-image resource with an empty blob — untouched.
            resource_blob_block("", "file:///tmp/data.bin", "application/octet-stream"),
            PromptInputBlock::Text {
                text: "hello".to_string(),
            },
        ];
        let before = format!("{blocks:?}");
        hydrate_prompt_blocks(&mut blocks, root.path())
            .await
            .unwrap();
        assert_eq!(format!("{blocks:?}"), before);
    }

    #[tokio::test]
    async fn rejects_when_duplicate_markers_exceed_the_aggregate_cap() {
        // One legitimate ≤20 MiB upload referenced enough times to blow the
        // 64 MiB aggregate: 4 × 18 MiB = 72 MiB. The cap must count every
        // marker occurrence (each becomes its own base64 copy), and reject in
        // pass 1 — before any content read — so a tiny HTTP body cannot
        // amplify into hundreds of MiB of server memory.
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("big.png");
        let f = std::fs::File::create(&file).unwrap();
        f.set_len(18 * 1024 * 1024).unwrap();
        drop(f);

        let uri = to_file_uri(&file);
        let mut blocks = (0..4)
            .map(|_| image_block("", Some(&uri)))
            .collect::<Vec<_>>();
        let err = hydrate_prompt_blocks(&mut blocks, root.path())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("total limit"),
            "unexpected error: {err}"
        );
        // Nothing was hydrated — all four blocks still carry empty data.
        for b in &blocks {
            match b {
                PromptInputBlock::Image { data, .. } => assert!(data.is_empty()),
                other => panic!("unexpected block: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn admits_markers_under_the_aggregate_cap() {
        // 3 × 18 MiB = 54 MiB < 64 MiB — must hydrate all three. Sparse files
        // read as zeros, which is fine for the size accounting under test.
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("ok.png");
        let f = std::fs::File::create(&file).unwrap();
        f.set_len(18 * 1024 * 1024).unwrap();
        drop(f);

        let uri = to_file_uri(&file);
        let mut blocks = (0..3)
            .map(|_| image_block("", Some(&uri)))
            .collect::<Vec<_>>();
        hydrate_prompt_blocks(&mut blocks, root.path())
            .await
            .unwrap();
        for b in &blocks {
            match b {
                PromptInputBlock::Image { data, .. } => assert!(!data.is_empty()),
                other => panic!("unexpected block: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn rejects_an_oversized_upload() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("huge.png");
        // Sparse-ish: metadata length is what matters; write real bytes only
        // up to a small amount then set_len past the cap.
        let f = std::fs::File::create(&file).unwrap();
        f.set_len(UPLOAD_MAX_BYTES + 1).unwrap();
        drop(f);

        let mut blocks = vec![image_block("", Some(&to_file_uri(&file)))];
        let err = hydrate_prompt_blocks(&mut blocks, root.path())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("exceeds"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn concurrent_hydrations_beyond_the_permit_count_all_complete() {
        // 5 concurrent hydrations against 2 global permits: the semaphore
        // must queue, not deadlock or starve. Small files keep it fast.
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("tiny.png");
        tokio::fs::write(&file, b"tiny").await.unwrap();
        let uri = to_file_uri(&file);

        let mut handles = Vec::new();
        for _ in 0..5 {
            let uri = uri.clone();
            let root_path = root.path().to_path_buf();
            handles.push(tokio::spawn(async move {
                let mut blocks = vec![image_block("", Some(&uri))];
                hydrate_prompt_blocks(&mut blocks, &root_path).await?;
                match &blocks[0] {
                    PromptInputBlock::Image { data, .. } => {
                        assert_eq!(data, &BASE64.encode(b"tiny"));
                    }
                    other => panic!("unexpected block: {other:?}"),
                }
                Ok::<(), AcpError>(())
            }));
        }
        for h in handles {
            h.await.expect("task").expect("hydration");
        }
    }

    #[test]
    fn file_uri_decoding_handles_encoding_and_fragments() {
        assert_eq!(
            file_uri_to_path("file:///a/b%20c.png"),
            Some(PathBuf::from("/a/b c.png"))
        );
        assert_eq!(
            file_uri_to_path("file:///a/b.png#L1-L3"),
            Some(PathBuf::from("/a/b.png"))
        );
        assert_eq!(file_uri_to_path("clipboard://x"), None);
        assert_eq!(file_uri_to_path("file://"), None);
    }
}
