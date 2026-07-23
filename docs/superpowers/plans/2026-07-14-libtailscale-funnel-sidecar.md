# Tailscale Funnel Sidecar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Codeg Web Service optional public HTTPS access via a private Tailscale Funnel userspace node, packaged as a Go `codeg-tsnet` sidecar for desktop and `codeg-server` (including Windows).

**Architecture:** Rust owns Axum and app token auth. A Go sidecar owns `tsnet` identity, OAuth/auth-key login, and Funnel reverse-proxy to `http://127.0.0.1:<webPort>`. Control plane is localhost HTTP JSON with bootstrap line on stdout. Funnel failures degrade only Funnel; local web keeps running.

**Tech Stack:** Go + `tailscale.com/tsnet`, Rust/Axum/Tauri, React/next-intl settings UI, Node `prepare-sidecars.mjs`, GitHub Actions release matrix, Docker.

## Global Constraints

- Never manage, replace, or write into system Tailscale state.
- Always use Codeg-owned state dir: `<CODEG_DATA_DIR>/tailscale` (override `CODEG_TS_STATE_DIR`).
- Hostname default: `codeg-<stable-short-id>` (override `CODEG_TS_HOSTNAME`).
- Desktop auth: browser OAuth/login URL. Server/Docker auth: `CODEG_TS_AUTHKEY` required when Funnel is enabled.
- Existing Codeg bearer token remains mandatory over Funnel.
- Sidecar binary name: `codeg-tsnet` / `codeg-tsnet.exe`.
- Binary resolution: `CODEG_TSNET_BIN` -> exe sibling -> Tauri externalBin/resource path.
- Packaging must follow `codeg-mcp` patterns and ship on macOS/Linux/Windows.
- Funnel missing/failed must not break local Web Service startup.
- Control API binds `127.0.0.1` only; auth header `X-Codeg-Tsnet-Token`.
- PR description must be bilingual (Chinese + English).

---

## File Structure

### Create
- `codeg-tsnet/go.mod`
- `codeg-tsnet/main.go`
- `codeg-tsnet/control.go`
- `codeg-tsnet/status.go`
- `codeg-tsnet/funnel.go`
- `codeg-tsnet/main_test.go`
- `codeg-tsnet/control_test.go`
- `src-tauri/src/web/tailscale/mod.rs`
- `src-tauri/src/web/tailscale/binary.rs`
- `src-tauri/src/web/tailscale/controller.rs`
- `src-tauri/src/web/tailscale/protocol.rs`
- `src-tauri/src/web/tailscale/status.rs`
- `src-tauri/src/web/handlers/tailscale.rs`
- `src/components/settings/web-service-funnel-section.tsx`
- `src/components/settings/web-service-funnel-section.test.tsx`

### Modify
- `src-tauri/src/web/mod.rs` -- config flag, start/stop hooks, status aggregation
- `src-tauri/src/web/handlers/mod.rs` -- export tailscale handlers
- `src-tauri/src/web/router.rs` -- register Funnel endpoints
- `src-tauri/src/lib.rs` -- register Tauri commands + auto-start Funnel after auto-start web
- `src-tauri/src/app_state.rs` -- hold `TailscaleController` handle
- `src-tauri/src/bin/codeg_server.rs` -- env-driven Funnel enable on server boot
- `src-tauri/scripts/prepare-sidecars.mjs` -- build/stage `codeg-tsnet` too
- `src-tauri/tauri.conf.json` -- `externalBin` add `binaries/codeg-tsnet`
- `.github/workflows/release.yml` -- Go setup, build/package/smoke `codeg-tsnet`
- `Dockerfile`, `Dockerfile.ci` -- include `codeg-tsnet`
- `install.sh`, `install.ps1` -- managed bin list includes `codeg-tsnet`
- `src/lib/api.ts` -- config/status/API wrappers
- `src/components/settings/web-service-settings.tsx` -- mount Funnel section
- `src/i18n/messages/en.json`, `zh-CN.json` (+ other locales with same keys)
- `README.md` / `docs/readme/README.zh-CN.md` -- Funnel env/docs section (minimal)

### Responsibility Boundaries
- `codeg-tsnet/*`: Tailscale userspace node + Funnel only
- `web/tailscale/*`: spawn/control/status mapping only
- `web/mod.rs`: web lifecycle orchestration only
- UI section: Funnel UX only; does not reimplement token/port logic

---

### Task 1: `codeg-tsnet` MVP + control protocol

