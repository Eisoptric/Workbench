# SDK & Crate Decoupling Architecture

Low-level infrastructure crates are separated from product rules (`workbench-domain`, `workbench-core`, `workbench-server`) so they can evolve and be published as standalone open-source Rust SDKs.

## Standalone SDK Layer (zero product-domain dependencies)

```text
workbench-foundation          (pure primitives: time, js, hex, plain_path, public_address, sync::locked)
 ├── workbench-telemetry      (ring-buffer spans/metrics, OTLP/SIEM export, SendNoted HTTP guard)
 ├── workbench-appcontainer   (Win32 AppContainer FFI isolation)
 ├── workbench-sandbox        (macOS Seatbelt, Linux bwrap, Windows AppContainer, process groups, SSRF public_host)
 │    ├── workbench-computer  (macOS Accessibility + Windows UI Automation computer-use SDK)
 │    ├── workbench-browser   (local Chrome CDP + macOS Safari browser automation SDK)
 │    └── workbench-programs  (resumable SHA-512 archive installer, system tar unpacker, bounded CLI prober)
 └── workbench-store          (AES-GCM encrypted secrets + OS key store + atomic JSON store)
```

## Product Layer

- `workbench-domain`: Pure product rules, permissions, research schema, prompts, diffs (re-exports primitives from `workbench-foundation`).
- `workbench-tools`: Agent-facing tool implementations (`file`, `code`, `web`, `general`, `jobs`), re-exporting `workbench-sandbox`.
- `workbench-llm` / `workbench-mcp`: Model providers and MCP client.
- `workbench-core`: Agent loop, research workflows (`research/remote/{mod,worker}.rs`), GitHub/git (`origin/{mod,github}.rs`).
- `workbench-server` / `workbench-cli`: HTTP API and terminal client.

## Next Roadmap Steps

1. **Phase 2 (remaining)**: Adopt `ts-rs` for Rust → TypeScript domain type generation, `@tanstack/react-query` in `desktop/` (re-enabling `react-hooks/exhaustive-deps`), and evaluate `rmcp` (official Rust MCP SDK) with `SendNoted` network observability.
2. **Phase 3 (remaining)**: Extract `claude_code` + `codex` + `agent_sessions` + `agent_programs` from `workbench-core` into a standalone `agent-cli-bridge` SDK crate, and publish the standalone SDK crates once public API names are finalized.
