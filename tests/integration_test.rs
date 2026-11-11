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

    let started = IslandState::Started {
        session_id: "test-s1".into(),
        project_name: "WNoobar".into(),
        role_type: "核心研发".into(),
        agent_name: "Antigravity".into(),
        task_summary: "开始处理".into(),
        started_at: std::time::Instant::now(),
        auto_hide_secs: 1.8,
    };
    assert_eq!(started.target_dimensions(EdgeDockMode::Normal), (320.0, 38.0));

    let working_short = IslandState::Working {
        session_id: "test-s1".into(),
        project_name: "WNoobar".into(),
        role_type: "助手".into(),
        agent_name: "Antigravity".into(),
        task_summary: "处理".into(),
        step_index: 1,
        start_time: std::time::Instant::now(),
    };
    assert_eq!(working_short.target_dimensions(EdgeDockMode::Normal), (360.0, 38.0));

    let working_long = IslandState::Working {
        session_id: "test-s1".into(),
        project_name: "WNoobar".into(),
        role_type: "远程管理ros-L2TP".into(),
        agent_name: "Antigravity".into(),
        task_summary: "收到任务 · 开始处理中，修改代码...".into(),
        step_index: 2,
        start_time: std::time::Instant::now(),
    };
    assert_eq!(working_long.target_dimensions(EdgeDockMode::Normal), (420.0, 38.0));

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