**Files:**
- Create: `codeg-tsnet/go.mod`
- Create: `codeg-tsnet/main.go`
- Create: `codeg-tsnet/control.go`
- Create: `codeg-tsnet/status.go`
- Create: `codeg-tsnet/funnel.go`
- Create: `codeg-tsnet/main_test.go`
- Create: `codeg-tsnet/control_test.go`

**Interfaces:**
- Consumes: none
- Produces:
  - CLI:
    ```text
    codeg-tsnet \
      --control-addr 127.0.0.1:0 \
      --state-dir <dir> \
      --hostname codeg-xxxx \
      [--auth-key ...] \
      --control-token <token>
    ```
  - stdout bootstrap (exactly one JSON line, then logs to stderr only):
    ```json
    {"controlAddr":"127.0.0.1:54321","pid":12345}
    ```
  - HTTP API (header `X-Codeg-Tsnet-Token: <token>` required on every request):
    - `GET /status` -> `StatusResponse`
    - `POST /up` body optional `{ "authKey": "..." }`
    - `POST /funnel` body `{ "enabled": true, "localhostPort": 3080 }`
    - `POST /logout`
    - `POST /shutdown`
  - `StatusResponse` JSON camelCase:
    ```go
    type StatusResponse struct {
      State        string `json:"state"`
      LoginURL     string `json:"loginUrl,omitempty"`
      FunnelURL    string `json:"funnelUrl,omitempty"`
      Hostname     string `json:"hostname,omitempty"`
      IPv4         string `json:"ipv4,omitempty"`
      LastError    string `json:"lastError,omitempty"`
      ErrorKey     string `json:"errorKey,omitempty"`
      BackendState string `json:"backendState,omitempty"`
    }
    ```
  - `state` values: `stopped|starting|needs_login|connecting|online|funnel_enabling|funnel_ready|error|stopping`

- [ ] **Step 1: Scaffold Go module**

```bash
mkdir -p codeg-tsnet
cd codeg-tsnet
go mod init github.com/xintaofei/codeg/codeg-tsnet
go get tailscale.com@v1.82.5
```

Pin a known-good Tailscale module version in `go.mod`. Start with `v1.82.5`; if the implementer toolchain rejects it, bump only the module pin, not the control protocol.

- [ ] **Step 2: Write failing unit tests for protocol parsing and auth middleware**

```go
// codeg-tsnet/control_test.go
package main

import (
  "encoding/json"
  "net/http"
  "net/http/httptest"
  "testing"
)

func TestStatusJSONShape(t *testing.T) {
  s := StatusResponse{State: "needs_login", LoginURL: "https://login.tailscale.com/a/x"}
  b, err := json.Marshal(s)
  if err != nil {
    t.Fatal(err)
  }
  var m map[string]any
  if err := json.Unmarshal(b, &m); err != nil {
    t.Fatal(err)
  }
  if m["state"] != "needs_login" {
    t.Fatalf("state=%v", m["state"])
  }
  if m["loginUrl"] != "https://login.tailscale.com/a/x" {
    t.Fatalf("loginUrl=%v", m["loginUrl"])
  }
}

func TestAuthMiddlewareRejectsMissingToken(t *testing.T) {
  h := withToken("secret", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
    w.WriteHeader(204)
  }))
  rr := httptest.NewRecorder()
  req := httptest.NewRequest(http.MethodGet, "/status", nil)
  h.ServeHTTP(rr, req)
  if rr.Code != http.StatusUnauthorized {
    t.Fatalf("code=%d", rr.Code)
  }
}
```

- [ ] **Step 3: Run tests, expect compile/fail**

Run: `cd codeg-tsnet && go test ./...`  
Expected: FAIL (missing types/handlers)

- [ ] **Step 4: Implement minimal sidecar**

Implement in pure Go (no CGO):

```go
// main.go responsibilities
// - parse flags
// - create tsnet.Server{Dir, Hostname, AuthKey}
// - start control HTTP on 127.0.0.1:0
// - print bootstrap JSON to stdout once
// - wait for /shutdown or signal
```

```go
// funnel.go responsibilities
// - when enabled: use tsnet Serve/Funnel APIs to reverse-proxy to
//   http://127.0.0.1:<localhostPort>
// - when disabled: clear serve/funnel config
// - derive public HTTPS funnel URL from serve config / cert domains
```

