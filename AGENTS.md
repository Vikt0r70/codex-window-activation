# Codex Window Activation

Windows Rust cdylib implementing CLIProxyAPI C ABI v1 / RPC schema 6.

## Checks

- `cargo test --offline`
- `cargo clippy --offline --all-targets -- -D warnings`
- `cargo build --release --offline`
- `python tests\dll_host.py` (requires the release build)
- For `quota-guard` changes, run tests/clippy with `--features quota-guard`,
  build with `--target-dir target/quota-guard`, then run
  `python tests\quota_guard_host.py` and the isolated core test when available.

`tests/live_probe.py` sends real inference and spends quota. Run it only with
explicit user authorization and an explicitly selected `CPA_LIVE_AUTH_FILE`.
Never commit credential files, tokens, live quota records, or real test output.

## Scope and constraints

- Keep activation pinned to one exact account; do not add fallback spending.
- Preserve disabled-account skips, persistent dedup, pre-request backoff,
  bounded attempts, cancelable HTTP, and safe DLL shutdown.
- Activation is not quota replenishment and not an 80% usage cap.
- The optional guard build is a separate plugin. Preserve native scheduler
  ownership, weekly-independent cutoff, explicit error veto, live-quota
  recovery, guard-owned pauses, and token preservation. Never enable both
  reset-activation implementations alongside Lamplighter.
- Do not change installed gateway configuration or deploy DLLs automatically.
- Verify host callback contracts against the matching CLIProxyAPI version.
- Future real-reset behavior is not proven merely by a simulated clock test.

GSD Core is installed project-locally in ignored `.opencode/`. `/gsd-help` shows
the workflows; `/gsd-onboard` can initialize planning for this existing codebase
when the user requests it. Installing GSD alone does not mean onboarding ran.
