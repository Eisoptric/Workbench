# workbench-programs

Lightweight, resumable program installer and CLI prober for desktop runtimes:

- **Resumable downloads** (`fetch_resumable`): HTTP Range resume across dropped connections with stall timeout and `SendNoted` network telemetry
- **Integrity verification** (`sha512_base64`): Streaming SHA-512 matching npm SRI (`sha512-...`) format
- **Archive extraction** (`untar`, `make_executable`): System `tar` / `bsdtar` unpacking without C/Rust archive dependencies
- **Bounded CLI probing** (`ask_program`): Timeout-guarded child execution with `kill_on_drop` and Windows console suppression

Depends only on `workbench-foundation`, `workbench-sandbox`, and `workbench-telemetry` (zero product-domain coupling).
