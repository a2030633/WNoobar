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

pub struct CopilotWatcher {
    workspace_storage_dir: PathBuf,
    sender: Sender<AgentEvent>,
    running: Arc<AtomicBool>,
}

pub struct CopilotSessionTrackInfo {
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
    pub action_needed_pending: Option<String>,
}

impl CopilotWatcher {
    pub fn new(sender: Sender<AgentEvent>) -> Result<Self, String> {
        let appdata = std::env::var("APPDATA")
            .map_err(|e| format!("Could not read APPDATA: {e}"))?;
        let workspace_storage_dir = PathBuf::from(appdata)
            .join("Code")
            .join("User")
            .join("workspaceStorage");

        if !workspace_storage_dir.exists() {
            return Err(format!(
                "VS Code workspaceStorage directory not found at {:?}",
                workspace_storage_dir
            ));
        }

        Ok(Self {
            workspace_storage_dir,
            sender,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn start(&mut self) {
        self.running.store(true, Ordering::SeqCst);
        let storage_dir = self.workspace_storage_dir.clone();
        let sender = self.sender.clone();
        let running = self.running.clone();

        thread::Builder::new()
            .name("copilot-watcher".into())
            .spawn(move || {
                Self::run_loop(storage_dir, sender, running);
            })
            .expect("Failed to spawn Copilot watcher thread");
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn run_loop(storage_dir: PathBuf, sender: Sender<AgentEvent>, running: Arc<AtomicBool>) {
        println!("[Watcher] VS Code GitHub Copilot Rhythm Engine active: {:?}", storage_dir);

        let mut tracked_sessions: HashMap<String, CopilotSessionTrackInfo> = HashMap::new();

        while running.load(Ordering::SeqCst) {
            let active_files = Self::scan_active_sessions(&storage_dir);

            for (session_id, file_path, project_name) in active_files {
                let track = tracked_sessions.entry(session_id.clone()).or_insert_with(|| {
                    let len = file_path.metadata().map(|m| m.len()).unwrap_or(0);

                    CopilotSessionTrackInfo {
                        session_id: session_id.clone(),
                        project_name: project_name.clone(),
                        role_type: "Copilot Chat".into(),
                        agent_name: "GitHub Copilot".into(),
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
                        action_needed_pending: None,
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

                        // 1. 【节拍 1：启动轻弹】
                        if track.task_start_pending {
                            track.task_start_pending = false;
                            track.is_working = true;
                            track.has_reported_done = false;
                            track.last_model_finished_time = None;
                            track.last_event_time = Instant::now();

                            println!("[Watcher] Copilot TaskStarted: [{}] - {}", track.project_name, track.task_summary_pending);
                            let _ = sender.send(AgentEvent::TaskStarted {
                                session_id: track.session_id.clone(),
                                project_name: track.project_name.clone(),
                                role_type: track.role_type.clone(),
                                agent_name: track.agent_name.clone(),
                                task_summary: track.task_summary_pending.clone(),
                            });
                        } else if let Some(prompt) = track.action_needed_pending.take() {
                            // 1.5 【节拍 1.5：人机交互确认 (Action Needed)】
                            track.is_working = true;
                            track.has_reported_done = false;
                            track.last_model_finished_time = None;

                            println!("[Watcher] Copilot ActionNeeded: [{}] - {}", track.project_name, prompt);
                            let _ = sender.send(AgentEvent::ActionNeeded {
                                session_id: track.session_id.clone(),
                                project_name: track.project_name.clone(),
                                role_type: track.role_type.clone(),
                                agent_name: track.agent_name.clone(),
                                prompt,
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
                            format!("Copilot 交付完成 (共 {} 步)", track.last_step_index)
                        } else {
                            "Copilot 对话已完成".into()
                        };

                        let duration = track.last_event_time.elapsed().as_secs().max(1);
                        println!("[Watcher] Copilot Task Done: [{}] - {}", track.project_name, summary);
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

            thread::sleep(Duration::from_millis(200));
        }
    }

    fn scan_active_sessions(storage_dir: &Path) -> Vec<(String, PathBuf, String)> {
        let mut result = Vec::new();
        if let Ok(entries) = fs::read_dir(storage_dir) {
            for entry in entries.flatten() {
                let ws_folder = entry.path();
                if ws_folder.is_dir() {
                    let project_name = Self::detect_project_name(&ws_folder);
                    let chat_sessions_dir = ws_folder.join("chatSessions");
                    if chat_sessions_dir.is_dir() {
                        if let Ok(files) = fs::read_dir(&chat_sessions_dir) {
                            for f in files.flatten() {
                                let p = f.path();
                                if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                                        result.push((stem.to_string(), p, project_name.clone()));
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

    pub fn detect_project_name(ws_folder: &Path) -> String {
        let ws_json = ws_folder.join("workspace.json");
        if ws_json.is_file() {
            if let Ok(content) = fs::read_to_string(&ws_json) {
                if let Ok(val) = serde_json::from_str::<Value>(&content) {
                    if let Some(folder_uri) = val.get("folder").and_then(|f| f.as_str()) {
                        return Self::clean_uri_path(folder_uri);
                    }
                    if let Some(workspace_uri) = val.get("workspace").and_then(|w| w.as_str()) {
                        // 尝试从工作区文件内部读取首个工程路径
                        let ws_file_path = Self::uri_to_path(workspace_uri);
                        if let Ok(ws_file_content) = fs::read_to_string(&ws_file_path) {
                            if let Ok(ws_val) = serde_json::from_str::<Value>(&ws_file_content) {
                                if let Some(folders) = ws_val.get("folders").and_then(|f| f.as_array()) {
                                    if let Some(first) = folders.first() {
                                        if let Some(path_str) = first.get("path").and_then(|p| p.as_str()) {
                                            return Self::clean_uri_path(path_str);
                                        }
                                    }
                                }
                            }
                        }
                        return Self::clean_uri_path(workspace_uri);
                    }
                }
            }
        }

        // 默认回退到文件夹名或 VS Code Workspace
        ws_folder.file_name()
            .and_then(|n| n.to_str())
            .map(|s| if s.len() > 8 { format!("VSCode-{}", &s[..8]) } else { s.to_string() })
            .unwrap_or_else(|| "VS Code Project".into())
    }

    fn uri_to_path(uri: &str) -> PathBuf {
        let mut s = uri.trim();
        if let Some(stripped) = s.strip_prefix("file:///") {
            s = stripped;
        } else if let Some(stripped) = s.strip_prefix("file://") {
            s = stripped;
        }
        let decoded = urlencoding_decode(s);
        PathBuf::from(decoded)
    }

    pub fn clean_uri_path(uri_or_path: &str) -> String {
        let decoded = urlencoding_decode(uri_or_path);
        let p = Path::new(&decoded);
        if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
            let trimmed = name.trim_end_matches(".json").trim_end_matches(".code-workspace");
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        "VS Code Project".into()
    }

    pub fn parse_entry(val: &Value, track: &mut CopilotSessionTrackInfo) -> bool {
        let kind = val.get("kind").and_then(|k| k.as_i64()).unwrap_or(-1);

        // kind == 1: 设置元数据（customTitle, result, modelState）
        if kind == 1 {
            let k_arr = val.get("k").and_then(|k| k.as_array());

            if let Some(arr) = k_arr {
                // 1. 自定义标题
                let first_key = arr.first().and_then(|v| v.as_str()).unwrap_or("");
                if first_key == "customTitle" {
                    if let Some(title) = val.get("v").and_then(|v| v.as_str()) {
                        if !title.trim().is_empty() {
                            track.role_type = Self::truncate(title.trim(), 14);
                            return true;
                        }
                    }
                }

                // 2. 状态判定 (modelState 等待授权 / 完成)
                let has_model_state = arr.iter().any(|item| item.as_str() == Some("modelState"));
                if has_model_state {
                    if let Some(v_obj) = val.get("v") {
                        let state_value = v_obj.get("value").and_then(|val| val.as_i64());
                        let completed = v_obj.get("completedAt").is_some() || state_value == Some(1);

                        if state_value == Some(4) {
                            // 等待用户权限确认 (Waiting For Confirmation)
                            if track.action_needed_pending.is_none() {
                                track.action_needed_pending = Some("请求确认操作权限 · 请在 VS Code 中允许".into());
                            }
                            track.is_working = true;
                            track.has_reported_done = false;
                            track.last_model_finished_time = None;
                            return true;
                        } else if completed {
                            track.action_needed_pending = None;
                            track.has_reported_done = false;
                            track.is_working = true;
                            track.last_model_finished_time = Some(Instant::now());
                            return true;
                        } else if state_value == Some(0) {
                            // 恢复到正常生成状态，确认已通过
                            track.action_needed_pending = None;
                        }
                    }
                }

                // 3. 生成结果 (result metadata / toolCallRounds)
                let has_result = arr.iter().any(|item| item.as_str() == Some("result"));
                if has_result {
                    track.action_needed_pending = None;
                    if let Some(v_obj) = val.get("v") {
                        if let Some(rounds) = v_obj.get("toolCallRounds").and_then(|r| r.as_array()) {
                            for round in rounds {
                                if let Some(tools) = round.get("toolCalls").and_then(|t| t.as_array()) {
                                    if !tools.is_empty() {
                                        track.last_step_index += tools.len() as u64;
                                        track.last_action_desc = format!("执行工具调用 (共 {} 步)", track.last_step_index);
                                    }
                                }
                                if let Some(resp) = round.get("response").and_then(|r| r.as_str()) {
                                    if !resp.trim().is_empty() {
                                        track.final_summary_candidate = Self::truncate(resp.trim(), 40);
                                    }
                                }
                            }
                        }
                        // 收到 result 亦说明本次响应结束
                        track.last_model_finished_time = Some(Instant::now());
                        return true;
                    }
                }
            }
        }

        // kind == 2: 数组变更（requests 整体新增 或 requests[idx].response 增量推流）
        if kind == 2 {
            if let Some(arr) = val.get("k").and_then(|k| k.as_array()) {
                let first_key = arr.first().and_then(|v| v.as_str()).unwrap_or("");
                let has_response_key = arr.iter().any(|item| item.as_str() == Some("response"));

                // 分支 A: requests[idx].response 增量流式更新（包含工具调用与权限确认）
                if has_response_key {
                    if let Some(resp_arr) = val.get("v").and_then(|v| v.as_array()) {
                        for item in resp_arr {
                            // 检查工具调用权限申请
                            let is_tool_invocation = item.get("kind").and_then(|k| k.as_str()) == Some("toolInvocationSerialized");
                            if is_tool_invocation {
                                if let Some(confirm_type) = item.pointer("/isConfirmed/type").and_then(|t| t.as_i64()) {
                                    if confirm_type == 5 {
                                        // 等待用户权限确认 (Pending Confirmation)
                                        let msg_raw = item.pointer("/invocationMessage/value")
                                            .and_then(|m| m.as_str())
                                            .unwrap_or("");
                                        let prompt = Self::format_action_needed_prompt(msg_raw);
                                        track.action_needed_pending = Some(prompt);
                                        track.is_working = true;
                                        track.has_reported_done = false;
                                        track.last_model_finished_time = None;
                                        return true;
                                    } else if confirm_type == 4 || confirm_type == 1 {
                                        // 用户已点击允许/确认通过
                                        track.action_needed_pending = None;
                                    }
                                }
                            }

                            // 提取文本回复作为摘要候选
                            if let Some(val_str) = item.get("value").and_then(|v| v.as_str()) {
                                if !val_str.trim().is_empty() {
                                    track.final_summary_candidate = Self::truncate(val_str.trim(), 40);
                                }
                            }
                        }
                    }
                }

                // 分支 B: requests 数组新增（用户提问）
                if first_key == "requests" && !has_response_key {
                    if let Some(requests_arr) = val.get("v").and_then(|v| v.as_array()) {
                        if let Some(latest_req) = requests_arr.last() {
                            let mut user_msg = String::new();
                            if let Some(txt) = latest_req.pointer("/message/text").and_then(|t| t.as_str()) {
                                user_msg.push_str(txt);
                            }

                            if !user_msg.trim().is_empty() {
                                track.action_needed_pending = None; // 新指令清除上一轮权限请求
                                track.task_start_pending = true;
                                track.is_working = true;
                                track.has_reported_done = false;
                                track.last_model_finished_time = None;
                                track.final_summary_candidate.clear();
                                track.last_step_index = 0;
                                track.task_summary_pending = format!("收到任务 · {}", Self::truncate(user_msg.trim(), 20));
                                track.last_action_desc = "收到新指令 · 正在处理...".into();

                                // 检查 initial response 是否包含已存在的工具授权或文本
                                if let Some(resp_arr) = latest_req.get("response").and_then(|r| r.as_array()) {
                                    for r in resp_arr {
                                        if r.get("kind").and_then(|k| k.as_str()) == Some("toolInvocationSerialized") {
                                            if let Some(5) = r.pointer("/isConfirmed/type").and_then(|t| t.as_i64()) {
                                                let msg_raw = r.pointer("/invocationMessage/value").and_then(|m| m.as_str()).unwrap_or("");
                                                track.action_needed_pending = Some(Self::format_action_needed_prompt(msg_raw));
                                            }
                                        }
                                        if let Some(val_str) = r.get("value").and_then(|v| v.as_str()) {
                                            if !val_str.trim().is_empty() {
                                                track.final_summary_candidate = Self::truncate(val_str.trim(), 40);
                                            }
                                        }
                                    }
                                }

                                return true;
                            }
                        }
                    }
                }
            }
        }

        false
    }

    pub fn format_action_needed_prompt(msg: &str) -> String {
        let trimmed = msg.trim();
        if trimmed.is_empty() {
            return "请求确认操作权限 · 请在 VS Code 中允许".into();
        }

        if trimmed.contains("fetchWebPage") || trimmed.contains("提取") || trimmed.contains("http://") || trimmed.contains("https://") {
            if let Some(idx) = trimmed.find("http") {
                let url_part = &trimmed[idx..];
                let host = url_part.split(|c: char| c == '/' || c == '?' || c == '&')
                    .find(|part| part.contains('.'))
                    .unwrap_or("网页");
                return format!("权限确认 · 提取网页 ({})", host);
            }
            return "权限确认 · 提取网页数据".into();
        }

        if trimmed.starts_with("正在") {
            let action = &trimmed["正在".len()..];
            return format!("权限确认 · {}", Self::truncate(action.trim(), 20));
        }

        format!("权限确认 · {}", Self::truncate(trimmed, 24))
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

fn urlencoding_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let h1 = bytes.next().unwrap_or(0);
            let h2 = bytes.next().unwrap_or(0);
            if let Ok(hex_str) = std::str::from_utf8(&[h1, h2]) {
                if let Ok(byte_val) = u8::from_str_radix(hex_str, 16) {
                    result.push(byte_val as char);
                    continue;
                }
            }
            result.push('%');
            result.push(h1 as char);
            result.push(h2 as char);
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_copilot_clean_uri_path() {
        assert_eq!(CopilotWatcher::clean_uri_path("file:///f%3A/%E9%A1%B9%E7%9B%AE/WNoobar"), "WNoobar");
        assert_eq!(CopilotWatcher::clean_uri_path("file:///f:/项目/agent-go-proxy"), "agent-go-proxy");
        assert_eq!(CopilotWatcher::clean_uri_path("F:/项目/QQ-BOT"), "QQ-BOT");
    }

    #[test]
    fn test_copilot_parse_user_request() {
        let mut track = CopilotSessionTrackInfo {
            session_id: "copilot-sess-1".into(),
            project_name: "WNoobar".into(),
            role_type: "Copilot Chat".into(),
            agent_name: "GitHub Copilot".into(),
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
            action_needed_pending: None,
        };

        let req_entry = json!({
            "kind": 2,
            "k": ["requests"],
            "v": [
                {
                    "requestId": "req-1",
                    "message": { "text": "重构 Rust 消息总线并优化响应延迟" },
                    "response": [
                        { "value": "好的，我来帮你重构 Rust 消息总线..." }
                    ]
                }
            ]
        });

        assert!(CopilotWatcher::parse_entry(&req_entry, &mut track));
        assert!(track.task_start_pending);
        assert!(track.is_working);
        assert!(track.task_summary_pending.contains("重构 Rust 消息总线"));
    }

    #[test]
    fn test_copilot_parse_model_state_completed() {
        let mut track = CopilotSessionTrackInfo {
            session_id: "copilot-sess-1".into(),
            project_name: "WNoobar".into(),
            role_type: "Copilot Chat".into(),
            agent_name: "GitHub Copilot".into(),
            file_offset: 0,
            last_event_time: Instant::now(),
            last_step_index: 0,
            last_action_desc: "执行中".into(),
            is_working: true,
            has_reported_done: false,
            task_start_pending: false,
            task_summary_pending: "".into(),
            last_model_finished_time: None,
            final_summary_candidate: "已完成消息总线优化".into(),
            action_needed_pending: None,
        };

        let state_entry = json!({
            "kind": 1,
            "k": ["requests", 0, "modelState"],
            "v": {
                "value": 1,
                "completedAt": 1794386310900i64
            }
        });

        assert!(CopilotWatcher::parse_entry(&state_entry, &mut track));
        assert!(track.last_model_finished_time.is_some());
    }

    #[test]
    fn test_copilot_action_needed_permission_request_and_confirmed() {
        let mut track = CopilotSessionTrackInfo {
            session_id: "copilot-weather-1".into(),
            project_name: "agent-go-proxy".into(),
            role_type: "Copilot Chat".into(),
            agent_name: "GitHub Copilot".into(),
            file_offset: 0,
            last_event_time: Instant::now(),
            last_step_index: 0,
            last_action_desc: "执行中".into(),
            is_working: true,
            has_reported_done: false,
            task_start_pending: false,
            task_summary_pending: "".into(),
            last_model_finished_time: None,
            final_summary_candidate: "".into(),
            action_needed_pending: None,
        };

        // 1. 模拟 Copilot 处于 modelState.value == 4 等待授权交互
        let state_waiting = json!({
            "kind": 1,
            "k": ["requests", 3, "modelState"],
            "v": { "value": 4 }
        });
        assert!(CopilotWatcher::parse_entry(&state_waiting, &mut track));
        assert!(track.action_needed_pending.is_some());

        // 2. 模拟收到包含 toolInvocationSerialized 与 isConfirmed.type == 5 的具体权限请求
        let tool_req = json!({
            "kind": 2,
            "k": ["requests", 3, "response"],
            "v": [{
                "kind": "toolInvocationSerialized",
                "invocationMessage": {
                    "value": "正在提取 https://api.open-meteo.com/v1/forecast?latitude=30.4217&longitude=120.2986"
                },
                "isConfirmed": { "type": 5 }
            }]
        });
        assert!(CopilotWatcher::parse_entry(&tool_req, &mut track));
        assert!(track.action_needed_pending.is_some());
        let prompt = track.action_needed_pending.as_deref().unwrap();
        assert!(prompt.contains("api.open-meteo.com"));
        assert!(prompt.contains("权限确认"));

        // 3. 用户在 VS Code 中点击“允许”：isConfirmed.type 变为 4
        let confirmed_event = json!({
            "kind": 2,
            "k": ["requests", 3, "response"],
            "v": [{
                "kind": "toolInvocationSerialized",
                "isConfirmed": { "type": 4 }
            }]
        });
        CopilotWatcher::parse_entry(&confirmed_event, &mut track);
        assert!(track.action_needed_pending.is_none());

        // 4. 测试后续收到完成状态
        let completed_event = json!({
            "kind": 1,
            "k": ["requests", 3, "modelState"],
            "v": { "value": 1, "completedAt": 1794464119442i64 }
        });
        assert!(CopilotWatcher::parse_entry(&completed_event, &mut track));
        assert!(track.action_needed_pending.is_none());
        assert!(track.last_model_finished_time.is_some());
    }

    #[test]
    fn test_format_action_needed_prompt() {
        let p1 = CopilotWatcher::format_action_needed_prompt(
            "正在提取 https://api.open-meteo.com/v1/forecast?latitude=30.4217"
        );
        assert_eq!(p1, "权限确认 · 提取网页 (api.open-meteo.com)");

        let p2 = CopilotWatcher::format_action_needed_prompt("正在运行终端命令 cargo build");
        assert!(p2.contains("权限确认 · 运行终端命令 cargo build"));

        let p3 = CopilotWatcher::format_action_needed_prompt("");
        assert_eq!(p3, "请求确认操作权限 · 请在 VS Code 中允许");
    }
}
