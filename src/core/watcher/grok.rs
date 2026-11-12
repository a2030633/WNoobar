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

pub struct GrokWatcher {
    sessions_dir: PathBuf,
    sender: Sender<AgentEvent>,
    running: Arc<AtomicBool>,
}

pub struct GrokSessionTrackInfo {
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
}

impl GrokWatcher {
    pub fn new(sender: Sender<AgentEvent>) -> Result<Self, String> {
        let user_profile = std::env::var("USERPROFILE")
            .map_err(|e| format!("Could not read USERPROFILE: {e}"))?;
        let sessions_dir = PathBuf::from(user_profile).join(".grok").join("sessions");

        if !sessions_dir.exists() {
            return Err(format!("Grok sessions directory not found at {:?}", sessions_dir));
        }

        Ok(Self {
            sessions_dir,
            sender,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn start(&mut self) {
        self.running.store(true, Ordering::SeqCst);
        let sessions_dir = self.sessions_dir.clone();
        let sender = self.sender.clone();
        let running = self.running.clone();

        thread::Builder::new()
            .name("grok-watcher".into())
            .spawn(move || {
                Self::run_loop(sessions_dir, sender, running);
            })
            .expect("Failed to spawn Grok watcher thread");
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn run_loop(sessions_dir: PathBuf, sender: Sender<AgentEvent>, running: Arc<AtomicBool>) {
        println!("[Watcher] Grok Three-Beat Rhythm Engine active: {:?}", sessions_dir);

        let mut tracked_sessions: HashMap<String, GrokSessionTrackInfo> = HashMap::new();

        while running.load(Ordering::SeqCst) {
            let active_sessions = Self::scan_active_sessions(&sessions_dir);

            for (session_id, chat_file, summary_file) in active_sessions {
                let track = tracked_sessions.entry(session_id.clone()).or_insert_with(|| {
                    let len = chat_file.metadata().map(|m| m.len()).unwrap_or(0);
                    let (project_name, role_type, agent_name) = Self::read_summary_info(&summary_file);

                    GrokSessionTrackInfo {
                        session_id: session_id.clone(),
                        project_name,
                        role_type,
                        agent_name,
                        file_offset: len, // 初始历史文件不回溯，从当前末尾只监听新事件
                        last_event_time: Instant::now() - Duration::from_secs(60),
                        last_step_index: 0,
                        last_action_desc: "待命就绪".into(),
                        is_working: false,
                        has_reported_done: true,
                        task_start_pending: false,
                        task_summary_pending: "收到任务 · 开始处理...".into(),
                        last_model_finished_time: None,
                        final_summary_candidate: String::new(),
                    }
                });

                if let Ok(mut file) = File::open(&chat_file) {
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

                        // 动态更新 summary 标题
                        let (proj, role, _) = Self::read_summary_info(&summary_file);
                        if !proj.is_empty() && proj != "Grok Project" {
                            track.project_name = proj;
                        }
                        if !role.is_empty() && role != "Grok 4.6" {
                            track.role_type = role;
                        }

                        // 1. 【节拍 1：启动轻弹】
                        if track.task_start_pending {
                            track.task_start_pending = false;
                            track.is_working = true;
                            track.has_reported_done = false;
                            track.last_model_finished_time = None;
                            track.last_event_time = Instant::now();

                            println!("[Watcher] Grok TaskStarted: [{}] - {}", track.project_name, track.task_summary_pending);
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
                            // 静默超时兜底：必须已有总结候选，且已静默超过 2.5s（防止在 tool 执行中途误收敛）
                            !track.final_summary_candidate.is_empty()
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
                            format!("Grok 交付完成 (共 {} 步)", track.last_step_index)
                        } else {
                            "Grok 对话已完成".into()
                        };

                        let duration = track.last_event_time.elapsed().as_secs().max(1);
                        println!("[Watcher] Grok Task Done: [{}] - {}", track.project_name, summary);
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

    fn scan_active_sessions(sessions_dir: &Path) -> Vec<(String, PathBuf, PathBuf)> {
        let mut result = Vec::new();
        // sessions/<workspace_enc>/<session_id>/
        if let Ok(workspace_entries) = fs::read_dir(sessions_dir) {
            for ws_entry in workspace_entries.flatten() {
                let ws_path = ws_entry.path();
                if ws_path.is_dir() {
                    if let Ok(session_entries) = fs::read_dir(&ws_path) {
                        for s_entry in session_entries.flatten() {
                            let s_path = s_entry.path();
                            if s_path.is_dir() {
                                let chat_file = s_path.join("chat_history.jsonl");
                                let summary_file = s_path.join("summary.json");
                                if chat_file.is_file() {
                                    let session_id = s_path.file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if !session_id.is_empty() {
                                        result.push((session_id, chat_file, summary_file));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        result
    }

    pub fn read_summary_info(summary_file: &Path) -> (String, String, String) {
        if !summary_file.is_file() {
            return ("Grok Project".into(), "Grok 4.6".into(), "Grok".into());
        }

        if let Ok(content) = fs::read_to_string(summary_file) {
            if let Ok(val) = serde_json::from_str::<Value>(&content) {
                let cwd = val.pointer("/info/cwd").and_then(|c| c.as_str()).unwrap_or("");
                let project_name = Self::clean_project_name(cwd);

                let title = val.get("generated_title")
                    .or_else(|| val.get("session_summary"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let role_type = if !title.trim().is_empty() {
                    Self::truncate(title.trim(), 14)
                } else {
                    let model = val.get("current_model_id").and_then(|m| m.as_str()).unwrap_or("grok-4.6");
                    model.to_string()
                };

                let agent_name = val.get("agent_name")
                    .and_then(|a| a.as_str())
                    .unwrap_or("Grok")
                    .to_string();

                return (project_name, role_type, agent_name);
            }
        }

        ("Grok Project".into(), "Grok 4.6".into(), "Grok".into())
    }

    pub fn clean_project_name(cwd: &str) -> String {
        let p = Path::new(cwd);
        if let Some(file_name) = p.file_name().and_then(|n| n.to_str()) {
            if !file_name.is_empty() && file_name != "." {
                return file_name.to_string();
            }
        }
        if cwd.is_empty() {
            "Grok Project".into()
        } else {
            cwd.to_string()
        }
    }

    pub fn parse_entry(val: &Value, track: &mut GrokSessionTrackInfo) -> bool {
        let entry_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match entry_type {
            "user" => {
                let mut text_content = String::new();

                if let Some(content_arr) = val.get("content").and_then(|c| c.as_array()) {
                    for item in content_arr {
                        if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                            if let Some(txt) = item.get("text").and_then(|t| t.as_str()) {
                                text_content.push_str(txt);
                            }
                        }
                    }
                } else if let Some(txt) = val.get("content").and_then(|c| c.as_str()) {
                    text_content.push_str(txt);
                }

                // 提取 <user_query> ... </user_query>
                let clean_query = if let Some(start) = text_content.find("<user_query>") {
                    if let Some(end) = text_content.find("</user_query>") {
                        text_content[start + "<user_query>".len()..end].trim().to_string()
                    } else {
                        text_content[start + "<user_query>".len()..].trim().to_string()
                    }
                } else {
                    text_content.trim().to_string()
                };

                // 过滤内部 system-reminder
                if clean_query.starts_with("<user_info>") || clean_query.starts_with("<system-reminder>") {
                    return false;
                }

                if !clean_query.is_empty() {
                    track.task_start_pending = true;
                    track.is_working = true;
                    track.has_reported_done = false;
                    track.last_model_finished_time = None;
                    track.final_summary_candidate.clear();
                    track.last_step_index = 0;
                    track.task_summary_pending = format!("收到任务 · {}", Self::truncate(&clean_query, 20));
                    track.last_action_desc = "收到新指令 · 正在处理...".into();
                    return true;
                }
                false
            }
            "assistant" => {
                let mut tool_used = None;

                if let Some(tools) = val.get("tool_calls").and_then(|t| t.as_array()) {
                    if let Some(first_tool) = tools.first() {
                        let name = first_tool.get("name").and_then(|n| n.as_str()).unwrap_or("工具调用");
                        tool_used = Some(name.to_string());
                    }
                }

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

                // 文本回复收口判定
                let content_str = val.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let trimmed = content_str.trim();

                if !trimmed.is_empty() {
                    track.final_summary_candidate = Self::truncate(trimmed, 40);
                    track.has_reported_done = false;
                    track.is_working = true;
                    track.last_model_finished_time = Some(Instant::now());
                    return true;
                }

                false
            }
            "reasoning" => {
                if !track.is_working {
                    track.task_start_pending = true;
                }
                track.is_working = true;
                track.has_reported_done = false;
                track.last_model_finished_time = None;
                track.last_action_desc = "Grok 正在深度思考...".into();
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
    fn test_grok_parse_user_query() {
        let mut track = GrokSessionTrackInfo {
            session_id: "grok-sess-1".into(),
            project_name: "项目".into(),
            role_type: "Grok 4.6".into(),
            agent_name: "Grok".into(),
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
        };

        let user_entry = json!({
            "type": "user",
            "content": [
                { "type": "text", "text": "<user_query>\n帮我抓取浏览器页面内容\n</user_query>" }
            ]
        });

        assert!(GrokWatcher::parse_entry(&user_entry, &mut track));
        assert!(track.task_start_pending);
        assert!(track.is_working);
        assert!(track.task_summary_pending.contains("帮我抓取浏览器页面内容"));
    }

    #[test]
    fn test_grok_parse_tool_call() {
        let mut track = GrokSessionTrackInfo {
            session_id: "grok-sess-1".into(),
            project_name: "项目".into(),
            role_type: "Grok 4.6".into(),
            agent_name: "Grok".into(),
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
        };

        let tool_entry = json!({
            "type": "assistant",
            "content": "",
            "tool_calls": [
                { "id": "call-1", "name": "run_terminal_command", "arguments": "{}" }
            ]
        });

        assert!(GrokWatcher::parse_entry(&tool_entry, &mut track));
        assert_eq!(track.last_step_index, 1);
        assert!(track.last_action_desc.contains("run_terminal_command"));
    }

    #[test]
    fn test_grok_parse_completion() {
        let mut track = GrokSessionTrackInfo {
            session_id: "grok-sess-1".into(),
            project_name: "项目".into(),
            role_type: "Grok 4.6".into(),
            agent_name: "Grok".into(),
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
        };

        let done_entry = json!({
            "type": "assistant",
            "content": "已成功为你创建并启动 CDP 抓取器，结果已分析完成！"
        });

        assert!(GrokWatcher::parse_entry(&done_entry, &mut track));
        assert!(track.last_model_finished_time.is_some());
        assert!(track.final_summary_candidate.contains("已成功为你创建并启动"));
    }
}
