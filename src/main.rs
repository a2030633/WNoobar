#![windows_subsystem = "windows"]

mod core;
mod ui;
mod win32;

use std::env;
use std::thread;
use std::time::Duration;

use core::bus::{AgentEvent, EventBus};
use core::watcher::{AntigravityWatcher, CodexWatcher};
use win32::{GdiplusContext, IslandWindow};

fn main() {
    println!("==================================================");
    println!("     WNoobar (Windows AI Agent Dynamic Island)    ");
    println!("           Version 1.0.0 (Three-Beat Rhythm)      ");
    println!("==================================================");

    // 1. 初始化 Windows GDI+ 原生渲染子系统
    let _gdiplus = GdiplusContext::init().expect("Failed to initialize GDI+");
    println!("[Init] GDI+ Graphics Subsystem Initialized (Retina ClearType Ready).");

    // 2. 初始化核心事件总线
    let bus = EventBus::new();
    let tx = bus.sender();
    let rx = bus.receiver().clone();

    // 3. 启动 Antigravity 状态引擎监听器
    match AntigravityWatcher::new(tx.clone()) {
        Ok(mut watcher) => {
            watcher.start();
            println!("[Watcher] Antigravity Three-Beat Rhythm monitor started.");
        }
        Err(e) => {
            eprintln!("[Error] Failed to initialize Antigravity watcher: {e}");
        }
    }

    // 4. 启动 Codex 状态引擎监听器
    match CodexWatcher::new(tx.clone()) {
        Ok(mut watcher) => {
            watcher.start();
            println!("[Watcher] Codex Three-Beat Rhythm monitor started.");
        }
        Err(e) => {
            println!("[Watcher] Codex monitor skipped: {e}");
        }
    }

    // 4. 如果是 demo 模式，模拟 Sprint 5 的三段式节律交互
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--demo") {
        println!("[Demo] Demo mode active: simulating Three-Beat Rhythm (Start Pop -> Silent -> Completion Pop)...");
        let tx_demo = tx.clone();
        thread::spawn(move || {
            // 节拍 1：启动轻弹（1.8s后自动贴边隐退）
            thread::sleep(Duration::from_millis(1200));
            println!("[Demo Beat 1] -> TaskStarted: 灵动滑出致意，1.8s后自动贴边隐藏...");
            let _ = tx_demo.send(AgentEvent::TaskStarted {
                session_id: "demo-s1".into(),
                project_name: "WNoobar".into(),
                role_type: "AI 结对助手".into(),
                agent_name: "Antigravity".into(),
                task_summary: "收到任务 · 开始处理...".into(),
            });

            // 节拍 2：执行静默期（后台静默工作，不弹窗打扰用户）
            thread::sleep(Duration::from_millis(2500));
            println!("[Demo Beat 2] -> Silent In-Progress: 后台执行中（气泡静默维持贴边隐藏）...");
            let _ = tx_demo.send(AgentEvent::Working {
                session_id: "demo-s1".into(),
                project_name: "WNoobar".into(),
                role_type: "系统研发".into(),
                agent_name: "Antigravity".into(),
                task_summary: "处理中 · 编译嵌入高精图标并优化命中测试".into(),
                step_index: 2,
            });

            // 节拍 3：完成交付（顺滑展开卡片，3.5s后回缩贴边）
            thread::sleep(Duration::from_millis(3000));
            println!("[Demo Beat 3] -> Completion Pop: 任务全部完成，滑出展开大卡片（3.5s后隐退）...");
            let _ = tx_demo.send(AgentEvent::Done {
                session_id: "demo-s1".into(),
                project_name: "WNoobar".into(),
                role_type: "AI 结对助手".into(),
                agent_name: "Antigravity".into(),
                summary: "已为您实现三段式灵动交互，任务完成！".into(),
                duration_secs: 5,
                artifacts_count: 2,
            });
        });
    }

    // 5. 创建并显示原生悬浮穿透灵动岛
    println!("[UI] Creating Topmost Click-Through Layered Island Window (Retina Enabled)...");
    let window = IslandWindow::new(rx).expect("Failed to create Island Window");
    println!("[UI] WNoobar window is active at top center of primary display.");
    println!("[UI] Default state: Edge Peeking (nearly invisible 2.5px strip).");
    println!("[UI] Press Ctrl+C in console to terminate.");

    // 6. 运行原生 Windows 消息循环
    window.run_message_loop();
}
