use crate::app_error::AppCommandError;

/// File status from git diff --name-status.
#[derive(Debug, Clone)]
pub struct FileStatus {
    pub file: String,
    pub status: String, // "A", "M", "D", "R"
    pub additions: i32,
    pub deletions: i32,
}

/// Get file statuses by combining git diff --name-status and git ls-files.
pub async fn get_file_statuses(path: &str, anchor: &str) -> Result<Vec<FileStatus>, AppCommandError> {
    let mut statuses = get_diff_statuses(path, anchor).await?;
    let untracked = get_untracked_files(path).await?;

    for file in untracked {
        statuses.push(FileStatus {
            file,
            status: "A".to_string(),
            additions: 0,
            deletions: 0,
        });
    }

    Ok(statuses)
}

async fn get_diff_statuses(path: &str, anchor: &str) -> Result<Vec<FileStatus>, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["diff", "--name-status", anchor])
        .current_dir(path)
        .output()
        .await
        .map_err(|e| AppCommandError::io_error(format!("{}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppCommandError::io_error(format!(
            "git diff --name-status: {}",
            stderr
        )));
    }

    let mut statuses = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.splitn(2, '\t');
        if let (Some(status), Some(file)) = (parts.next(), parts.next()) {
            let status = status.trim();
            statuses.push(FileStatus {
                file: file.to_string(),
                status: status.to_string(),
                additions: 0,
                deletions: 0,
            });
        }
    }

    Ok(statuses)
}

async fn get_untracked_files(path: &str) -> Result<Vec<String>, AppCommandError> {
    let output = crate::process::tokio_command("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(path)
        .output()
        .await
        .map_err(|e| AppCommandError::io_error(format!("{}", e)))?;

    if !output.status.success() {
        return Ok(Vec::new()); // Silently fail for untracked, not critical
    }

    let files = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect();

    Ok(files)
}
