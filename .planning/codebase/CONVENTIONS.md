# Coding Conventions

**Analysis Date:** 2026-10-09

## Naming Patterns

**Files:**
- Rust source is organized as the single snake-case module file `src/lib.rs`.
- Rust integration tests use descriptive snake-case names in `tests/policy.rs`.
- Python executable checks use snake-case filenames, `tests/dll_host.py` and `tests/live_probe.py`.

**Functions:**
- Rust functions use `snake_case`, including domain helpers such as `due_deadlines`, `state_path`, `decode_body`, `poll`, and `plugin_shutdown` in `src/lib.rs`.
- Test functions use behavior-oriented snake-case names that state the expected result, such as `due_five_hour_window_activates_but_not_before_reset` in `tests/policy.rs`.
- Python functions follow snake_case (`quota`, `callback`, `free`, and nested `call` in `tests/dll_host.py`).

**Variables:**
- Rust locals and parameters use short, lower-case snake_case names (`q`, `w`, `now`, `state`, `auth`, `entry`) when their surrounding function makes the domain clear; use descriptive names such as `attempt_key`, `last_verification`, and `ACTIVE_OPERATION` for stateful concepts in `src/lib.rs`.
- Python test state uses concise lower-case names (`allocations`, `prompts`, `quota_reads`, `requests`, and `results`) in `tests/dll_host.py` and `tests/live_probe.py`.

**Types:**
- Rust structs and enums use UpperCamelCase: `Decision`, `Window`, `Buffer`, `HostApi`, and `PluginApi` in `src/lib.rs`.
- Enum variants use UpperCamelCase (`Wait`, `Activate`, `Exhausted` in `src/lib.rs`).
- C ABI aliases use UpperCamelCase (`HostCall`, `PluginCall`, `Free`) while C-facing struct fields remain lower snake_case in `src/lib.rs`.
- Python `ctypes.Structure` classes use UpperCamelCase (`Buffer`, `Host`, `Plugin`) in `tests/dll_host.py`.

## Code Style

**Formatting:**
- Use standard `rustfmt`; `cargo fmt -- --check` passes for the tracked Rust code.
- The project has no checked-in `rustfmt.toml`, `.rustfmt.toml`, Python formatter configuration, or general style configuration. Preserve rustfmt output for `src/lib.rs` and `tests/policy.rs`.
- Keep Rust imports grouped at the top of `src/lib.rs`, with nested imports for related standard-library modules (`ffi`, `sync`, and `time`).
- Python scripts are direct executable checks with compact `ctypes` declarations and inline dictionaries; preserve their existing script-oriented style when extending the native harnesses in `tests/dll_host.py` and `tests/live_probe.py`.

**Linting:**
- The enforced lint command is `cargo clippy --offline --all-targets -- -D warnings`, documented in `AGENTS.md` and `README.md`.
- No Python linter configuration or Python lint command is present. Avoid adding dependencies solely for style checks without an explicit requirement.
- Keep unsafe ABI code explicit and narrow. The exported entry point and callback functions in `src/lib.rs` use `unsafe extern "C"`; safe domain logic remains in ordinary Rust functions.

## Import Organization

**Order:**
1. External crate imports (`use serde_json::{json, Value};`) in `src/lib.rs`.
2. Standard-library imports in one grouped `use std::{ ... };` statement.
3. Test-only imports inside the `#[cfg(test)] mod tests` block, as in `src/lib.rs`, or at the top of the integration test module, as in `tests/policy.rs`.

**Path Aliases:**
- No path aliases or workspace modules are configured. Integration tests import the library by crate name (`use codex_window_activation::{decision, windows, Decision};` in `tests/policy.rs`).
- Keep new Rust modules referenced directly from `src/lib.rs`; there is no `mod.rs` hierarchy or barrel module.

## Error Handling

