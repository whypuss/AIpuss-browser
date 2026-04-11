# AIpuss-browser

Fork of [agent-browser](https://github.com/jackwener/opencli) / [opencli](https://github.com/jackwener/opencli) — AI-first browser automation CLI for AI agents. Powered by Rust + CDP.

This fork adds:
- **Daemon mode** with WebSocket streaming for Hermes agent integration
- **macOS launchd watchdog** for 24/7 background operation
- **Dashboard UI** (Activity / Console / Network / Storage tabs, no Chat)

---

## Features

- **Daemon mode** — Start once, control browser via CLI commands or WebSocket indefinitely
- **Headless Chromium** — Ships its own Chrome, no system Chrome required
- **Accessibility tree snapshots** — `@e1`, `@e2` refs for precise AI targeting
- **Semantic locators** — Find by role, text, label, placeholder, ARIA
- **Browser profiles** — Reuse existing Chrome login state with `--profile`
- **Session isolation** — Multiple independent browser instances
- **Network interception** — Mock, block, or record HTTP requests
- **State persistence** — Auto-save/restore cookies and localStorage
- **Auth vault** — Encrypted credential storage
- **Diff tools** — Snapshot and screenshot diffing for regression testing
- **Dashboard** — Web UI at `http://localhost:<port>` with Activity / Console / Network / Storage tabs

---

## Installation

### From Source (Recommended for this fork)

```bash
git clone https://github.com/whypuss/AIpuss-browser
cd AIpuss-browser
pnpm install
pnpm build          # Build Next.js dashboard
pnpm build:native   # Build Rust binary (requires Rust: https://rustup.rs)
pnpm link --global  # Link as `aipuss-browser` globally
```

### Via npm

```bash
npm install -g agent-browser
agent-browser install
```

### Via Cargo

```bash
cargo install agent-browser
agent-browser install
```

### Via Homebrew

```bash
brew install agent-browser
agent-browser install
```

---

## Quick Start

```bash
# Open a page
aipuss-browser open example.com

# Get accessibility tree
aipuss-browser snapshot

# Click element by ref
aipuss-browser click @e2

# Fill form
aipuss-browser fill @e3 "test@example.com"
aipuss-browser press Enter

# Screenshot
aipuss-browser screenshot

# Close
aipuss-browser close
```

---

## Daemon Mode (Recommended for AI Agents)

Start the browser once, keep it running. All subsequent commands reuse the same browser instance.

### Start Daemon

```bash
aipuss-browser stream enable
```

This outputs the port, e.g.:
```
Stream enabled on port 62097
Socket: ~/.agent-browser/default.sock
```

The port is also saved to `~/.agent-browser/default.stream`.

### Using the Daemon

Every CLI command automatically connects to the running daemon:

```bash
aipuss-browser open github.com
aipuss-browser snapshot
aipuss-browser click @e5
aipuss-browser close
```

### Stop Daemon

```bash
aipuss-browser stream disable
```

### Key Daemon Files

| File | Purpose |
|------|---------|
| `~/.agent-browser/default.sock` | Unix socket for CLI commands |
| `~/.agent-browser/default.stream` | Port number for WebSocket/dashboard |

---

## Dashboard

When daemon is running, open `http://localhost:<port>` to see the dashboard.

Dashboard tabs:
- **Activity** — Live event stream
- **Console** — JavaScript console logs and errors
- **Network** — HTTP request/response inspector
- **Storage** — Cookies and web storage viewer

The Chat tab has been removed in this fork.

---

## Hermes Agent Integration

This fork is designed for use with the Hermes agent framework.

### Hermes Browser Tool

The Hermes `browser_tool.py` uses AIpuss-browser for all browser automation. With the daemon running, Hermes can:

- Navigate to URLs
- Take snapshots and screenshots
- Click, fill, scroll, and interact with pages
- Extract data from pages

### Running as a Service (macOS)

For 24/7 operation with automatic restart on crash:

```bash
# Install watchdog + launchd service
# (watchdog script at ~/.hermes/scripts/aipuss-watchdog.sh)
# launchd plist at ~/Library/LaunchAgents/com.hermes.aipuss-watchdog.plist)

# Load the service
launchctl load ~/Library/LaunchAgents/com.hermes.aipuss-watchdog.plist

# Check status
tail -f /tmp/aipuss-watchdog.log
```

The watchdog checks every 10 seconds and restarts the daemon if it dies.

---

## Browser Profiles

### Reuse Existing Chrome Login

```bash
# List available profiles
aipuss-browser profiles

# Use a profile
aipuss-browser --profile Default open github.com
```

### Persistent Profile Directory

```bash
# Use a dedicated profile directory
aipuss-browser --profile ~/.myapp-profile open myapp.com
```

### Session-based Persistence

```bash
# Auto-save/restore cookies + localStorage
aipuss-browser --session-name myapp open myapp.com
# State is automatically restored on next run with same --session-name
```

---

## Network Interception

```bash
# Block requests
aipuss-browser network route "**/ads/**" --abort

# Mock API response
aipuss-browser network route "**/api/user" --body '{"name":"Test"}'

# Record HAR
aipuss-browser network har start
# ... do things ...
aipuss-browser network har stop output.har
```

---

## Environment Variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `AGENT_BROWSER_SESSION` | Session name | `default` |
| `AGENT_BROWSER_SESSION_NAME` | Auto-save/restore state | — |
| `AGENT_BROWSER_PROFILE` | Chrome profile | — |
| `AGENT_BROWSER_HEADED` | Show browser window | `false` |
| `AGENT_BROWSER_STREAM_PORT` | WebSocket port | OS-assigned |
| `AI_MODEL` | AI model for chat | provider default |
| `AI_PROVIDER` | AI provider (nvidia, openai, etc.) | `nvidia` |
| `AI_API_KEY` | API key for AI | — |

---

## Project Structure

```
AIpuss-browser/
├── cli/                    # Rust binary source
│   ├── src/native/         # Core browser automation
│   ├── src/commands.rs     # CLI command definitions
│   └── src/native/stream/  # Daemon + WebSocket + Dashboard
├── packages/
│   └── dashboard/          # Next.js Web UI (Activity/Console/Network/Storage)
└── docs/                   # Full documentation
```

---

## License

MIT
