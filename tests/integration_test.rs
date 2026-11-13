use std::time::Duration;

use crossbeam_channel::unbounded;

use wnoobar::core::bus::AgentEvent;
use wnoobar::core::fsm::{EdgeDockMode, IslandState};
use wnoobar::ui::spring::SpringValue;

#[test]
fn test_spring_dynamics_convergence() {
    let mut spring = SpringValue::new(200.0);
    spring.set_target(380.0);

    let dt = 0.016; // 60 FPS
    let mut steps = 0;
    while spring.update(dt) && steps < 120 {
        steps += 1;
    }

    assert!((spring.current - 380.0).abs() < 0.2);
    assert_eq!(spring.velocity, 0.0);
}

#[test]
fn test_island_state_dimensions() {
    let idle = IslandState::Idle;
    assert_eq!(idle.target_dimensions(EdgeDockMode::Normal), (210.0, 32.0));
    assert_eq!(idle.target_dimensions(EdgeDockMode::Peeking), (140.0, 4.0));
    assert_eq!(idle.target_dimensions(EdgeDockMode::DeckExpanded), (440.0, 190.0));
    assert_eq!(idle.target_dimensions(EdgeDockMode::SwarmExpanded), (440.0, 190.0));

    let started = IslandState::Started {
        session_id: "test-s1".into(),
        project_name: "WNoobar".into(),
        role_type: "核心研发".into(),
        agent_name: "Antigravity".into(),
        task_summary: "开始处理".into(),
        started_at: std::time::Instant::now(),
        auto_hide_secs: 1.8,
    };
    assert_eq!(started.target_dimensions(EdgeDockMode::Normal), (400.0, 38.0));

    let working_short = IslandState::Working {
        session_id: "test-s1".into(),
        project_name: "WNoobar".into(),
        role_type: "助手".into(),
        agent_name: "Antigravity".into(),
        task_summary: "处理".into(),
        step_index: 1,
        start_time: std::time::Instant::now(),
    };
    assert_eq!(working_short.target_dimensions(EdgeDockMode::Normal), (400.0, 38.0));

    let working_long = IslandState::Working {
        session_id: "test-s1".into(),
        project_name: "WNoobar".into(),
        role_type: "远程管理ros-L2TP".into(),
        agent_name: "Antigravity".into(),
        task_summary: "收到任务 · 开始处理中，修改代码...".into(),
        step_index: 2,
        start_time: std::time::Instant::now(),
    };
    assert_eq!(working_long.target_dimensions(EdgeDockMode::Normal), (440.0, 38.0));

    let done_short = IslandState::Done {
        session_id: "test-s1".into(),
        project_name: "WNoobar".into(),
        role_type: "核心研发".into(),
        agent_name: "Antigravity".into(),
        summary: "构建成功".into(),
        duration_secs: 2,
        artifacts_count: 0,
        finished_at: std::time::Instant::now(),
        auto_collapse_secs: 3.5,
    };
    assert_eq!(done_short.target_dimensions(EdgeDockMode::Normal), (380.0, 58.0));

    let done_long = IslandState::Done {
        session_id: "test-s1".into(),
        project_name: "WNoobar".into(),
        role_type: "核心研发".into(),
        agent_name: "Antigravity".into(),
        summary: "这是一个非常长而详尽的交付结果总结描述，包含了大量业务细节".into(),
        duration_secs: 5,
        artifacts_count: 1,
        finished_at: std::time::Instant::now(),
        auto_collapse_secs: 3.5,
    };
    assert_eq!(done_long.target_dimensions(EdgeDockMode::Normal), (440.0, 72.0));

    let action_short = IslandState::ActionNeeded {
        session_id: "test-s1".into(),
        project_name: "WNoobar".into(),
        role_type: "核心研发".into(),
        agent_name: "Antigravity".into(),
        prompt: "是否同意执行？".into(),
    };
    assert_eq!(action_short.target_dimensions(EdgeDockMode::Normal), (380.0, 60.0));

    let action_long = IslandState::ActionNeeded {
        session_id: "test-s1".into(),
        project_name: "WNoobar".into(),
        role_type: "核心研发".into(),
        agent_name: "Antigravity".into(),
        prompt: "这是一个包含多个分支选项和详细说明的长问题提问描述".into(),
    };
    assert_eq!(action_long.target_dimensions(EdgeDockMode::Normal), (440.0, 76.0));
}

