#![windows_subsystem = "windows"]

mod core;
mod ui;
mod win32;

use std::env;
use std::thread;
use std::time::Duration;

use core::bus::{AgentEvent, EventBus};
use core::watcher::{
    AntigravityWatcher, ClaudeWatcher, CodexWatcher, CursorWatcher, DeepSeekWatcher, OpenCodeWatcher,
};
use win32::{GdiplusContext, IslandWindow};

fn main() {
    println!("==================================================");
    println!("     WNoobar (Windows AI Agent Dynamic Island)    ");
    println!("       Version 1.0.0 (Multi-Agent Ecosystem)      ");
    println!("==================================================");

    // 1. 初始化 Windows GDI+ 原生渲染子系统
    let _gdiplus = GdiplusContext::init().expect("Failed to initialize GDI+");
    println!("[Init] GDI+ Graphics Subsystem Initialized (Retina ClearType Ready).");

    // 2. 初始化核心事件总线
    let bus = EventBus::new();
    let tx = bus.sender();
    let rx = bus.receiver().clone();

    // 3. 启动 6 大 AI 助手状态引擎监听器（容错热插拔模式）
    // 3.1 Antigravity
    match AntigravityWatcher::new(tx.clone()) {
        Ok(mut watcher) => {
            watcher.start();
            println!("[Watcher] Antigravity monitor started.");
        }
        Err(e) => {
            println!("[Watcher] Antigravity monitor skipped: {e}");
        }
    }

    // 3.2 Codex
    match CodexWatcher::new(tx.clone()) {
        Ok(mut watcher) => {
            watcher.start();
            println!("[Watcher] Codex monitor started.");
        }
        Err(e) => {
            println!("[Watcher] Codex monitor skipped: {e}");
        }
    }

    // 3.3 Claude Code
    match ClaudeWatcher::new(tx.clone()) {
        Ok(mut watcher) => {
            watcher.start();
            println!("[Watcher] Claude Code monitor started.");
        }
        Err(e) => {
            println!("[Watcher] Claude Code monitor skipped: {e}");
        }
    }

    // 3.4 Cursor
    match CursorWatcher::new(tx.clone()) {
        Ok(mut watcher) => {
            watcher.start();
            println!("[Watcher] Cursor monitor started.");
        }
        Err(e) => {
            println!("[Watcher] Cursor monitor skipped: {e}");
        }
    }

    // 3.5 DeepSeek
    match DeepSeekWatcher::new(tx.clone()) {
        Ok(mut watcher) => {
            watcher.start();
            println!("[Watcher] DeepSeek monitor started.");
        }
        Err(e) => {
            println!("[Watcher] DeepSeek monitor skipped: {e}");
        }
    }

    // 3.6 OpenCode
    match OpenCodeWatcher::new(tx.clone()) {
        Ok(mut watcher) => {
            watcher.start();
            println!("[Watcher] OpenCode monitor started.");
        }
        Err(e) => {
            println!("[Watcher] OpenCode monitor skipped: {e}");
        }
    }

    // 4. 如果是 demo 模式，模拟多品牌 AI 助手的三段式节律交互
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--demo") {
        println!("[Demo] Demo mode active: simulating Multi-Agent Three-Beat Rhythm...");
        let tx_demo = tx.clone();
        thread::spawn(move || {
            // 品牌 1：Cursor 启动与生成
            thread::sleep(Duration::from_millis(1000));
            let _ = tx_demo.send(AgentEvent::TaskStarted {
                session_id: "demo-cursor".into(),
                project_name: "FrontendApp".into(),
                role_type: "Cursor Composer".into(),
                agent_name: "Cursor".into(),
                task_summary: "Composer 开始生成导航栏组件...".into(),
            });

            thread::sleep(Duration::from_millis(2000));
            let _ = tx_demo.send(AgentEvent::Done {
                session_id: "demo-cursor".into(),
                project_name: "FrontendApp".into(),
                role_type: "Cursor Composer".into(),
                agent_name: "Cursor".into(),
                summary: "组件生成完毕，已自动保存修改".into(),
                duration_secs: 4,
                artifacts_count: 1,
            });

            // 品牌 2：Claude Code 启动与执行
            thread::sleep(Duration::from_millis(4500));
            let _ = tx_demo.send(AgentEvent::TaskStarted {
                session_id: "demo-claude".into(),
                project_name: "agent-go-proxy".into(),
                role_type: "Claude Code".into(),
                agent_name: "Claude".into(),
                task_summary: "收到任务 · 优化路由与缓存...".into(),
            });

            thread::sleep(Duration::from_millis(2000));
            let _ = tx_demo.send(AgentEvent::Done {
                session_id: "demo-claude".into(),
                project_name: "agent-go-proxy".into(),
                role_type: "Claude Code".into(),
                agent_name: "Claude".into(),
                summary: "重构已完成，单元测试 100% 通过".into(),
                duration_secs: 6,
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
