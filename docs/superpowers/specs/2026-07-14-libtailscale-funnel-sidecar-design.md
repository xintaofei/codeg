# Codeg Tailscale Funnel Sidecar Design

**Date:** 2026-07-14  
**Status:** Approved for implementation planning  
**Scope:** Desktop embedded Web Service + standalone `codeg-server`  
**Approach:** Go `tsnet` sidecar (`codeg-tsnet`), not in-process libtailscale C bindings

## Problem

Codeg can expose a local Web Service (desktop embedded HTTP server or standalone `codeg-server`) on LAN/loopback with token auth. Users also need optional **public remote access** without:

- taking over the system Tailscale daemon
- sharing the system Tailscale node identity/state
- replacing Codeg's existing application token auth

## Goals

1. Optional public HTTPS access via **Tailscale Funnel**.
2. Application always uses its **own userspace node** and **independent state directory**.
3. Never manage or replace system Tailscale.
4. Desktop auth: browser OAuth/login guidance.
5. Server/Docker auth: auth key / headless credentials.
6. Enablement is an optional switch inside Web Service settings (not always-on).
7. Ship on the existing release matrix, including Windows, via sidecar packaging similar to `codeg-mcp`.
8. Funnel failure must not break local Web Service.

## Non-goals

- Using system `tailscaled` or `tailscale up` against the host node.
- Making Funnel the only access path.
- Replacing Codeg bearer token auth with Tailscale identity auth.
- Exposing raw tailnet `listen/accept` as the primary app serving path.
- Linking Go/CGO libtailscale into the main Rust binary.

## Why sidecar instead of in-process libtailscale

In-process libtailscale embeds `tsnet` through a C archive. That path is attractive on Unix, but:

- connection plumbing relies on Unix-style FD passing (`socketpair`, rights)
- upstream Windows port work is incomplete
- CGO + static archive complicates the Rust/Tauri build matrix

A dedicated Go sidecar:

- uses pure Go `tsnet` (first-class on Windows/macOS/Linux)
- keeps Funnel control in-process to the sidecar only
- reuses Codeg's existing sidecar packaging pattern (`codeg-mcp`)
- isolates crashes: sidecar death disables Funnel, not Axum

## Architecture

```text
Settings UI / env
        |
        v
codeg / codeg-server (Rust)
  - Axum Web Service on 0.0.0.0:<port>
  - TailscaleController (control plane only)
        |
        | spawn + localhost control API
        v
codeg-tsnet (Go sidecar)
  - tsnet.Server userspace node
  - state: <CODEG_DATA_DIR>/tailscale
  - OAuth / auth key
  - Funnel -> http://127.0.0.1:<webPort>
        |
        v
Public HTTPS Funnel URL
```

### Components

#### 1. `codeg-tsnet` (new Go binary)

Responsibilities:

- create/configure userspace node
- independent state dir + hostname
- login (browser URL or auth key)
- enable/disable Funnel to localhost web port
- report structured status
- graceful shutdown

Non-responsibilities:

- no business API
- no static asset serving
- no replacement for Axum

#### 2. `TailscaleController` (Rust)

Lives under `src-tauri` web/runtime code.

Responsibilities:

- locate sidecar binary
- start/stop lifecycle tied to Web Service funnel switch
- poll/map status for Tauri commands and HTTP handlers
- degrade cleanly when unsupported/missing/crashed

Binary resolution order (aligned with `codeg-mcp`):

1. `CODEG_TSNET_BIN`
2. sibling of current executable
3. Tauri externalBin / resource path

#### 3. Web Service integration

- Config flag: `funnelEnabled`
- Start order: bind web port successfully -> start sidecar -> enable Funnel
- Stop order: disable Funnel / stop sidecar -> stop web server (desktop path)
- `codeg-server` remains externally managed; Funnel can stop without stopping the whole process

#### 4. Frontend (Web Service settings)