#[test]
fn test_event_bus_delivery() {
    let (tx, rx) = unbounded();
    tx.send(AgentEvent::TaskStarted {
        session_id: "test-s1".into(),
        project_name: "WNoobar".into(),
        role_type: "核心研发".into(),
        agent_name: "TestAgent".into(),
        task_summary: "启动处理".into(),
    }).unwrap();

    let event = rx.recv_timeout(Duration::from_millis(100)).unwrap();
    if let AgentEvent::TaskStarted { project_name, role_type, .. } = event {
        assert_eq!(project_name, "WNoobar");
        assert_eq!(role_type, "核心研发");
    } else {
        panic!("Unexpected event received");
    }
}

#[test]
fn test_codex_watcher_init() {
    let (tx, _rx) = unbounded();
    let watcher = wnoobar::core::watcher::CodexWatcher::new(tx);
    assert!(watcher.is_ok());
}

#[test]
fn test_multi_agent_watchers_init() {
    let (tx, _rx) = unbounded();

    // 验证 8 大 Watcher 的容错初始化
    let _ = wnoobar::core::watcher::ClaudeWatcher::new(tx.clone());
    let _ = wnoobar::core::watcher::CursorWatcher::new(tx.clone());
    let deepseek = wnoobar::core::watcher::DeepSeekWatcher::new(tx.clone());
    assert!(deepseek.is_ok());
    let opencode = wnoobar::core::watcher::OpenCodeWatcher::new(tx.clone());
    assert!(opencode.is_ok());
    let grok = wnoobar::core::watcher::GrokWatcher::new(tx.clone());
    assert!(grok.is_ok());
    let copilot = wnoobar::core::watcher::CopilotWatcher::new(tx);
    assert!(copilot.is_ok());
}

#[test]
fn test_odometer_carousel_lifecycle() {
    use wnoobar::ui::odometer::OdometerCarousel;

    let mut odo = OdometerCarousel::new();

    // 1. 单会话：不启动轮播
    assert!(!odo.update(0.1, 1));
    assert_eq!(odo.active_index, 0);

    // 2. 双会话：进入 3.2s 驻留
    odo.update(1.0, 2);
    assert!(!odo.is_rolling);
    odo.update(2.0, 2);
    assert!(!odo.is_rolling);

    // 3. 超过 3.2s：触发翻滚
    assert!(odo.update(0.3, 2));
    assert!(odo.is_rolling);
    assert_eq!(odo.active_index, 1);
    assert_eq!(odo.prev_index, 0);

    // 4. 翻滚半程：验证位移
    odo.update(0.3, 2); // 0.6s 的一半
    let (prev_y, curr_y) = odo.offsets(24.0);
    assert!(prev_y < 0.0); // 向上滚出
    assert!(curr_y > 0.0); // 向上滚入

    // 5. 翻滚结束：进入落地微弹
    odo.update(0.4, 2);
    assert!(!odo.is_rolling);
    assert!(odo.landing_bounce_time > 0.0);
    let scale = odo.landing_scale();
    assert!(scale >= 1.0 && scale <= 1.045);

    // 6. 微弹消退：回到驻留
    odo.update(0.3, 2);
    assert_eq!(odo.landing_scale(), 1.0);
    assert_eq!(odo.active_index, 1);
}

