# Codebase Structure

**Analysis Date:** 2026-10-09

## Directory Layout

```text
codex-window-activation/
├── .gitignore                         # Generated output and secret/state exclusions
├── AGENTS.md                          # Repository constraints and required checks
├── Cargo.toml                         # Rust package and cdylib/rlib target configuration
├── Cargo.lock                         # Locked dependency graph
├── README.md                          # Runtime behavior, installation, and test instructions
├── src/
│   └── lib.rs                         # Complete production plugin implementation
├── tests/
│   ├── policy.rs                      # Rust integration tests for quota policy
│   ├── dll_host.py                    # Native DLL + mock host ABI integration harness
│   └── live_probe.py                  # Explicit opt-in live provider probe
├── .planning/
│   └── codebase/                      # GSD architecture and structure mapping documents
├── .opencode/                         # Local, ignored GSD/OpenCode installation
└── target/                            # Cargo build artifacts; ignored by Git
```

The committed implementation is intentionally small: one Rust library source file, one Rust integration-test file, and two Python host/probe harnesses. `target/` contains generated debug/release output and is excluded by `.gitignore`; `.opencode/` is local development tooling and is also excluded (`.gitignore:1-2`).

## Directory Purposes

**`src/`:**
- Purpose: Production Rust code for the CLIProxyAPI plugin.
- Contains: The ABI definitions, lifecycle dispatcher, worker, quota policy, host callback adapter, HTTP operations, persistence, and management reporting in `src/lib.rs`.
- Key files: `src/lib.rs:105-125` for ABI structs, `src/lib.rs:303-399` for account orchestration, and `src/lib.rs:464-538` for exported/plugin callbacks.

**`tests/`:**
- Purpose: Verify policy in Rust and exercise the actual compiled DLL through Python `ctypes`.
- Contains: `tests/policy.rs` for pure decision behavior, `tests/dll_host.py` for a mock native host and restart/backoff checks, and `tests/live_probe.py` for an explicitly authorized real request.
- Key files: `tests/policy.rs:4-51`, `tests/dll_host.py:31-104`, and `tests/live_probe.py:25-92`.

**`.planning/codebase/`:**
- Purpose: Repository mapping artifacts consumed by planning workflows.
- Contains: `ARCHITECTURE.md` and `STRUCTURE.md` for this architecture pass.
- Key files: `.planning/codebase/ARCHITECTURE.md` and `.planning/codebase/STRUCTURE.md`.

**`.opencode/`:**
- Purpose: Machine-local GSD/OpenCode command and skill installation.
- Contains: Project-local workflow assets such as `.opencode/skills/gsd-map-codebase/SKILL.md`.
- Key files: `.opencode/skills/gsd-map-codebase/SKILL.md`; this directory is ignored and is not a runtime dependency (`README.md:97-108`).

**`target/`:**
- Purpose: Cargo-generated build and incremental compilation output.
- Contains: Debug/release DLLs, libraries, dependency artifacts, and incremental state.
- Key files: `target/release/codex_window_activation.dll` is the DLL consumed by `tests/dll_host.py:58-62`; the directory is ignored by `.gitignore:1`.

## Key File Locations

**Entry Points:**
- `src/lib.rs:467`: `cliproxy_plugin_init`, the exported C ABI initialization function.
- `src/lib.rs:487`: `plugin_call`, the plugin RPC callback returned in `PluginApi`.
- `src/lib.rs:438-462`: `handle`, the internal dispatcher for lifecycle and management methods.

**Configuration:**
- `Cargo.toml:1-10`: Package name/version, Rust edition, `cdylib`/`rlib` crate types, and the pinned direct dependency.
- `README.md:75-95`: Installation and CLIProxyAPI plugin configuration shape.
- `AGENTS.md:5-24`: Required checks and architectural constraints for changes.
- `%USERPROFILE%\.cli-proxy-api\codex-window-activation-state.json` (runtime path derived by `src/lib.rs:99-103`): Persistent account deadline/attempt state, not repository configuration.

**Core Logic:**
- `src/lib.rs:19-91`: Quota window model and decision policy.
- `src/lib.rs:141-175`: Host callback request/response adapter.
- `src/lib.rs:216-292`: Cancelable host HTTP adapter and Codex quota/activation request builders.
- `src/lib.rs:303-399`: Sequential account polling, exact-account activation, verification, and report construction.
- `src/lib.rs:401-435`: Worker lifecycle and shutdown.
- `src/lib.rs:464-538`: C ABI and memory ownership boundary.

**Testing:**
- `src/lib.rs:540-558`: Unit tests for body decoding and remembered deadline behavior.
- `tests/policy.rs:1-51`: Integration tests for waiting, due resets, deduplication, exhaustion, swapped windows, and malformed quota.
- `tests/dll_host.py:58-104`: Release-DLL mock-host test covering ABI, account selection, activation, persistence, retry, attempt cap, and cleanup.
- `tests/live_probe.py:73-92`: Explicit live test using `CPA_LIVE_AUTH_FILE`; it spends provider quota and is not an ordinary test.

