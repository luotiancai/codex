//! Live command state for the persistent footer, independent of transcript grouping.
//!
//! Shell scripts classified as unknown commands still belong here. Replayed history
//! does not; restored background terminals are added only after a live process query.

use std::cell::Cell;
use std::time::Instant;

use crate::exec_command::split_command_string;
use crate::exec_command::strip_bash_lc_and_escape;
use crate::live_wrap::take_prefix_by_width;

#[derive(Default)]
pub(super) struct FooterCommandStatus {
    pub(super) tasks: crate::script_tasks::ScriptTasks,
    rendered_revision: Cell<u64>,
}

impl FooterCommandStatus {
    pub(super) fn start(&mut self, id: &str, command: &str, started_at: Option<Instant>) {
        self.tasks.start(id, command, started_at);
    }

    fn display(command: &str) -> String {
        let display = strip_bash_lc_and_escape(&split_command_string(command));
        let display = display.split_whitespace().collect::<Vec<_>>().join(" ");
        let display: String = display.chars().filter(|ch| !ch.is_control()).collect();
        let (mut display, rest, _) = take_prefix_by_width(&display, 48);
        if !rest.is_empty() {
            display.push('…');
        }
        display
    }

    #[cfg(test)]
    pub(super) fn finish(&mut self, id: &str) {
        self.tasks.finish(id, None, None);
    }

    pub(super) fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.tasks.retain_running(|id| keep(id));
    }

    pub(super) fn is_empty(&self) -> bool {
        !self
            .tasks
            .snapshot()
            .0
            .iter()
            .any(|task| task.status.is_running())
    }

    pub(super) fn needs_refresh(&self) -> bool {
        self.rendered_revision.get() != self.tasks.revision()
    }

    pub(super) fn summary(&self, now: Instant, background: bool) -> Option<String> {
        self.rendered_revision.set(self.tasks.revision());
        let commands: Vec<_> = self
            .tasks
            .snapshot()
            .0
            .into_iter()
            .filter(|task| task.status.is_running())
            .collect();
        let command = commands.first()?;
        let noun = if commands.len() == 1 {
            "script"
        } else {
            "scripts"
        };
        let label = if background {
            format!("background {noun}")
        } else {
            format!("{noun} running")
        };
        let mut text = format!(
            "▶ {} {label} · /tasks · {}",
            commands.len(),
            Self::display(&command.command)
        );
        if let Some(started_at) = command.started_at {
            let seconds = now.saturating_duration_since(started_at).as_secs();
            if seconds < 60 {
                text.push_str(&format!(" · {seconds}s"));
            } else {
                text.push_str(&format!(" · {}m {:02}s", seconds / 60, seconds % 60));
            }
        }
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::time::Duration;

    #[test]
    fn footer_command_status_tracks_parallel_commands_without_resetting_elapsed() {
        let now = Instant::now();
        let mut status = FooterCommandStatus::default();
        status.start("one", "bash -lc 'python3 test.py'", Some(now));
        status.start(
            "one",
            "bash -lc 'python3 test.py'",
            Some(now + Duration::from_secs(5)),
        );
        assert_eq!(
            status.summary(now + Duration::from_secs(12), false),
            Some("▶ 1 script running · /tasks · python3 test.py · 12s".into())
        );
        status.start("two", "npm test", Some(now + Duration::from_secs(10)));
        assert_eq!(
            status.summary(now + Duration::from_secs(12), false),
            Some("▶ 2 scripts running · /tasks · npm test · 2s".into())
        );
        status.finish("two");
        assert_eq!(
            status.summary(now + Duration::from_secs(65), true),
            Some("▶ 1 background script · /tasks · python3 test.py · 1m 05s".into())
        );
        status.finish("one");
        assert_eq!(status.summary(now, false), None);
    }

    #[test]
    fn footer_command_status_restored_process_has_no_invented_elapsed_time() {
        let mut status = FooterCommandStatus::default();
        status.start("restored", "sleep 100", None);
        assert_eq!(
            status.summary(Instant::now(), true),
            Some("▶ 1 background script · /tasks · sleep 100".into())
        );
    }

    #[test]
    fn footer_command_status_compacts_multiline_unicode_commands() {
        let mut status = FooterCommandStatus::default();
        status.start(
            "unicode",
            &format!("python3\n\t{}", "脚本".repeat(40)),
            None,
        );
        let summary = status.summary(Instant::now(), false).expect("live command");
        assert!(summary.starts_with("▶ 1 script running · /tasks · python3 "));
        assert!(summary.contains("脚本"));
        assert!(summary.ends_with('…'));
        assert!(!summary.chars().any(char::is_control));
        assert!(unicode_width::UnicodeWidthStr::width(summary.as_str()) <= 84);
    }
}
