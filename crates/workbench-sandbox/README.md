# workbench-sandbox

Cross-platform command sandboxing, process-group lifecycle management, and SSRF-safe public-host resolution for AI agents.

## Capabilities

- **macOS**: Seatbelt (`sandbox-exec`) workspace confinement with optional network isolation
- **Linux**: Bubblewrap (`bwrap`) workspace confinement
- **Windows**: Win32 AppContainer isolation via `workbench-appcontainer`
- **Process groups** (`process`): Spawn commands in their own process group, pause/continue/terminate trees without `unsafe` in this crate
- **SSRF protection** (`public_host`): DNS-rebinding-safe `fetch_public` and `assert_public_host` backed by `SendNoted` network telemetry

This crate has zero dependency on `workbench-domain` or `workbench-core`.
