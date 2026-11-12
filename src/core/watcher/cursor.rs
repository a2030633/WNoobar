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

pub struct CursorWatcher {
    cursor_user_dir: PathBuf,
    sender: Sender<AgentEvent>,
    running: Arc<AtomicBool>,
}

pub struct CursorSessionTrackInfo {
    pub workspace_id: String,
    pub project_name: String,
    pub role_type: String,
    pub agent_name: String,
    pub last_db_mod_time: Option<SystemTime>,
    pub last_event_time: Instant,
    pub is_generating: bool,
    pub is_working: bool,
    pub has_reported_done: bool,
    pub last_active_time: Option<Instant>,
    pub step_count: u64,
}

impl CursorWatcher {
    pub fn new(sender: Sender<AgentEvent>) -> Result<Self, String> {
        let app_data = std::env::var("APPDATA")
            .map_err(|e| format!("Could not read APPDATA: {e}"))?;
        let cursor_user_dir = PathBuf::from(app_data).join("Cursor").join("User");

        if !cursor_user_dir.exists() {
            return Err(format!("Cursor user directory not found at {:?}", cursor_user_dir));
        }

        Ok(Self {
            cursor_user_dir,
            sender,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn start(&mut self) {
        self.running.store(true, Ordering::SeqCst);
        let cursor_dir = self.cursor_user_dir.clone();
        let sender = self.sender.clone();
        let running = self.running.clone();

        thread::Builder::new()
            .name("cursor-watcher".into())
            .spawn(move || {
                Self::run_loop(cursor_dir, sender, running);
            })
            .expect("Failed to spawn Cursor watcher thread");
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn run_loop(cursor_dir: PathBuf, sender: Sender<AgentEvent>, running: Arc<AtomicBool>) {
        println!("[Watcher] Cursor Three-Beat Rhythm Engine active: {:?}", cursor_dir);

        let workspace_storage = cursor_dir.join("workspaceStorage");
        let global_storage = cursor_dir.join("globalStorage");

        let mut tracked_workspaces: HashMap<String, CursorSessionTrackInfo> = HashMap::new();

        while running.load(Ordering::SeqCst) {
            let active_targets = Self::scan_cursor_targets(&workspace_storage, &global_storage);

            for (target_id, db_path, project_name) in active_targets {
                let mod_time = db_path.metadata().and_then(|m| m.modified()).ok();

                let track = tracked_workspaces.entry(target_id.clone()).or_insert_with(|| {
                    CursorSessionTrackInfo {
                        workspace_id: target_id.clone(),
                        project_name: project_name.clone(),
                        role_type: "Cursor Composer".into(),
                        agent_name: "Cursor".into(),
                        last_db_mod_time: mod_time,
                        last_event_time: Instant::now() - Duration::from_secs(60),
                        is_generating: false,
                        is_working: false,
                        has_reported_done: true,
                        last_active_time: None,
                        step_count: 0,
                    }
                });

                if let Some(curr_time) = mod_time {
                    if Some(curr_time) != track.last_db_mod_time {
                        track.last_db_mod_time = Some(curr_time);

                        // 读取变更片段内容进行模式判断与智能体角色识别
                        let (is_gen, prompt_hint, role_hint) = Self::inspect_db_change(&db_path);

                        if is_gen {
                            if let Some(role) = role_hint {
                                track.role_type = role;
                            }
                            track.last_active_time = Some(Instant::now());
                            track.step_count += 1;

                            if !track.is_working {
                                // 1. 【节拍 1：启动轻弹】
                                track.is_working = true;
                                track.has_reported_done = false;
                                track.is_generating = true;
                                track.last_event_time = Instant::now();

                                let summary = prompt_hint.unwrap_or_else(|| "Composer 启动生成...".into());
                                println!("[Watcher] Cursor TaskStarted: [{}] - {}", track.project_name, summary);
                                let _ = sender.send(AgentEvent::TaskStarted {
                                    session_id: track.workspace_id.clone(),
                                    project_name: track.project_name.clone(),
                                    role_type: track.role_type.clone(),
                                    agent_name: track.agent_name.clone(),
                                    task_summary: summary,
                                });
                            } else {
                                // 2. 【节拍 2：执行静默期】
                                let now = Instant::now();
                                if now.duration_since(track.last_event_time) >= Duration::from_millis(600) {
                                    track.last_event_time = now;
                                    let _ = sender.send(AgentEvent::Working {
                                        session_id: track.workspace_id.clone(),
                                        project_name: track.project_name.clone(),
                                        role_type: track.role_type.clone(),
                                        agent_name: track.agent_name.clone(),
                                        task_summary: format!("正在生成代码 (第 {} 步)", track.step_count),
                                        step_index: track.step_count,
                                    });
                                }
                            }
                        }
                    }
                }

                // 3. 【节拍 3：完成交付 (停止活跃 1200ms 防抖收敛)】
                if track.is_working && !track.has_reported_done {
                    if let Some(act_time) = track.last_active_time {
                        if act_time.elapsed() >= Duration::from_millis(1200) {
                            track.is_working = false;
                            track.is_generating = false;
                            track.has_reported_done = true;
                            track.last_active_time = None;

                            let summary = format!("代码生成与修改完成 (共 {} 步)", track.step_count);
                            let duration = track.last_event_time.elapsed().as_secs().max(1);

                            println!("[Watcher] Cursor Task Done: [{}] - {}", track.project_name, summary);
                            let _ = sender.send(AgentEvent::Done {
                                session_id: track.workspace_id.clone(),
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

    fn scan_cursor_targets(ws_dir: &Path, global_dir: &Path) -> Vec<(String, PathBuf, String)> {
        let mut list = Vec::new();

        if ws_dir.exists() {
            if let Ok(entries) = fs::read_dir(ws_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        let id = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                        let ws_json = p.join("workspace.json");
                        let project_name = Self::detect_project_from_workspace_json(&ws_json)
                            .unwrap_or_else(|| "Cursor Workspace".into());

                        let db_file = p.join("state.vscdb-wal");
                        let final_db = if db_file.exists() { db_file } else { p.join("state.vscdb") };

                        if final_db.exists() {
                            list.push((id, final_db, project_name));
                        }
                    }
                }
            }
        }

        if global_dir.exists() {
            let wal = global_dir.join("state.vscdb-wal");
            let db = if wal.exists() { wal } else { global_dir.join("state.vscdb") };
            if db.exists() {
                list.push(("global".into(), db, "Cursor Global".into()));
            }
        }

        list
    }

    pub fn detect_project_from_workspace_json(path: &Path) -> Option<String> {
        if !path.exists() {
            return None;
        }
        if let Ok(bytes) = fs::read(path) {
            if let Ok(val) = serde_json::from_slice::<Value>(&bytes) {
                if let Some(folder) = val.get("folder").and_then(|f| f.as_str()) {
                    let decoded = folder.replace("%3A", ":").replace("%20", " ").replace('/', "\\");
                    let p = Path::new(&decoded);
                    if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                        if !name.is_empty() {
                            return Some(name.to_string());
                        }
                    }
                }
            }
        }
        None
    }

    pub fn inspect_db_change(path: &Path) -> (bool, Option<String>, Option<String>) {
        if let Ok(mut f) = File::open(path) {
            let mut buf = Vec::new();
            // 读取最新尾部 32KB 判断当前状态
            let len = f.metadata().map(|m| m.len()).unwrap_or(0);
            let read_start = len.saturating_sub(32768);
            if std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(read_start)).is_ok() {
                let _ = f.read_to_end(&mut buf);
                let text = String::from_utf8_lossy(&buf);

                let is_generating = text.contains("generating")
                    || text.contains("isGenerating\":true")
                    || text.contains("bubbleId")
                    || text.contains("composerData")
                    || text.contains("chat");

                if is_generating {
                    // 细分识别 Composer Agent 多分支/多文件模式
                    if text.contains("agentMode") || text.contains("\"mode\":\"agent\"") || text.contains("agent_run") {
                        return (true, Some("Composer Agent 正在规划并修改代码...".into()), Some("Composer Agent".into()));
                    }
                    if text.contains("multiFileEdit") || text.contains("multiFile") || text.contains("modifiedFiles") {
                        return (true, Some("Composer 正在多文件并发协同...".into()), Some("多文件协同".into()));
                    }
                    if text.contains("composerData") {
                        return (true, Some("Composer 正在生成代码...".into()), Some("Composer 智能体".into()));
                    }
                    return (true, Some("Composer 处理中...".into()), Some("Composer 协同".into()));
                }
            }
        }
        (false, None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_project_name_from_workspace_json() {
        let temp_dir = std::env::temp_dir().join(format!("cursor_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        let ws_file = temp_dir.join("workspace.json");

        let content = r#"{ "folder": "file:///c%3A/Users/WYF/Projects/MySuperApp" }"#;
        fs::write(&ws_file, content).unwrap();

        let detected = CursorWatcher::detect_project_from_workspace_json(&ws_file);
        let _ = fs::remove_dir_all(&temp_dir);

        assert_eq!(detected, Some("MySuperApp".into()));
    }

    #[test]
    fn test_cursor_composer_agent_mode() {
        let temp_dir = std::env::temp_dir().join(format!("cursor_db_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        let db_file = temp_dir.join("state.vscdb-wal");

        let agent_data = r#"{"composerData": {"agentMode": true, "generating": true}}"#;
        fs::write(&db_file, agent_data).unwrap();

        let (is_gen, hint, role) = CursorWatcher::inspect_db_change(&db_file);
        let _ = fs::remove_dir_all(&temp_dir);

        assert!(is_gen);
        assert_eq!(role, Some("Composer Agent".into()));
        assert!(hint.unwrap().contains("Composer Agent"));
    }
}
