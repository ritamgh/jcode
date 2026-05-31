# Project Context

Jcode is a Rust workspace for the Jcode coding agent, with the TUI/CLI as the default product focus in this checkout.

## Important paths
- `Cargo.toml`: workspace root and main `jcode` binary definition.
- `crates/jcode-base`: shared base functionality, including provider/auth/session utilities and the skill registry in `src/skill.rs`.
- `crates/jcode-app-core`: agent runtime, tools, background tasks, server/client protocol, and turn execution.
- `crates/jcode-tui`: terminal UI, input handling, local/remote session flow, and interleaved/queued message handling.
- `crates/jcode-desktop`: desktop app code. Do not default to this for TUI/root tasks.

## Agent orchestration gotchas
- The direct `subagent` tool lives in `crates/jcode-app-core/src/tool/task.rs`. It runs a child agent synchronously, but now bounds the wait with `timeout_secs` (default 300s) and returns a recoverable error containing the child session id if the child hangs. For long-running parallel work, prefer durable swarm sessions via `swarm spawn` / `swarm await_members`.

## Build and test
- Prefer the coordinated self-dev workflow for this repo: `selfdev build target=tui`, `selfdev test command="cargo test ..."`, then `selfdev reload` after successful TUI builds.
- Fallback local build: `scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode` if available, otherwise `cargo build --profile selfdev -p jcode --bin jcode`.
- `scripts/dev_cargo.sh` auto-selects fast Linux linkers only when the required linker tools are available; otherwise it should leave Cargo on the system linker.
- Targeted Rust tests use normal Cargo filters, for example `cargo test -p jcode-base skill_prompt_includes_jcode_ask_user_question_fallback`.

## Skill compatibility
- Skills are loaded from Claude/Codex/Jcode skill directories by `crates/jcode-base/src/skill.rs`.
- `Skill::get_prompt()` appends a Jcode compatibility section after the skill content.
- Jcode does not expose Claude Code native `AskUserQuestion`; the compatibility shim instructs the model to ask the same decision brief as a normal assistant message, accept later/interleaved user input as the answer, and wait at least 300 seconds before any skill-authorized default choice.
