#[cfg(feature = "tauri-runtime")]
const SERVICE_NAME: &str = "codeg";

fn token_key(account_id: &str) -> String {
    format!("github-token:{}", account_id)
}

fn channel_token_key(channel_id: i32) -> String {
    format!("chat-channel:{}", channel_id)
}

#[cfg(feature = "tauri-runtime")]
fn app_secret_key(key: &str) -> String {
    format!("app-secret:{key}")
}

// ── Tauri mode: OS keyring ──

#[cfg(feature = "tauri-runtime")]
pub fn set_token(account_id: &str, token: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &token_key(account_id))
        .map_err(|e| format!("keyring init error: {e}"))?;
    entry
        .set_password(token)
        .map_err(|e| format!("keyring set error: {e}"))
}

#[cfg(feature = "tauri-runtime")]
pub fn get_token(account_id: &str) -> Option<String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &token_key(account_id)).ok()?;
    entry.get_password().ok()
}

#[cfg(feature = "tauri-runtime")]
pub fn delete_token(account_id: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &token_key(account_id))
        .map_err(|e| format!("keyring init error: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete error: {e}")),
    }
}

#[cfg(feature = "tauri-runtime")]
pub fn set_app_secret(key: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &app_secret_key(key))
        .map_err(|error| format!("keyring init error: {error}"))?;
    entry
        .set_password(value)
        .map_err(|error| format!("keyring set error: {error}"))
}

#[cfg(feature = "tauri-runtime")]
pub fn get_app_secret(key: &str) -> Option<String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &app_secret_key(key)).ok()?;
    entry.get_password().ok()
}

#[cfg(feature = "tauri-runtime")]
pub fn delete_app_secret(key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &app_secret_key(key))
        .map_err(|error| format!("keyring init error: {error}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!("keyring delete error: {error}")),
    }
}

// ── Server mode: file-based token store ──

#[cfg(not(feature = "tauri-runtime"))]
fn tokens_file_path() -> std::path::PathBuf {
    tokens_file_path_for(std::env::var("CODEG_DATA_DIR").ok().as_deref())
}

/// Resolve the on-disk `tokens.json` path given an explicit
/// `CODEG_DATA_DIR` value (or `None` to fall back to the platform
/// default). Always returns an absolute path so subprocess credential
/// helpers — which inherit our env but run in git's CWD, not ours —
/// don't end up looking for `tokens.json` in the user's repo. Factored
/// out so tests can exercise path resolution without poking at process
/// env state.
#[cfg(not(feature = "tauri-runtime"))]
fn tokens_file_path_for(env_value: Option<&str>) -> std::path::PathBuf {
    let dir = env_value.map(std::path::PathBuf::from).unwrap_or_else(|| {
        dirs::data_dir()
            .map(|d| d.join("codeg"))
            .unwrap_or_else(|| std::path::PathBuf::from(".codeg-data"))
    });
    crate::git_credential::absolutize(&dir).join("tokens.json")
}

#[cfg(not(feature = "tauri-runtime"))]
fn read_tokens() -> std::collections::HashMap<String, String> {
    let path = tokens_file_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[cfg(not(feature = "tauri-runtime"))]
fn write_tokens(tokens: &std::collections::HashMap<String, String>) -> Result<(), String> {
    let path = tokens_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create token store directory: {e}"))?;
    }
    let json = serde_json::to_string_pretty(tokens)
        .map_err(|e| format!("failed to serialize tokens: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("failed to write token store: {e}"))
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn set_token(account_id: &str, token: &str) -> Result<(), String> {
    let mut tokens = read_tokens();
    tokens.insert(token_key(account_id), token.to_string());
    write_tokens(&tokens)
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn get_token(account_id: &str) -> Option<String> {
    read_tokens().get(&token_key(account_id)).cloned()
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn delete_token(account_id: &str) -> Result<(), String> {
    let mut tokens = read_tokens();
    tokens.remove(&token_key(account_id));
    write_tokens(&tokens)
}

// ── Chat channel token helpers ──
// Reuse the same storage mechanism (keyring or file) with a different key prefix.

#[cfg(feature = "tauri-runtime")]
pub fn set_channel_token(channel_id: i32, token: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &channel_token_key(channel_id))
        .map_err(|e| format!("keyring init error: {e}"))?;
    entry
        .set_password(token)
        .map_err(|e| format!("keyring set error: {e}"))
}

#[cfg(feature = "tauri-runtime")]
pub fn get_channel_token(channel_id: i32) -> Option<String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &channel_token_key(channel_id)).ok()?;
    entry.get_password().ok()
}

#[cfg(feature = "tauri-runtime")]
pub fn delete_channel_token(channel_id: i32) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, &channel_token_key(channel_id))
        .map_err(|e| format!("keyring init error: {e}"))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete error: {e}")),
    }
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn set_channel_token(channel_id: i32, token: &str) -> Result<(), String> {
    let mut tokens = read_tokens();
    tokens.insert(channel_token_key(channel_id), token.to_string());
    write_tokens(&tokens)
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn get_channel_token(channel_id: i32) -> Option<String> {
    read_tokens().get(&channel_token_key(channel_id)).cloned()
}

#[cfg(not(feature = "tauri-runtime"))]
pub fn delete_channel_token(channel_id: i32) -> Result<(), String> {
    let mut tokens = read_tokens();
    tokens.remove(&channel_token_key(channel_id));
    write_tokens(&tokens)
}

#[cfg(all(test, not(feature = "tauri-runtime")))]
mod tests {
    use super::*;

    #[test]
    fn test_tokens_file_path_absolutizes_relative_env() {
        // Regression: a relative `CODEG_DATA_DIR=data` previously made
        // `tokens.json` resolve against the helper subprocess's CWD (i.e.
        // git's repo dir), even after we'd absolutized the path used for
        // the database. The token store must always land on an absolute
        // path so DB lookup and token lookup point at the same root.
        let cwd = std::env::current_dir().expect("cwd");
        let resolved = tokens_file_path_for(Some("data"));
        assert!(
            resolved.is_absolute(),
            "tokens path must be absolute, got: {}",
            resolved.display()
        );
        assert_eq!(resolved, cwd.join("data").join("tokens.json"));
    }

    #[test]
    fn test_tokens_file_path_absolute_env_unchanged() {
        let data_dir = std::env::current_dir().expect("cwd").join("codeg-data");
        let data_dir_str = data_dir.to_string_lossy().to_string();
        let resolved = tokens_file_path_for(Some(&data_dir_str));
        assert_eq!(resolved, data_dir.join("tokens.json"));
    }

    #[test]
    fn test_tokens_file_path_default_when_unset() {
        // No env var → derived from `dirs::data_dir()` (always absolute on
        // every platform we ship to). Just verify we end at `tokens.json`
        // and that the result is absolute, not the literal default.
        let resolved = tokens_file_path_for(None);
        assert!(resolved.is_absolute());
        assert!(resolved.ends_with("tokens.json"));
    }
}
