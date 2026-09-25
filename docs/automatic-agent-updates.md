# Automatic agent updates

In **Settings > Agents**, choose **Automatic (latest)** under **Updates** and
save. The option is available for npm, binary and Python agents, including
custom entries and extra accounts.

Codeg checks for updates when creating an agent connection, at most once an
hour for each launch recipe. npm agents use the package's `latest` tag, binary
agents use the ACP registry's release for the current platform, and Python
agents use their published PyPI version. npm and Python launches use the exact
version that was prepared. Running conversations keep their existing process.

Claude, Codex and pi adapters have a separate runtime. Automatic mode prepares
both packages using their supported executable override. An explicitly supplied
runtime executable remains authoritative. Installing a custom version turns
Automatic off for that agent.

Models, reasoning controls and defaults still come from the agent's own ACP
response. Codeg does not invent model IDs, merge another account's model list,
or change a conversation's chosen model. Availability depends on what the agent
supports and what the account can access.

The existing **Pinned** and **Latest (unreviewed)** manual install modes keep
their behavior. Initial installation remains explicit. Automatic preparations
use separate package/version caches and preserve global CLI installations,
account directories and credentials. A failed check or preparation retains the
previous prepared launch or the ordinary installed agent, with a retry after
five minutes. URLs and local/git package sources without a published package
identity remain under the user's control.

Implementation: `src-tauri/src/acp/managed_updates.rs` owns release checks and
cache state; `managed-runtime.mjs` selects the exact prepared npm packages and
keeps arguments separate from shell commands. The common `build_agent` boundary
covers desktop, web, delegated agents and settings probes. The cache stores only
launch/version metadata, never credentials or model/session content.
