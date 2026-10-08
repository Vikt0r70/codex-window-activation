<!-- refreshed: 2026-10-09 -->
# Architecture

**Analysis Date:** 2026-10-09

## System Overview

```text
┌──────────────────────────────────────────────────────────────────────┐
│ CLIProxyAPI host process                                              │
│ loads `target/release/codex_window_activation.dll`                    │
└──────────────────────────────┬───────────────────────────────────────┘
                               │ C ABI v1 / JSON RPC schema 6
                               ▼
┌──────────────────────────────────────────────────────────────────────┐
│ Native plugin boundary — `src/lib.rs`                                 │
│ `cliproxy_plugin_init` → `plugin_call` → `handle`                     │
└───────────────┬──────────────────────┬────────────────────────────────┘
                │ lifecycle             │ management
                ▼                       ▼
┌───────────────────────────┐  ┌────────────────────────────────────────┐
│ Worker lifecycle           │  │ `management.handle` response            │
│ `start` / `stop`           │  │ `REPORT` JSON, served by host API       │
│ one polling thread         │  └────────────────────────────────────────┘
└──────────────┬────────────┘
               ▼
┌──────────────────────────────────────────────────────────────────────┐
│ Account orchestration — `poll`                                        │
│ list accounts → get exact credentials → read quota → decide →         │
│ persist attempt/backoff → optionally activate → verify quota          │
└──────────────┬───────────────────────────┬───────────────────────────┘
               │                           │
               ▼                           ▼
┌──────────────────────────────┐  ┌────────────────────────────────────┐
│ Policy/domain                 │  │ Host integration / transport        │
│ `windows`, `due_deadlines`,   │  │ `host`, `http`, `quota`, `activate` │
│ `decision`                    │  │ auth and cancelable HTTP callbacks │
└──────────────┬───────────────┘  └────────────────┬───────────────────┘
               │                                   │
               ▼                                   ▼
┌──────────────────────────────┐  ┌────────────────────────────────────┐
│ Persistent state              │  │ Upstream Codex endpoints             │
│ `%USERPROFILE%\.cli-proxy-api\`│  │ `/backend-api/wham/usage`           │
│ `...state.json`, temp-file    │  │ `/backend-api/codex/responses`       │
│ then rename                   │  └────────────────────────────────────┘
└──────────────────────────────┘
```

The project is an embedded Windows native plugin rather than a standalone server. The only Rust source file, `src/lib.rs`, contains the ABI boundary, method dispatch, background worker, quota policy, host callback adapter, HTTP operation management, state persistence, and management reporting. `Cargo.toml` builds both an `rlib` for Rust tests and a `cdylib` for the host-loaded DLL.

## Component Responsibilities

| Component | Responsibility | File |
|-----------|----------------|------|
| Native ABI entry point | Validates host ABI version and callback table, installs host callbacks, and returns the plugin function table | `src/lib.rs:464-485` |
| Plugin call boundary | Converts C strings/byte buffers to JSON, dispatches safely, catches panics, and allocates response buffers | `src/lib.rs:487-535` |
| RPC method dispatcher | Maps lifecycle and management RPC method names to plugin behavior | `src/lib.rs:438-462` |
| Worker lifecycle | Starts one polling thread, wakes it on shutdown/reconfigure, and joins it safely | `src/lib.rs:401-435` |
| Account poller | Discovers accounts, skips disabled accounts, reads credentials/quota, applies policy, performs activation, verifies usage, and updates state/report | `src/lib.rs:303-399` |
| Quota model and policy | Parses supported five-hour/weekly windows and produces `Wait`, `Activate`, or `Exhausted` decisions | `src/lib.rs:19-91` |
| Host callback adapter | Serializes requests to CLIProxyAPI host RPC and sanitizes callback failures | `src/lib.rs:141-175` |
| HTTP adapter | Opens/cancels host-managed operations, sends exact-account headers, decodes response bodies, and enforces 2xx results | `src/lib.rs:216-261` |
| State persistence | Stores per-account deadlines, completion records, attempts, retry time, and timestamps without credentials | `src/lib.rs:294-301`, `src/lib.rs:320-396` |
| Status reporting | Exposes the current worker/account report through a registered management route | `src/lib.rs:450-458` |
| ABI host test double | Loads the compiled DLL and emulates auth, HTTP, allocation, and lifecycle callbacks | `tests/dll_host.py:10-104` |

## Pattern Overview

**Overall:** Embedded native plugin with a functional policy core and imperative host-integration shell.

