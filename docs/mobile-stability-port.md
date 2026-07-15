# Mobile stability port inventory

Baseline: `xintaofei/codeg@v0.20.3` (`7c48df5ac4aa56912436983447af5b1ba061f3de`).

The mobile repository starts from the upstream tag, not from the customized
desktop checkout. Only transport behavior that is required by a remote mobile
client is retained:

- WebSocket `__ready__` handshake before commands that can emit early events.
- Subscribe-with-snapshot and replay support in `WebEventStream`.
- Reconnect health state, bounded health probes, and exponential backoff.
- Transport reconnect callbacks so conversation state can be re-fetched.
- Long web-call timeout matching the ACP probe deadline.
- Existing long-conversation virtualization and pagination behavior.

Desktop-only customization is intentionally excluded from the mobile shell:
ACP and Agent processes, PTY, sidecars, terminal, local Git/filesystem runtime,
tray, updater, single-instance, pets, and multi-window state.

Any future port from `codeg-custom` must be recorded here with its source
commit, affected protocol invariant, and a regression test. UI-only or
desktop-runtime changes must not be copied wholesale.
