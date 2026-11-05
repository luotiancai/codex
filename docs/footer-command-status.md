# Observable activity and persistent command status

The activity row above the prompt reports the current observable phase:

```text
Thinking (total 1m 08s · no data 0s • esc to interrupt)
Running python3 test.py (total 1m 12s · no data 2s • esc to interrupt)
Waiting for response (total 18m 08s · no data 30s • esc to interrupt)
Reconnecting... 2/5 (total 18m 12s · no data 34s • esc to interrupt)
```

`Thinking` and `Responding` follow received model stream events, including activity
from raw reasoning whose text remains hidden. When no model data arrives for five
seconds and no tool is running, the row switches to `Waiting for response`.
Live tools keep their execution label until they finish; new model output takes
priority while background commands remain visible in the footer. Retry attempts, compaction,
and approval-review status retain their existing labels.

`no data` is the time since the last meaningful live model/tool payload. Animation
frames, empty terminal polls, account metadata, and history replay do not reset it.
It is not a server-side token meter and does not claim the backend is still computing
during a silent connection. Resumed history shows `no live data yet` until new live
events arrive. The clock updates with animations disabled, and a silent answer
stream restores the activity row instead of hiding the wait indefinitely.

Long labels are shortened before the silence clock and interrupt hint, so those
remain visible in a normal-width terminal. `total` is the existing turn timer;
the footer below reports the current command's own elapsed time.

## Command footer

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
cargo test -p codex-tui --lib live_activity
cargo build -p codex-cli
```
