# Security policy

GeoGenesis is a local simulation engine and static web app. It has no server
component, no network access at runtime, and no untrusted input deserialisation
in the core (the WebAssembly ABI consumes a flat, bounded `f64` parameter array
and buffer slices).

## Scope and design notes

- The only `unsafe` code is in `crates/wasm/src/lib.rs`, confined to the C-ABI
  boundary (raw buffer marshalling) and documented per block. Treat any new
  `unsafe` outside that crate as a security-relevant change.
- The engine protects itself against pathological grids via parameter
  validation and memory guards (for example, single-shot generation above
  2048² requires an explicit `--allow-large-grid` opt-in).
- The Web UI runs the engine in a Worker; generated projects/parameter files
  are parsed as JSON with explicit validation before use.
- Exported maps and meshes are plain data files; they do not execute code.

## Reporting a vulnerability

Please do not open a public issue for a security problem. Report it privately to
the maintainers (open a draft security advisory via GitHub "Security" tab, or
contact the repository owner) with:

- affected version / commit,
- a minimal reproduction,
- impact assessment (memory safety, denial of service, data loss, XSS, ...).

We aim to acknowledge reports within a few days and will credit reporters in
the fix unless they prefer otherwise.
