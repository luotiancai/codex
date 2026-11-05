use super::*;
use pretty_assertions::assert_eq;

fn use_custom_footer(chat: &mut ChatWidget) {
    chat.local_settings.tui.status_line = Some(vec!["model".into(), "thread-title".into()]);
    chat.local_settings.tui.animations = false;
    chat.local_settings.tui.terminal_title = Some(Vec::new());
    chat.refresh_status_surfaces();
}

#[tokio::test]
async fn footer_command_status_shows_unknown_scripts_in_existing_custom_footer() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    use_custom_footer(&mut chat);
    chat.on_task_started();
    let before = status_line_text(&chat);
    let item = begin_unified_exec_startup(&mut chat, "script", "42", "python3 test.py");
    chat.pre_draw_tick();
    let text = status_line_text(&chat).expect("custom footer");
    assert!(text.starts_with("▶ Running python3 test.py · 0s · "));

    let rendered = render_bottom_popup(&chat, 100);
    assert!(
        rendered
            .lines()
            .last()
            .expect("footer row")
            .contains("▶ Running python3 test.py")
    );
    insta::assert_snapshot!("footer_command_status_custom_footer", rendered);
    end_exec(&mut chat, item, "done\n", "", 0);
    assert_eq!(status_line_text(&chat), before);
}

#[tokio::test]
async fn footer_command_status_ticks_without_output_or_animations() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    use_custom_footer(&mut chat);
    chat.on_task_started();
    begin_unified_exec_startup(&mut chat, "script", "42", "sleep 100");
    chat.footer_command_status.finish("script");
    chat.footer_command_status.start(
        "script",
        "sleep 100",
        Some(Instant::now() - Duration::from_secs(12)),
    );
    let (frame_requester, mut scheduled) = FrameRequester::test_channel();
    chat.frame_requester = frame_requester;
    chat.pre_draw_tick();
    assert!(
        status_line_text(&chat)
            .expect("footer")
            .starts_with("▶ Running sleep 100 · 12s · ")
    );
    let now = Instant::now();
    assert!(std::iter::from_fn(|| scheduled.try_recv().ok()).any(|deadline| deadline > now));
}

#[tokio::test]
async fn footer_command_status_deduplicates_waits_and_counts_parallel_jobs() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    use_custom_footer(&mut chat);
    chat.on_task_started();
    let first = begin_unified_exec_startup(&mut chat, "first", "41", "sleep 100");
    let second = begin_unified_exec_startup(&mut chat, "second", "42", "python3 test.py");
    let wait = begin_exec_with_source(
        &mut chat,
        "wait",
        "poll output",
        ExecCommandSource::UnifiedExecInteraction,
    );
    assert!(
        status_line_text(&chat)
            .expect("footer")
            .starts_with("▶ Running python3 test.py · 0s · 2 commands · ")
    );
    end_exec(&mut chat, wait, "", "", 0);
    end_exec(&mut chat, second, "", "", 1);
    assert!(
        status_line_text(&chat)
            .expect("footer")
            .starts_with("▶ Running sleep 100 · 0s · ")
    );
    end_exec(&mut chat, first, "", "", 0);
    assert!(!status_line_text(&chat).expect("footer").contains('▶'));
}

#[tokio::test]
async fn footer_command_status_clears_interrupted_foreground_but_keeps_live_background() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    use_custom_footer(&mut chat);
    chat.on_task_started();
    begin_exec(&mut chat, "foreground", "sleep 5");
    let background = begin_unified_exec_startup(&mut chat, "background", "42", "sleep 100");
    chat.on_interrupted_turn(TurnAbortReason::Interrupted);
    let text = status_line_text(&chat).expect("footer");
    assert!(text.starts_with("▶ Background sleep 100 · 0s · "));
    assert!(!text.contains("2 commands"));
    end_exec(&mut chat, background, "", "", 0);
    assert!(!status_line_text(&chat).expect("footer").contains('▶'));
}

#[tokio::test]
async fn footer_command_status_replay_is_not_a_running_script() {
    let (mut parent, _events, _ops) = make_chatwidget_manual(None).await;
    let item = begin_unified_exec_startup(&mut parent, "old", "42", "sleep 100");
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    use_custom_footer(&mut chat);
    let before = status_line_text(&chat);
    chat.replay_thread_item(item, "turn-1".into(), ReplayKind::ResumeInitialMessages);
    chat.pre_draw_tick();
    assert_eq!(status_line_text(&chat), before);
}

#[tokio::test]
async fn footer_command_status_does_not_keep_frozen_snapshots_running() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    use_custom_footer(&mut chat);
    chat.on_task_started();
    begin_exec(&mut chat, "foreground", "sleep 100");
    chat.show_external_writer_thread();
    assert!(!status_line_text(&chat).expect("footer").contains('▶'));
}

#[tokio::test]
async fn footer_command_status_restores_only_confirmed_live_terminals() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    use_custom_footer(&mut chat);
    let request_id = chat.begin_background_terminal_restore();
    let live = codex_app_server_protocol::ThreadBackgroundTerminal {
        item_id: "live".into(),
        process_id: "42".into(),
        command: "sleep 100".into(),
        cwd: chat.config.cwd.clone().into(),
        os_pid: None,
        cpu_percent: None,
        rss_kb: None,
    };
    let exited = codex_app_server_protocol::ThreadBackgroundTerminal {
        item_id: "exited".into(),
        process_id: "43".into(),
        ..live.clone()
    };
    chat.track_unified_exec_process_end("exited", Some("43"));
    chat.restore_background_terminals(request_id, vec![live, exited]);
    assert!(
        status_line_text(&chat)
            .expect("footer")
            .starts_with("▶ Background sleep 100 · ")
    );
    chat.clear_background_terminals();
    assert!(!status_line_text(&chat).expect("footer").contains('▶'));
}

#[tokio::test]
async fn footer_command_status_respects_disabled_footer_and_deduplicates_run_state() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    chat.local_settings.tui.status_line = Some(Vec::new());
    chat.on_task_started();
    begin_unified_exec_startup(&mut chat, "script", "42", "sleep 100");
    chat.pre_draw_tick();
    assert_eq!(status_line_text(&chat), None);

    chat.local_settings.tui.status_line = Some(vec!["run-state".into(), "model".into()]);
    chat.refresh_status_surfaces();
    let text = status_line_text(&chat).expect("footer");
    assert_eq!(text.matches("▶ Running").count(), 1);
    assert!(!text.contains("Working"));
}
