#![allow(dead_code)]
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::Sender;
use serde_json::Value;

use crate::core::bus::AgentEvent;

pub struct OpenCodeWatcher {
    storage_dir: PathBuf,
    sender: Sender<AgentEvent>,
    running: Arc<AtomicBool>,
}

pub struct OpenCodeSessionTrackInfo {
    pub session_id: String,
    pub project_name: String,
    pub role_type: String,
    pub agent_name: String,
    pub last_mod_time: Option<SystemTime>,
    pub last_event_time: Instant,
    pub is_working: bool,
    pub has_reported_done: bool,
    pub last_action_desc: String,
    pub last_active_time: Option<Instant>,
    pub step_count: u64,
}

impl OpenCodeWatcher {
    pub fn new(sender: Sender<AgentEvent>) -> Result<Self, String> {
        let user_profile = std::env::var("USERPROFILE").unwrap_or_default();
        let storage_dir = PathBuf::from(user_profile)
            .join(".local")
            .join("share")
            .join("opencode")
            .join("storage");

        Ok(Self {
            storage_dir,
            sender,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn start(&mut self) {
        self.running.store(true, Ordering::SeqCst);
        let storage_dir = self.storage_dir.clone();
        let sender = self.sender.clone();
        let running = self.running.clone();

        thread::Builder::new()
            .name("opencode-watcher".into())
            .spawn(move || {
                Self::run_loop(storage_dir, sender, running);
            })
            .expect("Failed to spawn OpenCode watcher thread");
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn run_loop(storage_dir: PathBuf, sender: Sender<AgentEvent>, running: Arc<AtomicBool>) {
        println!("[Watcher] OpenCode Three-Beat Rhythm Engine active: {:?}", storage_dir);

        let mut tracked_sessions: HashMap<String, OpenCodeSessionTrackInfo> = HashMap::new();

        while running.load(Ordering::SeqCst) {
            if storage_dir.exists() {
                let session_dir = storage_dir.join("session");
                let message_dir = storage_dir.join("message");

                let active_sessions = Self::scan_sessions(&session_dir);

                for (proj_id, sess_id, sess_file) in active_sessions {
                    let mod_time = sess_file.metadata().and_then(|m| m.modified()).ok();

                    let track = tracked_sessions.entry(sess_id.clone()).or_insert_with(|| {
                        let (proj_name, title) = Self::read_session_info(&sess_file, &proj_id);
                        OpenCodeSessionTrackInfo {
                            session_id: sess_id.clone(),
                            project_name: proj_name,
                            role_type: title,
                            agent_name: "OpenCode".into(),
                            last_mod_time: mod_time,
                            last_event_time: Instant::now() - Duration::from_secs(60),
                            is_working: false,
                            has_reported_done: true,
                            last_action_desc: "待命就绪".into(),
                            last_active_time: None,
                            step_count: 0,
                        }
                    });

                    // 检查该 session 目录下是否有新 message
                    let sess_msg_dir = message_dir.join(&sess_id);
                    if sess_msg_dir.exists() {
                        if let Ok(entries) = fs::read_dir(&sess_msg_dir) {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.is_file() {
                                    if let Ok(m_meta) = path.metadata() {
                                        if let Ok(m_mod) = m_meta.modified() {
                                            if Some(m_mod) > track.last_mod_time {
                                                track.last_mod_time = Some(m_mod);
                                                track.last_active_time = Some(Instant::now());
                                                track.step_count += 1;

                                                let (is_user, action_desc) = Self::parse_message_file(&path);

                                                if is_user {
                                                    // 1. 【节拍 1：启动轻弹】
                                                    track.is_working = true;
                                                    track.has_reported_done = false;
                                                    track.last_event_time = Instant::now();

                                                    println!("[Watcher] OpenCode TaskStarted: [{}] - {}", track.project_name, action_desc);
                                                    let _ = sender.send(AgentEvent::TaskStarted {
                                                        session_id: track.session_id.clone(),
                                                        project_name: track.project_name.clone(),
                                                        role_type: track.role_type.clone(),
                                                        agent_name: track.agent_name.clone(),
                                                        task_summary: action_desc,
                                                    });
                                                } else if track.is_working {
                                                    // 2. 【节拍 2：执行静默期】
                                                    let now = Instant::now();
                                                    if now.duration_since(track.last_event_time) >= Duration::from_millis(600) {
                                                        track.last_event_time = now;
                                                        let _ = sender.send(AgentEvent::Working {
                                                            session_id: track.session_id.clone(),
                                                            project_name: track.project_name.clone(),
                                                            role_type: track.role_type.clone(),
                                                            agent_name: track.agent_name.clone(),
                                                            task_summary: action_desc,
                                                            step_index: track.step_count,
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // 3. 【节拍 3：完成交付】
                    if track.is_working && !track.has_reported_done {
                        if let Some(act_time) = track.last_active_time {
                            if act_time.elapsed() >= Duration::from_millis(1000) {
                                track.is_working = false;
                                track.has_reported_done = true;
                                track.last_active_time = None;

                                let summary = format!("OpenCode 任务完成 (共 {} 步)", track.step_count);
                                let duration = track.last_event_time.elapsed().as_secs().max(1);

                                println!("[Watcher] OpenCode Task Done: [{}] - {}", track.project_name, summary);
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
            }

            thread::sleep(Duration::from_millis(300));
        }
    }

    fn scan_sessions(session_dir: &Path) -> Vec<(String, String, PathBuf)> {
        let mut list = Vec::new();
        if let Ok(proj_entries) = fs::read_dir(session_dir) {
            for proj_entry in proj_entries.flatten() {
                let proj_path = proj_entry.path();
                if proj_path.is_dir() {
                    let proj_id = proj_path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                    if let Ok(sess_entries) = fs::read_dir(&proj_path) {
                        for sess_entry in sess_entries.flatten() {
                            let p = sess_entry.path();
                            if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("json") {
                                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                                    list.push((proj_id.clone(), stem.to_string(), p));
                                }
                            }
                        }
                    }
                }
            }
        }
        list
    }

    pub fn map_subagent_role(agent_or_role: &str) -> String {
        let lower = agent_or_role.to_lowercase();
        if lower.contains("architect") || lower.contains("架构") {
            "子智能体 · 架构设计".into()
        } else if lower.contains("coder") || lower.contains("editor") || lower.contains("编码") || lower.contains("研发") {
            "子智能体 · 核心编码".into()
        } else if lower.contains("reviewer") || lower.contains("审查") || lower.contains("review") {
            "子智能体 · 代码审查".into()
        } else if lower.contains("tester") || lower.contains("测试") || lower.contains("qa") {
            "子智能体 · 单元测试".into()
        } else if !agent_or_role.trim().is_empty() {
            format!("子智能体 · {}", agent_or_role.trim())
        } else {
            "子智能体 · 协同处理".into()
        }
    }

    pub fn read_session_info(file_path: &Path, default_proj: &str) -> (String, String) {
        if let Ok(mut f) = File::open(file_path) {
            let mut s = String::new();
            if f.read_to_string(&mut s).is_ok() {
                if let Ok(val) = serde_json::from_str::<Value>(&s) {
                    let proj = val.get("projectName")
                        .or_else(|| val.get("project_name"))
                        .and_then(|p| p.as_str())
                        .unwrap_or(default_proj)
                        .to_string();

                    // 检测是否为子智能体 (parentId / parent_id 存在且非空)
                    let parent_id = val.get("parentId")
                        .or_else(|| val.get("parent_id"))
                        .and_then(|p| p.as_str())
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty());

                    let title = if parent_id.is_some() {
                        let agent_field = val.get("agent")
                            .or_else(|| val.get("role"))
                            .and_then(|a| a.as_str())
                            .unwrap_or("");
                        Self::map_subagent_role(agent_field)
                    } else {
                        val.get("title").and_then(|t| t.as_str()).unwrap_or("OpenCode 编程助手").to_string()
                    };

                    return (proj, title);
                }
            }
        }
        (default_proj.to_string(), "OpenCode 编程助手".into())
    }

    pub fn parse_message_file(path: &Path) -> (bool, String) {
        if let Ok(mut f) = File::open(path) {
            let mut s = String::new();
            if f.read_to_string(&mut s).is_ok() {
                if let Ok(val) = serde_json::from_str::<Value>(&s) {
                    let role = val.get("role").and_then(|r| r.as_str()).unwrap_or("");
                    let content = val.get("content").and_then(|c| c.as_str()).unwrap_or("");

                    if role == "user" {
                        let short = if content.chars().count() > 20 {
                            let s: String = content.chars().take(20).collect();
                            format!("{}...", s)
                        } else {
                            content.to_string()
                        };
                        return (true, format!("收到任务 · {short}"));
                    } else {
                        return (false, "OpenCode 正在编码处理...".into());
                    }
                }
            }
        }
        (false, "处理中...".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opencode_session_info_parse() {
        let temp_dir = std::env::temp_dir().join(format!("opencode_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        let file = temp_dir.join("test_session.json");

        let content = r#"{ "title": "优化数据库索引", "projectName": "BackendService" }"#;
        fs::write(&file, content).unwrap();

        let (proj, title) = OpenCodeWatcher::read_session_info(&file, "default");
        let _ = fs::remove_dir_all(&temp_dir);

        assert_eq!(proj, "BackendService");
        assert_eq!(title, "优化数据库索引");
    }

    #[test]
    fn test_opencode_subagent_roles() {
        let temp_dir = std::env::temp_dir().join(format!("opencode_sub_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);

        // 1. 架构师子智能体
        let f1 = temp_dir.join("sub_arch.json");
        fs::write(&f1, r#"{"parentId": "parent-123", "agent": "architect", "projectName": "WNoobar"}"#).unwrap();
        let (_, r1) = OpenCodeWatcher::read_session_info(&f1, "default");
        assert_eq!(r1, "子智能体 · 架构设计");

        // 2. 编码者子智能体
        let f2 = temp_dir.join("sub_coder.json");
        fs::write(&f2, r#"{"parent_id": "parent-123", "role": "coder", "projectName": "WNoobar"}"#).unwrap();
        let (_, r2) = OpenCodeWatcher::read_session_info(&f2, "default");
        assert_eq!(r2, "子智能体 · 核心编码");

        // 3. 审查者子智能体
        let f3 = temp_dir.join("sub_rev.json");
        fs::write(&f3, r#"{"parentId": "parent-123", "agent": "reviewer", "projectName": "WNoobar"}"#).unwrap();
        let (_, r3) = OpenCodeWatcher::read_session_info(&f3, "default");
        assert_eq!(r3, "子智能体 · 代码审查");

        // 4. 测试者子智能体
        let f4 = temp_dir.join("sub_test.json");
        fs::write(&f4, r#"{"parentId": "parent-123", "agent": "tester", "projectName": "WNoobar"}"#).unwrap();
        let (_, r4) = OpenCodeWatcher::read_session_info(&f4, "default");
        assert_eq!(r4, "子智能体 · 单元测试");

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
