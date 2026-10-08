# Testing Patterns

**Analysis Date:** 2026-10-09

## Test Framework

**Runner:**
- Rust's built-in test harness runs unit tests in `src/lib.rs` and integration tests in `tests/policy.rs`.
- The crate uses Rust edition 2021 and has no test-specific dependency beyond `serde_json`, declared in `Cargo.toml`.
- Native ABI checks are standalone Python scripts: `tests/dll_host.py` and `tests/live_probe.py`. They are not discovered by Cargo or a Python test runner.

**Assertion Library:**
- Rust uses the standard `assert!`, `assert_eq!`, `Result::is_err`, and `unwrap` assertions in `src/lib.rs` and `tests/policy.rs`.
- Python uses built-in `assert` statements in `tests/dll_host.py` and `tests/live_probe.py`; no `pytest`, `unittest`, or mocking package is configured.

**Run Commands:**
```bash
cargo test --offline                                # Rust unit, integration, and doctest harnesses
cargo clippy --offline --all-targets -- -D warnings # Rust lint gate
cargo build --release --offline                     # Build the DLL used by the native harness
python tests\dll_host.py                            # Mock-host native ABI/system integration check
```

The live probe is intentionally separate from the ordinary checks:
```bash
python tests\live_probe.py                          # Only with explicit authorization and CPA_LIVE_AUTH_FILE
```
`AGENTS.md` and `README.md` explicitly prohibit treating `tests/live_probe.py` as a routine test because it sends a real inference request and spends quota.

## Test File Organization

**Location:**
- Pure unit tests are co-located in the `#[cfg(test)] mod tests` block at the end of `src/lib.rs`.
- Rust integration tests are in the root-level `tests/` directory, currently `tests/policy.rs`.
- Native system/ABI probes are executable scripts in `tests/dll_host.py` and `tests/live_probe.py`.

**Naming:**
- Rust test names describe a behavior and its boundary, for example `simultaneous_resets_need_one_activation_and_completed_deadlines_dont_repeat` in `tests/policy.rs`.
- Python scripts are named after the exercised boundary: `dll_host.py` for a mock C-ABI host and `live_probe.py` for the explicitly authorized real provider path.

**Structure:**
```
src/lib.rs                 # 2 focused unit tests beside private policy/parsing helpers
tests/policy.rs            # 5 pure Rust integration tests for public quota policy
tests/dll_host.py          # end-to-end compiled release DLL against a deterministic callback host
tests/live_probe.py        # opt-in real HTTP/inference probe with isolated state
```

## Test Structure

**Suite Organization:**
```rust
#[test]
fn due_five_hour_window_activates_but_not_before_reset() {
    let q = json!({"rate_limit": {"primary_window": {
        "limit_window_seconds": 18000,
        "used_percent": 0,
        "reset_at": 1000
    }}});
    let w = windows(&q).unwrap();
    assert_eq!(decision(&w, 999, &json!({})), Decision::Wait);
    assert_eq!(decision(&w, 1031, &json!({})), Decision::Activate);
}
```
This is the established style in `tests/policy.rs`: construct an inline `serde_json::json!` fixture, parse it through the public helper, then assert the behavior at explicit timestamps.

**Patterns:**
- Keep policy tests deterministic by passing `now` explicitly to `decision` instead of reading the system clock (`src/lib.rs` and `tests/policy.rs`).
- Use inline JSON fixtures to cover protocol shapes, swapped primary/secondary windows, malformed quota, exhausted windows, retry backoff, and completed deadlines (`tests/policy.rs`).
- Test private parsing helpers in the co-located module when they are not part of the public policy API; `base64_host_body_decodes` directly covers `decode_body` in `src/lib.rs`.
- Validate aggregate outcomes in the native harness, including the request sequence, account routing, persistent state, buffer ownership, restart behavior, and shutdown (`tests/dll_host.py`).

The current Cargo run covers 2 unit tests, 5 integration tests, and 0 doctests. The enforced Rust suite passes with `cargo test --offline`.

## Mocking

**Framework:**
- No Rust mocking framework is used.
- `tests/dll_host.py` provides a deterministic host callback with `ctypes.CFUNCTYPE`, returning JSON envelopes for `host.auth.list`, `host.auth.get`, `host.http.operation_open`, `host.http.cancel`, and `host.http.do`.
- `tests/live_probe.py` keeps the host callbacks synthetic for auth discovery but forwards `host.http.do` to `urllib.request.urlopen` for the explicitly opt-in real provider check.

**Patterns:**
```python
@HostCall
def callback(ctx, method, request, size, out):
    req = json.loads(c.string_at(request, size))
    method = method.decode()
    if method == 'host.auth.list':
        result = {'files': accounts}
    elif method == 'host.http.do':
        # Return a base64 body in the same envelope shape as the native host.
        result = {'StatusCode': 200, 'Body': base64.b64encode(body).decode()}
    raw = json.dumps({'ok': True, 'result': result}).encode()
    ...
```
The callback shape and buffer ownership in this example are implemented in `tests/dll_host.py:31-53`; extend that callback when testing a new host method rather than bypassing the compiled DLL.

