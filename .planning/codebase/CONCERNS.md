# Codebase Concerns

**Analysis Date:** 2026-10-09

## Tech Debt

**Persistent state lifecycle:**
- Issue: `save` writes directly to a fixed temporary filename and does not create the parent directory, sync the file, or remove an orphaned temporary file. The persistent deduplication/backoff guarantees therefore depend on the CLIProxyAPI process having already created `%USERPROFILE%\.cli-proxy-api` and on ordinary filesystem caching being sufficient.
- Files: `src/lib.rs:294-301`, `src/lib.rs:401-412`, `README.md:33-47`
- Impact: A fresh or partially provisioned profile cannot persist state and cannot activate; a power loss can lose the pre-request backoff or completed-deadline record and permit duplicate upstream spending.
- Fix approach: Create and validate the state directory during startup, write through a uniquely named file, flush before replacement, clean up stale temporary files, and explicitly document the durability boundary.

**Unbounded account history:**
- Issue: Account entries remain in the state object after an auth file disappears, and `attempts` retains a key for every reset indefinitely. Only the `completed` array is capped at 32 records.
- Files: `src/lib.rs:320-322`, `src/lib.rs:334-347`, `src/lib.rs:353-360`
- Impact: Long-running installations with rotated accounts or many reset cycles grow the JSON state and make every 60-second save increasingly expensive.
- Fix approach: Add a state schema/version and prune attempts and removed-account entries using a retention policy that preserves the active deduplication window.

**Dynamic JSON state model:**
- Issue: Runtime state, host responses, quota windows, and management output all use untyped `serde_json::Value` indexing rather than validated structs.
- Files: `src/lib.rs:32-57`, `src/lib.rs:303-398`, `src/lib.rs:404-412`
- Impact: Missing, mistyped, or partially valid fields silently fall back to defaults in several places, making safety decisions difficult to audit and allowing malformed state to reach activation logic.
- Fix approach: Introduce typed deserialization for host contracts, quota responses, and versioned persistent records; reject invalid records before any spend decision.

**Contract/version coupling:**
- Issue: The plugin hard-codes CLIProxyAPI schema/ABI assumptions, private ChatGPT endpoints, a model name, and a Codex client user-agent in implementation code. The metadata reports schema 6 but startup only checks host ABI version 1.
- Files: `src/lib.rs:15-17`, `src/lib.rs:263-280`, `src/lib.rs:438-458`, `README.md:1-4`, `README.md:134-139`
- Impact: A gateway, quota response, model, header, or SSE contract change can disable activation or change routing behavior without a compile-time or startup compatibility signal.
- Fix approach: Centralize and version the external contract constants, validate required host capabilities, and maintain an automated compatibility harness for each supported CLIProxyAPI version.

## Known Bugs

**Missing state-directory creation:**
- Symptoms: `start` accepts an absent state file, but the first `save` fails when `%USERPROFILE%\.cli-proxy-api` does not exist.
- Files: `src/lib.rs:99-103`, `src/lib.rs:294-301`, `src/lib.rs:401-424`
- Trigger: Run the DLL in a Windows profile where the CLIProxyAPI data directory has not been created by another component.
- Workaround: Create `%USERPROFILE%\.cli-proxy-api` before registering the plugin.

**Partially corrupt records can lose deduplication:**
- Symptoms: Startup rejects malformed JSON and a non-object root `accounts` value, but an individual account record with a scalar value, `null`, or incomplete fields is accepted. `poll` replaces non-object records with an empty record, while invalid nested fields use defaults.
- Files: `src/lib.rs:320-322`, `src/lib.rs:331-347`, `src/lib.rs:373-386`, `src/lib.rs:404-412`
- Trigger: A truncated/hand-edited state record that remains valid JSON, or a state-format change that leaves the root `accounts` object intact.
- Workaround: Preserve the state file and restore the complete account record from a known-good backup; do not delete or hand-edit state during operation.

