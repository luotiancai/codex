//! Bounded, thread-local shell task history shared by the footer and task viewer.
//! Stop requests stay pending until the server confirms the targeted process result.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use codex_app_server_protocol::ThreadBackgroundTerminal;

const OUTPUT_LIMIT: usize = 64 * 1024;
const FINISHED_LIMIT: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScriptTaskStatus {
    Running,
    Stopping,
    Completed,
    Finished,
    Failed,
    Stopped,
}

impl ScriptTaskStatus {
    pub(crate) fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Stopping)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Stopping => "Stopping…",
            Self::Completed => "Completed",
            Self::Finished => "Finished",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ScriptTask {
    pub(crate) id: String,
    pub(crate) process_id: Option<String>,
    pub(crate) command: String,
    pub(crate) started_at: Option<Instant>,
    pub(crate) finished_at: Option<Instant>,
    pub(crate) status: ScriptTaskStatus,
    pub(crate) exit_code: Option<i32>,
    pub(crate) output: Arc<String>,
    pub(crate) truncated: bool,
    pub(crate) error: Option<String>,
}

#[derive(Default)]
struct State {
    tasks: Vec<ScriptTask>,
    revision: u64,
    refreshing: bool,
    error: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct ScriptTasks(Arc<Mutex<State>>);

// App events log their Debug representation; do not duplicate command output into it.
impl std::fmt::Debug for ScriptTasks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptTasks").finish_non_exhaustive()
    }
}

impl ScriptTasks {
    pub(crate) fn start(&self, id: &str, command: &str, started_at: Option<Instant>) {
        let mut state = self.0.lock().unwrap();
        if let Some(task) = state.tasks.iter_mut().find(|task| task.id == id) {
            task.started_at = task.started_at.or(started_at);
            return;
        }
        state.tasks.push(ScriptTask {
            id: id.to_string(),
            process_id: None,
            command: command.to_string(),
            started_at,
            finished_at: None,
            status: ScriptTaskStatus::Running,
            exit_code: None,
            output: Arc::default(),
            truncated: false,
            error: None,
        });
        state.revision += 1;
        Self::trim_finished(&mut state);
    }

    pub(crate) fn bind_process(&self, id: &str, process_id: Option<&str>) {
        let mut state = self.0.lock().unwrap();
        if let Some(task) = state.tasks.iter_mut().find(|task| task.id == id)
            && task.status.is_running()
            && let Some(process_id) = process_id
        {
            task.process_id = Some(process_id.to_string());
            state.revision += 1;
        }
    }

    pub(crate) fn output(&self, id: &str, delta: &str) {
        let mut state = self.0.lock().unwrap();
        if let Some(task) = state.tasks.iter_mut().find(|task| task.id == id) {
            Arc::make_mut(&mut task.output).push_str(delta);
            Self::trim_output(task);
            state.revision += 1;
        }
    }

    fn trim_output(task: &mut ScriptTask) {
        if task.output.len() > OUTPUT_LIMIT {
            let mut cut = task.output.len() - OUTPUT_LIMIT;
            while !task.output.is_char_boundary(cut) {
                cut += 1;
            }
            Arc::make_mut(&mut task.output).drain(..cut);
            task.truncated = true;
        }
    }

    pub(crate) fn finish(&self, id: &str, exit_code: Option<i32>, output: Option<&str>) {
        let mut state = self.0.lock().unwrap();
        if let Some(task) = state.tasks.iter_mut().find(|task| task.id == id) {
            if let Some(output) = output.filter(|output| !output.is_empty()) {
                task.output = Arc::new(output.to_string());
                Self::trim_output(task);
            }
            if task.status != ScriptTaskStatus::Stopped
                && (task.status.is_running() || exit_code.is_some())
            {
                task.status = match exit_code {
                    Some(0) => ScriptTaskStatus::Completed,
                    Some(_) => ScriptTaskStatus::Failed,
                    None => ScriptTaskStatus::Finished,
                };
            }
            task.exit_code = task.exit_code.or(exit_code);
            task.finished_at.get_or_insert_with(Instant::now);
            state.revision += 1;
            Self::trim_finished(&mut state);
        }
    }

    pub(crate) fn retain_running(&self, mut keep: impl FnMut(&str) -> bool) {
        let ids: Vec<_> = self
            .snapshot()
            .0
            .into_iter()
            .filter(|task| task.status.is_running() && !keep(&task.id))
            .map(|task| task.id)
            .collect();
        for id in ids {
            self.finish(&id, None, None);
        }
    }

    fn trim_finished(state: &mut State) {
        let mut finished = state
            .tasks
            .iter()
            .filter(|task| !task.status.is_running())
            .count();
        state.tasks.retain(|task| {
            if !task.status.is_running() && finished > FINISHED_LIMIT {
                finished -= 1;
                false
            } else {
                true
            }
        });
    }

    pub(crate) fn snapshot(&self) -> (Vec<ScriptTask>, bool, Option<String>) {
        let state = self.0.lock().unwrap();
        let mut tasks: Vec<_> = state.tasks.iter().rev().cloned().collect();
        tasks.sort_by_key(|task| !task.status.is_running());
        (tasks, state.refreshing, state.error.clone())
    }