**Patterns:**
- Domain and boundary helpers return `Result<T, String>` and use short stable error text, for example `windows`, `host`, `decode_body`, `http`, `quota`, `activate`, `save`, and `poll` in `src/lib.rs`.
- Convert absent or malformed JSON fields with `.ok_or(...)`, validate ranges explicitly, and use `.map_err(...)` to avoid leaking parser or host details; follow `windows` and `host` in `src/lib.rs`.
- Use `unwrap_or`/`unwrap_or_else` only for documented defaults or per-account reporting. `poll` converts an account failure into a JSON status instead of aborting all account processing (`src/lib.rs:323-393`).
- The FFI request boundary catches panics with `std::panic::catch_unwind` and serializes failures as `{ "ok": false, "error": ... }` in `plugin_call` (`src/lib.rs:487-526`). Keep this boundary behavior for any new plugin operation.
- Do not persist upstream error bodies or credentials. `host` emits only a sanitized status-based error, and `tests/dll_host.py` verifies that persisted state does not contain `access_token`.

## Logging

**Framework:** There is no logging framework or direct console logging in the Rust library (`src/lib.rs`).

**Patterns:**
- Report operational state through the shared `REPORT: Mutex<Value>` and the management response in `handle` (`src/lib.rs:138`, `src/lib.rs:438-459`).
- Store per-account failures as status strings in the report rather than printing from the worker thread (`src/lib.rs:392-398`).
- Python harnesses print one final PASS line, and `tests/live_probe.py` prints a JSON result summary; preserve these as test output, not production logging.

## Comments

**When to Comment:**
- Add comments for security, persistence, timing, or compatibility rationale rather than restating code. Existing examples explain why host error text is sanitized, why backoff is persisted before a request, why a successful prompt is saved before refreshing quota, and how an expired deadline is retained (`src/lib.rs:168-170`, `src/lib.rs:345`, `src/lib.rs:362`, `src/lib.rs:376`).
- Keep comments close to the invariant they protect. The FFI safety contract is documented directly above `cliproxy_plugin_init` (`src/lib.rs:464-467`).
- Test comments should explain test-only behavior, such as the simulated reset boundary in `tests/live_probe.py:51-56`.

**JSDoc/TSDoc:**
- Not applicable. This repository contains Rust and Python, not JavaScript or TypeScript.
- Rust doc comments are used for the exported unsafe ABI initializer's safety contract, but ordinary private functions do not have doc comments (`src/lib.rs:464-467`).

## Function Design

**Size:**
- Keep pure policy logic isolated in small helpers (`windows`, `due_deadlines`, and `decision` in `src/lib.rs`) so it can be exercised without the host ABI or network.
- Boundary orchestration may remain in the worker pipeline (`poll` in `src/lib.rs`), but place protocol parsing, HTTP callback handling, persistence, and activation in separate helpers as the current file does.

**Parameters:**
- Pass parsed JSON as `&Value` when a helper only reads it (`windows`, `decision`, `quota`) and `&mut Value` when it updates persistent state (`poll`) in `src/lib.rs`.
- Use explicit `method: &str`, `url: &str`, and `Option<Value>` parameters for the generic HTTP helper (`http` in `src/lib.rs`).
- Keep C ABI signatures exactly represented by `#[repr(C)]` structs and `unsafe extern "C"` function types in `src/lib.rs`; do not wrap them in Rust-only layouts.

**Return Values:**
- Return `Result` for fallible parsing, host calls, network operations, persistence, and plugin dispatch.
- Return domain enums for policy decisions (`Decision`) and typed structs for supported quota windows (`Window`) in `src/lib.rs`.
- Return JSON `Value` envelopes only at the host/plugin protocol boundary and management status boundary.

## Module Design

**Exports:**
- Keep the public Rust surface minimal: `Decision`, `Window`, `windows`, and `decision` are public for policy testing; ABI structs and the exported initializer are public where the host contract requires them (`src/lib.rs`).
- Keep worker state, HTTP helpers, persistence, dispatch, and callbacks private in `src/lib.rs` unless an external contract requires exposure.

**Barrel Files:**
- None. The crate currently has one source module, `src/lib.rs`; do not introduce a barrel file pattern without adding a real module boundary.

---

*Convention analysis: 2026-10-09*