Add a Funnel section to the existing Web Service page:

- enable switch
- state badge
- open login action
- funnel URL + copy/open
- error text
- note that Codeg token is still required
- note that Codeg uses a private node/state dir and does not touch system Tailscale

## State and identity

| Item | Value |
|---|---|
| State directory | `<CODEG_DATA_DIR>/tailscale` (desktop: effective app data dir) |
| Override | `CODEG_TS_STATE_DIR` |
| Hostname | `codeg-<stable-short-id>` |
| Override | `CODEG_TS_HOSTNAME` |
| Node type | userspace `tsnet`, Codeg-owned |
| System Tailscale | untouched |

The short id should be stable per installation/data dir so restarts reattach to the same node identity stored in the state directory.

## Control protocol

Sidecar starts with:

```bash
codeg-tsnet \
  --control-addr 127.0.0.1:0 \
  --state-dir <dir> \
  --hostname codeg-xxxx \
  [--auth-key ...] \
  [--control-token ...]
```

Bootstrap on stdout (single JSON line):

```json
{"controlAddr":"127.0.0.1:54321","pid":12345}
```

Localhost HTTP JSON API:

| Endpoint | Purpose |
|---|---|
| `GET /status` | state, IPs, loginUrl, funnelUrl, lastError |
| `POST /up` | connect; optional `{ "authKey": "..." }` |
| `POST /funnel` | `{ "enabled": true, "localhostPort": 3080 }` |
| `POST /logout` | optional session clear |
| `POST /shutdown` | graceful exit |

Security:

- control server binds `127.0.0.1` only
- optional/random control token required on requests
- auth keys never logged
- control port never published via Funnel

## State machine

```text
stopped
  -> starting
  -> needs_login
  -> connecting
  -> online
  -> funnel_enabling
  -> funnel_ready
  -> error
  -> stopping
  -> stopped
```

`needs_login` includes `loginUrl` for desktop browser guidance.  
Controller polls `/status` every 1-2s while login/funnel is in progress.

## Auth flows

### Desktop (browser OAuth)

1. User enables Funnel in Web Service settings.
2. Ensure local Web Service is running on `port`.
3. Start `codeg-tsnet` with Codeg state dir/hostname.
4. `POST /up` without auth key.
5. If `needs_login`, open `loginUrl` in browser and show waiting UI.
6. On `online`, `POST /funnel` with `localhostPort=port`.
7. Show `funnelUrl`.
8. On disable/stop: disable Funnel and shutdown sidecar.

### Server / Docker (auth key)

1. `CODEG_TS_FUNNEL=1` (or persisted equivalent) and web server running.
2. Require `CODEG_TS_AUTHKEY`; if missing, fail Funnel with explicit error (no interactive browser wait).
3. Start sidecar / `POST /up` with auth key.
4. Enable Funnel to local web port.
5. Emit Funnel URL on stderr/logs and status API.

### Environment variables

| Variable | Meaning |
|---|---|
| `CODEG_TS_FUNNEL` | enable Funnel for server mode |
| `CODEG_TS_AUTHKEY` | headless auth key |
| `CODEG_TS_STATE_DIR` | override state directory |
| `CODEG_TS_HOSTNAME` | override node hostname |
| `CODEG_TSNET_BIN` | override sidecar path |

## API / config surface

### Config extension

```ts
WebServiceConfig {
  token?: string | null
  port?: number | null
  autoStart: boolean
  funnelEnabled: boolean
}
```

### Status extension

```ts
TailscaleFunnelStatus {
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
```

Expose via:

- Tauri commands for desktop settings
- HTTP handlers for web mode / server UI
- either extend web-server status or add dedicated funnel status endpoints

Suggested stable error keys:

- `tailscale.sidecar_missing`
- `tailscale.start_failed`
- `tailscale.login_timeout`
- `tailscale.funnel_denied`
- `tailscale.funnel_failed`
- `tailscale.authkey_required`
- `tailscale.unsupported`

