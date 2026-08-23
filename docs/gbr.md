# Build Remote Agent (companion, not a replacement)

**Codeg** already has first-party iOS/Android clients that talk to your Codeg desktop/server. **Build Remote Agent** is a different pairing device: a phone that spectates a **desktop coding agent** (Grok Build, Claude Code, Codex, …) through `gbr-agent`.

Keep using Codeg mobile apps for Codeg itself. Pair `gbr-agent` on the coding-agent host if you want a spectator/veto phone for that CLI/GUI session.

## Install + pair

```bash
# macOS / Linux
curl -fsSL https://grokbuildremote.com/install.sh | bash
gbr-agent version          # must print v0.6.0 or newer
gbr-agent pair             # QR in browser + printed 8-char code
gbr-agent run              # leave running
```

```powershell
# Windows
irm https://grokbuildremote.com/install.ps1 | iex
gbr-agent version
gbr-agent pair
gbr-agent run
```

Phone: open [Build Remote Agent](https://grokbuildremote.com/) → **Scan QR from computer** (or type the 8-char code). Sessions appear in the app. **Unpair** in Settings before changing PCs. Force-close is not enough.

## Attach

After `gbr-agent run`:

| How | Where |
|-----|--------|
| Bot API | `http://127.0.0.1:8788` |
| MCP stdio | `node …/GrokBuildRemote-Agents/mcp/gbr-mcp/bin/gbr-mcp.js` |

```bash
curl -sS http://127.0.0.1:8788/health
curl -sS http://127.0.0.1:8788/v1/sessions
```

Phone is spectator + veto, not orchestrator. Do not commit mailbox keys. Phone **Settings → Bot API** is the only place a relay key is copied.

MCP clone:

```bash
git clone https://github.com/LinespottingOrg/GrokBuildRemote-Agents.git
cd GrokBuildRemote-Agents/mcp/gbr-mcp && npm install
node bin/gbr-mcp.js --diagnose
```

Protocol `gbr/1`. Independent product by Linespotting AB. Not affiliated with xAI or SpaceX.

Docs: https://github.com/LinespottingOrg/GrokBuildRemote-Agents/blob/main/docs/BOT-API.md
Website: https://grokbuildremote.com/

