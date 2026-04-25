fn main() {
    #[cfg(feature = "tauri-runtime")]
    tauri_build::build();

    if let Ok(profile) = std::env::var("PROFILE") {
        println!("cargo:rustc-env=CODEG_BUILD_PROFILE={profile}");
    }

    if let Some(commit) = git_output(&["rev-parse", "--short=12", "HEAD"]) {
        println!("cargo:rustc-env=CODEG_BUILD_GIT_COMMIT={commit}");
    }

    if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        println!("cargo:rustc-env=CODEG_BUILD_TIMESTAMP={}", now.as_secs());
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}
