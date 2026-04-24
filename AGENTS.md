# AGENTS.md

Instructions for AI coding agents working with this codebase.

## Package Manager

This project uses **pnpm**. Always use `pnpm` instead of `npm` or `yarn` for installing dependencies, running scripts, etc. (e.g., `pnpm install`, `pnpm run build`).

## Code Style

- Do not use emojis in code, output, or documentation. Unicode symbols (✓, ✗, →, ⚠) are acceptable.
- In documentation and markdown, never use double hyphens (`--`) as a dash. Use an emdash (—) sparingly when needed. Prefer rewriting the sentence to avoid dashes entirely.
- CLI colored output uses `cli/src/color.rs`. This module respects the `NO_COLOR` environment variable. Never use hardcoded ANSI color codes.
- CLI flags must always use kebab-case (e.g., `--auto-connect`, `--allow-file-access`). Never use camelCase for flags (e.g., `--autoConnect` is wrong).

## Documentation

When adding or changing user-facing features (new flags, commands, behaviors, environment variables, etc.), update **all** of the following:

1. `cli/src/output.rs` — `--help` output (flags list, examples, environment variables)
2. `README.md` — Options table, relevant feature sections, examples
3. `skills/aipuss-browser/SKILL.md` — so AI agents know about the feature
4. `docs/src/app/` — the Next.js docs site (MDX pages)
5. Inline doc comments in the relevant source files

This applies to changes that either human users or AI agents would need to know about. Do not skip any of these locations.

In the `docs/src/app/` MDX files, always use HTML `<table>` syntax for tables (not markdown pipe tables). This matches the existing convention across the docs site.

## Dashboard (packages/dashboard)

- Never use native browser dialogs (`alert`, `confirm`, `prompt`). Use shadcn/ui components (`Dialog`, `AlertDialog`, etc.) instead.
- Use param-case (kebab-case) for all file and folder names (e.g., `session-tree.tsx`, not `SessionTree.tsx`). The `ui/` directory follows shadcn conventions which already uses param-case.

## Releasing

Releases are manual, single-PR affairs. There is no changesets automation. The maintainer controls the changelog voice and format.

To prepare a release:

1. Create a branch (e.g. `prepare-v0.24.0`)
2. Bump `version` in `package.json`
3. Run `pnpm version:sync` to update `cli/Cargo.toml`, `cli/Cargo.lock`, and `packages/dashboard/package.json`
4. Write the changelog entry in `CHANGELOG.md` at the top, under a new `## <version>` heading, wrapped in `<!-- release:start -->` and `<!-- release:end -->` markers. Remove the `<!-- release:start -->` and `<!-- release:end -->` markers from the previous release entry so only the new release has markers.
5. Add a matching entry to `docs/src/app/changelog/page.mdx` at the top (below the `# Changelog` heading)
6. Open a PR and merge to `main`

When the PR merges, CI compares `package.json` version to what's on npm. If it differs, it builds all 7 platform binaries, publishes to npm, and creates the GitHub release automatically. The GitHub release body is extracted from the content between the `<!-- release:start -->` and `<!-- release:end -->` markers in `CHANGELOG.md`.

### Writing the changelog

Review the git log since the last release and write the entry in `CHANGELOG.md`. Follow the existing format and voice. Group changes under `### New Features`, `### Bug Fixes`, `### Improvements`, etc. Bold the feature/fix name, then describe it concisely. Reference PR numbers in parentheses.

Wrap the release notes (everything between the `## <version>` heading and the previous version) in markers so CI can extract them for the GitHub release. Only the current release should have markers; remove the `<!-- release:start -->` and `<!-- release:end -->` markers from any previous release entry:

```markdown
## 0.24.1

<!-- release:start -->
### Bug Fixes

- Fixed **baz** not working when qux is enabled (#1235)

### Contributors

- @ctate
<!-- release:end -->

## 0.24.0

### New Features

- **Foo command** - Added `foo` command for bar (#1234)
```

Include a `### Contributors` section listing the GitHub usernames (with `@` prefix) of everyone who contributed to the release. Check the git log between the previous tag and HEAD to find them.

Do not prefix entries with commit hashes. Do not use the changesets `### Patch Changes` / `### Minor Changes` headings. Use descriptive section names instead.

### Docs changelog

The docs changelog at `docs/src/app/changelog/page.mdx` mirrors `CHANGELOG.md` but uses a slightly different format. Each entry uses:

- A `v` prefix on the version (e.g. `## v0.24.0`)
- A date line with the full date: `<p className="text-[#888] text-sm">March 30, 2026</p>`
- A `---` separator between entries

Match the existing style in that file.

## Architecture

This is a Rust codebase. The browser automation daemon lives in `cli/src/native/` (daemon, actions, browser, CDP client, snapshot, state). The `--engine` flag selects Chrome vs Lightpanda. The `install` command downloads Chrome from Chrome for Testing directly.

### New Agent-Native Modules (2026-Q2)

All new modules are additive — they extend, not replace, existing API surfaces.

