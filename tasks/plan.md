# Model Provider Backend Design: models.json

## Goal

Use a pios-compatible `models.json` file as the source of truth for shared model providers
and their models. SQLite remains responsible only for codeg-owned runtime state such as
agent enablement/source and conversation records. The provider catalog itself is not copied
into database tables.

An agent setting only says whether the agent uses `native` config or the shared Model
Provider catalog. A conversation then chooses `provider -> model`. Login-only agents remain
native.

## Storage

### Path

- Default: `<CODEG_DATA_DIR>/models.json`
- Override: `CODEG_MODELS_JSON=/path/to/models.json`
- The override lets a user point codeg directly at an existing pios file such as
  `~/.pi/agent/models.json`, but codeg must not silently rewrite the pios path by default.
- Create the parent directory with restrictive permissions on first write; create the file
  with `0600` on Unix.

### Canonical shape

```json
{
  "version": 1,
  "providers": {
    "deepseek": {
      "enabled": true,
      "api": "openai-completions",
      "baseUrl": "https://api.deepseek.com/v1",
      "proxy": "http://127.0.0.1:7890",
      "apiKey": "sk-...",
      "authHeader": true,
      "models": [
        {
          "id": "deepseek-chat",
          "reasoning": false,
          "input": ["text"]
        },
        {
          "id": "deepseek-reasoner",
          "reasoning": true,
          "input": ["text"],
          "contextWindow": 128000,
          "maxTokens": 8192
        }
      ]
    }
  }
}
```

Rules:

- `providers` is a map whose key is the user-facing provider id.
- Provider order is file key order. Preserve insertion order on read and write.
- No `agent_type` is stored on a provider. Compatibility is derived from `api`.
- No provider display name and no model display name are required by codeg.
- `name`, `compat`, and `headers` may be accepted for pios import compatibility. `headers`
  can contain credentials and must never be returned to the browser.
- `input` uses the pios array form: `"text"` and/or `"image"`. The frontend's current
  `text-image` enum is a presentation mapping.
- New UI rows default to `reasoning: true` and `input: ["text", "image"]`.
- `apiKey` is stored directly in models.json. It is never returned raw by the API.

### JSON format

- Parse with comment stripping so an existing pios-style JSONC file can be read.
- Canonical writes are pretty JSON with a trailing newline. Comments are not preserved after
  a backend write.
- Use `serde_json` with ordered maps, or an equivalent ordered-map representation, so
  provider order survives a write.
- Missing file means an empty catalog. Invalid JSON is a hard read error: never silently
  replace it with `{}` and never overwrite it until the user fixes or explicitly resets it.

## Concurrency and durability

Wrap all reads and read-modify-write operations behind one store:

- In-process async mutex for normal CRUD.
- Advisory file lock, for example `models.json.lock`, for safety when multiple server
  processes are accidentally started.
- Atomic write sequence:
  1. lock
  2. read/parse
  3. mutate in memory
  4. write `models.json.tmp-<pid>-<random>`
  5. fsync file
  6. rename to `models.json`
  7. fsync parent directory where supported
- Cache the parsed catalog plus file metadata/hash. Invalidate on backend writes and on an
  external file watcher event.
- Emit `app://model-providers-updated` after a successful write.

The store owns parsing, validation, ordering, secret masking, and atomic persistence. Tauri
commands, Axum handlers, and launch adapters should not read the file directly.

## Data model

### `ModelsFile`

- `version`: `1`
- `providers`: ordered map from provider id to `ProviderConfig`

### `ProviderConfig`

- `providerId`: map key, immutable after creation
- `enabled`: boolean, default true
- `api`: `openai-completions`, `openai-responses`, `anthropic-messages`, or
  `google-generative-ai`
- `baseUrl`: required http/https URL
- `proxy`: optional http/https proxy URL
- `apiKey`: optional secret
- `authHeader`: optional boolean; false means API-native auth, true forces Bearer-style
- `compat`: optional pios-compatible compatibility overrides
- `headers`: optional backend-only headers
- `models`: at least one `ModelConfigEntry`

### `ModelConfigEntry`

- `id`: upstream model id, unique within the provider
- `reasoning`: boolean
- `input`: array containing `"text"` and/or `"image"`
- `contextWindow`: optional integer
- `maxTokens`: optional integer
- `baseInstructions`: optional text, only projected by adapters that support it
- `compat`: optional pios-compatible compatibility overrides

Provider ids are immutable because conversations reference them by id. The editor may allow
choosing an id only at creation; rename is not supported. This avoids rewriting conversation
references after a file edit.

## Validation

