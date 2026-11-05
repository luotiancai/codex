# Persistent command status

This fork shows live shell execution at the start of the existing bottom status line:

```text
▶ Running python3 test.py · 18s · GPT-6.1 Sol xhigh Fast · ~/project
▶ Running npm test · 4s · 2 commands · GPT-6.1 Sol xhigh Fast · ~/project
▶ Background python3 worker.py · 1m 05s · GPT-6.1 Sol xhigh Fast · ~/project
```

The command appears automatically with an existing custom `/statusline` selection.
When the process exits, the normal configured footer returns. Multiple processes show
the most recently observed command and the number of live commands. The timer updates
once per second even when output and animations are disabled.

Commands are compacted to one line and truncated to fit a short footer segment.
Use `/ps` for background process output or the transcript for execution details.
Replayed history is not reported as live execution. Resumed background processes are
shown only after the server confirms they are still running; their unknown start time
is omitted. An explicitly disabled status line stays disabled.

This change is presentation-only and does not change command execution, sandboxing,
approval rules, process cancellation, or session names.

## Build and test

Follow [Installing & building](install.md) to install the pinned Rust toolchain.
Then, from `codex-rs`:

```sh
cargo test -p codex-tui --lib footer_command_status
cargo build -p codex-cli
```