**Key Characteristics:**
- The CLIProxyAPI process owns loading, authentication, HTTP transport, management authentication, and plugin lifecycle; the DLL consumes those capabilities through the `HostApi` callback table in `src/lib.rs:105-132`.
- The plugin uses a single long-lived worker thread and process-wide synchronization primitives (`HOST`, `WORKER`, `STOP`, `WAKE`, `REPORT`, and `ACTIVE_OPERATION`) in `src/lib.rs:133-139`.
- Quota decision logic is separated into public, testable functions (`windows` and `decision`) that accept JSON and an explicit timestamp instead of reading process state directly (`src/lib.rs:32-91`).
- Account state is JSON-shaped and keyed by the host-provided `auth_index`; credentials are fetched for a request and never written by `save` (`src/lib.rs:294-301`, `src/lib.rs:320-328`).
- External spending is guarded by remembered deadlines, positive-usage detection, pre-request retry backoff, completed-deadline deduplication, and a three-attempt cap (`src/lib.rs:59-90`, `src/lib.rs:333-368`).

## Layers

**Foreign Function Interface:**
- Purpose: Establish and maintain the C ABI contract with CLIProxyAPI.
- Location: `src/lib.rs:105-132`, `src/lib.rs:464-538`
- Contains: `repr(C)` buffers and API tables, exported initializer, plugin callbacks, response ownership functions.
- Depends on: Rust JSON serialization and internal dispatcher.
- Used by: CLIProxyAPI and the native host test harness in `tests/dll_host.py`.

**RPC Dispatch and Lifecycle:**
- Purpose: Translate host method names into registration, configuration, quiesce, shutdown, and management behavior.
- Location: `src/lib.rs:401-462`
- Contains: `handle`, `start`, `stop`, worker creation, worker wake-up, join, and status route registration.
- Depends on: orchestration functions and synchronized global state.
- Used by: `plugin_call` and the host lifecycle callbacks.

**Account Orchestration:**
- Purpose: Run the sequential per-account polling and activation workflow.
- Location: `src/lib.rs:303-399`
- Contains: account discovery, disabled-account filtering, credential retrieval, quota reads, policy decisions, state transitions, status records, and verification reads.
- Depends on: host callbacks, HTTP adapter, policy functions, state persistence, and clock.
- Used by: the worker loop in `src/lib.rs:413-423`.

**Quota Policy:**
- Purpose: Convert upstream quota JSON and persisted deadline state into a safe action.
- Location: `src/lib.rs:19-91`
- Contains: `Window`, `Decision`, supported window filtering, deadline calculation, reset grace period, exhausted-window guard, and retry guard.
- Depends on: `serde_json::Value` supplied by the caller.
- Used by: `poll` and Rust unit/integration tests in `src/lib.rs:540-558` and `tests/policy.rs`.

**Host Integration and Transport:**
- Purpose: Use CLIProxyAPI's auth and HTTP facilities while preserving host-wide proxy and cancellation behavior.
- Location: `src/lib.rs:141-292`
- Contains: callback invocation, response buffer ownership, base64/byte-array decoding, operation watchdog cancellation, quota GET, and activation POST.
- Depends on: the installed host callback implementation and the exact auth JSON returned by `host.auth.get`.
- Used by: account orchestration and shutdown cancellation.

**Persistence and Reporting:**
- Purpose: Make activation decisions restart-aware and provide a management-visible snapshot.
- Location: `src/lib.rs:99-103`, `src/lib.rs:294-301`, `src/lib.rs:395-398`
- Contains: Windows user-profile state path, temp-file write/rename, bounded completion history, retry timestamps, and `REPORT` updates.
- Depends on: local filesystem and synchronized process state.
- Used by: startup, polling, and `management.handle`.

## Data Flow

### Primary Request Path

1. CLIProxyAPI calls `cliproxy_plugin_init` with `HostApi` and an output `PluginApi` table (`src/lib.rs:464-485`).
2. CLIProxyAPI calls `PluginApi.call`; `plugin_call` validates the method/request buffers, parses JSON, and invokes `handle` (`src/lib.rs:487-516`).
3. `plugin.register` or `plugin.reconfigure` invokes `start`, which stops any existing worker, loads/validates persistent state, and spawns a new worker (`src/lib.rs:440-445`, `src/lib.rs:401-425`).
4. The worker invokes `poll` immediately, then waits up to 60 seconds on `WAKE`/`WAIT` (`src/lib.rs:413-423`).
5. `poll` calls `host.auth.list`, filters to Codex accounts, skips disabled entries, and calls `host.auth.get` using each exact `auth_index` (`src/lib.rs:303-328`).
6. `quota` performs a host-managed GET to `https://chatgpt.com/backend-api/wham/usage`; `windows` parses supported windows; `decision` applies retry, exhaustion, reset, positive-usage, and completion guards (`src/lib.rs:263-271`, `src/lib.rs:32-91`).
7. For `Activate`, `poll` persists the attempt count and 15-minute retry deadline before calling `activate`; successful `response.completed` data causes completion records and a post-activation quota refresh to be persisted (`src/lib.rs:331-370`).
8. `poll` writes a sanitized per-account report to `REPORT`; a management call returns it as a JSON response body (`src/lib.rs:395-398`, `src/lib.rs:450-458`).

