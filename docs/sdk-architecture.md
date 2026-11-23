# SDK & Crate Decoupling Architecture

General-purpose infrastructure crates without official ecosystem equivalents have been decoupled from product rules and extracted into the standalone open-source workspace at `../workbench-sdk`.

## Standalone SDK Repository (`../workbench-sdk`, 5 crates, zero cross-dependencies)

Each functional SDK crate is completely independent (no shared private `*-foundation` or `*-telemetry` dependencies):

- `workbench-appcontainer` *(0 workspace deps)*: Win32 AppContainer FFI isolation.
- `workbench-sandbox` *(only `workbench-appcontainer` for Win32 FFI)*: macOS Seatbelt, Linux bubblewrap, Windows AppContainer, process-group lifecycle, and DNS public-host SSRF guard.
- `workbench-computer` *(0 workspace deps)*: macOS Accessibility & Windows UI Automation computer-use SDK.
- `workbench-browser` *(0 workspace deps)*: Chrome DevTools Protocol & Safari WebDriver browser automation SDK.
- `workbench-programs` *(0 workspace deps)*: Resumable SHA-512 archive downloader, system `tar` unpacker, and bounded CLI prober.

## Workbench Product Layer (`crates/`)

- `workbench-domain`: Pure product rules, permissions, research schema, prompts, diffs, time/JS primitives (generates shared TS types via `ts-rs`).
- `workbench-telemetry`: Ring-buffer spans/metrics, OTLP/SIEM export, and `SendNoted` HTTP network log.
- `workbench-store`: AES-GCM encrypted secrets, OS key store, and `AppState` persistence.
- `workbench-mcp`: Product MCP adapter built on the official `rmcp` SDK (both `TokioChildProcess` stdio and `StreamableHttpClientTransport` HTTP/SSE) with `FileStore` OAuth 2.1 PKCE credentials and `workbench-telemetry` spans.
- `workbench-tools`: Agent-facing tool implementations (`file`, `code`, `web`, `general`, `jobs`), re-exporting `workbench-sandbox`.
- `workbench-llm`: Multi-provider LLM client (OpenAI, Claude, Gemini, DeepSeek, local servers).
- `workbench-core`: Agent loop, research workflows (`research/remote/{mod,worker}.rs`), GitHub/git (`origin/{mod,github}.rs`), Claude Code & Codex session bridges.
- `workbench-server` / `workbench-cli`: Local HTTP API and terminal client.
