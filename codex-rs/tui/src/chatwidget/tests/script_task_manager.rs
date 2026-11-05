use super::*;

#[tokio::test]
async fn script_tasks_manager_keeps_draft_and_turn_while_one_process_is_stopped() {
    let (mut chat, mut events, _ops) = make_chatwidget_manual(None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.local_settings.tui.animations = false;
    chat.on_task_started();
    chat.insert_str("Keep this draft");
    begin_unified_exec_startup(&mut chat, "one", "41", "python3 one.py");
    begin_unified_exec_startup(&mut chat, "two", "42", "python3 two.py");
    chat.handle_key_event(KeyCode::F(4).into());
    chat.pre_draw_tick();
    let list = render_bottom_popup(&chat, 90);
    assert!(list.contains("Background tasks"));
    assert!(list.contains("2 running"));
    assert!(list.contains("python3 one.py") && list.contains("python3 two.py"));
    insta::assert_snapshot!("script_tasks_list", list);
    chat.handle_key_event(KeyCode::Down.into());
    chat.handle_key_event(KeyCode::Enter.into());
    chat.on_exec_command_output_delta("one", "step 1\nstep 2\n");
    chat.pre_draw_tick();
    let output = render_bottom_popup(&chat, 90);
    assert!(output.contains("step 2"));
    insta::assert_snapshot!("script_tasks_output", output);
    chat.handle_key_event(KeyCode::Char('k').into());
    let messages = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
    assert!(messages.iter().any(|event| matches!(event, AppEvent::StopScriptTask { thread_id: id, item_id, process_id, .. } if *id == thread_id && item_id == "one" && process_id == "41")));
    assert!(!messages.iter().any(|event| matches!(
        event,
        AppEvent::CodexOp(AppCommand::Interrupt | AppCommand::CleanBackgroundTerminals)
    )));
    chat.footer_command_status.tasks.stopped("one", Ok(true));
    chat.pre_draw_tick();
    chat.handle_key_event(KeyCode::Esc.into());
    chat.handle_key_event(KeyCode::Esc.into());
    assert!(chat.is_agent_turn_running());
    assert_eq!(chat.bottom_pane.composer_text(), "Keep this draft");
    chat.pre_draw_tick();
    assert!(
        status_line_text(&chat)
            .unwrap()
            .starts_with("▶ 1 script running · /tasks · python3 two.py")
    );
    assert!(!chat.bottom_pane.has_active_view());
}

#[tokio::test]
async fn script_tasks_slash_command_is_available_during_work_even_with_footer_disabled() {
    let (mut chat, _events, _ops) = make_chatwidget_manual(None).await;
    chat.on_task_started();
    chat.local_settings.tui.status_line = Some(Vec::new());
    begin_unified_exec_startup(&mut chat, "one", "41", "sleep 10");
    chat.dispatch_command(SlashCommand::Tasks);
    chat.pre_draw_tick();
    assert!(render_bottom_popup(&chat, 80).contains("Background tasks"));
    assert!(chat.is_agent_turn_running());
    assert_eq!(status_line_text(&chat), None);
}
