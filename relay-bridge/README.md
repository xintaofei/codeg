# Codeg Relay Bridge

This is the desktop-side outbound client for Codeg Mobile Relay mode. It runs
next to an existing Codeg desktop/server process and never opens an inbound
public port.

The bridge:

- connects outbound to Codeg Relay;
- authenticates a fresh P-256 ECDH session with each paired phone;
- forwards encrypted API calls to the local Codeg HTTP API;
- forwards the local Codeg WebSocket stream back through end-to-end
  encryption;
- keeps the Codeg token on the desktop.

## Development run

Copy relay-bridge.example.json outside the repository, replace every sample
secret, and restrict it to the current user. On macOS/Linux:

    chmod 600 /path/to/relay-bridge.json
    CODEG_RELAY_BRIDGE_CONFIG=/path/to/relay-bridge.json cargo run

On Windows, store the file under %APPDATA%/Codeg/relay-bridge.json and
restrict its ACL to the current account. The JSON config is a headless
development interface. The packaged Codeg desktop UI must store the same
secrets in the operating-system credential vault instead of a plaintext file.

Do not paste a real config into logs, issues, or chat. The bridge logs
connection state and routing identifiers only; it does not log decrypted
payloads or tokens.
