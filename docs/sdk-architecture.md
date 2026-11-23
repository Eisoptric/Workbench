# SDK & Crate Decoupling Architecture

Low-level infrastructure crates have been decoupled from product rules (`workbench-domain`, `workbench-core`, `workbench-server`) and extracted into the standalone open-source workspace at `../workbench-sdk`.

## Standalone SDK Repository (`../workbench-sdk`, 9 crates, zero product dependencies)

```text
workbench-foundation          (pure primitives: time, js, hex, plain_path, public_address, sync::locked)
 ├── workbench-telemetry      (ring-buffer spans/metrics, OTLP/SIEM export, SendNoted HTTP guard)
 ├── workbench-appcontainer   (Win32 AppContainer FFI isolation)
 ├── workbench-sandbox        (macOS Seatbelt, Linux bwrap, Windows AppContainer, process groups, SSRF public_host)
 │    ├── workbench-computer  (macOS Accessibility + Windows UI Automation computer-use SDK)
 │    ├── workbench-browser   (local Chrome CDP + macOS Safari browser automation SDK)
 │    └── workbench-programs  (resumable SHA-512 archive installer, system tar unpacker, bounded CLI prober)
 └── workbench-store          (AES-GCM encrypted secrets + OS key store + atomic JSON store)
      └── workbench-mcp       (MCP client: rmcp stdio, Streamable HTTP, eventsource-stream SSE, OAuth 2.1 PKCE)
```

## Workbench Product Layer (`crates/`)

- `workbench-domain`: Pure product rules, permissions, research schema, prompts, diffs (re-exports primitives from `workbench-foundation`; generates shared TS types via `ts-rs`).
- `workbench-tools`: Agent-facing tool implementations (`file`, `code`, `web`, `general`, `jobs`), re-exporting `workbench-sandbox`.
- `workbench-llm`: Multi-provider LLM client (OpenAI, Claude, Gemini, DeepSeek, local servers).
- `workbench-core`: Agent loop, research workflows (`research/remote/{mod,worker}.rs`), GitHub/git (`origin/{mod,github}.rs`), Claude Code & Codex session bridges.
- `workbench-server` / `workbench-cli`: Local HTTP API and terminal client.
