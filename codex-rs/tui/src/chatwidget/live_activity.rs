//! Observable live activity, rather than an inference that a busy model is using tokens.
//!
//! Only live stream payloads advance the data clock. History replay, terminal polls,
//! account updates, and animation frames do not count as progress.

use std::time::Duration;
use std::time::Instant;

use codex_app_server_protocol::CommandExecutionSource;
use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadItemPayload;

const MODEL_ACTIVITY_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Default)]
enum ModelPhase {
    #[default]
    Waiting,
    Thinking,
    Responding,
}

#[derive(Default)]
pub(super) struct LiveActivity {
    phase: ModelPhase,
    model_data_at: Option<Instant>,
    pub(super) last_data_at: Option<Instant>,
    reasoning_item: Option<String>,
    tools: Vec<(String, String)>,
}

impl LiveActivity {
    pub(super) fn started(now: Instant) -> Self {
        Self {
            last_data_at: Some(now),
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
        self.tools.retain(|(tool_id, _)| tool_id != id);
    }

    fn model_data(&mut self, phase: ModelPhase, now: Instant) {
        self.phase = phase;
        self.model_data_at = Some(now);
        self.last_data_at = Some(now);
    }

    fn tool_started(&mut self, id: &str, label: String, now: Instant) {
        if !self.tools.iter().any(|(tool_id, _)| tool_id == id) {
            self.tools.push((id.to_owned(), label));
        }
        self.phase = ModelPhase::Waiting;
        self.model_data_at = None;
        self.last_data_at = Some(now);
    }

    pub(super) fn header(&self, now: Instant) -> String {
        if self
            .model_data_at
            .is_some_and(|at| now.saturating_duration_since(at) < MODEL_ACTIVITY_GRACE)
        {
            match self.phase {
                ModelPhase::Thinking => return "Thinking".into(),
                ModelPhase::Responding => return "Responding".into(),
                ModelPhase::Waiting => {}
            }
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
            }
            // This records activity even when raw reasoning text is hidden. No content is exposed.
            ServerNotification::ReasoningTextDelta(delta)
                if !delta.delta.is_empty()
                    && self.reasoning_item.as_deref() == Some(delta.item_id.as_str()) =>
            {
                self.model_data(ModelPhase::Thinking, now);
            }
            ServerNotification::AgentMessageDelta(delta) if !delta.delta.is_empty() => {
                self.model_data(ModelPhase::Responding, now);
            }
            ServerNotification::PlanDelta(delta) if !delta.delta.is_empty() => {
                self.model_data(ModelPhase::Responding, now);
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
                    let label = namespace.as_ref().map_or_else(
                        || format!("Calling {tool}"),
                        |namespace| format!("Calling {namespace}.{tool}"),
                    );
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
                if let ThreadItemPayload::Reasoning { id, .. } = item
                    && self.reasoning_item.as_deref() == Some(id.as_str())
                {
                    self.reasoning_item = None;
                    self.phase = ModelPhase::Waiting;
                    self.model_data_at = None;
                } else if self.tools.len() == before {
                    return false;
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
    fn live_activity_model_stream_expires_without_fabricated_progress() {
        let now = Instant::now();
        let mut activity = LiveActivity::started(now);
        assert_eq!(activity.header(now), "Waiting for response");
        activity.observe(&start(reasoning("reasoning")), now);
        assert_eq!(activity.header(now), "Thinking");
        activity.observe(&delta("reasoning"), now + Duration::from_secs(2));
        assert_eq!(activity.header(now + Duration::from_secs(4)), "Thinking");
        assert_eq!(
            activity.header(now + Duration::from_secs(8)),
            "Waiting for response"
        );
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
            "Calling functions.exec"
        );
        activity.observe(&start(tool("two")), now + Duration::from_secs(31));
        activity.observe(&start(reasoning("thought")), now + Duration::from_secs(31));
        assert_eq!(activity.header(now + Duration::from_secs(31)), "Thinking");
        activity.observe(&complete(tool("one")), now + Duration::from_secs(32));
        assert_eq!(
            activity.header(now + Duration::from_secs(60)),
            "Calling functions.exec"
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
        assert_eq!(
            activity.header(now + Duration::from_secs(10)),
            "Waiting for response"
        );
    }
}
