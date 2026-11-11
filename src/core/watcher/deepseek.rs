#![allow(dead_code)]
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use serde_json::Value;

use crate::core::bus::AgentEvent;

pub struct DeepSeekWatcher {
    search_dirs: Vec<PathBuf>,
    sender: Sender<AgentEvent>,
    running: Arc<AtomicBool>,
}

pub struct DeepSeekSessionTrackInfo {
    pub session_id: String,
    pub project_name: String,
    pub role_type: String,
    pub agent_name: String,
    pub file_offset: u64,
    pub last_event_time: Instant,
    pub is_thinking: bool,
    pub is_working: bool,
    pub has_reported_done: bool,
    pub task_start_pending: bool,
    pub task_summary_pending: String,
    pub last_action_desc: String,
    pub last_model_finished_time: Option<Instant>,
    pub final_summary_candidate: String,
}

impl DeepSeekWatcher {
    pub fn new(sender: Sender<AgentEvent>) -> Result<Self, String> {
        let user_profile = std::env::var("USERPROFILE").unwrap_or_default();
        let local_app_data = std::env::var("LOCALAPPDATA").unwrap_or_default();

        let mut search_dirs = Vec::new();

        if !local_app_data.is_empty() {
            let p1 = PathBuf::from(&local_app_data).join("@deepseek-ai");
            if p1.exists() { search_dirs.push(p1); }
            let p2 = PathBuf::from(&local_app_data).join("deepseek");
            if p2.exists() { search_dirs.push(p2); }
        }

        if !user_profile.is_empty() {
            let p3 = PathBuf::from(&user_profile).join(".deepseek-tui");
            if p3.exists() { search_dirs.push(p3); }
            let p4 = PathBuf::from(&user_profile).join(".deepseek");
            if p4.exists() { search_dirs.push(p4); }
        }

        // 如果全部候选路径均不存在，保留默认目录以便后续热检测
        if search_dirs.is_empty() {
            let default_dir = PathBuf::from(&user_profile).join(".deepseek");
            search_dirs.push(default_dir);
        }

        Ok(Self {
            search_dirs,
            sender,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn start(&mut self) {
        self.running.store(true, Ordering::SeqCst);
        let search_dirs = self.search_dirs.clone();
        let sender = self.sender.clone();
        let running = self.running.clone();

        thread::Builder::new()
            .name("deepseek-watcher".into())
            .spawn(move || {
                Self::run_loop(search_dirs, sender, running);
            })
            .expect("Failed to spawn DeepSeek watcher thread");
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn run_loop(search_dirs: Vec<PathBuf>, sender: Sender<AgentEvent>, running: Arc<AtomicBool>) {
        println!("[Watcher] DeepSeek Three-Beat Rhythm Engine active: {:?}", search_dirs);

        let mut tracked_sessions: HashMap<String, DeepSeekSessionTrackInfo> = HashMap::new();

        while running.load(Ordering::SeqCst) {
            let active_files = Self::scan_active_files(&search_dirs);

            for (session_id, file_path) in active_files {
                let track = tracked_sessions.entry(session_id.clone()).or_insert_with(|| {
                    let len = file_path.metadata().map(|m| m.len()).unwrap_or(0);
                    DeepSeekSessionTrackInfo {
                        session_id: session_id.clone(),
                        project_name: "DeepSeek".into(),
                        role_type: "DeepSeek 思考模型".into(),
                        agent_name: "DeepSeek".into(),
                        file_offset: len, // 初始不回溯历史日志，只监听新事件
                        last_event_time: Instant::now() - Duration::from_secs(60),
                        is_thinking: false,
                        is_working: false,
                        has_reported_done: true,
                        task_start_pending: false,
                        task_summary_pending: "收到任务 · 深度推理中...".into(),
                        last_action_desc: "待命就绪".into(),
                        last_model_finished_time: None,
                        final_summary_candidate: String::new(),
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
                                if Self::parse_line(trimmed, track) {
                                    step_updated = true;
                                }
                            }
                            line.clear();
                        }

                        track.file_offset = current_len;

                        // 1. 【节拍 1：启动轻弹】
                        if track.task_start_pending {
                            track.task_start_pending = false;
                            track.is_working = true;
                            track.has_reported_done = false;
                            track.last_model_finished_time = None;
                            track.last_event_time = Instant::now();

                            println!("[Watcher] DeepSeek TaskStarted: [{}] - {}", track.project_name, track.task_summary_pending);
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
                            if now.duration_since(track.last_event_time) >= Duration::from_millis(600) {
                                track.last_event_time = now;
                                let _ = sender.send(AgentEvent::Working {
                                    session_id: track.session_id.clone(),
                                    project_name: track.project_name.clone(),
                                    role_type: track.role_type.clone(),
                                    agent_name: track.agent_name.clone(),
                                    task_summary: track.last_action_desc.clone(),
                                    step_index: 1,
                                });
                            }
                        }
                    }
                }

                // 3. 【节拍 3：完成交付】
                if track.is_working && !track.has_reported_done {
                    if let Some(finish_time) = track.last_model_finished_time {
                        if finish_time.elapsed() >= Duration::from_millis(800) {
                            track.is_working = false;
                            track.has_reported_done = true;
                            track.last_model_finished_time = None;

                            let summary = if !track.final_summary_candidate.is_empty() {
                                track.final_summary_candidate.clone()
                            } else {
                                "DeepSeek 思考与生成完毕".into()
                            };

                            let duration = track.last_event_time.elapsed().as_secs().max(1);
                            println!("[Watcher] DeepSeek Task Done: [{}] - {}", track.project_name, summary);
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
            }

            thread::sleep(Duration::from_millis(250));
        }
    }

    fn scan_active_files(search_dirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
        let mut list = Vec::new();
        for dir in search_dirs {
            if dir.exists() {
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_file() {
                            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
                            if ext == "jsonl" || ext == "log" || ext == "json" {
                                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                                    list.push((stem.to_string(), p));
                                }
                            }
                        }
                    }
                }
            }
        }
        list
    }

    pub fn parse_line(line: &str, track: &mut DeepSeekSessionTrackInfo) -> bool {
        // 尝试解析为 JSON
        if let Ok(val) = serde_json::from_str::<Value>(line) {
            let role = val.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role == "user" {
                let content = val.get("content").and_then(|c| c.as_str()).unwrap_or("用户指令");
                track.task_start_pending = true;
                track.is_working = true;
                track.has_reported_done = false;
                track.last_model_finished_time = None;
                track.task_summary_pending = format!("收到任务 · {}", Self::truncate(content, 20));
                track.last_action_desc = "收到新指令 · 正在准备...".into();
                return true;
            }

            if let Some(content) = val.get("content").and_then(|c| c.as_str()) {
                return Self::parse_raw_text(content, track);
            }
        }

        // 解析纯文本行
        Self::parse_raw_text(line, track)
    }

    pub fn parse_raw_text(text: &str, track: &mut DeepSeekSessionTrackInfo) -> bool {
        if text.contains("<think>") {
            track.is_thinking = true;
            if !track.is_working {
                track.task_start_pending = true;
            }
            track.is_working = true;
            track.has_reported_done = false;
            track.last_model_finished_time = None;
            track.last_action_desc = "深度思考中 (Reasoning Engine)...".into();
            return true;
        }

        if text.contains("</think>") {
            track.is_thinking = false;
            track.last_action_desc = "思考完毕 · 组织交付回答...".into();
            let after = text.split("</think>").nth(1).unwrap_or("");
            if !after.trim().is_empty() {
                track.final_summary_candidate = Self::truncate(after.trim(), 40);
            }
            track.last_model_finished_time = Some(Instant::now());
            return true;
        }

        if track.is_thinking {
            track.last_action_desc = "深度思考中 · 推理方案...".into();
            return true;
        }

        if text.contains("[DONE]") || text.contains("\"finish_reason\":\"stop\"") {
            track.last_model_finished_time = Some(Instant::now());
            track.has_reported_done = false;
            track.is_working = true;
            return true;
        }

        false
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

    #[test]
    fn test_deepseek_reasoning_flow() {
        let mut track = DeepSeekSessionTrackInfo {
            session_id: "ds-1".into(),
            project_name: "DeepSeek".into(),
            role_type: "DeepSeek R1".into(),
            agent_name: "DeepSeek".into(),
            file_offset: 0,
            last_event_time: Instant::now(),
            is_thinking: false,
            is_working: false,
            has_reported_done: true,
            task_start_pending: false,
            task_summary_pending: "".into(),
            last_action_desc: "".into(),
            last_model_finished_time: None,
            final_summary_candidate: "".into(),
        };

        // 1. 识别深度思考开始
        assert!(DeepSeekWatcher::parse_raw_text("<think>正在推导数学公式...", &mut track));
        assert!(track.is_thinking);
        assert!(track.is_working);
        assert_eq!(track.last_action_desc, "深度思考中 (Reasoning Engine)...");

        // 2. 识别深度思考结束并交付
        assert!(DeepSeekWatcher::parse_raw_text("</think>公式推导完毕，最终结论是收敛的。", &mut track));
        assert!(!track.is_thinking);
        assert!(track.last_model_finished_time.is_some());
        assert!(track.final_summary_candidate.contains("公式推导完毕"));
    }
}
