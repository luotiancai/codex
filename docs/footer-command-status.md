# Shell task manager and live model activity

The activity row above the prompt reports the current observable phase:

```text
Thinking (12s · ↓ ~1234 tokens • esc to interrupt)
Running script (8s · ↓ 2310 tokens • esc to interrupt)
Responding (3s · ↓ ~2486 tokens • esc to interrupt)
Reconnecting... 2/5 (4s · ↓ 2310 tokens • esc to interrupt)
```

The timer measures the current phase, rather than the whole turn. Further text
deltas do not restart it. `Thinking` remains visible until an explicit model/tool
transition; a quiet reasoning stream no longer changes the label after five seconds.
Model output takes priority while background scripts remain visible in the footer.
Retry attempts, compaction, and approval-review status keep their existing labels.

The downward counter measures output for this turn, including reasoning where
reported. While model text arrives, `~` marks a byte-based token estimate using
Codex's existing heuristic. Reasoning summaries and raw reasoning are not added
twice. New usage reports replace the estimate with server-reported output tokens;
duplicate reports do not erase a later response's in-progress estimate.
Shell output, terminal polling, redraws, and history replay never increment tokens.
The number stays still when no new model data arrives. Hidden reasoning not sent
to the client cannot be counted live; `—` means no count is available yet. This
counter is not a live billing meter or a guarantee that the backend is computing.
After reconnecting without an opening usage baseline, it starts from the first
returned response rather than inventing earlier usage.

Long labels are shortened before the phase timer, tokens, and interrupt hint.
The activity row stays visible while model output streams, so token increments
do not disappear behind reply rendering. Timers continue with animations disabled.
Closing the task manager restores the
draft and does not interrupt the model.

## Task manager

Run `/tasks`, or press the configured activity shortcut (F4 by default) when shell
tasks exist. The same shortcut keeps its normal transcript behavior without tasks.
The list stays live while the agent continues working:

- Up/down selects a task; Enter opens its full command and captured output.
- `k` stops only the selected process. Other processes and the agent turn continue.
- The row shows `Stopping…` until the server confirms the result. A failed request
  leaves the process visible with an error and allows a retry.
- `r` refreshes process IDs from the server, including restored background jobs.
- In output, up/down and Page Up/Down scroll. End follows new output again.
- Escape returns to the list, then closes it. Ctrl+C closes the viewer.

The viewer retains the latest 64 KiB of output per task and twenty finished tasks;
running jobs are never evicted. Completed jobs keep their exit codes and output.
Restored tasks have no invented start time. Old list replies cannot resurrect a
known completed task. Output captured before this client joined may be unavailable.

`/ps` still prints a recent-output summary. `/stop` still stops all background
terminals; the new manager uses the server's targeted terminate endpoint instead.

## Command footer

This fork shows live shell execution at the start of the existing bottom status line:

```text
▶ 1 script running · /tasks · python3 test.py · 18s · GPT-6.1 Sol
▶ 2 scripts running · /tasks · npm test · 4s · GPT-6.1 Sol
▶ 1 background script · /tasks · python3 worker.py · 1m 05s · GPT-6.1 Sol
```

The command appears automatically with an existing custom `/statusline` selection.
When the process exits, the normal configured footer returns. Multiple processes show
the most recently observed command and the number of live commands. The entry point
comes before the command so narrow terminals retain it. The timer updates
once per second even when output and animations are disabled.

Commands are compacted to one line and truncated to fit a short footer segment.
Use `/ps` for background process output or the transcript for execution details.
Replayed history is not reported as live execution. Resumed background processes are
shown only after the server confirms they are still running; their unknown start time
is omitted. An explicitly disabled status line stays disabled.

This change does not change sandboxing, approvals, model requests, or session names.
Only an explicit stop action sends a process termination request.

## Build and test

Follow [Installing & building](install.md) to install the pinned Rust toolchain.
Then, from `codex-rs`:

```sh
cargo test -p codex-tui --lib footer_command_status
cargo test -p codex-tui --lib live_activity
cargo test -p codex-tui --lib script_tasks
cargo build -p codex-cli
```