Important implementation notes:
- Use `tsnet.Server` userspace only; do not shell out to system `tailscale`.
- Prefer official Funnel/serve configuration APIs available in the pinned Tailscale module (LocalClient ServeConfig / Funnel helpers). If the module surface differs slightly, keep the control protocol fixed and adapt only the internal call.
- On desktop `/up` without auth key, expose `loginUrl` when backend needs interactive login.
- Never log auth keys or control tokens.
- All application logs go to stderr after bootstrap line.

- [ ] **Step 5: Re-run unit tests**

Run: `cd codeg-tsnet && go test ./...`  
Expected: PASS

- [ ] **Step 6: Manual local smoke (binary lifecycle; no real Tailscale account required)**

```bash
mkdir -p /tmp/codeg-ts-state
go build -o codeg-tsnet .
./codeg-tsnet --control-addr 127.0.0.1:0 --state-dir /tmp/codeg-ts-state --hostname codeg-test --control-token testtoken
# capture bootstrap JSON from stdout
curl -H "X-Codeg-Tsnet-Token: testtoken" http://127.0.0.1:<port>/status
curl -X POST -H "X-Codeg-Tsnet-Token: testtoken" http://127.0.0.1:<port>/shutdown
```

Expected: bootstrap JSON prints; `/status` returns JSON; process exits on `/shutdown`.

- [ ] **Step 7: Commit**

```bash
git add codeg-tsnet
git commit -m "feat(tsnet): add codeg-tsnet control-plane sidecar MVP"
```

---

### Task 2: Rust protocol types + binary locator

**Files:**
- Create: `src-tauri/src/web/tailscale/mod.rs`
- Create: `src-tauri/src/web/tailscale/protocol.rs`
- Create: `src-tauri/src/web/tailscale/binary.rs`
- Create: `src-tauri/src/web/tailscale/status.rs`
- Modify: `src-tauri/src/web/mod.rs` (add `pub mod tailscale;`)

**Interfaces:**
- Consumes: Task 1 control protocol
- Produces:
  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  #[serde(rename_all = "camelCase")]
  pub struct SidecarBootstrap {
      pub control_addr: String,
      pub pid: u32,
  }

  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  #[serde(rename_all = "camelCase")]
  pub struct SidecarStatus {
      pub state: String,
      pub login_url: Option<String>,
      pub funnel_url: Option<String>,
      pub hostname: Option<String>,
      pub ipv4: Option<String>,
      pub last_error: Option<String>,
      pub error_key: Option<String>,
      pub backend_state: Option<String>,
  }

  #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
  #[serde(rename_all = "camelCase")]
  pub struct TailscaleFunnelStatus {
      pub supported: bool,
      pub enabled: bool,
      pub state: String,
      pub login_url: Option<String>,
      pub funnel_url: Option<String>,
      pub hostname: Option<String>,
      pub ipv4: Option<String>,
      pub last_error: Option<String>,
  }

  pub fn locate_codeg_tsnet_binary() -> Option<PathBuf>;
  pub fn default_state_dir(data_dir: &Path) -> PathBuf; // data_dir.join("tailscale")
  pub fn default_hostname(data_dir: &Path) -> String;   // codeg-<8 hex of hash(data_dir)>
  ```

Error keys (stable):
- `tailscale.sidecar_missing`
- `tailscale.start_failed`
- `tailscale.login_timeout`
- `tailscale.funnel_denied`
- `tailscale.funnel_failed`
- `tailscale.authkey_required`
- `tailscale.unsupported`

- [ ] **Step 1: Write failing Rust unit tests**

```rust
#[test]
fn parses_bootstrap_json() {
    let raw = r#"{"controlAddr":"127.0.0.1:9","pid":42}"#;
    let b: SidecarBootstrap = serde_json::from_str(raw).unwrap();
    assert_eq!(b.control_addr, "127.0.0.1:9");
    assert_eq!(b.pid, 42);
}