**What to Mock:**
- Mock host auth enumeration and retrieval so tests can exercise enabled, disabled, exhausted, and failed accounts without reading real credential files (`tests/dll_host.py`).
- Mock quota and streaming completion bodies in `tests/dll_host.py` to make exact request routing, response parsing, backoff, deduplication, and retry caps deterministic.
- Use `tempfile.TemporaryDirectory` and override `USERPROFILE` so persistent state is isolated from the real `%USERPROFILE%` (`tests/dll_host.py:58-61`, `tests/live_probe.py:73-75`).

**What NOT to Mock:**
- Do not replace the Rust DLL with a Python reimplementation in the native check; load `target/release/codex_window_activation.dll` and call `cliproxy_plugin_init` through `ctypes` (`tests/dll_host.py:61-68`).
- Do not run the real provider path from ordinary test commands. `tests/live_probe.py` requires `CPA_LIVE_AUTH_FILE`, an explicitly selected account, and explicit authorization because it spends quota (`AGENTS.md:12-14`).
- Do not commit auth files, access tokens, live quota records, or live test output; `.gitignore` protects common state/auth patterns and the test documentation reiterates the restriction (`.gitignore`, `README.md`).

## Fixtures and Factories

**Test Data:**
```rust
let q = json!({
    "rate_limit": {
        "primary_window": {
            "limit_window_seconds": 18000,
            "used_percent": 0,
            "reset_at": 1000
        },
        "secondary_window": {
            "limit_window_seconds": 604800,
            "used_percent": 100,
            "reset_at": 10000
        }
    }
});
let w = windows(&q).unwrap();
assert_eq!(decision(&w, 1031, &json!({})), Decision::Exhausted);
```
Use inline `json!` values in `tests/policy.rs` for pure policy cases. The Python system fixture is generated by `quota(account)` and the module-level `accounts` list in `tests/dll_host.py`; there is no shared fixture directory or factory module.

**Location:**
- Rust fixtures live next to each test in `src/lib.rs` and `tests/policy.rs`.
- Native callback fixtures and captured observations live in the script module globals in `tests/dll_host.py` and `tests/live_probe.py`.
- No external fixture files are used. Keep any credential-like or state-like test data in temporary directories, not in the repository.

## Coverage

**Requirements:** No coverage target, coverage tool, or threshold configuration is detected. The quality gate is the passing Rust test suite plus `cargo clippy --offline --all-targets -- -D warnings`; native ABI behavior is covered by the standalone mock-host script.

**View Coverage:** No repository command is provided for coverage. If coverage is added, keep it offline and do not run `tests/live_probe.py` as a substitute for coverage.

## Test Types

**Unit Tests:**
- `src/lib.rs` unit tests cover base64 response-body decoding and remembered-deadline policy behavior without host callbacks or network access.
- Keep new deterministic parsing and decision rules in pure helpers so they can be tested in this layer (`src/lib.rs:32-90`, `src/lib.rs:177-214`).

**Integration Tests:**
- `tests/policy.rs` exercises the public `windows`, `decision`, and `Decision` API against representative JSON quota states.
- `tests/dll_host.py` is the native end-to-end integration test: it loads the release DLL, supplies the C ABI host, waits for the worker report, restarts/reconfigures the plugin, edits only temporary state, and verifies shutdown cleanup.

**E2E Tests:**
- `tests/live_probe.py` is the only real-provider probe. It performs `GET` quota, one `POST` inference, and a follow-up `GET` verification through `urllib.request`, while using a temporary state home.
- It is not part of the default check set and must only run with an explicitly selected `CPA_LIVE_AUTH_FILE` (`AGENTS.md`, `README.md`).

## Common Patterns

**Async Testing:**
```python
deadline = time.time() + 10
while time.time() < deadline:
    report = call('management.handle')
    status = json.loads(bytes(report['Body']))
    if status and status.get('accounts'):
        break
    time.sleep(.05)
```
The worker runs on a Rust thread, so `tests/dll_host.py` and `tests/live_probe.py` poll the management report with a bounded deadline instead of assuming immediate completion. Preserve a bounded timeout and call `plugin.quiesce`/`plugin.shutdown` on every native test path.

**Error Testing:**
```rust
#[test]
fn malformed_or_unknown_quota_is_not_permission_to_spend() {
    assert!(windows(&json!({})).is_err());
    assert!(windows(&json!({"rate_limit": {
        "primary_window": {
            "limit_window_seconds": 18000,
            "used_percent": 101,
            "reset_at": 1000
        }
    }})).is_err());
}
```
Use invalid JSON shapes, out-of-range usage, retry state, and exhausted windows to prove that unsafe activation paths resolve to errors or `Decision::Wait`/`Decision::Exhausted` (`tests/policy.rs`). Use mock HTTP 429 responses to verify persisted retry backoff and the three-attempt cap (`tests/dll_host.py:49-50`, `tests/dll_host.py:82-101`).

---

*Testing analysis: 2026-10-09*
