# workbench-browser

Lightweight local browser automation SDK for AI agents:

- **Chrome / Chromium**: Headless or visible profile-isolated Chrome over CDP (`tokio-tungstenite`) with live frame pacing
- **Safari (macOS)**: Native automation via `safaridriver`
- **Safety**: SSRF-checked navigation via `workbench-sandbox::public_host`

Depends only on `workbench-foundation`, `workbench-sandbox`, and `workbench-telemetry` (zero product-domain coupling).