#[test]
fn hostname_is_stable_for_same_data_dir() {
    let p = std::path::PathBuf::from("/tmp/codeg-data-a");
    assert_eq!(default_hostname(&p), default_hostname(&p));
    assert!(default_hostname(&p).starts_with("codeg-"));
}
```

- [ ] **Step 2: Run tests**

Run: `cd src-tauri && cargo test --no-default-features web::tailscale -- --nocapture`  
Expected: FAIL (module missing)

- [ ] **Step 3: Implement protocol + locator**

Mirror `locate_codeg_mcp_binary()` from `src-tauri/src/acp/connection.rs`:

```rust
pub fn locate_codeg_tsnet_binary() -> Option<PathBuf> {
    let filename = if cfg!(windows) { "codeg-tsnet.exe" } else { "codeg-tsnet" };
    if let Some(raw) = std::env::var_os("CODEG_TSNET_BIN") {
        let candidate = PathBuf::from(raw);
        if is_executable_file(&candidate) { return Some(candidate); }
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        let candidate = dir.join(filename);
        if is_executable_file(&candidate) { return Some(candidate); }
    }
    which::which(filename).ok().filter(|p| is_executable_file(p))
}
```

Reuse executable checks similar to MCP locator. Do not invent PATH heuristics beyond that.

- [ ] **Step 4: Re-run tests**

Run: `cd src-tauri && cargo test --no-default-features web::tailscale`  
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/web/tailscale src-tauri/src/web/mod.rs
git commit -m "feat(web): add Tailscale sidecar protocol types and binary locator"
```

---

### Task 3: `TailscaleController` lifecycle

**Files:**
- Create: `src-tauri/src/web/tailscale/controller.rs`
- Modify: `src-tauri/src/web/tailscale/mod.rs`
- Modify: `src-tauri/src/app_state.rs`
- Test: unit tests with mock mapping and missing-binary paths

**Interfaces:**
- Consumes: `locate_codeg_tsnet_binary`, protocol types
- Produces:
  ```rust
  pub struct TailscaleController { /* mutex inner */ }

  impl TailscaleController {
      pub fn new() -> Self;
      pub async fn status(&self) -> TailscaleFunnelStatus;
      pub async fn enable_funnel(&self, opts: EnableFunnelOpts) -> Result<TailscaleFunnelStatus, AppCommandError>;
      pub async fn disable_funnel(&self) -> Result<(), AppCommandError>;
      pub async fn open_login_hint(&self) -> Result<Option<String>, AppCommandError>;
      pub async fn shutdown(&self);
  }

  pub struct EnableFunnelOpts {
      pub data_dir: PathBuf,
      pub localhost_port: u16,
      pub auth_key: Option<String>,
      pub require_auth_key: bool, // true for server/headless
      pub state_dir_override: Option<PathBuf>,
      pub hostname_override: Option<String>,
  }
  ```

Behavior contract:
1. If binary missing -> `supported=false`, error key `tailscale.sidecar_missing`, local web unaffected.
2. Spawn via `crate::process::tokio_command` (Windows `CREATE_NO_WINDOW`).
3. Capture first stdout line as bootstrap JSON (timeout 5s).
4. All control requests include `X-Codeg-Tsnet-Token`.
5. Flow: start -> `POST /up` -> poll `/status` every 1s -> on `online` `POST /funnel` -> poll until `funnel_ready` or error.
6. Desktop: if `needs_login`, surface `loginUrl` and keep polling up to 10 minutes.
7. Server: if `require_auth_key` and no key -> immediate `tailscale.authkey_required`.
8. On disable/stop: `POST /funnel {enabled:false}` best-effort, then `POST /shutdown`, then kill if needed.
9. Sidecar crash -> state `error`; no infinite restart loop.
10. Never touch system Tailscale paths.

- [ ] **Step 1: Write controller tests**

Minimum required tests:
- missing binary -> unsupported/error key
- status mapping from sidecar JSON
- authkey required path

```rust
#[tokio::test]
async fn missing_binary_is_unsupported() {
    // ensure CODEG_TSNET_BIN points nowhere and no sibling binary
    let c = TailscaleController::new();
    let st = c.status().await;
    assert!(!st.supported);
}

#[test]
fn maps_sidecar_status_to_funnel_status() {
    let raw = SidecarStatus {
        state: "funnel_ready".into(),
        login_url: None,
        funnel_url: Some("https://codeg-abc.ts.net".into()),
        hostname: Some("codeg-abc".into()),
        ipv4: Some("100.64.0.1".into()),
        last_error: None,
        error_key: None,
        backend_state: Some("Running".into()),
    };
    let st = map_status(true, true, raw);
    assert_eq!(st.state, "funnel_ready");
    assert_eq!(st.funnel_url.as_deref(), Some("https://codeg-abc.ts.net"));
}
```

- [ ] **Step 2: Run tests, expect fail**

Run: `cd src-tauri && cargo test --no-default-features tailscale`  
Expected: FAIL

- [ ] **Step 3: Implement controller**

Key spawn snippet:

```rust
let mut cmd = crate::process::tokio_command(&bin);
cmd.arg("--control-addr").arg("127.0.0.1:0")
  .arg("--state-dir").arg(&state_dir)
  .arg("--hostname").arg(&hostname)
  .arg("--control-token").arg(&token)
  .kill_on_drop(true)
  .stdout(Stdio::piped())
  .stderr(Stdio::piped());
if let Some(k) = &auth_key { cmd.arg("--auth-key").arg(k); }
let mut child = cmd.spawn().map_err(...)?;
let bootstrap = read_bootstrap_line(child.stdout.take(), Duration::from_secs(5)).await?;
```

HTTP client: existing `reqwest` dependency is fine for localhost JSON.

- [ ] **Step 4: Wire into `AppState`**

```rust
// app_state.rs
pub tailscale: Arc<crate::web::tailscale::TailscaleController>,
```

Construct with `Arc::new(TailscaleController::new())` in desktop setup and `codeg-server` bootstrap.

- [ ] **Step 5: Tests pass + commit**

```bash
cd src-tauri && cargo test --no-default-features web::tailscale
git add src-tauri/src/web/tailscale src-tauri/src/app_state.rs src-tauri/src/bin/codeg_server.rs src-tauri/src/lib.rs
git commit -m "feat(web): add TailscaleController sidecar lifecycle"
```

---

### Task 4: Web Service config/status/API integration

**Files:**
- Modify: `src-tauri/src/web/mod.rs`
- Create: `src-tauri/src/web/handlers/tailscale.rs`
- Modify: `src-tauri/src/web/handlers/mod.rs`
- Modify: `src-tauri/src/web/router.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/bin/codeg_server.rs`

**Interfaces:**
- Consumes: controller + existing web start/stop
- Produces:

Config:
```rust
// WebServiceConfig gains:
pub funnel_enabled: bool,
```
metadata key: `web_service_funnel_enabled` (`true`/`false`)

Commands/HTTP:
- `get_tailscale_funnel_status` -> `TailscaleFunnelStatus`
- `set_tailscale_funnel_enabled { enabled: bool }` -> `TailscaleFunnelStatus`
- `open_tailscale_login` -> `{ loginUrl?: string }`

Lifecycle rules:
- Desktop start web success + `funnelEnabled` -> `enable_funnel`
- Desktop stop web -> `disable_funnel` first (best-effort), then stop axum
- Desktop auto-start web path in `lib.rs` also starts Funnel when configured
- Server: after bind/mark external running, if `CODEG_TS_FUNNEL=1` (or persisted funnelEnabled), call enable with `require_auth_key=true` and `CODEG_TS_AUTHKEY`
- Server stop endpoint still cannot stop whole server; Funnel disable remains allowed via set-enabled false
- Port change while funnel enabled: disable then enable with new port

- [ ] **Step 1: Extend config load/update**

Add `funnel_enabled` to `WebServiceConfig` with default `false`. Persist with existing transaction in `update_web_service_config_core`.

- [ ] **Step 2: Implement hooks**

After successful web bind (desktop path):

```rust
if load_web_service_config(...).await?.funnel_enabled {
    let _ = app_state.tailscale.enable_funnel(EnableFunnelOpts {
        data_dir: app_state.data_dir.clone(),
        localhost_port: actual_port,
        auth_key: std::env::var("CODEG_TS_AUTHKEY").ok(),
        require_auth_key: false,
        state_dir_override: std::env::var_os("CODEG_TS_STATE_DIR").map(PathBuf::from),
        hostname_override: std::env::var("CODEG_TS_HOSTNAME").ok(),
    }).await;
}
```

Prefer wrapper approach for stop to minimize signature churn:
- Tauri/HTTP `stop_web_server` call `state.tailscale.disable_funnel().await` first when not externally managed.
- External/server process shutdown path in `codeg_server.rs` also disables funnel before exit.

- [ ] **Step 3: Register routes and Tauri commands**

Router (auth-protected like other settings commands):

```rust
"/get_tailscale_funnel_status" => post(handlers::tailscale::get_status)
"/set_tailscale_funnel_enabled" => post(handlers::tailscale::set_enabled)
"/open_tailscale_login" => post(handlers::tailscale::open_login)
```

Tauri:

```rust
web::tailscale::get_tailscale_funnel_status,
web::tailscale::set_tailscale_funnel_enabled,
web::tailscale::open_tailscale_login,
```

- [ ] **Step 4: Server env path**

In `codeg_server.rs` after server is marked running:

