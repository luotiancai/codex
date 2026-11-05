//! Live shell task list and a scrollable output view. Closing preserves the draft
//! and the agent turn; stopping a task targets only its thread/process pair.

use std::cell::Cell;
use std::time::Duration;
use std::time::Instant;

use codex_protocol::ThreadId;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::keymap::ListAction;
use crate::keymap::ListKeymap;
use crate::render::renderable::Renderable;
use crate::script_tasks::ScriptTask;
use crate::script_tasks::ScriptTasks;
use crate::status_indicator_widget::fmt_elapsed_compact;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_lines;

pub(crate) struct ScriptTasksView {
    tasks: ScriptTasks,
    thread_id: Option<ThreadId>,
    tx: AppEventSender,
    keymap: ListKeymap,
    selected: Option<String>,
    detail: bool,
    follow: bool,
    scroll: Cell<usize>,
    page_size: Cell<usize>,
    complete: bool,
}

impl ScriptTasksView {
    pub(crate) fn new(
        tasks: ScriptTasks,
        thread_id: Option<ThreadId>,
        tx: AppEventSender,
        keymap: ListKeymap,
    ) -> Self {
        let selected = tasks.snapshot().0.first().map(|task| task.id.clone());
        Self {
            tasks,
            thread_id,
            tx,
            keymap,
            selected,
            detail: false,
            follow: true,
            scroll: Cell::new(0),
            page_size: Cell::new(10),
            complete: false,
        }
    }

    fn refresh(&self) {
        if let Some(thread_id) = self.thread_id
            && self.tasks.begin_refresh()
        {
            self.tx.send(AppEvent::RefreshScriptTasks {
                thread_id,
                tasks: self.tasks.clone(),
            });
        }
    }

    fn stop_selected(&self) {
        if let (Some(thread_id), Some(id)) = (self.thread_id, self.selected.as_deref())
            && let Some(process_id) = self.tasks.request_stop(id)
        {
            self.tx.send(AppEvent::StopScriptTask {
                thread_id,
                item_id: id.to_string(),
                process_id,
                tasks: self.tasks.clone(),
            });
        }
    }

    fn duration(task: &ScriptTask) -> String {
        task.started_at.map_or_else(
            || "resumed".into(),
            |at| {
                fmt_elapsed_compact(
                    task.finished_at
                        .unwrap_or_else(Instant::now)
                        .saturating_duration_since(at)
                        .as_secs(),
                )
            },
        )
    }

    fn command(task: &ScriptTask) -> String {
        crate::exec_command::strip_bash_lc_and_escape(&crate::exec_command::split_command_string(
            &task.command,
        ))
    }

    fn body(&self, width: u16) -> Vec<Line<'static>> {
        let (tasks, refreshing, error) = self.tasks.snapshot();
        if self.detail
            && let Some(task) = tasks
                .iter()
                .find(|task| Some(task.id.as_str()) == self.selected.as_deref())
        {
            let mut lines = vec![
                Line::from(
                    format!(
                        "{} · {}{}",
                        task.status.label(),
                        Self::duration(task),
                        task.exit_code
                            .map_or_else(String::new, |code| format!(" · exit {code}"))
                    )
                    .bold(),
                ),
                Line::default(),
            ];
            lines.extend(
                codex_ansi_escape::ansi_escape(&format!("$ {}", Self::command(task))).lines,
            );
            lines.push(Line::default());
            if task.truncated {
                lines.push(Line::from(
                    "Earlier output omitted; showing the latest 64 KiB.".dim(),
                ));
            }
            if task.output.is_empty() {
                lines.push(Line::from(
                    if task.status.is_running() {
                        "Waiting for output…"
                    } else {
                        "No output."
                    }
                    .dim(),
                ));
            } else {
                lines.extend(codex_ansi_escape::ansi_escape(task.output.as_str()).lines);
            }
            if let Some(error) = &task.error {
                lines.push(Line::from(error.clone().red()));
            }
            return word_wrap_lines(lines, RtOptions::new(usize::from(width.max(1))));
        }
        let mut lines = vec![
            Line::from(
                format!(
                    "{} running · {} finished",
                    tasks.iter().filter(|task| task.status.is_running()).count(),
                    tasks
                        .iter()
                        .filter(|task| !task.status.is_running())
                        .count()
                )
                .dim(),
            ),
            Line::default(),
        ];
        if tasks.is_empty() {
            lines.push(Line::from("No shell tasks in this session."));
        }
        for task in tasks {
            let selected = self.selected.as_deref() == Some(task.id.as_str());
            let command = Self::command(&task)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .filter(|ch| !ch.is_control())
                .collect::<String>();
            let line = Line::from(format!(
                "{} {} · {} · {}",
                if selected { "›" } else { " " },
                task.status.label(),
                Self::duration(&task),
                command
            ));
            lines.push(if selected { line.cyan().bold() } else { line });
            if selected && let Some(error) = task.error {
                lines.push(Line::from(format!("  {error}").red()));
            }
        }
        if refreshing {
            lines.push(Line::from("Refreshing…".dim()));
        }
        if let Some(error) = error {
            lines.push(Line::from(
                format!("Could not refresh tasks: {error}").red(),
            ));
        }
        lines
    }
}

