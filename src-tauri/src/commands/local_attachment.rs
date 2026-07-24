use std::collections::BTreeMap;
use std::path::Path;

use crate::app_error::{AppCommandError, UPLOAD_I18N_KEY_NOT_A_FILE, UPLOAD_I18N_KEY_TOO_LARGE};

pub(crate) const UPLOAD_MAX_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const IMAGE_MAX_BYTES: u64 = 20_000_000;

fn size_limit_error(size: u64, limit: u64) -> AppCommandError {
    AppCommandError::io_error("Local file exceeds the size limit")
        .with_detail(format!("size={size} limit={limit}"))
        .with_i18n(
            UPLOAD_I18N_KEY_TOO_LARGE,
            BTreeMap::from([
                ("size".to_string(), size.to_string()),
                ("limit".to_string(), limit.to_string()),
            ]),
        )
}

async fn read_regular_file_with_limit(path: &Path, limit: u64) -> Result<Vec<u8>, AppCommandError> {
    let metadata = tokio::fs::symlink_metadata(path).await.map_err(|e| {
        AppCommandError::io_error("Failed to stat local attachment")
            .with_detail(format!("{}: {e}", path.display()))
    })?;
    if !metadata.file_type().is_file() {
        return Err(AppCommandError::io_error("Not a regular file")
            .with_detail(path.display().to_string())
            .with_i18n(UPLOAD_I18N_KEY_NOT_A_FILE, BTreeMap::new()));
    }
    if metadata.len() > limit {
        return Err(size_limit_error(metadata.len(), limit));
    }

    let bytes = tokio::fs::read(path).await.map_err(|e| {
        AppCommandError::io_error("Failed to read local attachment")
            .with_detail(format!("{}: {e}", path.display()))
    })?;
    let actual_size = bytes.len() as u64;
    if actual_size > limit {
        return Err(size_limit_error(actual_size, limit));
    }

    Ok(bytes)
}

pub(crate) async fn read_upload_file(path: &Path) -> Result<Vec<u8>, AppCommandError> {
    read_regular_file_with_limit(path, UPLOAD_MAX_BYTES).await
}

pub(crate) async fn read_image_file(path: &Path) -> Result<Vec<u8>, AppCommandError> {
    read_regular_file_with_limit(path, IMAGE_MAX_BYTES).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    async fn create_sparse_file(dir: &Path, name: &str, size: u64) -> PathBuf {
        let path = dir.join(name);
        let file = tokio::fs::File::create(&path)
            .await
            .expect("create sparse test file");
        file.set_len(size).await.expect("size sparse test file");
        path
    }

    fn assert_not_regular_file(err: &AppCommandError) {
        assert_eq!(err.i18n_key.as_deref(), Some(UPLOAD_I18N_KEY_NOT_A_FILE));
        assert!(err.message.contains("Not a regular file"));
    }

    #[tokio::test]
    async fn image_read_accepts_file_above_upload_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let size = UPLOAD_MAX_BYTES + 1;
        let path = create_sparse_file(dir.path(), "medium.png", size).await;

        let file = read_image_file(&path)
            .await
            .expect("2-20 MB image should be accepted");

        assert_eq!(file.len() as u64, size);
    }

    #[tokio::test]
    async fn image_read_accepts_exact_twenty_million_byte_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = create_sparse_file(dir.path(), "boundary.png", IMAGE_MAX_BYTES).await;

        let file = read_image_file(&path)
            .await
            .expect("exact image limit should be accepted");

        assert_eq!(file.len() as u64, IMAGE_MAX_BYTES);
    }

    #[tokio::test]
    async fn image_read_rejects_above_twenty_million_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let size = IMAGE_MAX_BYTES + 1;
        let path = create_sparse_file(dir.path(), "oversize.png", size).await;

        let err = read_image_file(&path)
            .await
            .expect_err("image above limit should be rejected");

        assert_eq!(err.i18n_key.as_deref(), Some(UPLOAD_I18N_KEY_TOO_LARGE));
        assert_eq!(
            err.i18n_params
                .as_ref()
                .and_then(|params| params.get("limit"))
                .map(String::as_str),
            Some("20000000")
        );
    }

    #[tokio::test]
    async fn upload_read_still_rejects_file_above_two_mib() {
        let dir = tempfile::tempdir().expect("tempdir");
        let size = UPLOAD_MAX_BYTES + 1;
        let path = create_sparse_file(dir.path(), "oversize.txt", size).await;

        let err = read_upload_file(&path)
            .await
            .expect_err("ordinary upload above 2 MiB should stay rejected");

        assert_eq!(err.i18n_key.as_deref(), Some(UPLOAD_I18N_KEY_TOO_LARGE));
        assert_eq!(
            err.i18n_params
                .as_ref()
                .and_then(|params| params.get("limit"))
                .map(String::as_str),
            Some("2097152")
        );
    }

    #[tokio::test]
    async fn image_read_rejects_directory() {
        let dir = tempfile::tempdir().expect("tempdir");

        let err = read_image_file(dir.path())
            .await
            .expect_err("directory should be rejected");

        assert_not_regular_file(&err);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn image_read_rejects_symlink_fifo_and_device() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let target = create_sparse_file(dir.path(), "target.png", 1).await;
        let link = dir.path().join("link.png");
        symlink(&target, &link).expect("create symlink");

        let link_err = read_image_file(&link)
            .await
            .expect_err("symlink should be rejected");
        assert_not_regular_file(&link_err);

        let fifo = dir.path().join("image.fifo");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).expect("fifo path CString");
        let rc = unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "create FIFO: {}", std::io::Error::last_os_error());
        let fifo_err = read_image_file(&fifo)
            .await
            .expect_err("FIFO should be rejected without reading");
        assert_not_regular_file(&fifo_err);

        let device_err = read_image_file(Path::new("/dev/null"))
            .await
            .expect_err("device should be rejected");
        assert_not_regular_file(&device_err);
    }
}
