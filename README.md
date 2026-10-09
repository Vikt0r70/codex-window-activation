# Codex Window Activation

Local Windows native plugin for CLIProxyAPI v8.0.20/v8.0.21 ABI version 1,
RPC schema 6. Installed locally, not an official or published marketplace plugin.

## Behavior

- Runs inside CLIProxyAPI; no console windows, Task Scheduler, or separate resident script.
- Discovers enabled Codex OAuth accounts through `host.auth.list` and reads their
  current credentials through `host.auth.get`. Never saves tokens.
- Reads real 5-hour and weekly quota windows every 60 seconds, sequentially.
- Remembers reset timestamps. After a reset, waits at least 30 seconds, then sends
  one tiny `gpt-6.1-sol` request on that exact account, using its bearer token and
  `Chatgpt-Account-Id`. It cannot silently fall back to another account.
- Uses host HTTP callbacks and the core's global transport/proxy policy.
  Per-account proxy overrides are not supported by this plugin.
- Does not send an extra prompt if positive usage already confirms real traffic
  started the new window. Does not send before a known reset, or bootstrap an
  account whose reset time is unknown.
- Skips disabled accounts and accounts with another known window still exhausted.
- Requires a successful `response.completed` event; refreshes quota afterward.
- Persists completed reset records and backoff before sending requests. Failures
  retry after at least 15 minutes, at most three attempts per remembered reset.
- HTTP operations are canceled after 45 seconds, including during shutdown.
- A successful prompt starts ordinary provider usage; it cannot change OpenAI's
  weekly replenishment rules, add quota, or enforce an 80% usage ceiling.

It activates available accounts even when you are not otherwise using them. This
is intentional and spends a small amount of quota on every applicable idle reset.
Leave CLIProxyAPI running; it catches up on remembered resets when restarted.
Polling/network time means activation is not precisely at the reset second.

## Persistent state and status

State: `%USERPROFILE%\.cli-proxy-api\codex-window-activation-state.json`

Authenticated status endpoint:

```text
GET http://127.0.0.1:<your-core-port>/v0/management/plugins/codex-window-activation/status
Authorization: Bearer <your existing CLIProxyAPI management secret>
```

The endpoint is management-authenticated. State contains deadlines and attempt
records, not OAuth tokens. Do not delete the state file while the plugin runs;
deleting it loses deduplication. A damaged state file prevents activation rather
than silently resetting protection.

Enable/disable the plugin in EasyCLIProxyAPI's Plugins page. Its ID is
`codex-window-activation`; global Plugins must also be enabled. Disabled instances
stop their worker and cancel any active host HTTP operation.

## Build and checks

Requires Rust MSVC toolchain. `serde_json` is the only direct dependency.

```powershell
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
cargo build --release --offline
python tests\dll_host.py
```

`tests\dll_host.py` exercises the actual compiled DLL against a mock C-ABI host:
due windows, exact-account routing, disabled/exhausted skips, quota verification,
persistent dedup, restart/reconfigure, 429 backoff, three-attempt cap, and cleanup.

`python tests\live_probe.py` is **not** an ordinary unit test: it sends one real
tiny request on an explicitly selected account. Set `CPA_LIVE_AUTH_FILE` to that
account's existing OAuth JSON file before running it. It uses an isolated state
home and a simulated due deadline, then reads real quota again. It does not edit
production credentials or reset records. A verified run used 19 input + 5 output
tokens. Do not commit your credential file or test output.

## Installation

Build the DLL, then copy `target\release\codex_window_activation.dll` into your
CLIProxyAPI plugins directory as `codex-window-activation-v0.1.0.dll`.

Merge the following into your existing core configuration, using your actual
plugin directory. Keep other plugins and unrelated settings unchanged.

```yaml
plugins:
  enabled: true
  dir: "C:/path/to/your/plugins"
  configs:
    codex-window-activation:
      enabled: true
```

For EasyCLIProxyAPI, also enable its global Plugins switch. Verify the core's
plugin list reports `registered: true` and `effective_enabled: true`, then check
the authenticated status endpoint above. Do not assume that copying the DLL
alone activates it.

## Optional quota guard build

The `quota-guard` feature builds a **different plugin**, `codex-quota-guard`,
reusing only the ABI and cancelable HTTP implementation. It does not send reset
prompts or replace CPA's scheduler. Use native round-robin + session affinity for
session distribution and Lamplighter as the single reset-activation owner. Do not
enable the original reset-activation DLL alongside Lamplighter.

```powershell
cargo test --offline --features quota-guard
cargo clippy --offline --all-targets --features quota-guard -- -D warnings
cargo build --release --offline --features quota-guard --target-dir target/quota-guard
python tests\quota_guard_host.py
# Optional isolated real-core test, using an already installed executable:
$env:CPA_CORE_EXE = 'C:\path\to\cli-proxy-api.exe'
python tests\quota_guard_core.py
python tests\session_routing_core.py
# Optional: include your running Sleev in the local-only routing fixture:
$env:CPA_SLEEV_URL = 'http://127.0.0.1:17321/v1'
python tests\session_routing_core.py
```

