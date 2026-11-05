//! Observable live activity, rather than an inference that a busy model is using tokens.
//!
//! Only live stream payloads advance the data clock. History replay, terminal polls,
//! account updates, and animation frames do not count as progress.

use std::collections::HashMap;
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

use codex_app_server_protocol::CommandExecutionSource;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadItemPayload;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ModelPhase {
    #[default]
    Waiting,
    Thinking,
    Responding,
}

#[derive(Default)]
pub(super) struct LiveActivity {
    phase: ModelPhase,
    phase_started_at: Option<Instant>,
    pub(super) last_data_at: Option<Instant>,
    reasoning_item: Option<String>,
    tools: Vec<(String, String)>,
    tool_started_at: HashMap<String, Instant>,
    streamed_bytes: HashMap<String, [usize; 3]>,
    usage_baseline: Option<i64>,
    reported_total: Option<i64>,
    confirmed_output: u64,
    has_confirmed_usage: bool,
}

impl LiveActivity {
    pub(super) fn started(now: Instant) -> Self {
        Self {
            last_data_at: Some(now),
            phase_started_at: Some(now),
            ..Self::default()
        }
    }

    pub(super) fn recover_reasoning_item(&mut self, id: Option<&str>) {
        if self.reasoning_item.is_none() {
            self.reasoning_item = id.map(str::to_owned);
        }
    }

    /// Projection/replay may finalize a command without a live completion notification.
    /// Clear its label without inventing a new stream timestamp.
    pub(super) fn finish_tool(&mut self, id: &str) {
        let before = self.tools.len();
        self.tools.retain(|(tool_id, _)| tool_id != id);
        self.tool_started_at.remove(id);
        if before != self.tools.len() && self.phase == ModelPhase::Waiting {
            self.phase_started_at = Some(Instant::now());
        }
    }

    pub(super) fn set_usage_baseline(&mut self, output_tokens: i64) {
        self.usage_baseline = Some(output_tokens.max(0));
        self.reported_total = self.usage_baseline;
    }

    pub(super) fn phase_started_at(&self) -> Option<Instant> {
        if self.phase == ModelPhase::Waiting
            && let Some((id, _)) = self.tools.last()
        {
            return self.tool_started_at.get(id).copied();
        }
        self.phase_started_at
    }

    pub(super) fn tokens(&self) -> Option<crate::status_indicator_widget::LiveTokenCount> {
        if self.streamed_bytes.is_empty() && !self.has_confirmed_usage {
            return None;
        }
        let bytes = self.streamed_bytes.values().fold(0usize, |total, counts| {
            total
                .saturating_add(counts[0])
                .saturating_add(counts[1].max(counts[2]))
        });
        Some(crate::status_indicator_widget::LiveTokenCount {
            count: self
                .confirmed_output
                .saturating_add(codex_utils_string::approx_tokens_from_byte_count(bytes)),
            estimated: bytes > 0 || !self.has_confirmed_usage,
        })
    }

    fn text_delta(&mut self, id: &str, kind: usize, text: &str) {
        let counts = self.streamed_bytes.entry(id.to_owned()).or_default();
        counts[kind] = counts[kind].saturating_add(text.len());
    }

    fn model_data(&mut self, phase: ModelPhase, now: Instant) {
        if self.phase != phase || self.phase_started_at.is_none() {
            self.phase_started_at = Some(now);
        }
        self.phase = phase;
        self.last_data_at = Some(now);
    }

    fn tool_started(&mut self, id: &str, label: String, now: Instant) {
        if !self.tools.iter().any(|(tool_id, _)| tool_id == id) {
            self.tools.push((id.to_owned(), label));
            self.tool_started_at.insert(id.to_owned(), now);
        }
        self.phase = ModelPhase::Waiting;
        self.phase_started_at = Some(now);
        self.last_data_at = Some(now);
    }

    pub(super) fn header(&self, _now: Instant) -> String {
        match self.phase {
            ModelPhase::Thinking => return "Thinking".into(),
            ModelPhase::Responding => return "Responding".into(),
            ModelPhase::Waiting => {}
        }
        if let Some((_, label)) = self.tools.last() {
            return label.clone();
        }
        "Waiting for response".into()
    }