impl BottomPaneView for ScriptTasksView {
    fn keymap_contexts(&self) -> crate::keymap::KeymapContextSet {
        crate::keymap::KeymapContextSet::new(crate::keymap::KeymapContext::List)
    }
    fn handle_key_event(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.code == KeyCode::Char('k')
            && key.modifiers.is_empty()
            && key.kind == KeyEventKind::Press
        {
            self.stop_selected();
            return;
        }
        if key.code == KeyCode::Char('r')
            && key.modifiers.is_empty()
            && key.kind == KeyEventKind::Press
        {
            self.refresh();
            return;
        }
        let action = self.keymap.action_for(key);
        if action == Some(ListAction::Cancel) || key.code == KeyCode::Esc {
            if self.detail {
                self.detail = false;
                self.scroll.set(0);
            } else {
                self.complete = true;
            }
            return;
        }
        if action == Some(ListAction::Accept) && key.kind == KeyEventKind::Press {
            self.detail = self.selected.is_some();
            self.follow = true;
            self.scroll.set(0);
            return;
        }
        if self.detail {
            match key.code {
                KeyCode::Home => {
                    self.follow = false;
                    self.scroll.set(0);
                }
                KeyCode::End => self.follow = true,
                KeyCode::PageUp => {
                    self.follow = false;
                    self.scroll
                        .set(self.scroll.get().saturating_sub(self.page_size.get()));
                }
                KeyCode::PageDown => {
                    self.follow = false;
                    self.scroll
                        .set(self.scroll.get().saturating_add(self.page_size.get()));
                }
                _ if action == Some(ListAction::MoveUp) => {
                    self.follow = false;
                    self.scroll.set(self.scroll.get().saturating_sub(1));
                }
                _ if action == Some(ListAction::MoveDown) => {
                    self.follow = false;
                    self.scroll.set(self.scroll.get().saturating_add(1));
                }
                _ => {}
            }
        } else if matches!(
            action,
            Some(
                ListAction::MoveUp
                    | ListAction::MoveDown
                    | ListAction::PageUp
                    | ListAction::PageDown
                    | ListAction::JumpTop
                    | ListAction::JumpBottom
            )
        ) {
            let tasks = self.tasks.snapshot().0;
            if !tasks.is_empty() {
                let index = tasks
                    .iter()
                    .position(|task| Some(task.id.as_str()) == self.selected.as_deref())
                    .unwrap_or(0);
                let index = match action {
                    Some(ListAction::MoveUp) => index.saturating_sub(1),
                    Some(ListAction::PageUp) => index.saturating_sub(self.page_size.get()),
                    Some(ListAction::PageDown) => index
                        .saturating_add(self.page_size.get())
                        .min(tasks.len() - 1),
                    Some(ListAction::JumpTop) => 0,
                    Some(ListAction::JumpBottom) => tasks.len() - 1,
                    _ => (index + 1).min(tasks.len() - 1),
                };
                self.selected = Some(tasks[index].id.clone());
            }
        }
    }
    fn prefer_esc_to_handle_key_event(&self) -> bool {
        true
    }
    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.complete = true;
        CancellationEvent::Handled
    }
    fn is_complete(&self) -> bool {
        self.complete
    }
    fn pre_draw_tick(&mut self, _now: Instant) -> bool {
        let tasks = self.tasks.snapshot().0;
        if !tasks
            .iter()
            .any(|task| Some(task.id.as_str()) == self.selected.as_deref())
        {
            let selected = tasks.first().map(|task| task.id.clone());
            let changed = self.selected != selected;
            self.selected = selected;
            self.detail = false;
            return changed;
        }
        false
    }
    fn next_frame_delay(&self) -> Option<Duration> {
        Some(Duration::from_secs(1))
    }
}