### Shutdown and Cancellation Flow

1. `plugin.quiesce`, `plugin.shutdown`, or the exported `plugin_shutdown` calls `stop` (`src/lib.rs:446-448`, `src/lib.rs:536-538`).
2. `stop` sets `STOP`, wakes the worker condition variable, and checks `ACTIVE_OPERATION` (`src/lib.rs:427-432`).
3. If an HTTP operation is active, `stop` invokes `host.http.cancel`; it then joins the worker thread before returning (`src/lib.rs:430-435`).
4. Each HTTP call also has a 45-second watchdog that invokes `host.http.cancel` if the host operation does not finish (`src/lib.rs:228-251`).

**State Management:**
- Process state is held in synchronized statics: `OnceLock<Host>` for immutable host callbacks, a mutex-protected worker handle, `AtomicBool` stop flag, condition-variable wait pair, `Mutex<Value>` report, and active operation ID (`src/lib.rs:133-139`).
- Durable state is a JSON object under `%USERPROFILE%\.cli-proxy-api\codex-window-activation-state.json`, with an `accounts` object keyed by `auth_index`; only reset/attempt metadata is persisted (`src/lib.rs:99-103`, `src/lib.rs:294-301`).
- The worker owns its in-memory mutable `Value` state and writes it after each poll and before/after activation. Startup refuses malformed state instead of silently creating a new deduplication history (`src/lib.rs:404-412`).

## Key Abstractions

**`Window` and `Decision`:**
- Purpose: Represent supported quota windows and the safe action for the current account.
- Examples: `src/lib.rs:19-91`, `tests/policy.rs:4-51`.
- Pattern: Small data model plus pure functions over `serde_json::Value`; callers supply the current time and persisted account entry.

**`HostApi` / `PluginApi` / `Buffer`:**
- Purpose: Define the C-compatible ownership and callback contract between the DLL and CLIProxyAPI.
- Examples: `src/lib.rs:105-125`, `tests/dll_host.py:10-19`.
- Pattern: `repr(C)` structs, function pointers, explicit host-owned response freeing, and plugin-owned response freeing through `plugin_free`.

**`Host`:**
- Purpose: Store the validated host context and callbacks in a Rust-callable form after initialization.
- Examples: `src/lib.rs:127-175`.
- Pattern: Copyable callback table stored once in `OnceLock`; all host interactions go through `host`.

**JSON RPC envelope:**
- Purpose: Normalize plugin success and failure responses across the C boundary.
- Examples: `src/lib.rs:518-526`.
- Pattern: `{"ok":true,"result":...}` for successful dispatch and `{"ok":false,"error":...}` for handler errors or caught panics.

## Entry Points

**Native initialization:**
- Location: `src/lib.rs:467`
- Triggers: CLIProxyAPI loads the DLL and supplies ABI tables.
- Responsibilities: Validate non-null pointers and ABI v1, capture callbacks, and populate `PluginApi`.

**Plugin operation callback:**
- Location: `src/lib.rs:487`
- Triggers: CLIProxyAPI invokes a registered plugin RPC method.
- Responsibilities: Validate and parse input, invoke `handle`, catch Rust panics, serialize the response, and transfer buffer ownership.

**Lifecycle methods:**
- Location: `src/lib.rs:438-449`
- Triggers: `plugin.register`, `plugin.reconfigure`, `plugin.quiesce`, and `plugin.shutdown` RPC calls.
- Responsibilities: Start/restart or stop the polling worker and return JSON RPC results.

**Management endpoint:**
- Location: `src/lib.rs:450-458`
- Triggers: CLIProxyAPI registers the plugin route and later forwards `management.handle` requests.
- Responsibilities: Declare `GET /plugins/codex-window-activation/status` and return the current report body.

## Architectural Constraints

