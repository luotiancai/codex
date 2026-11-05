//! Live command state for the persistent footer, independent of transcript grouping.
//!
//! Shell scripts classified as unknown commands still belong here. Replayed history
//! does not; restored background terminals are added only after a live process query.

use std::time::Instant;

use crate::exec_command::split_command_string;
use crate::exec_command::strip_bash_lc_and_escape;
use crate::live_wrap::take_prefix_by_width;

#[derive(Default)]
pub(super) struct FooterCommandStatus {
    commands: Vec<Command>,
}

struct Command {
    id: String,
    display: String,
    started_at: Option<Instant>,
}

impl FooterCommandStatus {
    pub(super) fn start(&mut self, id: &str, command: &str, started_at: Option<Instant>) {
        if let Some(existing) = self.commands.iter_mut().find(|command| command.id == id) {
            existing.started_at = existing.started_at.or(started_at);
            return;
        }
        let display = strip_bash_lc_and_escape(&split_command_string(command));
        let display = display.split_whitespace().collect::<Vec<_>>().join(" ");
        let display: String = display.chars().filter(|ch| !ch.is_control()).collect();
        let (mut display, rest, _) = take_prefix_by_width(&display, 48);
        if !rest.is_empty() {
            display.push('…');
        }
        self.commands.push(Command {
            id: id.to_owned(),
            display,
            started_at,
        });
    }

    pub(super) fn finish(&mut self, id: &str) {
        self.retain(|command_id| command_id != id);
    }

    pub(super) fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.commands.retain(|command| keep(&command.id));
    }

    pub(super) fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub(super) fn running_header(&self) -> Option<String> {
        self.commands
            .last()
            .map(|command| format!("Running {}", command.display))
    }

    pub(super) fn summary(&self, now: Instant, background: bool) -> Option<String> {
        let command = self.commands.last()?;
        let label = if background { "Background" } else { "Running" };
        let mut text = format!("▶ {label} {}", command.display);
        if let Some(started_at) = command.started_at {
            let seconds = now.saturating_duration_since(started_at).as_secs();
            if seconds < 60 {
                text.push_str(&format!(" · {seconds}s"));
            } else {
                text.push_str(&format!(" · {}m {:02}s", seconds / 60, seconds % 60));
            }
        }
        if self.commands.len() > 1 {
            text.push_str(&format!(" · {} commands", self.commands.len()));
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
            Some("▶ Running python3 test.py · 12s".into())
        );
        status.start("two", "npm test", Some(now + Duration::from_secs(10)));
        assert_eq!(
            status.summary(now + Duration::from_secs(12), false),
            Some("▶ Running npm test · 2s · 2 commands".into())
        );
        status.finish("two");
        assert_eq!(
            status.summary(now + Duration::from_secs(65), true),
            Some("▶ Background python3 test.py · 1m 05s".into())
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
            Some("▶ Background sleep 100".into())
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
        assert!(summary.starts_with("▶ Running python3 "));
        assert!(summary.contains("脚本"));
        assert!(summary.ends_with('…'));
        assert!(!summary.chars().any(char::is_control));
        assert!(unicode_width::UnicodeWidthStr::width(summary.as_str()) <= 59);
    }
}
