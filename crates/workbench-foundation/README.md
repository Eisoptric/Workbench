# workbench-foundation

Pure, I/O-free shared primitives used across the Eisoptric Workbench SDK crates:

- `time`: UTC ISO-8601 formatting (`iso_time`, `now_millis`, `parse_iso`) and civil calendar math
- `js`: JavaScript-compatible string/number/truthiness coercion helpers
- `public_address`: SSRF-safe public HTTP(S) URL parsing and private IPv4/IPv6 detection
- `sync::locked`: Poison-tolerant `Mutex` locking helper
- `hex`, `random_hex`, `new_id`, `plain_path`

This crate has zero product-domain dependencies and can be published independently.
