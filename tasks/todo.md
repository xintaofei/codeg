# Model Provider Backend Tasks

## Phase 1

- [x] Add models.json Rust types and parser
- [x] Add ordered, atomic, locked file writer
- [x] Implement provider CRUD service and validation
- [x] Add Tauri commands and Axum routes
- [x] Mirror API types in TypeScript
- [x] Switch frontend `ModelProviderService` from mock to transport

## Phase 2

- [x] Add `agent_setting.model_source`
- [x] Return source from `AcpAgentInfo`
- [x] Implement `acp_update_agent_model_source`
- [x] Persist source-card UI state

## Phase 3

- [x] Add conversation provider/model selection columns
- [x] Implement selection validation and setter
- [x] Thread selection into launch and config fingerprint

## Phase 4

- [x] Add provider launch adapter (`apply_launch_adapter` + workspace writers)
- [x] Implement env-only agents (Claude Code, Gemini, Grok, DeepSeek, CodeBuddy, Kimi Code completions)
- [x] Implement Codex config projection (empty CODEX_HOME workspace)
- [x] Implement Claude/Gemini native parity (env-only projection)
- [x] Implement remaining agent adapters (Pi, OpenCode, Cline, Hermes, Kimi Code)
- [x] Fold projected workspace into config fingerprint; clean up on conversation delete

## Phase 5

- [x] Implement four API model probes (openai-completions / responses, anthropic, google; local mock-tested)
- [x] Implement per-model tests (minimal chat request returning the model reply)
- [x] Implement proxy support (provider proxy projected into session env)
- [x] Implement built-in clone templates (built in Phase 1; behavior tests added here)
- [x] Add models.json corruption, locking, and launch integration tests