```rust
let funnel_env = matches!(std::env::var("CODEG_TS_FUNNEL").as_deref(), Ok("1") | Ok("true") | Ok("yes"));
if funnel_env {
    match state.tailscale.enable_funnel(EnableFunnelOpts {
        data_dir: state.data_dir.clone(),
        localhost_port: port,
        auth_key: std::env::var("CODEG_TS_AUTHKEY").ok(),
        require_auth_key: true,
        state_dir_override: std::env::var_os("CODEG_TS_STATE_DIR").map(PathBuf::from),
        hostname_override: std::env::var("CODEG_TS_HOSTNAME").ok(),
    }).await {
        Ok(st) => tracing::info!(?st.funnel_url, ?st.state, "[SERVER] Tailscale Funnel status"),
        Err(err) => tracing::warn!(%err, "[SERVER][WARN] Tailscale Funnel not enabled"),
    }
}
```

Do not print auth key. Funnel URL may be logged.

- [ ] **Step 5: Verify compile**

Run:
```bash
cd src-tauri
cargo check --no-default-features --bin codeg-server
cargo check --bin codeg
```
Expected: success (or only pre-existing unrelated warnings)

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/web src-tauri/src/lib.rs src-tauri/src/app_state.rs src-tauri/src/bin/codeg_server.rs
git commit -m "feat(web): wire Tailscale Funnel into web service lifecycle and APIs"
```

---

### Task 5: Frontend settings UI + i18n + API client

**Files:**
- Modify: `src/lib/api.ts`
- Create: `src/components/settings/web-service-funnel-section.tsx`
- Create: `src/components/settings/web-service-funnel-section.test.tsx`
- Modify: `src/components/settings/web-service-settings.tsx`
- Modify: `src/i18n/messages/en.json`
- Modify: `src/i18n/messages/zh-CN.json`
- Modify other locale files under `src/i18n/messages/` with the same keys (English text acceptable if full translation not available)

**Interfaces:**
```ts
export interface TailscaleFunnelStatus {
  supported: boolean
  enabled: boolean
  state:
    | "stopped"
    | "starting"
    | "needs_login"
    | "connecting"
    | "online"
    | "funnel_enabling"
    | "funnel_ready"
    | "error"
    | "stopping"
  loginUrl?: string
  funnelUrl?: string
  hostname?: string
  ipv4?: string
  lastError?: string
}

export async function getTailscaleFunnelStatus(): Promise<TailscaleFunnelStatus>
export async function setTailscaleFunnelEnabled(enabled: boolean): Promise<TailscaleFunnelStatus>
export async function openTailscaleLogin(): Promise<{ loginUrl?: string | null }>
```

Extend:
```ts
export interface WebServiceConfig {
  token: string | null
  port: number | null
  autoStart: boolean
  funnelEnabled: boolean
}
```

UI requirements (Funnel section under Web Service):
- Switch: enable Funnel (disabled when web service stopped on desktop)
- Badge for state
- Button: open login when `needs_login`
- Funnel URL row with copy/open
- Error text mapped from known keys
- Notes:
  - Codeg token still required
  - Uses private node/state dir; does not touch system Tailscale
- Poll status every 1.5s while state is transitional (`starting|needs_login|connecting|online|funnel_enabling|stopping`)

- [ ] **Step 1: Add API wrappers**

Implement next to existing web-server API block in `src/lib/api.ts`.

- [ ] **Step 2: Write component test**

```tsx
it("shows login action when needs_login", async () => {
  // mock getTailscaleFunnelStatus -> needs_login + loginUrl
  // render section, assert login button visible
})