**Stopped workers report as running:**
- Symptoms: `REPORT` is updated with `"running":true` during polling but is not cleared by `stop`; a management request after quiesce can return stale account data and a false running status.
- Files: `src/lib.rs:138-139`, `src/lib.rs:395-398`, `src/lib.rs:427-435`, `src/lib.rs:450-458`
- Trigger: Call `plugin.quiesce` or `plugin.shutdown`, then query the management handler while the DLL remains loaded.
- Workaround: Treat the management endpoint as stale after quiesce and rely on plugin lifecycle state from the host.

## Security Considerations

**Persistent deduplication is not protected against concurrent writers or local tampering:**
- Risk: Multiple CLIProxyAPI processes, or another process running as the same user, can read and replace the same JSON file without an OS lock or integrity check. A stale read/write race or reset of `completed` records can cause repeated paid requests.
- Files: `src/lib.rs:99-103`, `src/lib.rs:294-301`, `src/lib.rs:303-398`, `README.md:44-47`
- Current mitigation: OAuth tokens are not written to state, writes use a same-directory temporary file, and the host process is expected to provide the normal single plugin lifecycle.
- Recommendations: Use a per-user single-instance lock or transactional state store, bind records to the authenticated account identity, and validate/version the state before spending.

**Crash window after provider acceptance:**
- Risk: There is no request idempotency key or durable completion record until after the response is fully received and saved. A crash, cancellation race, malformed SSE response, or post-request filesystem error can leave a paid request unrecorded and eligible for retry after the backoff.
- Files: `src/lib.rs:272-292`, `src/lib.rs:344-363`, `README.md:120-129`
- Current mitigation: Attempts and a 15-minute retry deadline are saved before the request, successful deadlines are persisted before quota verification, and attempts are capped at three.
- Recommendations: Use provider-supported idempotency if available; otherwise make the uncertain-request state explicit, surface it in status, and require operator confirmation before a recovery spend.

**Live test has a real spending side effect:**
- Risk: Invoking the live probe with `CPA_LIVE_AUTH_FILE` sends a real inference and consumes account quota; the script has no interactive confirmation or runtime allowlist beyond the environment variable and the selected JSON file.
- Files: `tests/live_probe.py:25-27`, `tests/live_probe.py:43-62`, `tests/live_probe.py:73-92`, `README.md:68-73`, `.gitignore:5-11`
- Current mitigation: The test is separate from ordinary tests, uses an isolated state home, requires an explicitly named environment variable, and documents the quota cost.
- Recommendations: Add an explicit opt-in/confirmation flag and fail closed in CI; keep credential-file and live-output patterns covered by repository ignore rules.

**Credential lifetime and trust boundaries are implicit:**
- Risk: `host.auth.get` data, including the bearer token, remains in a heap-backed `Value` for the polling operation and is passed through generic JSON/FFI plumbing. There is no explicit token redaction/zeroization policy beyond not serializing `auth` into state.
- Files: `src/lib.rs:216-260`, `src/lib.rs:303-329`, `README.md:8-15`, `README.md:44-47`
- Current mitigation: Host errors omit upstream body text, state excludes the auth object, and all network URLs are HTTPS endpoints delegated to host HTTP callbacks.
- Recommendations: Keep auth data in a narrower lifetime, avoid generic copies where practical, validate the host callback contract, and document the local-process memory threat model.

## Performance Bottlenecks

**Sequential per-account polling:**
- Problem: One worker performs auth lookup, quota GET, optional activation POST, and verification GET for each account in sequence.
- Files: `src/lib.rs:303-398`, `README.md:9-13`
- Cause: `poll` runs all network work on one thread and the 60-second wait starts only after the complete poll returns.
- Improvement path: Bound the account set and measure cycle duration; if concurrency is safe under the host contract, use bounded workers while retaining per-account serialization and a single state transaction.

**Unbounded response buffering:**
- Problem: Host response bodies are copied into a `Vec`, decoded into another buffer, and then parsed as JSON or scanned as text.
- Files: `src/lib.rs:145-174`, `src/lib.rs:177-214`, `src/lib.rs:254-270`, `src/lib.rs:282-292`
- Cause: The host response contract is consumed fully before quota or SSE processing, with no body-size limit or streaming parser.
- Improvement path: Enforce bounded body sizes for quota and activation responses and use incremental SSE processing if the host supports it.