## Failure handling

| Scenario | Behavior |
|---|---|
| Sidecar binary missing | Funnel unavailable; local web continues |
| Login timeout | error state; retryable; web continues |
| Funnel not permitted on account/policy | error with actionable key |
| Sidecar crash | mark error; optional single restart; web continues |
| Port change while Funnel enabled | rebind Funnel to new port or recycle sidecar cleanly |
| Server external mode stop web | reject stopping whole server; only Funnel stop is valid |

## Packaging and CI

Follow `codeg-mcp` patterns.

### Artifacts

- Desktop: `codeg-tsnet-<triple>` via Tauri `externalBin`
- Server tarball/zip: place next to `codeg-server` and `codeg-mcp`
- Docker image: include `codeg-tsnet` discoverable by sibling lookup

### GitHub Actions changes

- install Go in desktop/server build jobs
- build `codeg-tsnet` for each release matrix target:
  - macOS x64/arm64
  - Linux x64/arm64
  - Windows x64 (and desktop Windows arm64 if matrix includes it)
- smoke `--help` / version on native runners
- missing/failed Funnel support must not fail basic app startup tests
- release packaging must include the sidecar where the platform build succeeds

Windows is a first-class packaging target with sidecar. Runtime Funnel on Windows is an intended supported path; if a host-specific runtime failure occurs, surface a clear error and keep local web working.

## Security

1. Funnel makes the service publicly reachable; **Codeg token remains mandatory**.
2. State dir permissions restricted to the user/process owner.
3. Auth keys and control tokens never appear in info logs or UI secrets panels as re-readable secrets beyond intentional local config storage.
4. Control API localhost-only.
5. Do not write Funnel public endpoints that bypass app auth.

## Testing strategy

1. **Unit:** state machine, config parsing, binary lookup, error key mapping.
2. **Controller integration:** mock sidecar HTTP for login/online/funnel/crash paths.
3. **Sidecar contract tests:** schema and status transitions with a fake/minimal control server or tsnet test doubles where practical.
4. **UI tests:** switch, needs_login, funnel URL, error rendering.
5. **Manual acceptance:** real Tailscale account on macOS/Linux; auth-key path for server; Windows verification as available.

Automated CI should not require a real Tailscale account.

## Acceptance criteria

1. Enabling Funnel on desktop guides browser login when needed and yields a public HTTPS URL.
2. Access via Funnel still requires Codeg token.
3. Node state lives only under Codeg data dir; system Tailscale remains untouched.
4. Disabling Funnel / stopping Web Service stops the sidecar.
5. `codeg-server` can enable Funnel headlessly with auth key.
6. Release artifacts for Linux/macOS/Windows include `codeg-tsnet` when built.
7. Sidecar absence degrades Funnel only.
8. PR description is bilingual (Chinese + English).

## Implementation phases

1. `codeg-tsnet` MVP + control protocol + status schema.
2. Rust `TailscaleController` + Web Service config/status hooks.
3. Settings UI + i18n.
4. Sidecar packaging in prepare-sidecars / server / Docker / release workflow.
5. Server env path, docs, and acceptance verification.

## Open implementation choices (non-blocking)

These may be settled in the implementation plan without changing the architecture:

- exact control auth header name
- whether funnel fields are embedded in web-server status or separate endpoints
- whether sidecar is always ephemeral-restarted or long-lived across web restarts when funnel stays enabled
- exact Funnel URL formatting from tsnet cert domain / serve config

## References

- Existing Web Service: `src-tauri/src/web/mod.rs`, settings UI under `src/components/settings/web-service-settings.tsx`
- Existing sidecar pattern: `codeg-mcp` + `pnpm tauri:prepare-sidecars`
- Upstream capability basis: Tailscale `tsnet` + Funnel serve config
- Rejected alternative: in-process libtailscale C binding as primary integration