#[test]
fn test_app_config_and_shared_noise_filter_integration() {
    use std::fs;
    use wnoobar::core::bus::NoiseFilterLevel;
    use wnoobar::core::config::{AppConfig, MonitorMode, SharedNoiseFilter};
    use wnoobar::core::watcher::AntigravityWatcher;

    let temp_dir = std::env::temp_dir().join(format!(
        "wnoobar_integration_cfg_{}",
        std::process::id()
    ));
    let cfg_path = temp_dir.join("WNoobar").join("config.json");
    let _ = fs::remove_dir_all(&temp_dir);

    // 缺失配置文件应安全回退默认值
    let fallback = AppConfig::load_from(&cfg_path);
    assert_eq!(fallback, AppConfig::default());

    let custom = AppConfig {
        noise_level: NoiseFilterLevel::Silent,
        auto_start: false,
        sound_enabled: true,
        dnd_fullscreen: false,
        monitor_mode: MonitorMode::FollowCursor,
        start_pop_secs: 2.2,
        done_pop_secs: 4.5,
    };
    custom.save_to(&cfg_path).expect("save custom config");

    let loaded = AppConfig::load_from(&cfg_path);
    assert_eq!(loaded, custom);

    // 验证 AppConfig -> SharedNoiseFilter -> AntigravityWatcher 联动
    let shared_filter = SharedNoiseFilter::new(loaded.noise_level);
    assert_eq!(shared_filter.get(), NoiseFilterLevel::Silent);

    let (tx, _rx) = unbounded();
    let mut watcher = AntigravityWatcher::new(tx).expect("AntigravityWatcher init");
    watcher.set_shared_filter(shared_filter.clone());

    shared_filter.set(NoiseFilterLevel::Verbose);
    assert_eq!(shared_filter.get(), NoiseFilterLevel::Verbose);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_focus_score_window_candidate_integration() {
    use wnoobar::win32::focus::score_window_candidate;

    let cursor_exact = score_window_candidate(
        "window.rs - WNoobar - Cursor",
        "Cursor.exe",
        "WNoobar",
        "Cursor",
        "Cursor Composer",
    );
    let cursor_other = score_window_candidate(
        "other-app - Cursor",
        "cursor.exe",
        "WNoobar",
        "Cursor",
        "Cursor Composer",
    );
    let copilot_exact = score_window_candidate(
        "main.ts - billing-api - Visual Studio Code",
        "Code.exe",
        "billing-api",
        "GitHub Copilot",
        "Copilot Chat",
    );
    let claude_terminal = score_window_candidate(
        "Claude Code - agent-hub",
        "WindowsTerminal.exe",
        "agent-hub",
        "Claude",
        "Claude Code",
    );
    let grok_terminal = score_window_candidate(
        "Grok - crawler",
        "pwsh.exe",
        "crawler",
        "Grok",
        "Grok",
    );

    assert!(cursor_exact > cursor_other && cursor_other > 0);
    assert!(copilot_exact > 100);
    assert!(claude_terminal > 100);
    assert!(grok_terminal > 100);
    assert!(score_window_candidate("WNoobar", "wnoobar.exe", "WNoobar", "Antigravity", "") <= 0);
    assert!(score_window_candidate("billing-api - Chrome", "chrome.exe", "billing-api", "Cursor", "") <= 0);
}

#[test]
fn test_system_monitor_placement_and_fullscreen_rect_integration() {
    use wnoobar::win32::system::{rect_covers_monitor, MonitorPlacement};

    let primary_mon = (0, 0, 1920, 1080);
    assert!(rect_covers_monitor((0, 0, 1920, 1080), primary_mon));
    assert!(rect_covers_monitor((-8, -8, 1928, 1088), primary_mon));
    assert!(!rect_covers_monitor((0, 0, 1920, 1032), primary_mon));

    let left_mon = (-2560, 0, 0, 1440);
    assert!(rect_covers_monitor((-2560, 0, 0, 1440), left_mon));
    assert!(!rect_covers_monitor((0, 0, 1920, 1080), left_mon));

    let primary_place = MonitorPlacement {
        origin_x: 0,
        origin_y: 0,
        monitor_w: 1920,
        dpi_scale: 1.0,
    };
    assert_eq!(primary_place.centered_window_x(480), 720);

    let secondary_hidpi = MonitorPlacement {
        origin_x: 1920,
        origin_y: 0,
        monitor_w: 2560,
        dpi_scale: 1.5,
    };
    assert_eq!(secondary_hidpi.centered_window_x(720), 1920 + 920);
}

#[test]
fn test_capsule_renderer_hit_geometry_integration() {
    use wnoobar::ui::capsule::CapsuleRenderer;
    use wnoobar::win32::GdiplusContext;

    let _gdip = GdiplusContext::init().expect("GdiplusContext init");
    let renderer = CapsuleRenderer::new();
    assert_eq!(renderer.hovered_deck_idx, None);
    assert!(!renderer.is_action_hovered);
    assert!(!renderer.is_silent_mode);
    assert!(!renderer.is_dnd_active);

    // DeckExpanded: 440x190 centered in 480 canvas => cap_x = 20, cap_y = 4
    let (deck_w, deck_h) = IslandState::Idle.target_dimensions(EdgeDockMode::DeckExpanded);
    let deck_x = (480.0 - deck_w) / 2.0;
    let deck_y = 4.0;
    assert!(CapsuleRenderer::is_left_glyph_hit(deck_x, deck_y, deck_h, deck_x + 18.0, deck_y + 18.0));
    assert_eq!(CapsuleRenderer::deck_card_index_at(deck_x, deck_y, deck_w, 240.0, deck_y + 50.0, 3), Some(0));
    assert_eq!(CapsuleRenderer::deck_card_index_at(deck_x, deck_y, deck_w, 240.0, deck_y + 100.0, 3), Some(1));
    assert_eq!(CapsuleRenderer::deck_card_index_at(deck_x, deck_y, deck_w, 240.0, deck_y + 150.0, 3), Some(2));
    // Gap 0-1 lies in (86.0, 92.0), i.e., deck_y + 85.0 = 89.0
    assert_eq!(CapsuleRenderer::deck_card_index_at(deck_x, deck_y, deck_w, 240.0, deck_y + 85.0, 3), None);

    // ActionNeeded: 380x60 centered in 480 canvas => cap_x = 50, cap_y = 4
    let (act_w, act_h) = (380.0, 60.0);
    let act_x = (480.0 - act_w) / 2.0;
    let act_y = 4.0;
    assert!(!CapsuleRenderer::is_action_jump_hit(act_x, act_y, act_w, act_h, act_x + 18.0, act_y + 18.0));
    assert!(CapsuleRenderer::is_action_jump_hit(act_x, act_y, act_w, act_h, act_x + 320.0, act_y + 20.0));
    assert!(!CapsuleRenderer::is_action_jump_hit(act_x, act_y, act_w, act_h, act_x - 5.0, act_y + 20.0));

    // Peeking 状态不可触发左侧微标与 ActionNeeded 跳转
    assert!(!CapsuleRenderer::is_left_glyph_hit(170.0, 0.0, 4.0, 180.0, 2.0));
    assert!(!CapsuleRenderer::is_action_jump_hit(170.0, 0.0, 140.0, 4.0, 240.0, 2.0));
}

#[test]
fn test_tray_menu_rows_hit_test_and_popup_origin_integration() {
    use std::collections::HashSet;
    use wnoobar::core::bus::NoiseFilterLevel;
    use wnoobar::core::config::MonitorMode;
    use wnoobar::ui::tray_menu::{
        build_main_rows, build_submenu_rows, hit_test_row, measure_panel_size, row_top_y,
        MenuCommand, MenuRowKind, SubmenuKind, TrayMenuSnapshot, CARD_PAD_Y, MAIN_CARD_W,
        SHADOW_PAD, SUBMENU_CARD_W,
    };
    use wnoobar::win32::tray_popup::{
        compute_main_menu_origin, compute_submenu_origin, menu_command_id,
    };

    let snap = TrayMenuSnapshot {
        is_expanded: false,
        active_sessions_count: 3,
        noise_level: NoiseFilterLevel::Normal,
        monitor_mode: MonitorMode::Primary,
        dnd_fullscreen: true,
        sound_enabled: false,
        auto_start: true,
    };

    let main_rows = build_main_rows(&snap);
    assert_eq!(main_rows.len(), 12);
    let (main_cw, main_ch) = measure_panel_size(&main_rows, MAIN_CARD_W);
    assert!((main_cw - (MAIN_CARD_W + SHADOW_PAD * 2.0)).abs() < f32::EPSILON);
    assert!(main_ch > 300.0);

    // 主菜单命中测试：ToggleIsland、分隔线跳过、Submenu(Noise/Monitor)、Exit
    let hit_x = SHADOW_PAD + 24.0;
    assert_eq!(
        hit_test_row(&main_rows, MAIN_CARD_W, hit_x, row_top_y(&main_rows, 0) + 12.0),
        Some(0)
    );
    assert_eq!(
        hit_test_row(&main_rows, MAIN_CARD_W, hit_x, row_top_y(&main_rows, 2) + 4.0),
        None
    );
    let noise_hit = hit_test_row(&main_rows, MAIN_CARD_W, hit_x, row_top_y(&main_rows, 3) + 16.0);
    assert_eq!(noise_hit, Some(3));
    match &main_rows[3].kind {
        MenuRowKind::Item {
            command, submenu, ..
        } => {
            assert_eq!(*command, None);
            assert_eq!(*submenu, Some(SubmenuKind::Noise));
        }
        _ => panic!("row 3 must be Noise submenu"),
    }
    assert_eq!(
        hit_test_row(&main_rows, MAIN_CARD_W, hit_x, row_top_y(&main_rows, 4) + 16.0),
        Some(4)
    );
    assert_eq!(
        hit_test_row(&main_rows, MAIN_CARD_W, hit_x, row_top_y(&main_rows, 11) + 16.0),
        Some(11)
    );

    // 子菜单行构建 + 命中 + 命令映射唯一性
    let noise_rows = build_submenu_rows(SubmenuKind::Noise, &snap);
    assert_eq!(noise_rows.len(), 3);
    for (idx, expected_lvl) in [
        NoiseFilterLevel::Silent,
        NoiseFilterLevel::Normal,
        NoiseFilterLevel::Verbose,
    ]
    .iter()
    .enumerate()
    {
        let hit = hit_test_row(
            &noise_rows,
            SUBMENU_CARD_W,
            SHADOW_PAD + 20.0,
            row_top_y(&noise_rows, idx) + 16.0,
        );
        assert_eq!(hit, Some(idx));
        match &noise_rows[idx].kind {
            MenuRowKind::Item {
                command, checked, ..
            } => {
                assert_eq!(*command, Some(MenuCommand::SetNoise(*expected_lvl)));
                assert_eq!(*checked, *expected_lvl == NoiseFilterLevel::Normal);
            }
            _ => panic!("expected noise submenu item"),
        }
    }

    let monitor_rows = build_submenu_rows(SubmenuKind::Monitor, &snap);
    assert_eq!(monitor_rows.len(), 2);
    assert_eq!(
        hit_test_row(
            &monitor_rows,
            SUBMENU_CARD_W,
            SHADOW_PAD + 20.0,
            row_top_y(&monitor_rows, 1) + 16.0
        ),
        Some(1)
    );

    let mut unique_ids = HashSet::new();
    for row in main_rows
        .iter()
        .chain(noise_rows.iter())
        .chain(monitor_rows.iter())
    {
        if let MenuRowKind::Item {
            command: Some(cmd), ..
        } = row.kind
        {
            assert!(unique_ids.insert(menu_command_id(cmd)));
        }
    }
    assert_eq!(unique_ids.len(), 12);

    // 边界定位：150% DPI 底部任务栏 + 顶部任务栏 + 负坐标副屏 + 子菜单右侧/左侧翻转
    let dpi = 1.5_f32;
    let main_phys_w = (main_cw * dpi).round() as i32;
    let main_phys_h = (main_ch * dpi).round() as i32;
    let pad_px = (SHADOW_PAD * dpi).round() as i32;
    let card_pad_y_px = (CARD_PAD_Y * dpi).round() as i32;

    let (sub_cw, sub_ch) = measure_panel_size(&noise_rows, SUBMENU_CARD_W);
    let sub_phys_w = (sub_cw * dpi).round() as i32;
    let sub_phys_h = (sub_ch * dpi).round() as i32;
    let row_top_px = (row_top_y(&main_rows, 3) * dpi).round() as i32;

    // 底部任务栏托盘右下角弹出
    let bottom_work = (0, 0, 2560, 1380);
    let (mx, my) = compute_main_menu_origin(2490, 1410, main_phys_w, main_phys_h, bottom_work, pad_px);
    assert_eq!(mx, bottom_work.2 - main_phys_w);
    assert_eq!(my, bottom_work.3 - main_phys_h);

    // 主菜单贴右边缘 -> 子菜单自动向左侧翻转展开且保留 4px 卡片间隙
    let (sx_flip, sy_flip) = compute_submenu_origin(
        mx,
        my,
        main_phys_w,
        row_top_px,
        card_pad_y_px,
        sub_phys_w,
        sub_phys_h,
        bottom_work,
        pad_px,
    );
    assert_eq!((mx + pad_px) - (sx_flip + sub_phys_w - pad_px), 4);
    assert!(sx_flip >= bottom_work.0 && sx_flip + sub_phys_w <= bottom_work.2);
    assert!(sy_flip >= bottom_work.1 && sy_flip + sub_phys_h <= bottom_work.3);

    // 顶部任务栏 + 负坐标副屏：主菜单向下弹出，子菜单向右展开并精确对齐父行
    let top_left_work = (-2560, 60, 0, 1440);
    let (mx2, my2) =
        compute_main_menu_origin(-2000, 25, main_phys_w, main_phys_h, top_left_work, pad_px);
    assert_eq!(my2, top_left_work.1);
    let (sx_right, sy_right) = compute_submenu_origin(
        mx2,
        my2,
        main_phys_w,
        row_top_px,
        card_pad_y_px,
        sub_phys_w,
        sub_phys_h,
        top_left_work,
        pad_px,
    );
    assert_eq!((sx_right + pad_px) - (mx2 + main_phys_w - pad_px), 4);
    assert_eq!(sy_right + pad_px + card_pad_y_px, my2 + row_top_px);
}

#[test]
fn test_swarm_win32_event_flow_aggregation_hover_and_deferred_done_integration() {
    use std::time::Instant;
    use wnoobar::core::bus::{EventBus, NoiseFilterLevel};
    use wnoobar::core::config::{AppConfig, SharedNoiseFilter};
    use wnoobar::core::swarm::SwarmWorkerStatus;
    use wnoobar::win32::system::CueKind;
    use wnoobar::win32::window::{CapsuleClickAction, IslandRuntime};

    // 1. 验证各状态下的 EdgeDockMode::SwarmExpanded 尺寸严格为 (440.0, 190.0)
    let working_sample = IslandState::Working {
        session_id: "p-1".into(),
        project_name: "WNoobar".into(),
        role_type: "Swarm".into(),
        agent_name: "Antigravity".into(),
        task_summary: "2/4 运行中".into(),
        step_index: 5,
        start_time: Instant::now(),
    };
    assert_eq!(
        working_sample.target_dimensions(EdgeDockMode::SwarmExpanded),
        (440.0, 190.0)
    );

    // 2. 通过真实 EventBus 推送父任务 + 4 个 SubagentStatus 事件
    let bus = EventBus::new();
    let tx = bus.sender();
    let rx = bus.receiver();

    let mut runtime = IslandRuntime::new(
        AppConfig::default(),
        SharedNoiseFilter::new(NoiseFilterLevel::Normal),
    );
    // 预置两个非 Swarm 项目，确认不会被 4 个子代理挤出 Session Deck
    runtime.recent_sessions.push((
        "unrelated-1".into(),
        "OtherRepoA".into(),
        "Agent".into(),
        "待命".into(),
        "Cursor".into(),
    ));
    runtime.recent_sessions.push((
        "unrelated-2".into(),
        "OtherRepoB".into(),
        "Agent".into(),
        "完成".into(),
        "Codex".into(),
    ));

    tx.send(AgentEvent::TaskStarted {
        session_id: "parent-swarm-1".into(),
        project_name: "WNoobar".into(),
        role_type: "Coordinator".into(),
        agent_name: "Antigravity".into(),
        task_summary: "启动 Swarm 协作".into(),
    })
    .unwrap();

    for (idx, worker_name) in ["@core", "@ui", "@win32", "@qa"].iter().enumerate() {
        tx.send(AgentEvent::SubagentStatus {
            group_key: "parent-swarm-1".into(),
            parent_session_id: Some("parent-swarm-1".into()),
            worker_id: format!("worker-{}", idx + 1),
            worker_name: (*worker_name).to_string(),
            project_name: "WNoobar".into(),
            agent_name: "Antigravity".into(),
            status: SwarmWorkerStatus::Working,
            task_summary: format!("执行分片 {}", idx + 1),
            step_index: (idx as u64) + 10,
        })
        .unwrap();
    }

    let t0 = Instant::now();
    while let Ok(ev) = rx.try_recv() {
        runtime.handle_event_at(ev, t0);
    }

    // 池长必须严格为 1 且挂有 4 Worker 的 SwarmSummary；recent_sessions 未被 4 个子代理污染
    assert_eq!(runtime.active_sessions_pool.len(), 1);
    assert_eq!(runtime.active_sessions_pool[0].session_id, "parent-swarm-1");
    let swarm_snap = runtime.active_sessions_pool[0]
        .swarm
        .as_ref()
        .expect("representative active item must hold SwarmSummary");
    assert_eq!(swarm_snap.total_workers, 4);
    assert_eq!(swarm_snap.working_count, 4);
    assert_eq!(runtime.recent_sessions.len(), 3);
    assert!(runtime
        .recent_sessions
        .iter()
        .any(|(id, ..)| id == "unrelated-1"));
    assert!(runtime
        .recent_sessions
        .iter()
        .any(|(id, ..)| id == "unrelated-2"));

    // 3. 悬停 260ms 自动展开 SwarmExpanded、期间暂停 Odometer、点击面板直跳主窗、移出 120ms 收起
    runtime.set_dock_mode(EdgeDockMode::Normal);
    assert!(runtime.should_update_odometer());
    let (cap_x, cap_y, cap_w, cap_h) = (40.0, 4.0, 400.0, 38.0);
    runtime.update_hover_state_at(
        cap_x,
        cap_y,
        cap_w,
        cap_h,
        240.0,
        20.0,
        480.0,
        t0 + Duration::from_millis(10),
    );
    assert_eq!(runtime.dock_mode, EdgeDockMode::Normal);

    runtime.update_hover_state_at(
        cap_x,
        cap_y,
        cap_w,
        cap_h,
        240.0,
        20.0,
        480.0,
        t0 + Duration::from_millis(280),
    );
    assert_eq!(runtime.dock_mode, EdgeDockMode::SwarmExpanded);
    assert!(!runtime.should_update_odometer());
    assert_eq!(runtime.width_spring.target, 440.0);
    assert_eq!(runtime.height_spring.target, 190.0);
    assert_eq!(
        runtime.resolve_capsule_click(20.0, 4.0, 440.0, 190.0, 300.0, 90.0),
        CapsuleClickAction::FocusSession("parent-swarm-1".into())
    );

    // 移出 SwarmExpanded 保持盒超过 120ms 收缩回 Peeking
    runtime.update_hover_state_at(
        20.0,
        4.0,
        440.0,
        190.0,
        5.0,
        250.0,
        480.0,
        t0 + Duration::from_millis(300),
    );
    runtime.update_hover_state_at(
        20.0,
        4.0,
        440.0,
        190.0,
        5.0,
        250.0,
        480.0,
        t0 + Duration::from_millis(430),
    );
    assert_eq!(runtime.dock_mode, EdgeDockMode::Peeking);
    assert!(runtime.swarm_hover_since.is_none());

    // 4. 父会话过早 Done 被延迟抑制；逐一完结前 3 个仍抑制；第 4 个完结后统一收官 Done
    tx.send(AgentEvent::Done {
        session_id: "parent-swarm-1".into(),
        project_name: "WNoobar".into(),
        role_type: "Coordinator".into(),
        agent_name: "Antigravity".into(),
        summary: "主会话等待".into(),
        duration_secs: 8,
        artifacts_count: 1,
    })
    .unwrap();
    let eff_early = runtime.handle_event_at(
        rx.recv_timeout(Duration::from_millis(100)).unwrap(),
        t0 + Duration::from_millis(500),
    );
    assert_eq!(eff_early.sound_cue, None);
    assert!(matches!(runtime.current_state, IslandState::Working { .. }));
    assert_eq!(runtime.active_sessions_pool.len(), 1);

    // Done 保护未触发时，先完结前 3 个 worker
    for idx in 1..=3 {
        tx.send(AgentEvent::SubagentStatus {
            group_key: "parent-swarm-1".into(),
            parent_session_id: Some("parent-swarm-1".into()),
            worker_id: format!("worker-{}", idx),
            worker_name: format!("@w{}", idx),
            project_name: "WNoobar".into(),
            agent_name: "Antigravity".into(),
            status: SwarmWorkerStatus::Done,
            task_summary: "完成".into(),
            step_index: 20,
        })
        .unwrap();
        let eff = runtime.handle_event_at(
            rx.recv_timeout(Duration::from_millis(100)).unwrap(),
            t0 + Duration::from_millis(600 + idx as u64 * 10),
        );
        assert_eq!(eff.sound_cue, None);
        assert!(matches!(runtime.current_state, IslandState::Working { .. }));
        assert_eq!(runtime.active_sessions_pool.len(), 1);
    }

    // 第 4 个 worker 收官 -> 立即弹出统一 Done ("Swarm 4/4 已收官")，并清空池中占位
    tx.send(AgentEvent::SubagentStatus {
        group_key: "parent-swarm-1".into(),
        parent_session_id: Some("parent-swarm-1".into()),
        worker_id: "worker-4".into(),
        worker_name: "@qa".into(),
        project_name: "WNoobar".into(),
        agent_name: "Antigravity".into(),
        status: SwarmWorkerStatus::Done,
        task_summary: "全部验证通过".into(),
        step_index: 25,
    })
    .unwrap();
    let eff_final = runtime.handle_event_at(
        rx.recv_timeout(Duration::from_millis(100)).unwrap(),
        t0 + Duration::from_millis(700),
    );
    assert_eq!(eff_final.sound_cue, Some(CueKind::Done));
    assert!(runtime.active_sessions_pool.is_empty());
    assert_eq!(runtime.dock_mode, EdgeDockMode::Normal);
    match &runtime.current_state {
        IslandState::Done {
            session_id,
            summary,
            ..
        } => {
            assert_eq!(session_id, "parent-swarm-1");
            assert_eq!(summary, "Swarm 4/4 已收官");
        }
        other => panic!("Expected unified Done state, got {:?}", other),
    }

    // Done 保护期内即使光标停留 > 260ms 也不得弹出 SwarmExpanded 覆盖 Done 卡片
    runtime.update_hover_state_at(
        50.0,
        4.0,
        380.0,
        58.0,
        240.0,
        25.0,
        480.0,
        t0 + Duration::from_millis(750),
    );
    runtime.update_hover_state_at(
        50.0,
        4.0,
        380.0,
        58.0,
        240.0,
        25.0,
        480.0,
        t0 + Duration::from_millis(1100),
    );
    assert_eq!(runtime.dock_mode, EdgeDockMode::Normal);
}