    /// The caller filters replay and routes notifications to the owning thread.
    pub(super) fn observe(&mut self, notification: &ServerNotification, now: Instant) -> bool {
        match notification {
            ServerNotification::ReasoningSummaryTextDelta(delta)
                if !delta.delta.is_empty()
                    && self.reasoning_item.as_deref() == Some(delta.item_id.as_str()) =>
            {
                self.model_data(ModelPhase::Thinking, now);
                self.text_delta(&delta.item_id, 2, &delta.delta);
            }
            // This records activity even when raw reasoning text is hidden. No content is exposed.
            ServerNotification::ReasoningTextDelta(delta)
                if !delta.delta.is_empty()
                    && self.reasoning_item.as_deref() == Some(delta.item_id.as_str()) =>
            {
                self.model_data(ModelPhase::Thinking, now);
                self.text_delta(&delta.item_id, 1, &delta.delta);
            }
            ServerNotification::AgentMessageDelta(delta) if !delta.delta.is_empty() => {
                self.model_data(ModelPhase::Responding, now);
                self.text_delta(&delta.item_id, 0, &delta.delta);
            }
            ServerNotification::PlanDelta(delta) if !delta.delta.is_empty() => {
                self.model_data(ModelPhase::Responding, now);
                self.text_delta(&delta.turn_id, 0, &delta.delta);
            }
            ServerNotification::ThreadTokenUsageUpdated(update) => {
                let total = update.token_usage.total.output_tokens.max(0);
                if self.reported_total != Some(total) {
                    let baseline = *self.usage_baseline.get_or_insert_with(|| {
                        total.saturating_sub(update.token_usage.last.output_tokens.max(0))
                    });
                    self.confirmed_output = total.saturating_sub(baseline).max(0) as u64;
                    self.streamed_bytes.clear();
                    self.reported_total = Some(total);
                    self.has_confirmed_usage = true;
                }
            }
            ServerNotification::CommandExecutionOutputDelta(delta) if !delta.delta.is_empty() => {
                self.last_data_at = Some(now);
            }
            ServerNotification::FileChangeOutputDelta(delta) if !delta.delta.is_empty() => {
                self.last_data_at = Some(now);
            }
            ServerNotification::McpToolCallProgress(progress) if !progress.message.is_empty() => {
                self.last_data_at = Some(now);
            }
            ServerNotification::HookStarted(_) | ServerNotification::HookCompleted(_) => {
                self.last_data_at = Some(now);
            }
            ServerNotification::ItemStarted(notification) => match &notification.item.payload {
                ThreadItemPayload::Reasoning { id, .. } => {
                    self.reasoning_item = Some(id.clone());
                    self.model_data(ModelPhase::Thinking, now);
                }
                ThreadItemPayload::AgentMessage { .. } | ThreadItemPayload::Plan { .. } => {
                    self.model_data(ModelPhase::Responding, now);
                }
                ThreadItemPayload::CommandExecution { id, source, .. }
                    if *source != CommandExecutionSource::UnifiedExecInteraction =>
                {
                    self.tool_started(id, "Running command".into(), now);
                }
                ThreadItemPayload::McpToolCall {
                    id, server, tool, ..
                } => {
                    self.tool_started(id, format!("Calling {server}.{tool}"), now);
                }
                ThreadItemPayload::DynamicToolCall {
                    id,
                    namespace,
                    tool,
                    ..
                } => {
                    let label = if namespace.as_deref() == Some("functions") && tool == "exec" {
                        "Running tools".into()
                    } else {
                        format!("Using {tool}")
                    };
                    self.tool_started(id, label, now);
                }
                ThreadItemPayload::FileChange { id, .. } => {
                    self.tool_started(id, "Editing files".into(), now);
                }
                ThreadItemPayload::WebSearch(item) => {
                    self.tool_started(&item.id, "Searching the web".into(), now);
                }
                ThreadItemPayload::ImageGeneration(item) => {
                    self.tool_started(&item.id, "Generating image".into(), now);
                }
                ThreadItemPayload::CollabAgentToolCall { id, .. } => {
                    self.tool_started(id, "Waiting for agents".into(), now);
                }
                ThreadItemPayload::Sleep(item) => {
                    self.tool_started(&item.id, "Waiting for timer".into(), now);
                }
                _ => return false,
            },
            ServerNotification::ItemCompleted(notification) => {
                let item = &notification.item.payload;
                let before = self.tools.len();
                self.tools.retain(|(id, _)| id != item.id());
                self.tool_started_at.remove(item.id());
                if let ThreadItemPayload::Reasoning { id, .. } = item
                    && self.reasoning_item.as_deref() == Some(id.as_str())
                {
                    self.reasoning_item = None;
                    self.phase = ModelPhase::Waiting;
                    self.phase_started_at = Some(now);
                } else if matches!(
                    item,
                    ThreadItemPayload::AgentMessage { .. } | ThreadItemPayload::Plan { .. }
                ) && self.phase == ModelPhase::Responding
                {
                    self.phase = ModelPhase::Waiting;
                    self.phase_started_at = Some(now);
                } else if self.tools.len() == before {
                    return false;
                } else if self.phase == ModelPhase::Waiting {
                    self.phase_started_at = Some(now);
                }
                self.last_data_at = Some(now);
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::DynamicToolCallStatus;
    use codex_app_server_protocol::ItemCompletedNotification;
    use codex_app_server_protocol::ItemStartedNotification;
    use codex_app_server_protocol::ReasoningTextDeltaNotification;
    use codex_app_server_protocol::TerminalInteractionNotification;
    use pretty_assertions::assert_eq;

    fn usage(total: i64, last: i64) -> ServerNotification {
        let count = |n| codex_app_server_protocol::TokenUsageBreakdown {
            total_tokens: n,
            input_tokens: 0,
            cached_input_tokens: 0,
            cache_write_input_tokens: 0,
            output_tokens: n,
            reasoning_output_tokens: n / 2,
        };
        ServerNotification::ThreadTokenUsageUpdated(
            codex_app_server_protocol::ThreadTokenUsageUpdatedNotification {
                thread_id: "thread".into(),
                turn_id: "turn".into(),
                token_usage: codex_app_server_protocol::ThreadTokenUsage {
                    total: count(total),
                    last: count(last),
                    model_context_window: None,
                },
            },
        )
    }

    #[test]
    fn live_activity_tokens_reconcile_usage_without_counting_stdout_or_duplicate_reports() {
        let now = Instant::now();
        let mut activity = LiveActivity::started(now);
        activity.set_usage_baseline(50);
        activity.observe(&start(reasoning("r")), now);
        activity.observe(&delta("r"), now + Duration::from_secs(1));
        assert!(activity.tokens().unwrap().estimated);
        activity.observe(&usage(58, 8), now + Duration::from_secs(2));
        assert_eq!(activity.tokens().unwrap().count, 8);
        assert!(!activity.tokens().unwrap().estimated);
        let delta = ServerNotification::AgentMessageDelta(
            codex_app_server_protocol::AgentMessageDeltaNotification {
                thread_id: "thread".into(),
                turn_id: "turn".into(),
                item_id: "reply".into(),
                delta: "x".repeat(100),
            },
        );
        activity.observe(&delta, now + Duration::from_secs(3));
        let expected = activity.tokens().unwrap();
        assert_eq!(expected.count, 33);
        activity.observe(&usage(58, 8), now + Duration::from_secs(4));
        assert_eq!(activity.tokens().unwrap(), expected);
        let stdout = ServerNotification::CommandExecutionOutputDelta(
            codex_app_server_protocol::CommandExecutionOutputDeltaNotification {
                thread_id: "thread".into(),
                turn_id: "turn".into(),
                item_id: "script".into(),
                delta: "tool output".repeat(1000),
            },
        );
        activity.observe(&stdout, now + Duration::from_secs(5));
        assert_eq!(activity.tokens().unwrap(), expected);
        activity.observe(&usage(90, 32), now + Duration::from_secs(6));
        assert_eq!(activity.tokens().unwrap().count, 40);
        assert!(!activity.tokens().unwrap().estimated);
    }

    #[test]
    fn live_activity_tokens_do_not_double_count_summary_and_hidden_reasoning() {
        let now = Instant::now();
        let mut activity = LiveActivity::started(now);
        activity.observe(&start(reasoning("r")), now);
        let raw = |text: &str| {
            ServerNotification::ReasoningTextDelta(ReasoningTextDeltaNotification {
                thread_id: "thread".into(),
                turn_id: "turn".into(),
                item_id: "r".into(),
                delta: text.into(),
                content_index: 0,
            })
        };
        activity.observe(&raw("123"), now);
        activity.observe(&raw("45678"), now);
        activity.observe(
            &ServerNotification::ReasoningSummaryTextDelta(
                codex_app_server_protocol::ReasoningSummaryTextDeltaNotification {
                    thread_id: "thread".into(),
                    turn_id: "turn".into(),
                    item_id: "r".into(),
                    delta: "summary".into(),
                    summary_index: 0,
                },
            ),
            now,
        );
        assert_eq!(activity.tokens().unwrap().count, 2);
        assert_eq!(activity.phase_started_at(), Some(now));
        activity.observe(&raw(""), now + Duration::from_secs(100));
        assert_eq!(activity.tokens().unwrap().count, 2);
        assert_eq!(activity.phase_started_at(), Some(now));
        assert_eq!(activity.header(now + Duration::from_secs(100)), "Thinking");
    }

    fn reasoning(id: &str) -> ThreadItemPayload {
        ThreadItemPayload::Reasoning {
            id: id.into(),
            summary: Vec::new(),
            content: Vec::new(),
        }
    }

    fn tool(id: &str) -> ThreadItemPayload {
        ThreadItemPayload::DynamicToolCall {
            id: id.into(),
            namespace: Some("functions".into()),
            tool: "exec".into(),
            arguments: serde_json::Value::Null,
            status: DynamicToolCallStatus::InProgress,
            content_items: None,
            success: None,
            duration_ms: None,
        }
    }

    fn start(item: ThreadItemPayload) -> ServerNotification {
        ServerNotification::ItemStarted(ItemStartedNotification::new(
            "thread".into(),
            "turn".into(),
            item,
            0,
        ))
    }

    fn complete(item: ThreadItemPayload) -> ServerNotification {
        ServerNotification::ItemCompleted(ItemCompletedNotification::new(
            "thread".into(),
            "turn".into(),
            item,
            None,
            0,
        ))
    }

    fn delta(id: &str) -> ServerNotification {
        ServerNotification::ReasoningTextDelta(ReasoningTextDeltaNotification {
            thread_id: "thread".into(),
            turn_id: "turn".into(),
            item_id: id.into(),
            delta: "private text is never displayed here".into(),
            content_index: 0,
        })
    }

    #[test]
    fn live_activity_phase_clock_survives_silence_without_fabricated_tokens() {
        let now = Instant::now();
        let mut activity = LiveActivity::started(now);
        assert_eq!(activity.header(now), "Waiting for response");
        activity.observe(&start(reasoning("reasoning")), now);
        assert_eq!(activity.header(now), "Thinking");
        activity.observe(&delta("reasoning"), now + Duration::from_secs(2));
        assert_eq!(activity.header(now + Duration::from_secs(4)), "Thinking");
        assert_eq!(activity.header(now + Duration::from_secs(8)), "Thinking");
        let count = activity.tokens().unwrap().count;
        assert_eq!(activity.tokens().unwrap().count, count);
        assert_eq!(activity.phase_started_at(), Some(now));
        assert_eq!(activity.last_data_at, Some(now + Duration::from_secs(2)));
        activity.observe(&delta("reasoning"), now + Duration::from_secs(9));
        assert_eq!(activity.header(now + Duration::from_secs(9)), "Thinking");
        activity.observe(
            &complete(reasoning("reasoning")),
            now + Duration::from_secs(10),
        );
        assert_eq!(
            activity.header(now + Duration::from_secs(10)),
            "Waiting for response"
        );
    }

    #[test]
    fn live_activity_polling_does_not_reset_silence_or_hide_running_tools() {
        let now = Instant::now();
        let mut activity = LiveActivity::started(now);
        activity.observe(&start(tool("one")), now);
        let poll = ServerNotification::TerminalInteraction(TerminalInteractionNotification {
            thread_id: "thread".into(),
            turn_id: "turn".into(),
            item_id: "poll".into(),
            process_id: "42".into(),
            stdin: String::new(),
        });
        assert!(!activity.observe(&poll, now + Duration::from_secs(30)));
        assert_eq!(activity.last_data_at, Some(now));
        assert_eq!(
            activity.header(now + Duration::from_secs(30)),
            "Running tools"
        );
        activity.observe(&start(tool("two")), now + Duration::from_secs(31));
        activity.observe(&start(reasoning("thought")), now + Duration::from_secs(31));
        assert_eq!(activity.header(now + Duration::from_secs(31)), "Thinking");
        activity.observe(&complete(tool("one")), now + Duration::from_secs(32));
        activity.observe(
            &complete(reasoning("thought")),
            now + Duration::from_secs(33),
        );
        assert_eq!(
            activity.header(now + Duration::from_secs(60)),
            "Running tools"
        );
        activity.observe(&complete(tool("two")), now + Duration::from_secs(61));
        assert_eq!(
            activity.header(now + Duration::from_secs(61)),
            "Waiting for response"
        );
    }

    #[test]
    fn live_activity_late_reasoning_is_not_new_progress() {
        let now = Instant::now();
        let mut activity = LiveActivity::started(now);
        activity.observe(&start(reasoning("old")), now);
        activity.observe(&start(reasoning("new")), now + Duration::from_secs(1));
        assert!(!activity.observe(&delta("old"), now + Duration::from_secs(10)));
        assert!(!activity.observe(&complete(reasoning("old")), now + Duration::from_secs(10)));
        assert_eq!(activity.last_data_at, Some(now + Duration::from_secs(1)));
        assert_eq!(activity.header(now + Duration::from_secs(10)), "Thinking");
    }
}