it("renders funnel url when ready", async () => {
  // mock funnel_ready + funnelUrl
})
```

- [ ] **Step 3: Implement section component and mount it**

In `web-service-settings.tsx`:
- include `funnelEnabled` in persist payload
- render `<WebServiceFunnelSection webRunning={isRunning} />` below start/stop controls

Use existing patterns: `Switch`, `openUrl`, `copyTextToClipboard`, `useTranslations("WebServiceSettings")`.

- [ ] **Step 4: i18n keys**

Add under `WebServiceSettings` in en + zh-CN at minimum:

```json
"funnelTitle": "Public Access (Tailscale Funnel)",
"funnelDescription": "Expose this Web Service on a public HTTPS URL via Codeg's private Tailscale node.",
"funnelEnable": "Enable Funnel",
"funnelEnableHint": "Requires local Web Service running. Does not use or replace system Tailscale.",
"funnelState": "Funnel status",
"funnelLogin": "Open Tailscale login",
"funnelUrl": "Public URL",
"funnelTokenNote": "Your Codeg access token is still required after opening the public URL.",
"funnelPrivateNodeNote": "Codeg keeps Tailscale state under its own data directory and never manages system Tailscale.",
"funnelUnsupported": "Funnel sidecar is unavailable in this install",
"funnelErrors": {
  "sidecarMissing": "codeg-tsnet sidecar binary not found",
  "startFailed": "Failed to start Tailscale sidecar",
  "loginTimeout": "Tailscale login timed out",
  "funnelDenied": "Funnel is not permitted for this Tailscale account/policy",
  "funnelFailed": "Failed to enable Funnel",
  "authkeyRequired": "CODEG_TS_AUTHKEY is required for headless Funnel",
  "unsupported": "Tailscale Funnel is not supported here"
}
```

- [ ] **Step 5: Run frontend tests**

Run:
```bash
pnpm test -- web-service-funnel-section.test.tsx
```
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/lib/api.ts src/components/settings/web-service-funnel-section.tsx src/components/settings/web-service-funnel-section.test.tsx src/components/settings/web-service-settings.tsx src/i18n/messages
git commit -m "feat(ui): add Tailscale Funnel controls to Web Service settings"
```

---

### Task 6: Packaging, CI, installers, Docker

**Files:**
- Modify: `src-tauri/scripts/prepare-sidecars.mjs`
- Modify: `src-tauri/tauri.conf.json`
- Modify: `.github/workflows/release.yml`
- Modify: `Dockerfile`
- Modify: `Dockerfile.ci`
- Modify: `install.sh`
- Modify: `install.ps1`

**Interfaces / packaging contract:**
- Desktop externalBin: `["binaries/codeg-mcp", "binaries/codeg-tsnet"]`
- `prepare-sidecars.mjs` builds both sidecars for target triple
- Server artifacts include `codeg-tsnet` next to `codeg-server` and `codeg-mcp`
- Docker image includes `/usr/local/bin/codeg-tsnet`
- Installers manage `codeg-tsnet` as a third managed bin
- `CODEG_SKIP_SIDECAR=1` skips both sidecars (dev convenience)

- [ ] **Step 1: Extend prepare-sidecars**

Refactor script from single `BIN_NAME` to a list of sidecars:
1. `codeg-mcp` via existing cargo build
2. `codeg-tsnet` via `go build` with `GOOS`/`GOARCH` derived from rust triple

Triple -> Go env mapping:
- `x86_64-apple-darwin` -> `GOOS=darwin GOARCH=amd64`
- `aarch64-apple-darwin` -> `GOOS=darwin GOARCH=arm64`
- `x86_64-unknown-linux-gnu` -> `GOOS=linux GOARCH=amd64`
- `aarch64-unknown-linux-gnu` -> `GOOS=linux GOARCH=arm64`
- `x86_64-pc-windows-msvc` -> `GOOS=windows GOARCH=amd64`
- `aarch64-pc-windows-msvc` -> `GOOS=windows GOARCH=arm64`

Output staged as `src-tauri/binaries/codeg-tsnet-<triple>{.exe}`.

- [ ] **Step 2: Update tauri.conf.json externalBin**

```json
"externalBin": ["binaries/codeg-mcp", "binaries/codeg-tsnet"]
```

- [ ] **Step 3: Update release.yml**

For desktop + server jobs:
1. Add Go setup:
```yaml
- uses: actions/setup-go@v5
  with:
    go-version: "1.22.x"
```
2. Build/stage `codeg-tsnet` for matrix target (via prepare-sidecars or explicit `go build`).
3. Verify file exists next to existing `codeg-mcp` checks.
4. Package into tar.gz/zip and Docker artifact copies.
5. Smoke: `codeg-tsnet --help` on native runners (Windows/macOS/Linux x64; skip cross arm exec).

- [ ] **Step 4: Docker + installers**

Dockerfile backend stage needs Go and must ship `/usr/local/bin/codeg-tsnet`.

`install.sh` / `install.ps1`:
```bash
MANAGED_BINS=(codeg-server codeg-mcp codeg-tsnet)
```
```powershell
$ManagedBins = @("codeg-server", "codeg-mcp", "codeg-tsnet")
```

Include existence smoke for `codeg-tsnet` like `codeg-mcp`.

- [ ] **Step 5: Local packaging dry-run**

```bash
pnpm tauri:prepare-sidecars
ls src-tauri/binaries
```