## Fragile Areas

**Shutdown and cancellation:**
- Files: `src/lib.rs:216-252`, `src/lib.rs:303-318`, `src/lib.rs:401-435`, `AGENTS.md:18-22`
- Why fragile: HTTP work has a watchdog, but `host.auth.list`, `host.auth.get`, and `host.http.operation_open` have no timeout or cancellation path. `stop` joins the worker unconditionally, so a blocked host callback can block plugin quiesce/shutdown indefinitely.
- Safe modification: Keep callback ownership and host-lifetime requirements explicit; add bounded/cancelable auth operations or avoid joining indefinitely, then test cancellation at every callback boundary.
- Test coverage: `tests/dll_host.py:81-103` checks normal cleanup only; it does not simulate a blocked auth callback, blocked operation-open call, or cancellation race.

**Global mutable lifecycle state:**
- Files: `src/lib.rs:133-139`, `src/lib.rs:401-435`, `src/lib.rs:464-537`
- Why fragile: A process-wide `OnceLock<Host>`, worker handle, stop flag, condition variable, report, and active operation coordinate all calls. `HOST` cannot be replaced after initialization, lock poisoning is unrecoverable because locks use `unwrap`, and worker panics are not reflected in status.
- Safe modification: Treat initialization as strictly one-shot, validate/reject repeated init explicitly, isolate lifecycle state in a tested owner, and surface join/panic failures in the management report.
- Test coverage: `tests/dll_host.py:63-103` covers one init, reconfigure, and normal shutdown but not repeated initialization, poisoned locks, or worker panic recovery.

**Wall-clock scheduling:**
- Files: `src/lib.rs:59-97`, `src/lib.rs:329-349`, `README.md:28-31`, `AGENTS.md:23-24`
- Why fragile: Reset decisions use `SystemTime` epoch seconds and persisted provider deadlines. Large system-clock jumps can make a deadline appear due early or delay activation; the tests induce due time through a simulated response rather than proving real reset behavior.
- Safe modification: Compare fresh provider deadlines with a monotonic observation window, reject implausible clock/deadline changes, and retain an operator-visible uncertain state.
- Test coverage: `tests/policy.rs:5-50` covers fixed integer timestamps only; `tests/live_probe.py:51-56` uses a test-only clock mutation.

**Low-fidelity failure reporting:**
- Files: `src/lib.rs:166-173`, `src/lib.rs:392-398`, `src/lib.rs:413-418`, `src/lib.rs:450-458`
- Why fragile: Host callback errors discard the host error code/message and retain only an HTTP status, while each poll overwrites the single global report. Authentication failures, cancellation, malformed host responses, and filesystem failures are difficult to distinguish operationally.
- Safe modification: Keep secrets out of persisted logs but expose a typed, redacted error class, timestamps, retry state, and worker health in the authenticated status response.
- Test coverage: `tests/dll_host.py:49-57` checks one 429 path but does not verify status quality for callback, filesystem, parse, or shutdown failures.

## Scaling Limits

**Account-count-dependent poll latency:**
- Current capacity: Every account requires at least one sequential quota request per 60-second cycle; an activation requires an additional POST and verification GET.
- Limit: With enough accounts or repeated 45-second network timeouts, a poll can exceed the intended interval and delay reset activation for all later accounts.
- Scaling path: Add bounded concurrency or a scheduler with per-account deadlines, and expose cycle duration/queue depth in `REPORT`.
- Files: `src/lib.rs:235-239`, `src/lib.rs:303-398`, `README.md:9-13`

**Shared state across processes:**
- Current capacity: State coordination is process-local and the file is accessed by ordinary reads/writes.
- Limit: Concurrent plugin instances sharing a user profile can both decide that the same reset is due before either persists completion.
- Scaling path: Add an OS-level per-state lock and atomic compare/update semantics, or make the state location instance-specific and reject duplicate account ownership.
- Files: `src/lib.rs:99-103`, `src/lib.rs:294-301`, `src/lib.rs:401-424`

## Dependencies at Risk

