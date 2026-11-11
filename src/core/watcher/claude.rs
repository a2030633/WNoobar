#![allow(dead_code)]
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use serde_json::Value;

use crate::core::bus::AgentEvent;

pub struct ClaudeWatcher {
    projects_dir: PathBuf,
    sender: Sender<AgentEvent>,
    running: Arc<AtomicBool>,
}

pub struct ClaudeSessionTrackInfo {
    pub session_id: String,
    pub project_name: String,
    pub role_type: String,
    pub agent_name: String,
    pub file_offset: u64,
    pub last_event_time: Instant,
    pub last_step_index: u64,
    pub last_action_desc: String,
    pub is_working: bool,
    pub has_reported_done: bool,
    pub task_start_pending: bool,
    pub task_summary_pending: String,
    pub last_model_finished_time: Option<Instant>,
    pub final_summary_candidate: String,
    pub done_pending: Option<(String, u64)>,
}

impl ClaudeWatcher {
    pub fn new(sender: Sender<AgentEvent>) -> Result<Self, String> {
        let user_profile = std::env::var("USERPROFILE")
            .map_err(|e| format!("Could not read USERPROFILE: {e}"))?;
        let projects_dir = PathBuf::from(user_profile).join(".claude").join("projects");

        if !projects_dir.exists() {
            return Err(format!("Claude projects directory not found at {:?}", projects_dir));
        }

        Ok(Self {
            projects_dir,
            sender,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn start(&mut self) {
        self.running.store(true, Ordering::SeqCst);
        let projects_dir = self.projects_dir.clone();
        let sender = self.sender.clone();
        let running = self.running.clone();

        thread::Builder::new()
            .name("claude-watcher".into())
            .spawn(move || {
                Self::run_loop(projects_dir, sender, running);
            })
            .expect("Failed to spawn Claude watcher thread");
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn run_loop(projects_dir: PathBuf, sender: Sender<AgentEvent>, running: Arc<AtomicBool>) {
        println!("[Watcher] Claude Code Three-Beat Rhythm Engine active: {:?}", projects_dir);

        let mut tracked_sessions: HashMap<String, ClaudeSessionTrackInfo> = HashMap::new();

        while running.load(Ordering::SeqCst) {
            let active_files = Self::scan_active_sessions(&projects_dir);

            for (session_id, file_path, folder_name) in active_files {
                let track = tracked_sessions.entry(session_id.clone()).or_insert_with(|| {
                    let len = file_path.metadata().map(|m| m.len()).unwrap_or(0);
                    let project_name = Self::clean_project_name(&folder_name);

                    ClaudeSessionTrackInfo {
                        session_id: session_id.clone(),
                        project_name,
                        role_type: "Claude Code".into(),
                        agent_name: "Claude".into(),
                        file_offset: len, // 初始不回溯历史日志，只监听新事件
                        last_event_time: Instant::now() - Duration::from_secs(60),
                        last_step_index: 0,
                        last_action_desc: "待命就绪".into(),
                        is_working: false,
                        has_reported_done: true,
                        task_start_pending: false,
                        task_summary_pending: "收到任务 · 开始处理...".into(),
                        last_model_finished_time: None,
                        final_summary_candidate: String::new(),
                        done_pending: None,
                    }
                });

                if let Ok(mut file) = File::open(&file_path) {
                    let current_len = file.metadata().map(|m| m.len()).unwrap_or(0);
                    if current_len > track.file_offset {
                        let _ = file.seek(SeekFrom::Start(track.file_offset));
                        let mut reader = BufReader::new(file);
                        let mut line = String::new();
                        let mut step_updated = false;

                        while reader.read_line(&mut line).unwrap_or(0) > 0 {
                            let trimmed = line.trim();
                            if !trimmed.is_empty() {
                                if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                                    if Self::parse_entry(&val, track) {
                                        step_updated = true;
                                    }
                                }
                            }
                            line.clear();
                        }

                        track.file_offset = current_len;

                        // 0. 【立即完成 / 用户中断收敛节拍】
                        if let Some((summary, duration_secs)) = track.done_pending.take() {
                            track.is_working = false;
                            track.has_reported_done = true;
                            track.last_model_finished_time = None;

                            println!("[Watcher] Claude Task Interrupted/Done: [{}] - {}", track.project_name, summary);
                            let _ = sender.send(AgentEvent::Done {
                                session_id: track.session_id.clone(),
                                project_name: track.project_name.clone(),
                                role_type: track.role_type.clone(),
                                agent_name: track.agent_name.clone(),
                                summary,
                                duration_secs,
                                artifacts_count: 0,
                            });
                        } else if track.task_start_pending {
                            // 1. 【节拍 1：启动轻弹】
                            track.task_start_pending = false;
                            track.is_working = true;
                            track.has_reported_done = false;
                            track.last_model_finished_time = None;
                            track.last_event_time = Instant::now();

                            println!("[Watcher] Claude TaskStarted: [{}] - {}", track.project_name, track.task_summary_pending);
                            let _ = sender.send(AgentEvent::TaskStarted {
                                session_id: track.session_id.clone(),
                                project_name: track.project_name.clone(),
                                role_type: track.role_type.clone(),
                                agent_name: track.agent_name.clone(),
                                task_summary: track.task_summary_pending.clone(),
                            });
                        } else if step_updated && track.is_working {
                            // 2. 【节拍 2：执行静默期】
                            let now = Instant::now();
                            track.last_event_time = now;
                            let _ = sender.send(AgentEvent::Working {
                                session_id: track.session_id.clone(),
                                project_name: track.project_name.clone(),
                                role_type: track.role_type.clone(),
                                agent_name: track.agent_name.clone(),
                                task_summary: track.last_action_desc.clone(),
                                step_index: track.last_step_index,
                            });
                        }
                    }
                }

                // 3. 【节拍 3：完成交付 (800ms 防抖收敛 / 2.5s 静默超时双保险)】
                if track.is_working && !track.has_reported_done && !track.task_start_pending {
                    let should_deliver = match track.last_model_finished_time {
                        Some(finish_time) => finish_time.elapsed() >= Duration::from_millis(800),
                        None => {
                            // 静默超时兜底：至少已有总结候选或已有执行步骤，且已静默超过 2.5s
                            (!track.final_summary_candidate.is_empty() || track.last_step_index > 0)
                                && track.last_event_time.elapsed() >= Duration::from_millis(2500)
                        }
                    };

                    if should_deliver {
                        track.is_working = false;
                        track.has_reported_done = true;
                        track.last_model_finished_time = None;

                        let summary = if !track.final_summary_candidate.is_empty() {
                            track.final_summary_candidate.clone()
                        } else if track.last_step_index > 0 {
                            format!("Claude 交付完成 (共 {} 步)", track.last_step_index)
                        } else {
                            "Claude 对话已完成".into()
                        };

                        let duration = track.last_event_time.elapsed().as_secs().max(1);
                        println!("[Watcher] Claude Task Done: [{}] - {}", track.project_name, summary);
                        let _ = sender.send(AgentEvent::Done {
                            session_id: track.session_id.clone(),
                            project_name: track.project_name.clone(),
                            role_type: track.role_type.clone(),
                            agent_name: track.agent_name.clone(),
                            summary,
                            duration_secs: duration,
                            artifacts_count: 0,
                        });
                    }
                }
            }

            thread::sleep(Duration::from_millis(180));
        }
    }

    fn scan_active_sessions(projects_dir: &Path) -> Vec<(String, PathBuf, String)> {
        let mut result = Vec::new();
        if let Ok(entries) = fs::read_dir(projects_dir) {
            for entry in entries.flatten() {
                let folder_path = entry.path();
                if folder_path.is_dir() {
                    let folder_name = folder_path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                    if let Ok(files) = fs::read_dir(&folder_path) {
                        for file_entry in files.flatten() {
                            let p = file_entry.path();
                            if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                                    result.push((stem.to_string(), p, folder_name.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
        result
    }

    pub fn clean_project_name(name_or_path: &str) -> String {
        let p = Path::new(name_or_path);
        if let Some(file_name) = p.file_name().and_then(|n| n.to_str()) {
            if !file_name.is_empty() && file_name != "." {
                return file_name.to_string();
            }
        }
        if name_or_path.contains("-----") {
            if let Some(last) = name_or_path.split("-----").last() {
                if !last.is_empty() {
                    return last.to_string();
                }
            }
        }
        if name_or_path.is_empty() {
            "Claude Project".into()
        } else {
            name_or_path.to_string()
        }
    }

    pub fn parse_entry(val: &Value, track: &mut ClaudeSessionTrackInfo) -> bool {
        let entry_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");

        // 更新 cwd 探测到的项目名
        if let Some(cwd) = val.get("cwd").and_then(|c| c.as_str()) {
            let cleaned = Self::clean_project_name(cwd);
            if !cleaned.is_empty() {
                track.project_name = cleaned;
            }
        }

        match entry_type {
            "ai-title" => {
                if let Some(title) = val.get("aiTitle").and_then(|t| t.as_str()) {
                    if !title.trim().is_empty() {
                        track.role_type = title.trim().to_string();
                    }
                }
                false
            }
            "last-prompt" => {
                if let Some(prompt) = val.get("lastPrompt").and_then(|p| p.as_str()) {
                    let trimmed = prompt.trim();
                    if !trimmed.is_empty() && track.role_type == "Claude Code" {
                        track.role_type = Self::truncate(trimmed, 12);
                    }
                }
                false
            }
            "user" => {
                let mut text_content = String::new();
                if let Some(content_arr) = val.pointer("/message/content").and_then(|c| c.as_array()) {
                    for item in content_arr {
                        if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                            if let Some(txt) = item.get("text").and_then(|t| t.as_str()) {
                                text_content.push_str(txt);
                            }
                        }
                    }
                }

                if text_content.contains("[Request interrupted by user]") {
                    let duration = track.last_event_time.elapsed().as_secs().max(1);
                    track.done_pending = Some(("任务已由用户取消".into(), duration));
                    track.is_working = false;
                    track.has_reported_done = true;
                    track.last_model_finished_time = None;
                    return true;
                }

                if !text_content.trim().is_empty() {
                    track.task_start_pending = true;
                    track.is_working = true;
                    track.has_reported_done = false;
                    track.last_model_finished_time = None;
                    track.final_summary_candidate.clear();
                    track.last_step_index = 0;
                    track.task_summary_pending = format!("收到任务 · {}", Self::truncate(text_content.trim(), 20));
                    track.last_action_desc = "收到新指令 · 正在处理...".into();
                    return true;
                }
                false
            }
            "assistant" => {
                let mut tool_used = None;
                let mut text_candidate = String::new();

                if let Some(content_arr) = val.pointer("/message/content").and_then(|c| c.as_array()) {
                    for item in content_arr {
                        let ctype = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        if ctype == "tool_use" {
                            let tool_name = item.get("name").and_then(|n| n.as_str()).unwrap_or("工具调用");
                            tool_used = Some(tool_name.to_string());
                        } else if ctype == "text" {
                            if let Some(txt) = item.get("text").and_then(|t| t.as_str()) {
                                text_candidate.push_str(txt);
                            }
                        }
                    }
                }

                let stop_reason = val.pointer("/message/stop_reason").and_then(|r| r.as_str()).unwrap_or("");

                if let Some(tool) = tool_used {
                    track.last_step_index += 1;
                    if !track.is_working {
                        track.task_start_pending = true;
                    }
                    track.is_working = true;
                    track.has_reported_done = false;
                    track.last_model_finished_time = None;
                    track.last_action_desc = format!("执行 (第 {} 步) · {}", track.last_step_index, tool);
                    return true;
                }

                // 收官完成判定：
                // 1) 只要没有 tool_use 且输出了非空文本（解决第三方中转/VS Code 中没有 stop_reason 的缺陷）
                // 2) 或者 stop_reason 明确指示为 end_turn / stop
                let has_text = !text_candidate.trim().is_empty();
                let is_stopped = stop_reason == "end_turn" || stop_reason == "stop";

                if has_text || is_stopped {
                    if has_text {
                        track.final_summary_candidate = Self::truncate(text_candidate.trim(), 40);
                    }
                    track.has_reported_done = false;
                    track.is_working = true;
                    track.last_model_finished_time = Some(Instant::now());
                    return true;
                }

                false
            }
            "mode" => {
                let mode = val.get("mode").and_then(|m| m.as_str()).unwrap_or("");
                if mode == "normal" && track.is_working && !track.has_reported_done {
                    // 当检测到 VS Code 回到 normal 待命状态时，激活防抖完成计时
                    if track.last_model_finished_time.is_none() {
                        track.last_model_finished_time = Some(Instant::now());
                    }
                    return true;
                }
                false
            }
            "progress" | "api-request" => {
                if !track.is_working {
                    track.task_start_pending = true;
                }
                track.is_working = true;
                track.has_reported_done = false;
                track.last_model_finished_time = None;
                track.last_action_desc = "正在请求大模型 API...".into();
                true
            }
            _ => false,
        }
    }

    fn truncate(s: &str, max_len: usize) -> String {
        if s.chars().count() > max_len {
            let mut res: String = s.chars().take(max_len).collect();
            res.push_str("...");
            res
        } else {
            s.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_claude_parse_user_input() {
        let mut track = ClaudeSessionTrackInfo {
            session_id: "claude-sess-1".into(),
            project_name: "agent-go-proxy".into(),
            role_type: "Claude Code".into(),
            agent_name: "Claude".into(),
            file_offset: 0,
            last_event_time: Instant::now(),
            last_step_index: 0,
            last_action_desc: "待命就绪".into(),
            is_working: false,
            has_reported_done: true,
            task_start_pending: false,
            task_summary_pending: "".into(),
            last_model_finished_time: None,
            final_summary_candidate: "".into(),
            done_pending: None,
        };

        let user_entry = json!({
            "type": "user",
            "cwd": "F:\\项目\\agent-go-proxy",
            "message": {
                "role": "user",
                "content": [
                    { "type": "text", "text": "优化代理路由缓存策略" }
                ]
            }
        });

        assert!(ClaudeWatcher::parse_entry(&user_entry, &mut track));
        assert!(track.task_start_pending);
        assert!(track.is_working);
        assert_eq!(track.project_name, "agent-go-proxy");
    }

    #[test]
    fn test_claude_parse_user_interrupted() {
        let mut track = ClaudeSessionTrackInfo {
            session_id: "claude-sess-interrupted".into(),
            project_name: "agent-go-proxy".into(),
            role_type: "Claude Code".into(),
            agent_name: "Claude".into(),
            file_offset: 0,
            last_event_time: Instant::now(),
            last_step_index: 2,
            last_action_desc: "执行中".into(),
            is_working: true,
            has_reported_done: false,
            task_start_pending: false,
            task_summary_pending: "".into(),
            last_model_finished_time: None,
            final_summary_candidate: "".into(),
            done_pending: None,
        };

        let interrupt_entry = json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [
                    { "type": "text", "text": "[Request interrupted by user]" }
                ]
            }
        });

        assert!(ClaudeWatcher::parse_entry(&interrupt_entry, &mut track));
        assert!(track.done_pending.is_some());
        let (summary, _) = track.done_pending.unwrap();
        assert_eq!(summary, "任务已由用户取消");
        assert!(!track.is_working);
        assert!(track.has_reported_done);
    }

    #[test]
    fn test_claude_parse_tool_use() {
        let mut track = ClaudeSessionTrackInfo {
            session_id: "claude-sess-1".into(),
            project_name: "agent-go-proxy".into(),
            role_type: "Claude Code".into(),
            agent_name: "Claude".into(),
            file_offset: 0,
            last_event_time: Instant::now(),
            last_step_index: 0,
            last_action_desc: "待命就绪".into(),
            is_working: true,
            has_reported_done: false,
            task_start_pending: false,
            task_summary_pending: "".into(),
            last_model_finished_time: None,
            final_summary_candidate: "".into(),
            done_pending: None,
        };

        let tool_entry = json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "tool_use", "name": "Bash", "input": { "command": "go test ./..." } }
                ],
                "stop_reason": "tool_use"
            }
        });

        assert!(ClaudeWatcher::parse_entry(&tool_entry, &mut track));
        assert_eq!(track.last_step_index, 1);
        assert!(track.last_action_desc.contains("Bash"));
    }

    #[test]
    fn test_claude_parse_completion() {
        let mut track = ClaudeSessionTrackInfo {
            session_id: "claude-sess-1".into(),
            project_name: "agent-go-proxy".into(),
            role_type: "Claude Code".into(),
            agent_name: "Claude".into(),
            file_offset: 0,
            last_event_time: Instant::now(),
            last_step_index: 2,
            last_action_desc: "执行中".into(),
            is_working: true,
            has_reported_done: false,
            task_start_pending: false,
            task_summary_pending: "".into(),
            last_model_finished_time: None,
            final_summary_candidate: "".into(),
            done_pending: None,
        };

        let done_entry = json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "已成功优化代理路由，所有单元测试均通过。" }
                ],
                "stop_reason": "end_turn"
            }
        });

        assert!(ClaudeWatcher::parse_entry(&done_entry, &mut track));
        assert!(track.last_model_finished_time.is_some());
        assert!(track.final_summary_candidate.contains("已成功优化代理路由"));
    }

    #[test]
    fn test_claude_parse_text_response_without_stop_reason() {
        let mut track = ClaudeSessionTrackInfo {
            session_id: "claude-sess-2".into(),
            project_name: "agent-go-proxy".into(),
            role_type: "Claude Code".into(),
            agent_name: "Claude".into(),
            file_offset: 0,
            last_event_time: Instant::now(),
            last_step_index: 0,
            last_action_desc: "收到新指令 · 正在处理...".into(),
            is_working: true,
            has_reported_done: false,
            task_start_pending: false,
            task_summary_pending: "".into(),
            last_model_finished_time: None,
            final_summary_candidate: "".into(),
            done_pending: None,
        };

        // 模拟 VS Code 中 Claude Code 回复天气问题，无 stop_reason 字段
        let text_reply_without_stop_reason = json!({
            "parentUuid": "abc-123",
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "我现在无法直接调用实时天气插件，但余杭区预计今天多云转晴。" }
                ]
            }
        });

        assert!(ClaudeWatcher::parse_entry(&text_reply_without_stop_reason, &mut track));
        assert!(track.last_model_finished_time.is_some());
        assert!(track.final_summary_candidate.contains("我现在无法直接调用实时天气插件"));
    }

    #[test]
    fn test_claude_parse_mode_normal() {
        let mut track = ClaudeSessionTrackInfo {
            session_id: "claude-sess-3".into(),
            project_name: "agent-go-proxy".into(),
            role_type: "Claude Code".into(),
            agent_name: "Claude".into(),
            file_offset: 0,
            last_event_time: Instant::now(),
            last_step_index: 1,
            last_action_desc: "执行中".into(),
            is_working: true,
            has_reported_done: false,
            task_start_pending: false,
            task_summary_pending: "".into(),
            last_model_finished_time: None,
            final_summary_candidate: "".into(),
            done_pending: None,
        };

        let mode_entry = json!({
            "type": "mode",
            "mode": "normal"
        });

        assert!(ClaudeWatcher::parse_entry(&mode_entry, &mut track));
        assert!(track.last_model_finished_time.is_some());
    }
}
