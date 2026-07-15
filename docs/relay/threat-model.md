# Codeg Relay v1 threat model

## Protected assets

Codeg access tokens, chat and Agent output, source code, paths, attachments,
permission decisions and pairing roots are confidential.
Command integrity, event ordering, one-time execution and immediate device
revocation are equally required.

## Adversaries

- A compromised or curious Relay operator that records and modifies traffic.
- A network attacker despite WSS, including DNS or certificate incidents.
- An attacker who guesses identifiers, replays frames or floods connections.
- A person who briefly sees a QR code or steals a routing credential.
- A stolen unlocked phone or compromised desktop account.

## Controls

- Pairing-root-authenticated ephemeral ECDH plus HKDF and AES-256-GCM provides
  E2EE, peer authentication and forward secrecy. Routing metadata is
  authenticated as AAD.
- One-use 256-bit pairing secrets, five-minute expiry, local desktop confirmation
  and matching SAS prevent remote enrollment and detect QR substitution.
- Monotonic sequence nonces, replay windows, cumulative acknowledgements and
  idempotency keys prevent replay and duplicate tool execution.
- Android Keystore and iOS Keychain protect mobile secrets; desktop secrets use
  the operating-system credential store. Secrets never enter localStorage.
- Relay enforces authenticated routing, frame and connection limits, heartbeat,
  rate limits and immediate revocation. Logs contain IDs and counters only.
- Push payloads contain opaque task IDs and generic status, never content.

## Residual risks

- Relay observes traffic timing, approximate payload sizes, IP addresses and
  online presence. Padding and anonymity are outside v1.
- Malware running as the user on either endpoint can read plaintext while the
  app is active. OS secure storage does not defend a fully compromised endpoint.
- A photographed, unused QR can enroll within its short validity window unless
  the user rejects the unexpected desktop confirmation.
- Push providers learn that a device received a generic Codeg notification.
- Revocation cannot erase ciphertext already recorded, but fresh ephemeral ECDH
  prevents a later endpoint compromise from recovering completed session keys.

## Required negative tests

- Alter AAD, ciphertext, nonce, sequence or peer ID and prove decryption fails.
- Replay an accepted frame and prove it is rejected without application dispatch.
- Reuse, expire and race a pairing code and prove only one request can win.
- Revoke a connected device and prove its socket closes and reconnect fails.
- Retry an identical idempotency key and prove the command executes once.
- Record Relay logs during chat and upload and prove no content or Codeg token is
  present.
