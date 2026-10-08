# External Integrations

**Analysis Date:** 2026-10-09

## APIs & External Services

**OpenAI ChatGPT Wham Usage API:**
- Quota inspection service: `GET https://chatgpt.com/backend-api/wham/usage` (`src/lib.rs:266`)
  - Reads `primary_window` (18,000s / 5 hours) and `secondary_window` (604,800s / 7 days) quota limits, usage percentages, and reset deadlines.
  - SDK/Client: Direct HTTP via host callback `host.http.do` (`src/lib.rs:244`).
  - Auth: Dynamic Bearer token from CLIProxyAPI account storage (`host.auth.get`).

**OpenAI Codex Responses API:**
- Inference activation service: `POST https://chatgpt.com/backend-api/codex/responses` (`src/lib.rs:275`)
  - Submits minimal prompt (`"Say OK."` with instructions `"Reply with one word only."`, low reasoning effort, low verbosity, model `gpt-6.1-sol`, stream enabled, store disabled) to kickstart quota window after reset.
  - Expects Server-Sent Event `data: {"type":"response.completed",...}` (`src/lib.rs:284`).
  - Headers: `Authorization: Bearer <token>`, `Chatgpt-Account-Id: <account>`, `User-Agent: codex-tui/0.154.0...`, `Originator: codex-tui`, `Accept: text/event-stream`.
  - SDK/Client: Direct HTTP via host callback `host.http.do` (`src/lib.rs:244`).

**CLIProxyAPI Plugin Host RPC:**
- Host callbacks exposed via `HostApi` table (`src/lib.rs:114`):
  - `host.auth.list`: Enumerates configured provider accounts (`src/lib.rs:304`).
  - `host.auth.get`: Retrieves account OAuth tokens and ChatGPT account identifiers (`src/lib.rs:324`).
  - `host.http.operation_open`: Opens cancelable host HTTP operation (`src/lib.rs:228`).
  - `host.http.do`: Executes network request through CLIProxyAPI transport and proxy policy (`src/lib.rs:244`).
  - `host.http.cancel`: Aborts active HTTP operations on timeout (45s) or plugin quiesce/shutdown (`src/lib.rs:237`, `src/lib.rs:431`).

## Data Storage

**Databases:**
- None.

**File Storage:**
- Local filesystem only (`src/lib.rs:99-103`).
- State file: `%USERPROFILE%\.cli-proxy-api\codex-window-activation-state.json`.
- Atomic persistence pattern: Writes to `.tmp` file and replaces via `std::fs::rename` (`src/lib.rs:294-301`).

**Caching:**
- In-memory thread-safe state via static globals in `src/lib.rs`:
  - `HOST`: `OnceLock<Host>` stores host callback table and context pointer.
  - `REPORT`: `Mutex<Value>` stores latest account poll status and timestamps for management endpoint.
  - `ACTIVE_OPERATION`: `Mutex<Option<String>>` tracks currently in-flight host HTTP operation ID for immediate cancellation.

## Authentication & Identity

**Auth Provider:**
- Delegated to CLIProxyAPI host OAuth storage:
  - Account discovery filters on `provider == "codex"` or `type == "codex"`.
  - Reads `access_token` and `account_id` on each poll cycle without persisting tokens to disk.
  - Skips disabled accounts (`disabled == true`).

**Plugin Management API:**
- Management route: `GET /plugins/codex-window-activation/status` registered via `management.register` (`src/lib.rs:450`).
- Protected by CLIProxyAPI core management secret / bearer auth.

## Monitoring & Observability

**Error Tracking:**
- None (no external telemetry/Sentry; errors captured locally in `REPORT` mutex and logged via host HTTP failure statuses).

**Logs:**
- Status endpoint `GET /plugins/codex-window-activation/status` returns JSON report:
  - `running`: boolean worker state.
  - `checked_at`: Unix timestamp of last poll cycle.
  - `accounts`: Per-account array with status string and window data.
  - `state_file`: Absolute path to state JSON.
  - `poll_seconds`: Fixed polling interval (60 seconds).

## CI/CD & Deployment

**Hosting:**
- Runs embedded inside local CLIProxyAPI process on Windows hosts.

**CI Pipeline:**
- None in repository (local developer verification suite via Cargo and Python).

## Environment Configuration

**Required env vars:**
- `USERPROFILE`: Standard Windows user profile directory for state path resolution.
- `CPA_LIVE_AUTH_FILE`: Optional, strictly for manual live probe testing (`tests/live_probe.py`).

**Secrets location:**
- Stored exclusively in CLIProxyAPI host account storage (`host.auth.get`).
- No secrets or credentials are ever written to `codex-window-activation-state.json` (`tests/dll_host.py:83`).

## Webhooks & Callbacks

**Incoming:**
- Native C ABI callbacks from host to DLL:
  - `cliproxy_plugin_init` (`src/lib.rs:467`)
  - `plugin_call` (`src/lib.rs:487`) handling methods:
    - `plugin.register`, `plugin.reconfigure`: Starts worker thread (`src/lib.rs:440`).
    - `plugin.quiesce`, `plugin.shutdown`: Stops worker thread and cancels active HTTP operations (`src/lib.rs:446`).
    - `management.register`: Exposes management route (`src/lib.rs:450`).
    - `management.handle`: Returns JSON status response (`src/lib.rs:453`).
  - `plugin_free` (`src/lib.rs:528`): Frees memory allocated by plugin for host.
  - `plugin_shutdown` (`src/lib.rs:536`): Final cleanup callback.

**Outgoing:**
- None.

---

*Integration audit: 2026-10-09*
