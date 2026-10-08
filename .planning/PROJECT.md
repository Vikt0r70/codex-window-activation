# Codex Window Activation — Verification & Repair

## What This Is

A Windows Rust cdylib (`codex_window_activation.dll`) loaded by CLIProxyAPI v8.0.20/v8.0.21 that
implements the CLIProxyAPI Plugin C ABI v1 / RPC schema 6. It polls Codex accounts on a background
thread, reads their 5-hour/weekly quota windows, and activates (spends a real request on) accounts
whose window is due — with persistent dedup, backoff, and bounded attempts guarding every spend.

**This milestone is not a feature build.** It is a verification-and-repair pass: confirm the DLL
that exists is built as described and is actually good, then fix what is found broken. No new
capabilities.

## Core Value

Confidence: every claim the repo docs make about the DLL is true in the code, the code honors the
real CLIProxyAPI contract, and the AGENTS.md safety constraints hold — with confirmed defects fixed
without regressing any safety guarantee.

## Requirements

### Validated

<!-- Shipped and confirmed valuable. Inferred from existing code (see .planning/codebase/). -->

- ✓ Native ABI boundary — `cliproxy_plugin_init` / `plugin_call` / response-ownership matching CLIProxyAPI C ABI v1 — existing
- ✓ RPC method dispatch — lifecycle (`start`/`stop`/quiesce/shutdown) and management `REPORT` route — existing
- ✓ Sequential per-account polling — list accounts → exact credentials → quota read → decision (`Wait`/`Activate`/`Exhausted`) → activate → verify — existing
- ✓ Quota policy core — supported five-hour/weekly window parsing, deadline calc, reset grace, exhausted-window guard, retry guard — existing
- ✓ Persistent per-account state — deadlines, attempts, 15-min retry backoff, completed-dedup (capped 32), temp-file-then-rename, no credentials stored — existing
- ✓ Spending guards — disabled-account skip, remembered deadlines, positive-usage detection, pre-request backoff, completed-deadline dedup, three-attempt cap — existing
- ✓ Host-managed cancelable HTTP — operation open/cancel watchdog, exact-account headers, 2xx enforcement — existing
- ✓ Test harnesses — Rust unit/policy tests, Python `dll_host.py` ABI host double, gated `live_probe.py` — existing

### Active

<!-- Current scope. Building toward these. Verification then repair. -->

- [ ] Doc-claim verification — every behavioral claim in README.md and AGENTS.md is true in code, or the doc is corrected
- [ ] Contract verification — ABI/buffer/callback behavior matches the matching CLIProxyAPI version (v8.0.20/v8.0.21), checked against its source, not just assumptions
- [ ] Safety-constraint verification — all AGENTS.md constraints hold in code: exact-account pinning, no fallback spending, disabled-account skips, persistent dedup, pre-request backoff, bounded attempts, cancelable HTTP, safe DLL shutdown
- [ ] Known-bug triage — reproduce or refute each known bug from the codebase map (state-dir creation, corrupt-record dedup loss, stale `running` report after quiesce)
- [ ] Confirmed defects fixed — verified fixes, preserving every safety constraint; each fix lands with tests

### Out of Scope

<!-- Explicit boundaries. Includes reasoning to prevent re-adding. -->

- New routing strategies (session-sticky rotation etc.) — gateway-side concern; CLIProxyAPI already ships `routing.session-affinity` upstream; DLL does activation, not request routing
- Fallback spending across accounts — explicitly forbidden by AGENTS.md
- Real-reset behavior proof via simulated clock — a simulated-clock test passing does not prove real resets; would need live authorization
- Live probe runs (`tests/live_probe.py`) — spends real quota; only with explicit user authorization and a selected `CPA_LIVE_AUTH_FILE`
- Automatic deployment of DLLs or changes to installed gateway config — forbidden by AGENTS.md
- Typed-state refactor and other tech-debt rewrites from CONCERNS.md — only if verification proves a defect that requires it; debt alone does not trigger rework

## Context

- Single-file implementation: `src/lib.rs` holds the ABI boundary, dispatch, worker, policy, host
  adapter, HTTP ops, persistence, and reporting (`Cargo.toml` builds `cdylib` + `rlib`).
- Host dependency is pinned by behavior: CLIProxyAPI v8.0.20/v8.0.21 (see `.planning/codebase/STACK.md`).
- The codebase map (`.planning/codebase/CONCERNS.md`) already lists 3 known bugs and several
  security/tech-debt concerns — these are the primary verification targets, not the only ones.
- Routing-strategy question resolved during init: CLIProxyAPI supports `routing.session-affinity`
  (with `-ttl` and `-subagents`) combined with `strategy: round-robin` for per-session account
  stickiness; recognition of Codex `Session-Id` / `prompt_cache_key` confirmed via upstream source.
  This is config, not DLL work — recorded here so it is not re-litigated.
- Verification tools available offline: `cargo test --offline`, `cargo clippy --offline
  --all-targets -- -D warnings`, `cargo build --release --offline`, `python tests\dll_host.py`.

## Constraints

- **Safety**: Activation pinned to one exact account; no fallback spending; preserve disabled-account
  skips, persistent dedup, pre-request backoff, bounded attempts, cancelable HTTP, safe DLL shutdown — non-negotiable (AGENTS.md)
- **Verification**: Every code change is followed by `cargo test/clippy/build --offline` and
  `python tests\dll_host.py` (requires the release build)
- **Live testing**: `tests/live_probe.py` requires explicit user authorization and an explicitly
  selected `CPA_LIVE_AUTH_FILE`; never commit credentials, tokens, live quota records, or real test output
- **Compatibility**: Fix behavior against the matching CLIProxyAPI version; host callback contracts
  must be verified against its actual source
- **Minimal change**: This is a repair pass — minimum necessary change, no opportunistic refactors

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Verify-then-fix (report + fixes) | User wants working software, not just a verdict; fixes stay within safety constraints | — Pending |
| Dual source of truth: repo docs AND upstream CLIProxyAPI contract | Docs can drift from both the code and the host they talk to; both directions must be checked | — Pending |
| Routing strategy excluded from DLL scope | Session-affinity exists upstream in CLIProxyAPI config; DLL's job is activation, not request routing | — Pending |

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd-complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-10-09 after initialization*