Install `target/quota-guard/release/codex_window_activation.dll` as
`codex-quota-guard-v0.2.1.dll`; enable `plugins.configs.codex-quota-guard.enabled`.
For each protected **existing** Codex OAuth JSON, set
`"codex_quota_guard_limit": 80` for a 20% remaining five-hour reserve, or `90`
for a 10% remaining reserve. Add `"codex_quota_guard_weekly_limit": 90` to stop
at 10% weekly remaining. These fields are **percent used**, not percent remaining.
Each window is optional; omit both for unrestricted accounts. Never share those
credential files. All participating accounts should
have equal CPA priority for round-robin distribution of new sessions.

The guard reads configured live 18,000-second and 604,800-second windows every
30 seconds and at protected request boundaries. Either configured cutoff pauses
the account; unconfigured windows do not trigger the guard. It persists `disabled:true` plus
`codex_quota_guard_paused:true` at cutoff or quota-read failure, and updates the
core through `host.auth.save`. A selected protected request is explicitly vetoed
if quota cannot be verified or is already at the cutoff, including explicit
credential pins. Native CPA selects another eligible account after the pause;
the request that discovers the cutoff can return 429 and need a retry. If no
eligible accounts remain, the request fails rather than spending the reserve.

Only guard-owned pauses are lifted after successful live readings below **all
configured limits**. A five-hour reset cannot clear a weekly cutoff. A passed
clock deadline alone is insufficient. To manually disable an
already guard-paused account permanently, remove `codex_quota_guard_paused` while
leaving `disabled:true`. Unknown or malformed quota fails closed. Expired tokens
can therefore pause protected accounts until CPA refreshes credentials or you
re-authenticate. Per-account proxy overrides are unsupported and fail closed.
Keep the guard enabled: its absence/disabling removes request-time enforcement.

Authenticated management endpoints:
`GET /v0/management/plugins/codex-quota-guard/status` and
`POST /v0/management/plugins/codex-quota-guard/refresh`.
Status never contains OAuth tokens. HTTP cancellation is bounded to 45 seconds;
quota verification can add latency. Already-running requests and provider quota
reporting lag can overshoot the configured limit; this is **not** an exact ceiling.
Other gateways/direct account usage are outside its protection.

`tests/session_routing_core.py` runs the actual CPA executable with three fake
API-key credentials and a local mock provider, with the guard installed. It checks
new-session distribution, repeated-turn and parent/child affinity, isolated
failover, and empty-pool failure. It sends no real provider requests.

## Project-local GSD for OpenCode

GSD is development tooling, not a runtime dependency of the DLL. Install it from
the repository directory:

```powershell
npx -y @opengsd/gsd-core@1.16.0 --opencode --local
```

The generated `.opencode/` installation is ignored by Git because it contains
machine-specific paths. Run `/gsd-help` in OpenCode from this repository to see
the available workflow commands. This does not install GSD globally.

GSD Core 1.16.0 declares Node.js 24+ and npm 10+ requirements. If your normal
Node version is older, run the installer with a temporary npm-provided runtime:

```powershell
npx -y --package=node@24 --package=@opengsd/gsd-core@1.16.0 -- gsd-core --opencode --local --no-legacy-cleanup
```

This does not replace the machine's global Node installation. The
`--no-legacy-cleanup` flag avoids scanning/removing unrelated legacy GSD installs.

## Limits

Exactly-once external spending cannot be guaranteed across a crash between the
provider accepting a request and saving its completion. A saved 15-minute backoff
plus fresh quota readings reduces accidental repeats. Normal operation records
each completed reset and deduplicates simultaneous 5-hour/weekly activation.

Expired/revoked credentials, quota API changes, offline time, or a renamed model
can prevent activation. Status reports failures; the plugin never reauthenticates,
changes routing priorities, or re-enables disabled accounts.

Any guard implemented for another gateway is separate. It does not protect
inference sent directly by this CLIProxyAPI plugin.

## Source contracts

- [Native ABI examples](https://github.com/router-for-me/CLIProxyAPI/tree/v8.0.20/examples/plugin)
- [Host auth callbacks](https://github.com/router-for-me/CLIProxyAPI/blob/v8.0.20/internal/pluginhost/auth_callbacks.go)
- [Cancelable host HTTP](https://github.com/router-for-me/CLIProxyAPI/blob/v8.0.20/internal/pluginhost/host_callbacks.go)
- [Codex request contract](https://github.com/router-for-me/CLIProxyAPI/blob/v8.0.20/internal/runtime/executor/codex_executor_request.go)