    pub(crate) fn revision(&self) -> u64 {
        self.0.lock().unwrap().revision
    }

    pub(crate) fn begin_refresh(&self) -> bool {
        let mut state = self.0.lock().unwrap();
        if state.refreshing {
            return false;
        }
        state.refreshing = true;
        state.error = None;
        true
    }

    pub(crate) fn refreshed(&self, result: Result<Vec<ThreadBackgroundTerminal>, String>) {
        match result {
            Ok(terminals) => {
                for terminal in terminals {
                    // Existing completed IDs are never resurrected by an older list reply.
                    self.start(&terminal.item_id, &terminal.command, None);
                    self.bind_process(&terminal.item_id, Some(&terminal.process_id));
                }
            }
            Err(error) => self.0.lock().unwrap().error = Some(error),
        }
        self.0.lock().unwrap().refreshing = false;
    }

    pub(crate) fn request_stop(&self, id: &str) -> Option<String> {
        let mut state = self.0.lock().unwrap();
        let task = state.tasks.iter_mut().find(|task| task.id == id)?;
        if task.status != ScriptTaskStatus::Running {
            return None;
        }
        let Some(process_id) = task.process_id.clone() else {
            task.error = Some("Process ID not available yet. Press r to refresh.".into());
            return None;
        };
        task.status = ScriptTaskStatus::Stopping;
        task.error = None;
        state.revision += 1;
        Some(process_id)
    }

    pub(crate) fn stopped(&self, id: &str, result: Result<bool, String>) {
        let mut state = self.0.lock().unwrap();
        if let Some(task) = state.tasks.iter_mut().find(|task| task.id == id) {
            match result {
                Ok(stopped) => {
                    if stopped {
                        task.status = ScriptTaskStatus::Stopped;
                    } else if task.status.is_running() {
                        task.status = ScriptTaskStatus::Finished;
                    }
                    task.finished_at.get_or_insert_with(Instant::now);
                }
                Err(error) => {
                    if task.status == ScriptTaskStatus::Stopping {
                        task.status = ScriptTaskStatus::Running;
                    }
                    task.error = Some(format!("Could not stop task: {error}"));
                }
            }
            state.revision += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn script_tasks_stop_is_pending_targeted_and_retryable() {
        let tasks = ScriptTasks::default();
        let now = Instant::now();
        tasks.start("one", "python3 worker.py", Some(now));
        tasks.start("two", "npm test", Some(now));
        tasks.bind_process("one", Some("41"));
        tasks.bind_process("two", Some("42"));
        assert_eq!(tasks.request_stop("one"), Some("41".into()));
        assert_eq!(tasks.request_stop("one"), None);
        assert_eq!(
            tasks
                .snapshot()
                .0
                .iter()
                .filter(|task| task.status.is_running())
                .count(),
            2
        );
        tasks.stopped("one", Err("connection lost".into()));
        assert_eq!(tasks.request_stop("one"), Some("41".into()));
        tasks.stopped("one", Ok(true));
        tasks.finish("one", Some(137), Some("last line\n"));
        let (items, _, _) = tasks.snapshot();
        assert_eq!(items[0].id, "two");
        assert_eq!(items[0].status, ScriptTaskStatus::Running);
        assert_eq!(items[1].status, ScriptTaskStatus::Stopped);
        assert_eq!(items[1].exit_code, Some(137));
        assert_eq!(items[1].output.as_str(), "last line\n");
        tasks.start(
            "one",
            "older list reply",
            Some(now + Duration::from_secs(5)),
        );
        tasks.bind_process("one", Some("99"));
        assert_eq!(tasks.request_stop("one"), None);
        assert_eq!(tasks.snapshot().0[1].started_at, Some(now));
        assert_eq!(tasks.request_stop("two"), Some("42".into()));
        tasks.finish("two", Some(1), None);
        tasks.stopped("two", Ok(false));
        let task = tasks
            .snapshot()
            .0
            .into_iter()
            .find(|task| task.id == "two")
            .unwrap();
        assert_eq!(task.status, ScriptTaskStatus::Failed);
        assert_eq!(task.exit_code, Some(1));
    }

    #[test]
    fn script_tasks_bound_unicode_output_and_finished_history_without_dropping_live_jobs() {
        let tasks = ScriptTasks::default();
        tasks.start("live", "worker", None);
        tasks.output("live", &"日本語".repeat(OUTPUT_LIMIT));
        let task = tasks.snapshot().0.remove(0);
        assert!(task.output.len() <= OUTPUT_LIMIT);
        assert!(task.truncated);
        assert!(task.output.ends_with("日本語"));
        assert_eq!(tasks.request_stop("live"), None);
        for i in 0..FINISHED_LIMIT + 5 {
            let id = format!("done-{i}");
            tasks.start(&id, "short command", None);
            tasks.finish(&id, Some(0), None);
        }
        let (items, _, _) = tasks.snapshot();
        assert_eq!(items.len(), FINISHED_LIMIT + 1);
        assert_eq!(items[0].id, "live");
    }
}
