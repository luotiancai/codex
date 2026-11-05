use super::*;
use codex_app_server_protocol::ReasoningTextDeltaNotification;
use pretty_assertions::assert_eq;

fn row(chat: &mut ChatWidget) -> String {
    chat.pre_draw_tick();
    render_bottom_popup(chat, 120)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn live_activity_hidden_reasoning_updates_phase_without_exposing_text() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    chat.config.show_raw_agent_reasoning = false;
    chat.on_task_started();
    handle_agent_reasoning_started(&mut chat, "reasoning");
    chat.handle_server_notification(
        ServerNotification::ReasoningTextDelta(ReasoningTextDeltaNotification {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            item_id: "reasoning".into(),
            delta: "hidden content".into(),
            content_index: 0,
        }),
        None,
    );
    assert!(row(&mut chat).contains("Thinking"));
    assert!(chat.reasoning_buffer.is_empty());
    assert!(!render_bottom_popup(&chat, 120).contains("hidden content"));
}

#[tokio::test]
async fn live_activity_status_shows_command_then_waiting_and_preserves_retry() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    chat.on_task_started();
    let item = begin_unified_exec_startup(&mut chat, "script", "42", "python3 test.py");
    assert!(row(&mut chat).contains("Running python3 test.py"));
    end_exec(&mut chat, item, "done\n", "", 0);
    assert!(row(&mut chat).contains("Waiting for response"));
    handle_stream_error(&mut chat, "Reconnecting... 2/5", None);
    let retry = row(&mut chat);
    assert!(retry.contains("Reconnecting... 2/5"));
    assert!(retry.contains("no data"));
    handle_agent_reasoning_started(&mut chat, "reasoning");
    assert!(row(&mut chat).contains("Thinking"));
    chat.on_interrupted_turn(TurnAbortReason::Interrupted);
    chat.pre_draw_tick();
    assert!(!render_bottom_popup(&chat, 120).contains("no data"));
}

#[tokio::test]
async fn live_activity_projected_command_completion_clears_both_surfaces_without_new_data() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    chat.on_task_started();
    let item = begin_unified_exec_startup(&mut chat, "script", "42", "python3 test.py");
    assert!(row(&mut chat).contains("Running python3 test.py"));
    let last_data_at = chat.live_activity.last_data_at;
    chat.handle_command_execution_completed_now(item);
    assert!(row(&mut chat).contains("Waiting for response"));
    assert!(chat.footer_command_status.is_empty());
    assert_eq!(chat.live_activity.last_data_at, last_data_at);
}

#[tokio::test]
async fn live_activity_model_stream_is_visible_while_background_command_runs() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    chat.on_task_started();
    begin_unified_exec_startup(&mut chat, "script", "42", "python3 worker.py");
    handle_agent_reasoning_started(&mut chat, "thought");
    assert!(row(&mut chat).contains("Thinking"));
    assert!(
        chat.footer_command_status
            .summary(Instant::now(), false)
            .unwrap()
            .contains("worker.py")
    );
}

#[tokio::test]
async fn live_activity_silent_stream_restores_status_without_animations() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    chat.local_settings.tui.animations = false;
    chat.local_settings.tui.terminal_title = Some(Vec::new());
    chat.on_task_started();
    chat.on_agent_message_delta("Preamble line\n".into());
    chat.on_commit_tick();
    chat.bottom_pane.hide_status_indicator();
    chat.live_activity.last_data_at = Some(Instant::now() - Duration::from_secs(30));
    let visible = row(&mut chat);
    assert!(visible.contains("Waiting for response"));
    assert!(visible.contains("no data 30s"));
    insta::assert_snapshot!("live_activity_waiting_without_data", visible);
}

#[tokio::test]
async fn live_activity_replay_does_not_refresh_the_data_clock() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    chat.on_replayed_task_started(None);
    assert_eq!(chat.live_activity.last_data_at, None);
    let item = AppServerThreadItemPayload::Reasoning {
        id: "old".into(),
        summary: Vec::new(),
        content: Vec::new(),
    };
    chat.replay_thread_item(item, "turn-1".into(), ReplayKind::ThreadSnapshot);
    assert_eq!(chat.live_activity.last_data_at, None);
    let visible = row(&mut chat);
    assert!(visible.contains("Waiting for response"));
    assert!(visible.contains("no live data yet"));
}

#[tokio::test]
async fn live_activity_recovers_only_when_new_reasoning_data_arrives() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    chat.on_replayed_task_started(None);
    chat.on_reasoning_item_started("resumed".into());
    assert_eq!(chat.live_activity.last_data_at, None);
    chat.handle_server_notification(
        ServerNotification::ReasoningTextDelta(ReasoningTextDeltaNotification {
            thread_id: "thread-1".into(),
            turn_id: "turn-1".into(),
            item_id: "resumed".into(),
            delta: "new hidden data".into(),
            content_index: 0,
        }),
        None,
    );
    assert!(chat.live_activity.last_data_at.is_some());
    assert!(row(&mut chat).contains("Thinking"));
}
