# Technology Stack

**Analysis Date:** 2026-10-09

## Languages

**Primary:**
- Rust 2021 Edition (`Cargo.toml`, `src/lib.rs`) - Core native plugin implementation, ABI bindings, background worker, HTTP logic, and quota scheduling.

**Secondary:**
- Python 3 (`tests/dll_host.py`, `tests/live_probe.py`) - C-ABI mock test host and end-to-end live verification harnesses using `ctypes`.
- PowerShell (`README.md`, `AGENTS.md`) - Development checks, build commands, and execution scripts.

## Runtime

**Environment:**
- Windows MSVC Native Runtime (`x86_64-pc-windows-msvc`) - Compiled as a Windows Dynamic Link Library (`cdylib` / `target/release/codex_window_activation.dll`) loaded dynamically by CLIProxyAPI v8.0.20/v8.0.21.
- Multi-threaded execution model using Rust standard library threads (`std::thread`, `std::sync::Mutex`, `std::sync::Condvar`, `std::sync::atomic::AtomicBool`).

**Package Manager:**
- Cargo (Rust package manager)
- Lockfile: `Cargo.lock` present and pinned (`serde_json = "=1.0.151"`).

## Frameworks

**Core:**
- Native C ABI (`CLIProxyAPI Plugin ABI v1`, `RPC schema 6`) - Implements foreign function interface exported as `cliproxy_plugin_init` in `src/lib.rs:467`.

**Testing:**
- Built-in Rust test framework (`tests/policy.rs`, `src/lib.rs:540`) - Unit and policy regression tests run via `cargo test --offline`.
- Python `ctypes` test harness (`tests/dll_host.py`) - In-process host simulator validating DLL ABI loading, memory buffer lifecycle, and state persistence.
- Python live probe (`tests/live_probe.py`) - Isolated live API invocation test harness.

**Build/Dev:**
- Cargo (`cargo build --release --offline`, `cargo clippy --offline --all-targets -- -D warnings`)
- Project-local GSD Core 1.16.0 (`.opencode/gsd-core/`) - Development and planning orchestration workflow.

## Key Dependencies

**Critical:**
- `serde_json` `1.0.151` (`Cargo.toml:10`) - JSON serialization/deserialization for host RPC messages, state persistence, quota payload parsing, and inference event decoding. Exactly one external runtime dependency.

**Infrastructure:**
- Rust Standard Library (`std`) - Provides memory management (`std::ffi::CStr`, `std::ffi::CString`), threading, synchronization, and atomic primitives without external async runtime (no Tokio; synchronous thread worker).

## Configuration

**Environment:**
- `USERPROFILE`: Used in `src/lib.rs:100` to resolve state directory (`%USERPROFILE%\.cli-proxy-api\codex-window-activation-state.json`).
- `CPA_LIVE_AUTH_FILE`: Required environment variable for `tests/live_probe.py:25` pointing to isolated OAuth JSON credentials for real inference testing.

**Build:**
- `Cargo.toml`: Configures crate-type as `["cdylib", "rlib"]` for DLL output alongside static library linking.

## Platform Requirements

**Development:**
- Windows 10/11 x86_64 with Visual Studio MSVC C/C++ Build Tools and Rust MSVC toolchain (`stable-x86_64-pc-windows-msvc`).
- Python 3.8+ with standard library (`ctypes`, `json`, `urllib.request`).

**Production:**
- CLIProxyAPI v8.0.20 or v8.0.21 on Windows x86_64.
- Target library placed into CLIProxyAPI plugins directory as `codex-window-activation-v0.1.0.dll`.

---

*Stack analysis: 2026-10-09*
