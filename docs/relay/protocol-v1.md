# Codeg Relay Protocol v1

status: draft-implementable
version: 1
updated: 2026-07-15

## Security boundary

Codeg Relay is an untrusted store-and-forward router. TLS protects each hop,
while application frames are encrypted end to end between a paired desktop and
mobile device. The Relay may observe device routing identifiers, frame sizes,
timing, connection state and IP addresses. It must never receive a Codeg access
token, request parameters, Agent output, code, filenames, attachment bytes or
permission answers in plaintext.

## Roles and endpoints

- Desktop opens outbound `WSS /v1/desktop` and owns one stable `desktop_id`.
- Mobile opens outbound `WSS /v1/mobile` and owns one stable `device_id` per
  installation.
- Relay authenticates both sockets with relay-scoped bearer credentials. These
  credentials authorize routing only and are not Codeg credentials.
- All protocol messages are UTF-8 JSON. Encrypted binary data uses unpadded
  base64url in JSON v1.

## Clear routing envelope

```json
{
  "v": 1,
  "type": "frame",
  "desktop_id": "d_...",
  "device_id": "m_...",
  "connection_id": "c_...",
  "frame_id": "019...",
  "seq": 42,
  "ack": 39,
  "nonce": "base64url(12 bytes)",
  "ciphertext": "base64url(AES-GCM output)"
}
```

Allowed clear envelope types are `hello`, `pair`, `frame`, `ack`, `ping`,
`pong`, `revoke` and `error`. `error` contains only a stable routing error code,
never decrypted application details. Unknown versions or fields that violate
size/type limits close the socket with code `1008`.

The canonical UTF-8 encoding of
`v|desktop_id|device_id|connection_id|frame_id|seq|ack` is AES-GCM additional
authenticated data. A Relay therefore cannot silently reroute, renumber or
replay a ciphertext under different metadata.

## Encrypted payloads

After decryption, `kind` is one of:

- `request`: `request_id`, `method`, `params`, `idempotency_key`, deadline.
- `response`: `request_id`, success result or stable Codeg error.
- `event`: `stream_id`, monotonically increasing `event_seq`, event name and
  payload.
- `snapshot`: stream snapshot and last included event sequence.
- `upload`: `upload_id`, chunk index/count, total size, SHA-256 and bytes.
- `cancel`: request or upload identifier.
- `pair_request`, `pair_accept`, `pair_reject` during pairing only.

Requests are idempotent for 24 hours by `(device_id, idempotency_key)`. A retry
with the same key returns the original terminal response and never executes a
tool twice. Receivers retain the last 4,096 frame identifiers and reject a
duplicate before decrypting application data.

## Pairing

1. Desktop generates a long-term P-256 signing identity and a 256-bit
   `pair_secret`, registers `SHA-256(pair_secret)` as a 5-minute one-use
   `pair_id`, then displays a QR code.
2. QR contains protocol version, Relay URL, desktop ID, desktop signing public
   key, `pair_id`, `pair_secret` and expiry. The QR is the authenticated channel;
   it must only be shown locally and becomes invalid after one use or timeout.
3. Mobile generates its signing identity and an ephemeral P-256 ECDH key. Its
   `pair_request` is encrypted with a key derived from ECDH plus `pair_secret`.
4. Desktop decrypts the request and displays device name plus a six-digit SAS
   derived from both identity keys and `pair_secret`. Mobile shows the same SAS.
5. User confirms on the desktop. Relay issues a random routing credential;
   desktop returns it inside encrypted `pair_accept` and erases `pair_secret`.
6. Both peers store identity material and pairing root in OS secure storage.
   Reject, timeout, a second use or desktop cancellation destroys all temporary
   state.

Each normal connection uses fresh ephemeral P-256 ECDH keys. Peers sign the
ephemeral public keys and connection ID with their paired signing identities.
HKDF-SHA-256 derives independent `mobile-to-desktop` and
`desktop-to-mobile` AES-256-GCM keys from the ephemeral shared secret, pairing
root and connection ID. This provides mutual authentication and forward
secrecy for completed sessions.

## Ordering, replay and recovery

- `seq` starts at 1 for each direction and connection. The 96-bit nonce is a
  four-byte direction tag followed by the big-endian 64-bit sequence.
- A sender may keep at most 256 unacknowledged frames or 8 MiB, whichever comes
  first. It pauses application reads when the limit is reached.
- `ack` is cumulative. A reconnect starts a new encrypted connection and then
  requests a Codeg snapshot using the last applied `event_seq` per stream.
- The desktop reuses Codeg's existing snapshot/replay source of truth. Relay
  transport retries must not synthesize Agent events.
- Foreground recovery target is five seconds when both peers and Relay are
  reachable.

## Attachments

- Default encrypted chunk size: 256 KiB.
- Maximum encrypted routing frame: 1 MiB.
- Default attachment limit: 100 MiB, configurable lower by the desktop.
- Every upload declares total bytes and SHA-256 before chunk 0.
- Chunks are independently authenticated and resumable by missing chunk index.
- Cancel deletes partial data. Partial data expires after one hour.

## Revocation and compatibility

Desktop is authoritative for paired devices. Revocation closes matching Relay
sockets, deletes the routing credential, rotates the desktop pairing root for
that device and rejects all later frames. A stolen routing token without the OS
protected identity and pairing root cannot decrypt or forge application frames.

Protocol changes that alter cryptography, envelope canonicalization or payload
semantics require a new integer version. Peers advertise supported versions in
`hello`; no silent downgrade is allowed.

## Stable routing errors

- `unsupported_version`
- `unauthorized`
- `pair_expired`
- `pair_consumed`
- `peer_offline`
- `device_revoked`
- `frame_too_large`
- `rate_limited`
- `backpressure`
- `protocol_violation`