| Module | File | Purpose |
|--------|------|---------|
| Semantic snapshot | `cli/src/native/agent_snapshot.rs` | `take_agent_snapshot()` — priority scoring, ad filtering, noise assessment, action hints |
| State tracking | `cli/src/native/agent_state.rs` | `AgentStateTracker` + `StateDiff` + `SelfCorrection` — step recording, LCS tree diff, self-correction hint generation |
| Pre-fetch cache | `cli/src/native/prefetch.rs` | `PrefetchCache` — background link scan, HTTP pre-fetch, DNS pre-connect, LRU eviction |
| NL commands | `cli/src/native/agent_commands.rs` | `CommandRegistry` — 10 built-in commands (github_search_repos, find_best_repo, fill_form_fields, etc.) |
| Visual anchoring | `cli/src/native/screenshot.rs` | `capture_element_crops()` + `ElementCropCapture` — per-element crop thumbnails via CDP clip viewport |
| Fingerprint | `cli/src/native/cdp/chrome.rs` | `LaunchOptions { fingerprint_randomizer, timezone_override, accept_language_override }` |
| Skyvern adapter | `cli/src/native/skyvern_adapter.rs` | Skyvern-compatible protocol bridge — `SkyvernTask`, `TaskRegistry`, `LLMProvider` trait, `SkyvernSDKAction` (ai_click/ai_input_text/extract/validate/prompt), observe-think-act-verify loop, OpenAI + Anthropic providers |
| MCP server | `cli/src/native/mcp.rs` | Native MCP (Model Context Protocol) stdio server — 20 browser tools (navigate, snapshot, click, type, screenshot, etc.), JSON-RPC 2.0 over stdin/stdout, auto-registered with daemon when `--enable-mcp` is set |

All new modules are registered in `cli/src/native/mod.rs`. Key integration points:

- `ScreenshotResult.element_crops` is populated by `capture_element_crops()` when `ScreenshotOptions.element_crop` is `Some`
- `AgentStateTracker` can be embedded in the `Browser` struct to auto-record every action
- `PrefetchCache::scan_links()` parses the DOM tree for href/src, skips anchors/mailto/tel/javascript/data URI
- `CommandRegistry::execute()` accepts `{command: String, args: Value}` and returns `CommandResult`
- Fingerprint flags are applied in `build_chrome_args()` when `fingerprint_randomizer: true`
- MCP stdio server spawns in a dedicated `std::thread` when `AGENT_BROWSER_ENABLE_MCP=1`, creating its own `tokio::runtime::Builder::new_current_thread()`. Commands flow back to the daemon via a oneshot channel to avoid `block_on` deadlock
- `run_daemon(session, enable_mcp)` accepts the flag; when true, spawns the MCP thread and registers a tool handler in `run_socket_server` that dispatches to `execute_command()`

#### When adding a new native module

1. Create `cli/src/native/agent_your_module.rs` with full doc comments
2. Add `#[allow(dead_code)] pub mod agent_your_module;` to `cli/src/native/mod.rs`
3. If the module exposes a public async function, document the error handling strategy
4. Do not modify existing files unless the new capability requires it — additive is the goal

## Testing

### Unit Tests

```bash
cd cli && cargo test
```

Runs all unit tests (~320 tests). These are fast and don't require Chrome.

### End-to-End Tests

```bash
cd cli && cargo test e2e -- --ignored --test-threads=1
```

Runs 18 e2e tests that launch real headless Chrome instances and exercise the full native daemon command pipeline. Requirements:

- Chrome must be installed
- Must run serially (`--test-threads=1`) to avoid Chrome instance contention
- Tests are `#[ignore]`'d so they don't run during normal `cargo test`

The e2e tests live in `cli/src/native/e2e_tests.rs` and cover: launch/close, navigation, snapshots, screenshots, form interaction, cookies, storage, tabs, element queries, viewport/emulation, domain filtering, diff, state management, error handling, and Phase 8 commands.

### Linting and Formatting

```bash
cd cli && cargo fmt -- --check   # Check formatting
cd cli && cargo clippy            # Lint
```

## Windows Debugging

A remote Windows Server 2022 EC2 instance is available for debugging Windows-specific issues. It uses AWS Systems Manager (SSM) with no SSH or open ports. Commands run via `aws ssm send-command` and return stdout/stderr.

### Prerequisites

The instance must be provisioned first (one-time, by a human):

```bash
./scripts/windows-debug/provision.sh
```

Requires: AWS CLI v2 configured with `ec2:*`, `iam:CreateRole`, `iam:AttachRolePolicy`, `ssm:SendCommand`, `ssm:GetCommandInvocation` permissions and a default VPC.

### Usage

Start the instance (if stopped):

```bash
./scripts/windows-debug/start.sh
```

Run a command on Windows:

```bash
./scripts/windows-debug/run.sh "<powershell-command>"
```

Sync the current git branch and rebuild:

```bash
./scripts/windows-debug/sync.sh
```

Stop the instance when done (avoids cost):

```bash
./scripts/windows-debug/stop.sh
```

### Common Workflows

Run unit tests on Windows:

```bash
./scripts/windows-debug/run.sh "cd C:\agent-browser && cargo test --manifest-path cli\Cargo.toml"
```

Run e2e tests on Windows:

```bash
./scripts/windows-debug/run.sh "cd C:\agent-browser && cargo test e2e --manifest-path cli\Cargo.toml -- --ignored --test-threads=1"
```

Check bootstrap progress (first boot only):

```bash
./scripts/windows-debug/run.sh "Get-Content C:\bootstrap.log"
```

The repo lives at `C:\agent-browser` on the instance. Rust, Git, and Chrome are pre-installed. The `run.sh` wrapper automatically adds cargo and git to PATH.

<!-- opensrc:start -->

## Source Code Reference

Source code for dependencies is available in `opensrc/` for deeper understanding of implementation details.

See `opensrc/sources.json` for the list of available packages and their versions.

Use this source code when you need to understand how a package works internally, not just its types/interface.

### Fetching Additional Source Code

To fetch source code for a package or repository you need to understand, run:

```bash
npx opensrc <package>           # npm package (e.g., npx opensrc zod)
npx opensrc pypi:<package>      # Python package (e.g., npx opensrc pypi:requests)
npx opensrc crates:<package>    # Rust crate (e.g., npx opensrc crates:serde)
npx opensrc <owner>/<repo>      # GitHub repo (e.g., npx opensrc vercel/ai)
```

<!-- opensrc:end -->