**CLIProxyAPI and private upstream contracts:**
- Risk: Supported core versions, RPC schema, auth fields, management route shape, private quota endpoint, response endpoint, model, and headers are all externally controlled.
- Impact: Contract drift can produce failed activation, quota misclassification, or incorrect account routing without a local compile failure.
- Migration plan: Maintain versioned contract fixtures and DLL-host integration tests for every supported core release; gate installation on the reported core/plugin compatibility.
- Files: `README.md:1-4`, `README.md:134-139`, `src/lib.rs:110-125`, `src/lib.rs:263-280`, `src/lib.rs:438-458`

**Exact dependency pin without update policy:**
- Risk: `serde_json` is pinned exactly and the lockfile is committed, but there is no checked-in dependency audit, update policy, or CI workflow.
- Impact: Security fixes and compiler compatibility changes require manual discovery, while dependency changes are difficult to validate consistently on the Windows MSVC target.
- Migration plan: Add an explicit supported Rust/MSVC version, automated offline/online dependency checks as appropriate, and a reviewed update cadence.
- Files: `Cargo.toml:1-10`, `Cargo.lock:1-105`, `README.md:53-62`

## Missing Critical Features

**No checked-in continuous validation for the native boundary:**
- Problem: The repository documents Cargo checks and a Python DLL harness, but contains no CI workflow that builds the MSVC DLL and executes `tests/dll_host.py`.
- Blocks: ABI regressions, Windows-specific filesystem behavior, and cross-language buffer ownership errors can reach installation without automated validation.
- Files: `README.md:53-66`, `tests/dll_host.py:1-104`, `Cargo.toml:1-10`

**No durable uncertain-request state:**
- Problem: The state model distinguishes pending attempts and completed deadlines but has no explicit state for “the provider may have accepted the request and the result is unknown.”
- Blocks: Safe operator recovery after a timeout, process crash, cancellation race, or response parse failure without choosing between duplicate spend and missed activation.
- Files: `src/lib.rs:320-363`, `README.md:120-129`

**No schema migration or integrity metadata for state:**
- Problem: The JSON file has no version, account identity fingerprint, checksum, or migration handler.
- Blocks: Safe upgrades, detection of partial corruption, and protection of reset records when auth indexes are reused.
- Files: `src/lib.rs:294-301`, `src/lib.rs:320-322`, `src/lib.rs:404-412`

## Test Coverage Gaps

**Persistent state safety:**
- What's not tested: Missing parent directory, invalid nested account records, partial writes, power-loss durability, stale temporary files, concurrent writers, state schema upgrades, and account-index reuse.
- Files: `src/lib.rs:294-301`, `src/lib.rs:320-322`, `src/lib.rs:404-412`, `tests/dll_host.py:58-104`
- Risk: Deduplication can be lost and paid requests can repeat without a failing unit test.
- Priority: High

**Lifecycle and cancellation failures:**
- What's not tested: Repeated `cliproxy_plugin_init`, host callback blocking, operation-open timeout, cancellation races, worker panic, poisoned mutexes, and status after quiesce.
- Files: `src/lib.rs:133-139`, `src/lib.rs:216-252`, `src/lib.rs:401-435`, `tests/dll_host.py:63-103`
- Risk: Shutdown can hang or report a healthy worker after it has stopped.
- Priority: High

**Real external-contract behavior:**
- What's not tested: CLIProxyAPI version mismatches, changed quota field types, multiline/fragmented SSE data, changed response casing, model retirement, auth expiry, and provider-side acceptance with an incomplete response.
- Files: `src/lib.rs:32-57`, `src/lib.rs:254-292`, `README.md:134-139`, `tests/live_probe.py:43-62`
- Risk: The plugin can silently stop activating or retry an already accepted request after upstream drift.
- Priority: High

**Automated integration coverage:**
- What's not tested: The Python DLL harness is not part of `cargo test` and no CI configuration invokes it; the live probe is intentionally manual and spends quota.
- Files: `README.md:53-73`, `tests/dll_host.py:1-104`, `tests/live_probe.py:1-4`
- Risk: The repository's passing Rust unit tests do not establish the native ABI, Windows runtime, or real provider behavior.
- Priority: Medium

---

*Concerns audit: 2026-10-09*
