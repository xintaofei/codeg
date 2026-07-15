# Codeg Mobile shell

This package is the Android/iOS-only Tauri shell. It bundles the repository's
static Next.js export and connects to a remote Codeg server. It intentionally
does not link the desktop Rust backend or any local Agent runtime.

## Android build

Set `JAVA_HOME`, `ANDROID_HOME`, `ANDROID_SDK_ROOT`, and `NDK_HOME`, then run:

```bash
pnpm mobile:android:init
pnpm mobile:android:build
```

The first launch asks for the HTTPS Codeg server URL and access token. The URL
is ordinary app configuration. The token is held in memory while the app runs
and persisted with Android Keystore or iOS Keychain; it is never written to
browser local storage.

## Runtime boundary

`mobile/src-tauri` is an independent mobile-only crate with the
`mobile-client` feature. It does not link the desktop Codeg Rust crate, local
Agent runtimes, ACP, PTY, Git, sidecars, tray, updater, desktop pets, or
multi-window code. The bundled React application uses `WebTransport` to call a
remote Codeg server over HTTPS and WebSocket.

## Android requirements

- Android 7.0 / API 24 or newer
- JDK 17
- Android SDK 36, build-tools 36.0.0 and NDK 28.2.13676358
- Rust target `aarch64-linux-android`

For a debug APK:

```bash
pnpm install
pnpm --dir mobile tauri android init --ci
pnpm --dir mobile tauri android build --apk --debug --target aarch64 --ci
```

## Release signing

Release signing follows the Tauri Android signing model but reads credentials
from environment variables so no password file is created inside the checkout:

```bash
export CODEG_ANDROID_KEYSTORE_PATH=/absolute/path/codeg-mobile-release.jks
export CODEG_ANDROID_KEYSTORE_PASSWORD='...'
export CODEG_ANDROID_KEY_ALIAS=codeg-mobile
export CODEG_ANDROID_KEY_PASSWORD='...'
pnpm --dir mobile tauri android build --apk --target aarch64 --ci
```

The private GitHub workflow uses matching repository secrets and publishes the
signed APK as an Actions artifact. Tags named `mobile-v*` also create a private
GitHub Release.

## iOS compile smoke

The iOS project is generated from the same shell and kept simulator-buildable:

```bash
pnpm --dir mobile tauri ios init --ci
pnpm --dir mobile tauri ios build --debug --target aarch64-sim --ci
```

An Apple Developer team and distribution certificate are required for a real
device archive or TestFlight upload.
