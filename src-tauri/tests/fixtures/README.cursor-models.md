# Cursor model-catalog protocol snapshot

These two test-only fixtures were captured from Cursor CLI
`2026.07.23-e383d2b` without issuing a prompt:

- `cursor-models-2026.07.23-e383d2b.txt` is stdout from
  `cursor-agent models` (204 public display rows).
- `cursor-available-models-2026.07.23-e383d2b.json` is the `result` body of
  `cursor/list_available_models` (base models plus their dynamic config
  options).

The capture contains no headers, credentials, account identifiers, home paths,
private endpoints, or request metadata. It is a regression snapshot only;
production always discovers the current account/version catalog dynamically.
