const COMMANDS: &[&str] = &["store_secret", "load_secret", "delete_secret"];

fn main() {
  tauri_plugin::Builder::new(COMMANDS)
    .android_path("android")
    .ios_path("ios")
    .build();
}