impl Renderable for ScriptTasksView {
    fn desired_height(&self, width: u16) -> u16 {
        self.body(width).len().saturating_add(4).clamp(7, 20) as u16
    }
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        Clear.render(area, buf);
        Paragraph::new(
            Line::from(if self.detail {
                "Task output"
            } else {
                "Background tasks"
            })
            .bold(),
        )
        .render(Rect::new(area.x, area.y, area.width, 1), buf);
        let body_area = Rect::new(
            area.x,
            area.y.saturating_add(2),
            area.width,
            area.height.saturating_sub(4),
        );
        let lines = self.body(area.width);
        self.page_size.set(usize::from(body_area.height).max(1));
        let max = lines.len().saturating_sub(usize::from(body_area.height));
        let offset = if self.detail && self.follow {
            max
        } else if self.detail {
            self.scroll.get().min(max)
        } else {
            let tasks = self.tasks.snapshot().0;
            let selected = tasks
                .iter()
                .position(|task| Some(task.id.as_str()) == self.selected.as_deref())
                .unwrap_or(0)
                + 2;
            selected
                .saturating_add(1)
                .saturating_sub(usize::from(body_area.height))
                .min(max)
        };
        self.scroll.set(offset);
        Paragraph::new(
            lines
                .into_iter()
                .skip(offset)
                .take(usize::from(body_area.height))
                .collect::<Vec<_>>(),
        )
        .render(body_area, buf);
        let hint = if self.detail {
            "↑/↓ scroll · end follow · k stop · esc back"
        } else {
            "↑/↓ select · enter output · k stop · r refresh · esc close"
        };
        Paragraph::new(Line::from(hint.dim())).render(
            Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1),
            buf,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::RuntimeKeymap;
    use crossterm::event::KeyModifiers;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn script_tasks_view_stops_only_the_selected_task_and_esc_keeps_agent_work_running() {
        let tasks = ScriptTasks::default();
        tasks.start("one", "python3 one.py", None);
        tasks.bind_process("one", Some("41"));
        tasks.start("two", "python3 two.py", None);
        tasks.bind_process("two", Some("42"));
        let (tx, mut events) = unbounded_channel();
        let thread_id = ThreadId::new();
        let mut view = ScriptTasksView::new(
            tasks.clone(),
            Some(thread_id),
            AppEventSender::new(tx),
            RuntimeKeymap::defaults().list,
        );
        view.handle_key_event(KeyCode::Down.into());
        view.handle_key_event(KeyCode::Enter.into());
        tasks.output("one", "new output without reopening\n");
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 12));
        view.render(buffer.area, &mut buffer);
        let visible = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(visible.contains("new output without reopening"));
        view.handle_key_event(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(events.try_recv().is_err());
        view.handle_key_event(KeyCode::Down.into());
        view.handle_key_event(KeyCode::Char('k').into());
        view.handle_key_event(KeyCode::Char('k').into());
        assert!(
            matches!(events.try_recv(), Ok(AppEvent::StopScriptTask { thread_id: id, item_id, process_id, .. }) if id == thread_id && item_id == "one" && process_id == "41")
        );
        assert!(events.try_recv().is_err());
        view.handle_key_event(KeyCode::Esc.into());
        assert!(!view.is_complete());
        view.handle_key_event(KeyCode::Esc.into());
        assert!(view.is_complete());
        assert!(events.try_recv().is_err());
        assert_eq!(
            tasks
                .snapshot()
                .0
                .iter()
                .find(|task| task.id == "two")
                .unwrap()
                .status,
            crate::script_tasks::ScriptTaskStatus::Running
        );
    }

    #[test]
    fn script_tasks_view_keeps_selection_when_new_tasks_arrive_and_output_scroll_is_bounded() {
        let tasks = ScriptTasks::default();
        tasks.start("one", "worker", None);
        tasks.output(
            "one",
            &(0..100).map(|i| format!("line {i}\n")).collect::<String>(),
        );
        let (tx, _events) = unbounded_channel();
        let mut view = ScriptTasksView::new(
            tasks.clone(),
            None,
            AppEventSender::new(tx),
            RuntimeKeymap::defaults().list,
        );
        tasks.start("two", "other worker", None);
        view.pre_draw_tick(Instant::now());
        assert_eq!(view.selected.as_deref(), Some("one"));
        assert!(
            !view.pre_draw_tick(Instant::now()),
            "stable views must not schedule a redraw loop"
        );
        view.handle_key_event(KeyCode::Enter.into());
        let mut buffer = Buffer::empty(Rect::new(0, 0, 60, 12));
        view.render(buffer.area, &mut buffer);
        let tail = view.scroll.get();
        assert!(tail > 10);
        view.handle_key_event(KeyCode::PageUp.into());
        view.render(buffer.area, &mut buffer);
        assert!(view.scroll.get() < tail);
        view.handle_key_event(KeyCode::End.into());
        view.render(buffer.area, &mut buffer);
        assert_eq!(view.scroll.get(), tail);
        for (width, height) in [(0, 0), (1, 1), (20, 3)] {
            let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));
            view.render(buffer.area, &mut buffer);
        }
    }
}