## Naming Conventions

**Files:**
- Rust source and Python harnesses use lowercase snake_case: `src/lib.rs`, `tests/policy.rs`, `tests/dll_host.py`, and `tests/live_probe.py`.
- The package name is kebab-case (`codex-window-activation` in `Cargo.toml:2`), while Cargo's generated DLL name is underscore-separated (`target/release/codex_window_activation.dll`).
- Planning documents use uppercase names: `.planning/codebase/ARCHITECTURE.md` and `.planning/codebase/STRUCTURE.md`.

**Directories:**
- Cargo follows conventional `src/` and `tests/` directories.
- Generated/tooling directories are lowercase and dot-prefixed: `.planning/`, `.opencode/`, and `target/`.

**Rust symbols:**
- Types and enums use PascalCase: `Decision`, `Window`, `Buffer`, `HostApi`, and `PluginApi` (`src/lib.rs:19-125`).
- Functions, locals, and fields use snake_case: `due_deadlines`, `state_path`, `active_operation`, and `free_buffer` (`src/lib.rs:59-139`).
- Constants use uppercase snake case: `ID` and `USER_AGENT` (`src/lib.rs:15-17`).
- Host RPC methods and management paths use dot-separated method names and slash-separated paths: `host.auth.list`, `host.http.cancel`, and `/plugins/codex-window-activation/status` (`src/lib.rs:304`, `src/lib.rs:431`, `src/lib.rs:451`).

**Python symbols:**
- Classes and `ctypes` structures use PascalCase (`Buffer`, `Host`, `Plugin`); functions and collections use snake_case (`quota`, `callback`, `quota_reads`) in `tests/dll_host.py:10-29`.

## Where to Add New Code

**New Feature:**
- Primary code: Add the smallest feature-specific functions to `src/lib.rs`, preserving the existing boundary ordering: parse/validate near the boundary, orchestration in `poll`, policy in pure functions, and host I/O through `host`/`http`.
- Tests: Add deterministic policy cases to `tests/policy.rs` or `#[cfg(test)]` tests beside the relevant function in `src/lib.rs`; use `tests/dll_host.py` when the behavior depends on ABI, lifecycle, persistence, or host callbacks.
- If the feature introduces a second independent subsystem, extract a Rust module under `src/` only after defining its boundary and updating this structure map; the current production layout has no existing module directory to follow.

**New Component/Module:**
- Implementation: Keep C ABI structs and exported callbacks in `src/lib.rs:105-125` and `src/lib.rs:464-538`; keep RPC names in `handle` at `src/lib.rs:438-462`; keep recurring work under `start`/`stop` at `src/lib.rs:401-435`.
- Host integration: Extend the callback adapter in `src/lib.rs:141-175` and transport in `src/lib.rs:216-261`; do not create a second HTTP or credential path.
- Policy: Put deterministic quota or spending rules beside `windows`, `due_deadlines`, and `decision` in `src/lib.rs:32-91`, with explicit time/state parameters.

**Utilities:**
- Shared helpers: Place helpers in `src/lib.rs` near the layer they serve (`decode_body` near transport, `state_path`/`save` near persistence). There is no `src/util/` or `src/common/` directory in the current tree.
- Test utilities: Keep mock-host state and C ABI definitions in the test harness that owns them (`tests/dll_host.py:10-56`); avoid importing live credentials or using `tests/live_probe.py` as a general fixture.

**Documentation:**
- Runtime and installation behavior belongs in `README.md`.
- Repository constraints belong in `AGENTS.md`.
- Architecture and placement guidance belongs in `.planning/codebase/ARCHITECTURE.md` and `.planning/codebase/STRUCTURE.md`.

## Special Directories

**`target/`:**
- Purpose: Cargo output, including the release DLL used by the native harness.
- Generated: Yes.
- Committed: No; excluded by `.gitignore:1`.

**`.opencode/`:**
- Purpose: Project-local OpenCode/GSD installation and machine-specific workflow assets.
- Generated: Yes, by the GSD installation process described in `README.md:97-118`.
- Committed: No; excluded by `.gitignore:2`.

**`.planning/codebase/`:**
- Purpose: GSD-generated repository analysis documents.
- Generated: Yes, by mapping workflows.
- Committed: Not excluded by the current `.gitignore`; treat the Markdown files as repository planning artifacts and update their date/path references when the structure changes.

**Runtime `%USERPROFILE%\.cli-proxy-api\`:**
- Purpose: CLIProxyAPI's host-managed configuration area and this plugin's persistent state file.
- Generated: The plugin creates/updates `codex-window-activation-state.json` through `src/lib.rs:294-301`.
- Committed: No; it is outside the repository and contains runtime state. Repository `.gitignore:9-11` also excludes state/auth filename patterns if copied into the checkout.

---

*Structure analysis: 2026-10-09*