- Provider id must match `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`.
- Duplicate provider id is a conflict.
- Base URL must be http/https and at most 2048 characters.
- API key must be at most 4096 characters.
- A provider must contain at least one model.
- Every model id must be non-empty and unique within the provider.
- Numeric metadata must be positive when present.
- Proxy, when present, must be http/https.
- `compat` unknown keys are dropped; malformed values are rejected or normalized like pios.
- Third-party probe responses are untrusted and parsed defensively.

## Capabilities

Move the API capability table into Rust:

- Claude Code: `anthropic-messages`
- Codex: `openai-completions`, `openai-responses`
- Gemini: `google-generative-ai`
- Open Code, Pi, Kimi Code: all four
- Cline: `anthropic-messages`, `openai-completions`, `google-generative-ai`
- Grok: `openai-completions`, `openai-responses`, `anthropic-messages`
- Hermes: `openai-completions`, `anthropic-messages`, `google-generative-ai`
- Code Buddy: `openai-completions`
- DeepSeek: `openai-completions`
- Antigravity: `google-generative-ai`
- Open Claw, Cursor, Qoder: no Model Provider source

Custom ACP agents can later declare a list in `custom_agent`; empty means disabled.

## API contract

Keep one core service, one Tauri command layer, and one Axum handler layer. JSON payload
shapes stay aligned with the current frontend `ModelProviderService` mock.

- `list_model_providers() -> ModelProviderRecord[]`
- `list_builtin_model_providers() -> BuiltinProviderInfo[]`
- `create_model_provider(draft) -> SaveResult`
- `update_model_provider(draft) -> SaveResult`
- `delete_model_provider(providerId) -> void`
- `set_model_provider_enabled(providerId, enabled) -> void`
- `reorder_model_providers(providerIds) -> void`
- `probe_model_provider_models(params) -> ProbeOutcome`
- `test_model_provider_model(providerId, modelId, apiKey?) -> TestOutcome`
- `clone_builtin_model_provider(builtinId) -> ModelProviderDraft`

`ModelProviderRecord` returns only safe fields:

- `providerId`
- `api`
- `baseUrl`
- `proxy`
- `enabled`
- `models`
- `apiKeyMasked`
- `hasApiKey`

Key semantics:

- A blank API key on update means “keep the existing key”.
- Add `clearApiKey: true` for an explicit clear action; this avoids overloading blank text.
- Raw API keys and `headers` never enter responses or logs.

Built-in templates remain Rust constants. `configured` is computed from a matching saved
base URL plus a stored API key. Cloning returns a non-persisted draft with a unique provider
id and empty API key.

## Agent source state

Extend `agent_setting` with `model_source`:

- `native`: existing agent-owned configuration.
- `provider`: use `models.json`.

Expose `model_source` on `AcpAgentInfo`.

New API:

```text
acp_update_agent_model_source(agentType, modelSource) -> affectedRunningSessions
```

Saving emits the agents-updated event and marks running sessions stale through the existing
config-fingerprint mechanism.

There is no provider or model selection in Agent Settings.

## Realtime picker updates

Catalog changes must become visible in the conversation model selector without restarting
codeg or the frontend.

Flow:

1. A successful models.json write updates the backend cache.
2. Backend emits `app://model-providers-updated`.
3. Frontend provider/model stores invalidate their query cache and refetch the catalog.
4. The conversation selector recomputes the visible list from the new catalog:
   - enabled providers/models appear
   - disabled providers/models disappear
   - providers whose API family is incompatible with the selected agent disappear
   - deleted providers/models disappear
5. Counts such as `Model Provider (N)` are recomputed from the refreshed catalog.

Selected-value rules:

- If the selected provider/model still exists and remains enabled/compatible, keep it.
- If only provider metadata such as base URL, API key, proxy, or model metadata changed, keep
  the selection.
- If the selected model was removed or disabled, clear only the model part and keep the
  provider if it still has compatible models.
- If the provider was removed/disabled/became incompatible, clear the whole selection.
- Historical transcript display may retain the old model id; only future launch/turn
  validation uses the current catalog.

Active-session rules:

- Changing catalog data does not mutate an already-running agent process.
- A running session that selected affected provider/model data is marked stale/restart-required.
- For a not-yet-launched conversation or next turn after restart, the latest catalog and
  selection apply immediately.

## Conversation selection

SQLite still stores the selected reference because conversations are runtime/session state,
not provider catalog data.

Add to conversation:

- `model_provider_id`: provider id string from models.json
- `model_provider_model_id`: model id string
- `model_source`: nullable `native | provider`; null preserves legacy behavior

The existing `conversation.model` remains the display/runtime model id. In provider mode it
is set to the selected model id.

Validation before save and launch:

- provider exists in models.json
- provider is enabled
- model exists in that provider
- provider API is in the agent capability list

Changing provider/model for a live session marks it restart-required unless the agent adapter
explicitly supports a live model switch.

## Launch flow

Extend runtime resolution with:

```rust
pub struct ModelSelection {
    pub provider_id: String,
    pub model_id: String,
}
```

Launch sequence:

1. Resolve agent settings.
2. If `model_source == provider`, require a concrete selection.
3. Load/resolve provider/model from the models.json store.
4. Validate capability, enabled state, and model membership.
5. Run the agent-specific launch adapter.
6. Merge resulting env/config artifacts into the runtime launch.
7. Include resolved provider/model material in the config fingerprint.

Delegation and background launches inherit the parent conversation’s selection. If no
selection exists, fail clearly rather than silently choosing an arbitrary provider.

## Launch adapters

Add a small adapter layer instead of scattering provider logic through `acp.rs`:

```rust
pub trait ModelProviderLaunchAdapter {
    fn api_types(&self) -> &[ModelProviderApiType];
    fn prepare(&self, ctx: &ModelLaunchContext) -> Result<PreparedModelLaunch, AcpError>;
}
```

`PreparedModelLaunch` contains environment overlays, optional config artifacts, fingerprint
material, and cleanup hooks.

Adapter strategy:

1. Prefer env-only projection for Claude, Gemini, DeepSeek, Code Buddy, Kimi, Open Code,
   Hermes, Cline and similar agents.
2. Use config projection for Codex, Grok, Pi and other catalog-driven agents.
3. If an agent reads a global config file and concurrent sessions may select different
   models, materialize a session-scoped config home instead of mutating shared state.
4. Keep existing Claude/Codex/Gemini native provider modes when `model_source == native`.
   The new adapters take over only for `model_source == provider`.

Implementation order:

1. Env-only agents.
2. Codex.
3. Claude/Gemini parity.
4. Remaining source-card agents.

## Probe and test

All network calls happen in Rust.

Probe endpoints:

- OpenAI Completions/Responses: `GET {base_url}/models`
- Anthropic: model-list endpoint with `x-api-key` and `anthropic-version`
- Google: Generative Language model-list endpoint with `x-goog-api-key`
- Respect `authHeader` and provider-level backend-only headers

Merge semantics follow pios:

- Existing manually entered model rows win.
- New fetched ids are appended.
- Fetched metadata fills only blank fields.
- Disabled providers remain editable and probe-able.

Model testing sends a tiny non-streaming request using the selected API family and returns
the first assistant text truncated to 200 characters. Requests have short timeouts and API
keys are never logged.

## Migration

No database migration is required for the provider catalog.

Implementation migration:

1. Add the models.json store.
2. Switch the frontend service from mock to transport.
3. Add `agent_setting.model_source`.
4. Add conversation selection columns.
5. Remove the old provider CRUD path once Claude/Codex/Gemini have native parity under the
   new source mode.

If `<CODEG_DATA_DIR>/models.json` does not exist and the old `model_provider` table has rows,
offer an explicit one-time export/import command. Do not auto-copy secrets into a new file
without user action.

## Implementation phases

### Phase 1: file store and CRUD

- Add Rust models.json types, parser, atomic writer, and locking.
- Implement provider CRUD, enable/reorder, and validation.
- Add Tauri/web handlers.
- Mirror API types in TypeScript.
- Switch frontend `ModelProviderService` to transport.

### Phase 2: source state

- Add `agent_setting.model_source`.
- Expose it in `AcpAgentInfo`.
- Implement `acp_update_agent_model_source`.
- Persist the current source-card UI state.

### Phase 3: conversation selection

- Add conversation model-selection columns.
- Implement selection validation and setter.
- Thread selection into connection creation and fingerprinting.
- Exclude disabled/incompatible providers from the picker.

### Phase 4: launch adapters

- Implement the adapter trait and env-only agents.
- Implement Codex config projection.
- Implement Claude/Gemini native parity.
- Implement remaining agent adapters.
- Verify stale/restart behavior.

### Phase 5: probe/test and polish

- Implement four API probes and model tests.
- Implement proxy support.
- Implement built-in clone templates.
- Add models.json corruption, locking, and launch integration tests.

## Verification

- `cargo check`
- `cargo test --features test-utils`
- `cargo clippy --all-targets --features test-utils -- -D warnings`
- Server-mode equivalents with `--no-default-features`
- Focused models.json parse/write/locking tests
- Provider CRUD and validation tests
- Integration test: provider + model -> conversation selection -> resolved launch env
- Integration test: disabled/incompatible provider is rejected at launch
- Integration test: provider edit marks the matching running session stale
