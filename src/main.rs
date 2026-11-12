#![windows_subsystem = "windows"]

mod core;
mod ui;
mod win32;

use std::env;
use std::thread;
use std::time::Duration;

use core::bus::{AgentEvent, EventBus};
use core::watcher::{
    AntigravityWatcher, ClaudeWatcher, CodexWatcher, CopilotWatcher, CursorWatcher, DeepSeekWatcher,
    GrokWatcher, OpenCodeWatcher,
};
use win32::{GdiplusContext, IslandWindow};

fn main() {
    println!("==================================================");
    println!("     WNoobar (Windows AI Agent Dynamic Island)    ");
    println!("       Version 1.1.0 (8-Agent Ecosystem)          ");
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

    // 3.7 xAI Grok
    match GrokWatcher::new(tx.clone()) {
        Ok(mut watcher) => {
            watcher.start();
            println!("[Watcher] Grok monitor started.");
        }
        Err(e) => {
            println!("[Watcher] Grok monitor skipped: {e}");
        }
    }

    // 3.8 VS Code GitHub Copilot
    match CopilotWatcher::new(tx.clone()) {
        Ok(mut watcher) => {
            watcher.start();
            println!("[Watcher] GitHub Copilot monitor started.");
        }
        Err(e) => {
            println!("[Watcher] GitHub Copilot monitor skipped: {e}");
        }
    }

    // 4. 如果是 demo 模式，模拟多会话并发里程表垂直滚轮轮播
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--demo") {
        println!("[Demo] Demo mode active: simulating Odometer Carousel (WNoobar + QQ-BOT)...");
        let tx_demo = tx.clone();
        thread::spawn(move || {
            // 会话 1：WNoobar · AI 结对助手 (Antigravity)
            thread::sleep(Duration::from_millis(1000));
            let _ = tx_demo.send(AgentEvent::TaskStarted {
                session_id: "demo-s1".into(),
                project_name: "WNoobar".into(),
                role_type: "AI 结对助手".into(),
                agent_name: "Antigravity".into(),
                task_summary: "收到任务 · 开启多任务并发轮播...".into(),
            });

            // 会话 2：QQ-BOT · Claude (Claude Code) 同时并发启动！
            thread::sleep(Duration::from_millis(1200));
            let _ = tx_demo.send(AgentEvent::TaskStarted {
                session_id: "demo-s2".into(),
                project_name: "QQ-BOT".into(),
                role_type: "Claude".into(),
                agent_name: "Claude".into(),
                task_summary: "收到指令 · 正在处理群消息...".into(),
            });

            // 两大会话并发工作推进步数，触发里程表垂直滚轮换页
            for step in 1..=8 {
                thread::sleep(Duration::from_millis(1500));
                let _ = tx_demo.send(AgentEvent::Working {
                    session_id: "demo-s1".into(),
                    project_name: "WNoobar".into(),
                    role_type: "AI 结对助手".into(),
                    agent_name: "Antigravity".into(),
                    task_summary: format!("优化里程表轮播动效与微弹强调 (第 {} 步)", 220 + step),
                    step_index: 220 + step,
                });

                thread::sleep(Duration::from_millis(1500));
                let _ = tx_demo.send(AgentEvent::Working {
                    session_id: "demo-s2".into(),
                    project_name: "QQ-BOT".into(),
                    role_type: "Claude".into(),
                    agent_name: "Claude".into(),
                    task_summary: format!("同步 WebSocket 消息流 (第 {} 步)", 15 + step),
                    step_index: 15 + step,
                });
            }

            // 会话 1 率先完成交付
            thread::sleep(Duration::from_millis(2000));
            let _ = tx_demo.send(AgentEvent::Done {
                session_id: "demo-s1".into(),
                project_name: "WNoobar".into(),
                role_type: "AI 结对助手".into(),
                agent_name: "Antigravity".into(),
                summary: "里程表轮播动效系统已全部上线完成！".into(),
                duration_secs: 15,
                artifacts_count: 2,
            });

            // 会话 2 随后完成交付
            thread::sleep(Duration::from_millis(3500));
            let _ = tx_demo.send(AgentEvent::Done {
                session_id: "demo-s2".into(),
                project_name: "QQ-BOT".into(),
                role_type: "Claude".into(),
                agent_name: "Claude".into(),
                summary: "群消息流连接成功，全部就绪。".into(),
                duration_secs: 18,
                artifacts_count: 1,
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