Expected: both `codeg-mcp-<triple>` and `codeg-tsnet-<triple>` present.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/scripts/prepare-sidecars.mjs src-tauri/tauri.conf.json .github/workflows/release.yml Dockerfile Dockerfile.ci install.sh install.ps1
git commit -m "build: package codeg-tsnet sidecar across desktop, server, and Docker"
```

---

### Task 7: Docs + acceptance verification + bilingual PR

**Files:**
- Modify: `README.md` (env table section if present)
- Modify: `docs/readme/README.zh-CN.md`

**Docs content to add:**
- Funnel is optional
- Desktop: enable in Web Service settings, browser login
- Server:
  - `CODEG_TS_FUNNEL=1`
  - `CODEG_TS_AUTHKEY=...`
  - optional `CODEG_TS_STATE_DIR`, `CODEG_TS_HOSTNAME`, `CODEG_TSNET_BIN`
- State lives under `<CODEG_DATA_DIR>/tailscale`
- Does not use system Tailscale
- Codeg token still required

- [ ] **Step 1: Write docs deltas**

Keep concise; mirror existing `CODEG_MCP_BIN` documentation style.

- [ ] **Step 2: Acceptance checklist (manual)**

Desktop (macOS or Windows or Linux):
1. Start Web Service locally -> works without Funnel.
2. Enable Funnel without sidecar binary present -> error state, local web still up.
3. With sidecar present + Tailscale account: enable Funnel -> login URL -> after login show public URL.
4. Open public URL -> still prompted for Codeg token.
5. Disable Funnel / stop Web Service -> `codeg-tsnet` process exits.
6. Confirm no changes to system Tailscale prefs/state.

Server:
1. `CODEG_TS_FUNNEL=1` without auth key -> Funnel error `authkey_required`, server still serves local/LAN.
2. With auth key -> Funnel URL logged/status API returns ready.
3. Artifact contains `codeg-tsnet` sibling binary.

Automated:
```bash
cd codeg-tsnet && go test ./...
cd src-tauri && cargo test --no-default-features web::tailscale
pnpm test -- web-service-funnel-section.test.tsx
pnpm tauri:prepare-sidecars
```

- [ ] **Step 3: Open bilingual PR from fork**

Remote push target: `fork` (`ijry/codeg-plus`), base `xintaofei/codeg` `main`.

PR title:
```text
feat: optional Tailscale Funnel public access via codeg-tsnet sidecar
```

PR body structure:

```markdown
## 中文
### 背景
...
### 方案
- Go sidecar `codeg-tsnet`（tsnet userspace）
- 独立状态目录，不接管系统 Tailscale
- 桌面浏览器登录 / 服务端 auth key
- Windows 随 sidecar 一并支持
### 验证
...

## English
### Background
...
### Approach
...
### Validation
...
```

- [ ] **Step 4: Final commit if docs remain**

```bash
git add README.md docs/readme/README.zh-CN.md
git commit -m "docs: document optional Tailscale Funnel sidecar"
```

Then push branch and create PR:
```bash
git push -u fork HEAD
gh pr create --repo xintaofei/codeg --base main --title "..." --body "..."
```

---

## Spec Coverage Self-Review

| Spec requirement | Task |
|---|---|
| Optional Funnel public HTTPS | 3,4,5 |
| Own userspace node + independent state dir | 1,2,3 |
| Never touch system Tailscale | 1,3,7 |
| Desktop browser OAuth guidance | 1,3,5 |
| Server auth key headless | 3,4,7 |
| Settings switch `funnelEnabled` | 4,5 |
| Sidecar packaging like `codeg-mcp` | 6 |
| Windows first-class via sidecar | 1,6 |
| Funnel failure does not break local web | 3,4 |
| Codeg token still required | 5,7 |
| Control protocol + status machine | 1,2,3 |
| Release matrix / Docker / installers | 6 |
| Bilingual PR | 7 |

## Placeholder / Consistency Check

- Control auth header fixed: `X-Codeg-Tsnet-Token`
- Funnel status is dedicated endpoints (not overloaded into `WebServerInfo`)
- Sidecar is started on demand when Funnel enabled; stopped on disable/web stop
- Funnel URL comes from sidecar status field `funnelUrl`
- No TBD left for architecture decisions; only Tailscale module pin may adjust if `v1.82.5` fails to build, without changing protocol

## Execution Notes

- Work from repo root `codeg/`.
- Do not push to `origin` (`xintaofei/codeg`); use `fork`.
- Design doc already committed on docs branch; implementation should use a feature branch from latest `main` (or continue and include docs commit) before PR.
- Prefer small commits at each task boundary above.