- **Threading:** The plugin uses one background worker thread per active lifecycle, guarded by `WORKER`; host callbacks and global report/operation state are synchronized with mutexes and atomics (`src/lib.rs:133-139`, `src/lib.rs:413-435`).
- **Global state:** Host callbacks are installed once in `HOST`; worker, stop, wake, report, and active operation state are process-wide statics in `src/lib.rs:133-139`.
- **Circular imports:** No module graph or circular import chain is present; all production Rust code is in `src/lib.rs` and imports only standard-library modules plus `serde_json` (`src/lib.rs:1-13`).
- **ABI stability:** `Buffer`, `HostApi`, and `PluginApi` use `#[repr(C)]`; the host must keep callbacks alive through shutdown (`src/lib.rs:105-125`, `src/lib.rs:464-467`).
- **Host-owned networking:** Auth and HTTP access must use `host.auth.*` and `host.http.*` callbacks so the core owns credentials, proxy policy, and cancellation (`src/lib.rs:216-260`, `src/lib.rs:303-328`).
- **Exact-account spending:** Activation uses the credentials and `account_id` returned for the current `auth_index`; no fallback account selection exists (`src/lib.rs:220-249`, `src/lib.rs:323-349`).
- **Durable deduplication:** The state file must remain available and valid; an invalid file prevents startup rather than resetting protection (`src/lib.rs:404-412`).

## Anti-Patterns

### Bypassing the Host Transport

**What happens:** A new feature opens its own network client or reads provider credential files directly instead of calling `host.auth.get` and `host.http.*` from `src/lib.rs:216-260` and `src/lib.rs:303-328`.
**Why it's wrong:** It bypasses CLIProxyAPI's global transport/proxy policy, host-managed cancellation, exact-account routing, and callback ownership contract.
**Do this instead:** Extend the host callback method flow in `src/lib.rs` and keep credentials in request scope; use `http` for operation open/do/cancel and `save` only for non-secret state.

### Persisting Authentication Material

**What happens:** A caller adds the `access_token` or complete auth JSON to the durable object passed to `save` in `src/lib.rs:294-301`.
**Why it's wrong:** The state file is intentionally restart-persistent and user-profile scoped; storing credentials expands exposure and violates the separation between host auth and plugin state.
**Do this instead:** Persist only account IDs, deadlines, attempt counts, retry timestamps, completion records, and status timestamps under `state["accounts"]` in `src/lib.rs:320-396`.

### Starting Work Outside Lifecycle Control

**What happens:** A new thread or recurring timer is launched from a method other than `start`, or shutdown returns without joining the worker in `src/lib.rs:401-435`.
**Why it's wrong:** Reconfigure can create duplicate pollers, and shutdown can race host callback teardown or leave an HTTP operation active.
**Do this instead:** Route recurring work through `start`/`stop`, use `STOP` plus `WAKE`, cancel `ACTIVE_OPERATION`, and join the worker before lifecycle methods return (`src/lib.rs:427-435`).

## Error Handling

**Strategy:** Internal functions return `Result<T, String>`; the worker converts poll failures into the synchronized status report, while the C ABI boundary converts handler errors and panics into JSON envelopes.

**Patterns:**
- Input and host-response shape checks fail closed with messages such as `Missing access token`, `Invalid usage response`, and `Missing HTTP status` (`src/lib.rs:166-174`, `src/lib.rs:220-270`).
- `host` reports only the upstream HTTP status in its error string and deliberately excludes upstream body text or credential material (`src/lib.rs:167-172`).
- Per-account failures are captured as an account status entry so another account can still be polled (`src/lib.rs:392-394`).
- Worker-level failures update `REPORT` with `running`, `error`, and `checked_at` rather than unwinding the worker (`src/lib.rs:415-418`).
- `plugin_call` wraps dispatch in `catch_unwind` and returns a generic panic envelope across the ABI (`src/lib.rs:500-526`).
- Persistence uses a temporary file followed by rename, and failure to read or validate startup state prevents activation (`src/lib.rs:294-301`, `src/lib.rs:404-412`).

## Cross-Cutting Concerns

**Logging:** No logging framework or direct console logging is present. Operational visibility is the JSON `REPORT` returned by `management.handle` (`src/lib.rs:138`, `src/lib.rs:395-398`, `src/lib.rs:450-458`).

**Validation:** Boundary validation checks C pointers, ABI version, UTF-8 method names, JSON input, callback response envelopes, quota window shape/ranges, HTTP status, and SSE `response.completed` completion (`src/lib.rs:467-516`, `src/lib.rs:166-174`, `src/lib.rs:32-56`, `src/lib.rs:254-291`).

**Authentication:** CLIProxyAPI supplies account credentials through `host.auth.get`; the plugin constructs bearer and `Chatgpt-Account-Id` headers only for the selected account in `http` (`src/lib.rs:220-249`). Management authentication is owned by the host around the registered route described in `README.md:37-46`.

**Resource ownership:** Host callback response buffers are copied and freed through the host's `free_buffer`; plugin response buffers are allocated with `Box` and released through `plugin_free` (`src/lib.rs:145-165`, `src/lib.rs:523-534`).

---

*Architecture analysis: 2026-10-09*
