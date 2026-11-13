#![allow(dead_code)]
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use serde_json::Value;

use crate::core::bus::{AgentEvent, NoiseFilterLevel};
use crate::core::config::SharedNoiseFilter;
use crate::core::swarm::SwarmWorkerStatus;

pub struct AntigravityWatcher {
    brain_dir: PathBuf,
    sender: Sender<AgentEvent>,
    running: Arc<AtomicBool>,
    filter_level: NoiseFilterLevel,
    shared_filter: SharedNoiseFilter,
}

struct SessionTrackInfo {
    session_id: String,
    project_name: String,
    role_type: String,
    agent_name: String,
    is_subagent: bool,
    parent_session_id: Option<String>,
    worker_name: String,
    is_waiting: bool,
    waiting_pending: bool,
    last_emitted_subagent_status: Option<SwarmWorkerStatus>,
    file_offset: u64,
    last_event_time: Instant,
    last_activity_time: Instant,
    last_step_index: u64,
    last_action_desc: String,
    is_working: bool,
    has_reported_done: bool,
    last_model_finished_time: Option<Instant>,
    final_summary_candidate: String,
    task_start_pending: bool,
    action_needed_pending: Option<String>,
    is_initial_catchup: bool,
}

impl SessionTrackInfo {
    fn group_key(&self) -> String {
        self.parent_session_id
            .clone()
            .unwrap_or_else(|| format!("{}::{}", self.agent_name, self.project_name))
    }

    fn resolved_worker_name(&self) -> String {
        if !self.worker_name.trim().is_empty() {
            self.worker_name.clone()
        } else {
            AntigravityWatcher::clean_role_name(&self.role_type)
        }
    }
}

impl AntigravityWatcher {
    pub fn new(sender: Sender<AgentEvent>) -> Result<Self, String> {
        let user_profile = std::env::var("USERPROFILE")
            .map_err(|e| format!("Could not read USERPROFILE: {e}"))?;
        let brain_dir = PathBuf::from(user_profile)
            .join(".gemini")
            .join("antigravity")
            .join("brain");

        Ok(Self {
            brain_dir,
            sender,
            running: Arc::new(AtomicBool::new(false)),
            filter_level: NoiseFilterLevel::Normal,
            shared_filter: SharedNoiseFilter::new(NoiseFilterLevel::Normal),
        })
    }

    pub fn set_filter_level(&mut self, level: NoiseFilterLevel) {
        self.filter_level = level;
        self.shared_filter.set(level);
    }

    pub fn set_shared_filter(&mut self, filter: SharedNoiseFilter) {
        self.filter_level = filter.get();
        self.shared_filter = filter;
    }

    pub fn start(&mut self) {
        self.running.store(true, Ordering::SeqCst);
        let brain_dir = self.brain_dir.clone();
        let sender = self.sender.clone();
        let running = self.running.clone();
        let shared_filter = self.shared_filter.clone();

        thread::Builder::new()
            .name("antigravity-watcher".into())
            .spawn(move || {
                Self::run_loop(brain_dir, sender, running, shared_filter);
            })
            .expect("Failed to spawn Antigravity watcher thread");
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn should_emit_working(
        filter_level: NoiseFilterLevel,
        last_event_time: Instant,
        now: Instant,
    ) -> bool {
        match filter_level {
            NoiseFilterLevel::Silent => false,
            NoiseFilterLevel::Verbose => true,
            NoiseFilterLevel::Normal => {
                now.saturating_duration_since(last_event_time) >= Duration::from_millis(600)
            }
        }
    }

    /// 子智能体 SubagentStatus 心跳节流：不受 Silent 完全静音拦截（>= 600ms 节流），
    /// 保证长时子代理在 Silent 模式下仍可在 Swarm 雷达中保活。
    fn should_emit_subagent_heartbeat(
        filter_level: NoiseFilterLevel,
        last_event_time: Instant,
        now: Instant,
    ) -> bool {
        match filter_level {
            NoiseFilterLevel::Verbose => true,
            NoiseFilterLevel::Normal | NoiseFilterLevel::Silent => {
                now.saturating_duration_since(last_event_time) >= Duration::from_millis(600)
            }
        }
    }

    fn run_loop(
        brain_dir: PathBuf,
        sender: Sender<AgentEvent>,
        running: Arc<AtomicBool>,
        shared_filter: SharedNoiseFilter,
    ) {
        Self::run_loop_internal(brain_dir, sender, running, shared_filter, None);
    }

    fn emit_subagent_status(
        sender: &Sender<AgentEvent>,
        track: &mut SessionTrackInfo,
        status: SwarmWorkerStatus,
        task_summary: String,
    ) {
        track.last_emitted_subagent_status = Some(status);
        let _ = sender.send(AgentEvent::SubagentStatus {
            group_key: track.group_key(),
            parent_session_id: track.parent_session_id.clone(),
            worker_id: track.session_id.clone(),
            worker_name: track.resolved_worker_name(),
            project_name: track.project_name.clone(),
            agent_name: track.agent_name.clone(),
            status,
            task_summary,
            step_index: track.last_step_index,
        });
    }

    fn run_loop_internal(
        brain_dir: PathBuf,
        sender: Sender<AgentEvent>,
        running: Arc<AtomicBool>,
        shared_filter: SharedNoiseFilter,
        tick_counter: Option<&AtomicUsize>,
    ) {
        println!("[Watcher] Antigravity Three-Beat Rhythm Engine active: {:?}", brain_dir);

        let mut tracked_sessions: HashMap<String, SessionTrackInfo> = HashMap::new();

        while running.load(Ordering::SeqCst) {
            // 扫描活跃会话
            let recent_dirs = Self::scan_active_sessions(&brain_dir);

            for (session_id, session_path) in recent_dirs {
                let log_file = session_path.join(".system_generated").join("logs").join("transcript.jsonl");
                if !log_file.exists() {
                    continue;
                }

                let track = tracked_sessions.entry(session_id.clone()).or_insert_with(|| {
                    let (project_name, role_type) = Self::detect_project_and_role(&session_path);
                    let len = log_file.metadata().map(|m| m.len()).unwrap_or(0);
                    let is_subagent = Self::is_subagent_session(&session_path, &session_id);
                    let (parent_session_id, worker_name) = if is_subagent {
                        Self::detect_subagent_metadata(&session_path, &role_type)
                    } else {
                        (None, String::new())
                    };
                    let is_recent = log_file.metadata().ok().and_then(|m| m.modified().ok())
                        .and_then(|t| t.elapsed().ok())
                        .map_or(false, |el| el < Duration::from_secs(120));

                    let file_offset = if is_subagent && is_recent {
                        if len <= 32 * 1024 {
                            0
                        } else {
                            len.saturating_sub(16 * 1024)
                        }
                    } else {
                        len
                    };

                    SessionTrackInfo {
                        session_id: session_id.clone(),
                        project_name,
                        role_type,
                        agent_name: "Antigravity".into(),
                        is_subagent,
                        parent_session_id,
                        worker_name,
                        is_waiting: false,
                        waiting_pending: false,
                        last_emitted_subagent_status: None,
                        file_offset,
                        last_event_time: Instant::now() - Duration::from_secs(60),
                        last_activity_time: Instant::now(),
                        last_step_index: 0,
                        last_action_desc: "待命就绪".into(),
                        is_working: false,
                        has_reported_done: true, // 初始历史文件不弹完成
                        last_model_finished_time: None,
                        final_summary_candidate: String::new(),
                        task_start_pending: false,
                        action_needed_pending: None,
                        is_initial_catchup: true,
                    }
                });

                // 读取日志新行
                if let Ok(mut file) = File::open(&log_file) {
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
                        if step_updated {
                            track.last_activity_time = Instant::now();
                        }

                        // 冷启动回放历史日志时的防误触发保护：
                        // 如果初次解析时日志末尾已处于完成状态（或既非工作也非等待/待确认），直接归为已完结，严禁误弹 TaskStarted！
                        if track.is_initial_catchup {
                            track.is_initial_catchup = false;
                            if track.last_model_finished_time.is_some()
                                || (!track.is_working
                                    && !track.is_waiting
                                    && track.action_needed_pending.is_none())
                            {
                                track.task_start_pending = false;
                                track.waiting_pending = false;
                                track.is_working = false;
                                track.is_waiting = false;
                                track.has_reported_done = true;
                                track.last_model_finished_time = None;
                                track.final_summary_candidate.clear();
                            }
                        }

                        // 同步用户在 annotations pbtxt 中重命名的最新会话名称
                        if let Some(custom_title) = Self::read_session_custom_title(&session_path, &session_id) {
                            track.role_type = custom_title;
                        }

                        if track.is_subagent {
                            // 子智能体统一归并为 AgentEvent::SubagentStatus，严禁再发独立 TaskStarted/Working/ActionNeeded/Done 污染主会话池
                            if let Some(prompt) = track.action_needed_pending.take() {
                                track.task_start_pending = false;
                                track.waiting_pending = false;
                                track.is_working = true;
                                track.is_waiting = false;
                                track.has_reported_done = false;
                                track.last_model_finished_time = None;
                                track.final_summary_candidate.clear();
                                Self::emit_subagent_status(
                                    &sender,
                                    track,
                                    SwarmWorkerStatus::ActionNeeded,
                                    prompt,
                                );
                            } else if track.waiting_pending {
                                track.waiting_pending = false;
                                track.task_start_pending = false;
                                track.is_working = false;
                                track.is_waiting = true;
                                track.has_reported_done = false;
                                track.last_model_finished_time = None;
                                track.last_event_time = Instant::now();
                                let desc = track.last_action_desc.clone();
                                Self::emit_subagent_status(
                                    &sender,
                                    track,
                                    SwarmWorkerStatus::Waiting,
                                    desc,
                                );
                            } else if track.task_start_pending {
                                track.task_start_pending = false;
                                track.is_working = true;
                                track.is_waiting = false;
                                track.has_reported_done = false;
                                if track.last_model_finished_time.is_none() {
                                    track.last_event_time = Instant::now();
                                }
                                let desc = track.last_action_desc.clone();
                                Self::emit_subagent_status(
                                    &sender,
                                    track,
                                    SwarmWorkerStatus::Working,
                                    desc,
                                );
                            } else if step_updated && track.is_working {
                                let now = Instant::now();
                                let status_changed =
                                    track.last_emitted_subagent_status != Some(SwarmWorkerStatus::Working);
                                let current_filter = shared_filter.get();
                                if status_changed
                                    || Self::should_emit_subagent_heartbeat(current_filter, track.last_event_time, now)
                                {
                                    track.last_event_time = now;
                                    let desc = track.last_action_desc.clone();
                                    Self::emit_subagent_status(
                                        &sender,
                                        track,
                                        SwarmWorkerStatus::Working,
                                        desc,
                                    );
                                }
                            }
                        } else {
                            // 1. 【启动节拍 (Start Pop)】：收到新任务，主动弹一下（1.8s后自动贴边隐藏）
                            if track.task_start_pending {
                                track.task_start_pending = false;
                                track.is_working = true;
                                track.has_reported_done = false;
                                // 关键保护：若同批次中已产生 last_model_finished_time，绝不抹除！
                                if track.last_model_finished_time.is_none() {
                                    track.last_event_time = Instant::now();
                                }

                                println!("[Watcher] TaskStarted: [{}] - {}", track.project_name, track.last_action_desc);
                                let _ = sender.send(AgentEvent::TaskStarted {
                                    session_id: track.session_id.clone(),
                                    project_name: track.project_name.clone(),
                                    role_type: track.role_type.clone(),
                                    agent_name: track.agent_name.clone(),
                                    task_summary: "收到任务 · 开始处理...".into(),
                                });
                            } else if let Some(prompt) = track.action_needed_pending.take() {
                                // 1.5 【人机交互确认 (Action Needed)】：遇到 ask_question 或待授权，主动弹窗警示！
                                track.is_working = true;
                                track.has_reported_done = false;
                                track.last_model_finished_time = None;
                                track.final_summary_candidate.clear();
                                println!("[Watcher] ActionNeeded: [{}] - {}", track.project_name, prompt);
                                let _ = sender.send(AgentEvent::ActionNeeded {
                                    session_id: track.session_id.clone(),
                                    project_name: track.project_name.clone(),
                                    role_type: track.role_type.clone(),
                                    agent_name: track.agent_name.clone(),
                                    prompt,
                                });
                            } else if step_updated && track.is_working {
                                // 2. 【执行静默期】：仅在后台更新进度，绝不弹窗打扰用户
                                let now = Instant::now();
                                let current_filter = shared_filter.get();
                                let should_emit = Self::should_emit_working(
                                    current_filter,
                                    track.last_event_time,
                                    now,
                                );

                                if should_emit {
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
                    }
                }

                // 3. 【完成交付节拍 (Completion Pop)】：确定性收敛触发 Done 大卡片（子智能体发射 SubagentStatus::Done）
                if track.is_working && !track.has_reported_done {
                    if let Some(finish_time) = track.last_model_finished_time {
                        // 模型回答完毕后防抖 800ms 确认完结
                        if finish_time.elapsed() >= Duration::from_millis(800) {
                            track.is_working = false;
                            track.has_reported_done = true;
                            track.last_model_finished_time = None;

                            let summary = if !track.final_summary_candidate.is_empty() {
                                track.final_summary_candidate.clone()
                            } else {
                                format!("任务交付完成 (共 {} 步)", track.last_step_index)
                            };
                            track.final_summary_candidate.clear();

                            if track.is_subagent {
                                Self::emit_subagent_status(
                                    &sender,
                                    track,
                                    SwarmWorkerStatus::Done,
                                    summary,
                                );
                            } else {
                                let artifacts = Self::count_artifacts(&session_path);
                                let duration = track.last_event_time.elapsed().as_secs().max(1);

                                println!("[Watcher] Task Done Delivered: [{}] - {}", track.project_name, summary);
                                let _ = sender.send(AgentEvent::Done {
                                    session_id: track.session_id.clone(),
                                    project_name: track.project_name.clone(),
                                    role_type: track.role_type.clone(),
                                    agent_name: track.agent_name.clone(),
                                    summary,
                                    duration_secs: duration,
                                    artifacts_count: artifacts,
                                });
                            }
                        }
                    }
                }

                // 3.5 【长休眠会话静默回收安全兜底】：会话处于 is_working 且超过 15 分钟无任何活动，静默复位，严禁向用户误报 Done！
                if track.is_working && !track.has_reported_done && track.action_needed_pending.is_none() {
                    if track.last_activity_time.elapsed() >= Duration::from_secs(900) {
                        track.is_working = false;
                        track.has_reported_done = true;
                        track.last_model_finished_time = None;
                        track.final_summary_candidate.clear();
                        println!("[Watcher] Inactive session silently retired: [{}]", track.project_name);
                    }
                }
            }

            if let Some(counter) = tick_counter {
                counter.fetch_add(1, Ordering::SeqCst);
            }

            thread::sleep(Duration::from_millis(180));
        }
    }

    fn scan_active_sessions(brain_dir: &Path) -> Vec<(String, PathBuf)> {
        let mut result = Vec::new();
        if !brain_dir.exists() {
            return result;
        }

        if let Ok(entries) = fs::read_dir(brain_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let log_file = path.join(".system_generated").join("logs").join("transcript.jsonl");
                    if log_file.exists() {
                        if let Ok(meta) = log_file.metadata() {
                            if let Ok(mod_time) = meta.modified() {
                                if let Ok(elapsed) = mod_time.elapsed() {
                                    if elapsed < Duration::from_secs(900) {
                                        if let Some(id) = path.file_name().and_then(|n| n.to_str()) {
                                            result.push((id.to_string(), path));
                                        }
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

    /// 判定是否为由 invoke_subagent 或 swarm 创建的子智能体会话（无 annotations pbtxt 且含有 messages 或起始指派标记）
    fn is_subagent_session(session_path: &Path, session_id: &str) -> bool {
        let annotations_dir = session_path.parent().and_then(|p| p.parent()).map(|p| p.join("annotations"));
        let has_pbtxt = annotations_dir.map(|d| d.join(format!("{}.pbtxt", session_id)).exists()).unwrap_or(false);
        if has_pbtxt {
            return false;
        }

        let messages_dir = session_path.join(".system_generated").join("messages");
        if messages_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(&messages_dir) {
                if entries.flatten().any(|e| {
                    e.path().extension().and_then(|s| s.to_str()) == Some("json")
                }) {
                    return true;
                }
            }
        }

        let log_file = session_path
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl");
        if let Ok(file) = File::open(&log_file) {
            let reader = BufReader::new(file);
            let mut saw_subagent_marker = false;
            for line in reader.lines().take(15).flatten() {
                if let Ok(val) = serde_json::from_str::<Value>(line.trim()) {
                    let source = val.get("source").and_then(|s| s.as_str()).unwrap_or("");
                    if source == "USER_EXPLICIT" {
                        return false;
                    }
                    if source == "SYSTEM" {
                        if let Some(content) = val.get("content").and_then(|c| c.as_str()) {
                            if content.contains("You have just started.")
                                || (content.contains("[Message]") && content.contains("for @"))
                            {
                                saw_subagent_marker = true;
                            }
                        }
                    }
                }
            }
            return saw_subagent_marker;
        }

        false
    }

    fn extract_sender_uuid(content: &str) -> Option<String> {
        if content.contains("/task-") {
            return None;
        }
        let idx = content.find("sender=")?;
        let after = &content[idx + 7..];
        let token: String = after
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '<' && *c != '>' && *c != '"' && *c != '\'')
            .collect();
        if token.is_empty() || token.contains('/') {
            None
        } else {
            Some(token)
        }
    }

    fn extract_worker_name(content: &str) -> Option<String> {
        let mut search_from = 0;
        while let Some(rel_idx) = content[search_from..].find("for @") {
            let start = search_from + rel_idx + 5;
            let rest = &content[start..];
            let name: String = rest
                .chars()
                .take_while(|c| {
                    !c.is_whitespace()
                        && *c != ')'
                        && *c != ':'
                        && *c != '：'
                        && *c != ','
                        && *c != '\n'
                        && *c != '\r'
                })
                .collect();
            if !name.is_empty() {
                let after_name = &rest[name.len()..];
                if after_name.starts_with("):") || after_name.starts_with(')') {
                    return Some(format!("@{}", name));
                }
            }
            search_from = start;
        }
        if let Some(idx) = content.find("for @") {
            let rest = &content[idx + 5..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if !name.is_empty() {
                return Some(format!("@{}", name));
            }
        }
        None
    }

    /// 从 subagent transcript.jsonl 前几行解析 parent_session_id 与 @worker_name
    fn detect_subagent_metadata(session_path: &Path, fallback_role: &str) -> (Option<String>, String) {
        let mut priority_parent: Option<String> = None;
        let mut fallback_parent: Option<String> = None;
        let mut worker_name: Option<String> = None;

        let log_file = session_path
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl");
        if let Ok(file) = File::open(&log_file) {
            let reader = BufReader::new(file);
            for line in reader.lines().take(30).flatten() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                    if let Some(content) = val.get("content").and_then(|c| c.as_str()) {
                        if priority_parent.is_none() && content.contains("You have just started.") {
                            priority_parent = Self::extract_sender_uuid(content);
                        } else if fallback_parent.is_none() && content.contains("[Message]") {
                            fallback_parent = Self::extract_sender_uuid(content);
                        }
                        if worker_name.is_none() {
                            worker_name = Self::extract_worker_name(content);
                        }
                        if priority_parent.is_some() && worker_name.is_some() {
                            break;
                        }
                    }
                }
            }
        }

        let parent_id = priority_parent.or(fallback_parent);
        let resolved_name = worker_name.unwrap_or_else(|| Self::clean_role_name(fallback_role));
        (parent_id, resolved_name)
    }

    /// 从 annotations/<session_id>.pbtxt 中提取用户自定义的真实会话名称
    fn read_session_custom_title(session_path: &Path, session_id: &str) -> Option<String> {
        let annotations_dir = session_path.parent().and_then(|p| p.parent()).map(|p| p.join("annotations"));
        let pbtxt_path = annotations_dir.map(|d| d.join(format!("{}.pbtxt", session_id)));

        if let Some(path) = pbtxt_path {
            if path.exists() {
                if let Ok(bytes) = fs::read(&path) {
                    if let Ok(content) = String::from_utf8(bytes) {
                        if let Some(idx) = content.find("title:\"") {
                            let remain = &content[idx + 7..];
                            if let Some(end_idx) = remain.find('"') {
                                let title = &remain[..end_idx];
                                if !title.trim().is_empty() {
                                    return Some(title.trim().to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// 动态探测真实项目名与角色名（彻底干掉硬编码）
    fn detect_project_and_role(session_path: &Path) -> (String, String) {
        let session_id = session_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let mut detected_project: Option<String> = None;
        let mut detected_role: Option<String> = Self::read_session_custom_title(session_path, session_id);

        let log_file = session_path.join(".system_generated").join("logs").join("transcript.jsonl");
        if log_file.exists() {
            if let Ok(file) = File::open(&log_file) {
                let reader = BufReader::new(file);
                for line in reader.lines().take(60).flatten() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    if detected_project.is_none() {
                        if let Some(pos) = trimmed.find("Active Workspaces:")
                            .or_else(|| trimmed.find("workspaceUris"))
                            .or_else(|| trimmed.find("工作区"))
                            .or_else(|| trimmed.find("Command Working Directory:"))
                        {
                            let sub = &trimmed[pos..];
                            if let Some(proj) = Self::extract_project_from_text(sub) {
                                detected_project = Some(proj);
                            }
                        }
                    }

                    if detected_role.is_none() {
                        if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                            if val.get("source").and_then(|s| s.as_str()) == Some("USER_EXPLICIT") {
                                if let Some(content) = val.get("content").and_then(|c| c.as_str()) {
                                    detected_role = Some(Self::extract_user_intent(content));
                                }
                            } else if let Some(role) = val.get("role").and_then(|r| r.as_str()) {
                                detected_role = Some(Self::clean_role_name(role));
                            }
                        }
                    }
                }
            }
        }

        let project_name = detected_project.unwrap_or_else(|| "WNoobar".into());

        let role_type = detected_role.unwrap_or_else(|| "AI 结对助手".into());

        (project_name, role_type)
    }

    fn extract_project_from_text(text: &str) -> Option<String> {
        for part in text.split(|c: char| c == ' ' || c == '"' || c == '\'' || c == '\n' || c == '`') {
            if part.contains(":\\") || part.contains(":/") || part.contains("%3A") {
                let decoded = part.replace("%3A", ":").replace("%2F", "/").replace("%20", " ");
                let clean = decoded.trim_matches(|c| c == '[' || c == ']' || c == '(' || c == ')' || c == ',');
                let p = Path::new(clean);
                if let Some(file_name) = p.file_name().and_then(|n| n.to_str()) {
                    if !file_name.is_empty() && file_name != "." {
                        return Some(file_name.to_string());
                    }
                }
            }
        }
        None
    }

    fn extract_user_intent(content: &str) -> String {
        let clean = content.replace("<USER_REQUEST>", "")
            .replace("</USER_REQUEST>", "")
            .trim()
            .to_string();

        let first_line = clean.lines().next().unwrap_or("用户咨询").trim();
        if first_line.chars().count() > 8 {
            let res: String = first_line.chars().take(8).collect();
            format!("{}...", res)
        } else if !first_line.is_empty() {
            first_line.to_string()
        } else {
            "咨询互动".into()
        }
    }

    fn clean_role_name(role: &str) -> String {
        let r = role.trim_matches('"');
        if r.contains("Lead") || r.contains("Engineer") {
            "系统研发".into()
        } else if r.contains("Product") || r.contains("PM") {
            "产品设计".into()
        } else {
            r.to_string()
        }
    }

    fn parse_entry(val: &Value, track: &mut SessionTrackInfo) -> bool {
        let step_index = val.get("step_index").and_then(|v| v.as_u64()).unwrap_or(track.last_step_index);
        track.last_step_index = step_index;

        let source = val.get("source").and_then(|s| s.as_str()).unwrap_or("");
        let entry_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let status = val.get("status").and_then(|s| s.as_str()).unwrap_or("");

        // 0. 工具返回结果 / 用户问答选择响应 / 后台命令完成通知 (task-xxx)
        let is_tool_response = entry_type == "GENERIC"
            || entry_type == "TOOL_RESPONSE"
            || source == "TOOL"
            || (source == "SYSTEM" && entry_type == "SYSTEM_MESSAGE" && val.get("content").and_then(|c| c.as_str()).map_or(false, |s| s.contains("/task-")));

        if is_tool_response {
            // 工具返回结果（或用户跳过/选择了选项、或后台命令执行完毕），立即清除 action_needed_pending
            track.action_needed_pending = None;
            return false;
        }

        // 1. 用户提问或子智能体新任务指派：标记新任务启动！
        let is_user_input = source == "USER_EXPLICIT" || entry_type == "USER_INPUT";
        let is_subagent_task = if source == "SYSTEM" && entry_type == "SYSTEM_MESSAGE" {
            if let Some(content) = val.get("content").and_then(|c| c.as_str()) {
                // 必须包含 [Message]，且绝不能是后台命令完成通知
                content.contains("[Message]") && !content.contains("/task-")
            } else {
                false
            }
        } else {
            false
        };

        if is_user_input || is_subagent_task {
            if track.is_subagent && is_subagent_task {
                if let Some(content) = val.get("content").and_then(|c| c.as_str()) {
                    if content.contains("You have just started.") {
                        if let Some(pid) = Self::extract_sender_uuid(content) {
                            track.parent_session_id = Some(pid);
                        }
                    } else if track.parent_session_id.is_none() {
                        if let Some(pid) = Self::extract_sender_uuid(content) {
                            track.parent_session_id = Some(pid);
                        }
                    }
                    if track.worker_name.trim().is_empty()
                        || content.contains("You have just started.")
                    {
                        if let Some(name) = Self::extract_worker_name(content) {
                            track.worker_name = name;
                        }
                    }
                }
            }

            // 新一轮指令到来，彻底重置上一轮可能残留的状态
            track.action_needed_pending = None;
            track.is_waiting = false;
            track.waiting_pending = false;
            track.task_start_pending = true;
            track.is_working = true;
            track.has_reported_done = false;
            track.last_model_finished_time = None;
            track.final_summary_candidate.clear();
            track.last_action_desc = if is_subagent_task {
                "子智能体已唤醒 · 准备中...".into()
            } else {
                "收到新任务 · 准备中...".into()
            };
            return true;
        }

        // 2. 工具调用分支（明确是进行中状态，或 swarm_wait 休眠监控状态）
        if let Some(tool_calls) = val.get("tool_calls").and_then(|t| t.as_array()) {
            if !tool_calls.is_empty() {
                let ask_call = tool_calls
                    .iter()
                    .find(|c| c.get("name").and_then(|n| n.as_str()) == Some("ask_question"));
                let has_swarm_wait = tool_calls
                    .iter()
                    .any(|c| c.get("name").and_then(|n| n.as_str()) == Some("swarm_wait"));

                if ask_call.is_none() && has_swarm_wait {
                    track.action_needed_pending = None;
                    if track.is_subagent {
                        track.task_start_pending = false;
                        track.is_working = false;
                        track.is_waiting = true;
                        track.waiting_pending = true;
                        track.has_reported_done = false;
                        track.last_model_finished_time = None;
                        track.last_action_desc = format!("待命监控 (第 {} 步) · 等待看板消息", step_index);
                    } else {
                        track.is_waiting = false;
                        track.waiting_pending = false;
                        if !track.is_working {
                            track.task_start_pending = true;
                        }
                        track.is_working = true;
                        track.has_reported_done = false;
                        track.last_model_finished_time = None;
                        track.last_action_desc = format!("处理中 (第 {} 步) · 等待 Swarm 协同", step_index);
                    }
                    return true;
                }

                let first_call = ask_call.unwrap_or(&tool_calls[0]);
                let name = first_call.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                let action_summary = match name {
                    "ask_question" => {
                        let mut q_text = String::new();
                        if let Some(questions) = first_call["args"]["questions"].as_array() {
                            if let Some(first_q) = questions.get(0) {
                                if let Some(q) = first_q["question"].as_str() {
                                    q_text = q.to_string();
                                }
                            }
                        }
                        if q_text.is_empty() {
                            if let Some(q_str) = first_call["args"]["questions"].as_str() {
                                if let Ok(q_val) = serde_json::from_str::<Value>(q_str) {
                                    if let Some(first_q) = q_val.as_array().and_then(|a| a.get(0)) {
                                        if let Some(q) = first_q["question"].as_str() {
                                            q_text = q.to_string();
                                        }
                                    }
                                }
                            }
                        }
                        if q_text.is_empty() {
                            if let Some(q) = first_call["args"]["question"].as_str() {
                                q_text = q.to_string();
                            } else if let Some(p) = first_call["args"]["prompt"].as_str() {
                                q_text = p.to_string();
                            }
                        }
                        if q_text.is_empty() {
                            q_text = "请确认下一步操作或做出选择".into();
                        }
                        track.action_needed_pending = Some(q_text.clone());
                        format!("交互 · {}", Self::truncate(&q_text, 18))
                    }
                    "run_command" => {
                        track.action_needed_pending = None;
                        let summary = first_call["args"]["toolSummary"].as_str()
                            .or_else(|| first_call["args"]["CommandLine"].as_str())
                            .unwrap_or("执行终端命令");
                        format!("执行 · {}", Self::truncate(summary, 18))
                    }
                    "write_file" => {
                        track.action_needed_pending = None;
                        let file = first_call["args"]["Path"].as_str().unwrap_or("文件");
                        format!("编写 · {}", Self::short_filename(file))
                    }
                    "edit_file" => {
                        track.action_needed_pending = None;
                        let file = first_call["args"]["Path"].as_str().unwrap_or("文件");
                        format!("修改 · {}", Self::short_filename(file))
                    }
                    "view_file" | "read_file" => {
                        track.action_needed_pending = None;
                        let file = first_call["args"]["AbsolutePath"]
                            .as_str()
                            .or_else(|| first_call["args"]["Path"].as_str())
                            .unwrap_or("文件");
                        format!("查阅 · {}", Self::short_filename(file))
                    }
                    "search_web" => {
                        track.action_needed_pending = None;
                        "检索 · 联网查询".into()
                    }
                    "swarm" => {
                        track.action_needed_pending = None;
                        "协同 · 调度 Swarm".into()
                    }
                    "post_message" => {
                        track.action_needed_pending = None;
                        "协同 · 发送看板消息".into()
                    }
                    "invoke_subagent" => {
                        track.action_needed_pending = None;
                        "协同 · 委派子智能体".into()
                    }
                    other => {
                        track.action_needed_pending = None;
                        format!("调用 · {other}")
                    }
                };

                // 如果之前未处于 working（例如从 swarm_wait 唤醒或直接从文件捕获步骤），标记启动
                track.is_waiting = false;
                track.waiting_pending = false;
                if !track.is_working {
                    track.task_start_pending = true;
                }
                track.is_working = true;
                track.has_reported_done = false;
                track.last_model_finished_time = None; // 工具在跑，绝不算完成
                track.last_action_desc = format!("处理中 (第 {} 步) · {}", step_index, action_summary);
                return true;
            }
        }

        // 3. 【核心判定点】：模型输出最终回复且没有 tool_calls（意味着本轮会话收官）
        let has_tool_calls = val.get("tool_calls")
            .and_then(|t| t.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false);

        if source == "MODEL" && entry_type == "PLANNER_RESPONSE" && status == "DONE" && !has_tool_calls {
            // 本轮模型已给出最终回复，必须清理 action_needed_pending 与 waiting 状态
            track.action_needed_pending = None;
            track.is_waiting = false;
            track.waiting_pending = false;

            if let Some(content) = val.get("content").and_then(|c| c.as_str()) {
                let clean_text = Self::extract_clean_summary(content);
                if !clean_text.is_empty() {
                    track.final_summary_candidate = clean_text;
                }
            }

            track.has_reported_done = false;
            track.is_working = true;
            track.last_model_finished_time = Some(Instant::now());
            return true;
        }

        // 4. 规划/思考分支
        if val.get("thinking").is_some() || (entry_type == "PLANNER_RESPONSE" && status != "DONE") {
            track.action_needed_pending = None;
            track.is_waiting = false;
            track.waiting_pending = false;

            if !track.is_working {
                track.task_start_pending = true;
            }
            track.is_working = true;
            track.has_reported_done = false;
            track.last_model_finished_time = None;
            track.final_summary_candidate.clear();
            track.last_action_desc = format!("规划中 (第 {} 步) · 梳理方案...", step_index);
            return true;
        }

        false
    }

    fn extract_clean_summary(content: &str) -> String {
        let mut lines_collected = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim()
                .trim_start_matches(|c| c == '#' || c == '*' || c == '-' || c == '>' || c == '`' || c == ' ');
            if !trimmed.is_empty() && trimmed.len() > 3 {
                if trimmed.starts_with("---") || trimmed.starts_with("===") {
                    continue;
                }
                lines_collected.push(trimmed);
                if lines_collected.len() >= 2 {
                    break;
                }
            }
        }

        if !lines_collected.is_empty() {
            let joined = lines_collected.join(" · ");
            let clean = joined.replace('`', "").replace("**", "");
            if clean.chars().count() > 48 {
                let s: String = clean.chars().take(48).collect();
                format!("{}...", s)
            } else {
                clean
            }
        } else {
            "任务执行完毕 · 已交付".into()
        }
    }

    fn count_artifacts(session_dir: &Path) -> usize {
        let mut count = 0;
        if let Ok(entries) = fs::read_dir(session_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if ext == "md" || ext == "rs" || ext == "json" {
                            count += 1;
                        }
                    }
                }
            }
        }
        count
    }

    fn short_filename(path_str: &str) -> String {
        Path::new(path_str)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path_str)
            .to_string()
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

    fn make_test_track(session_id: &str, is_subagent: bool) -> SessionTrackInfo {
        SessionTrackInfo {
            session_id: session_id.into(),
            project_name: "WNoobar".into(),
            role_type: "AI 结对助手".into(),
            agent_name: "Antigravity".into(),
            is_subagent,
            parent_session_id: None,
            worker_name: String::new(),
            is_waiting: false,
            waiting_pending: false,
            last_emitted_subagent_status: None,
            file_offset: 0,
            last_event_time: Instant::now(),
            last_activity_time: Instant::now(),
            last_step_index: 0,
            last_action_desc: "待命就绪".into(),
            is_working: false,
            has_reported_done: true,
            last_model_finished_time: None,
            final_summary_candidate: String::new(),
            task_start_pending: false,
            action_needed_pending: None,
            is_initial_catchup: false,
        }
    }

    #[test]
    fn test_parse_entry_done_with_thinking() {
        let mut track = make_test_track("test-sess-1", false);
        track.last_step_index = 10;
        track.last_action_desc = "处理中 (第 10 步) · 执行命令".into();
        track.is_working = true;
        track.has_reported_done = false;

        // 模拟 Antigravity 模型在完成时输出带有 thinking 字段的收官响应
        let entry = json!({
            "step_index": 11,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "content": "已完成全部缺陷修复与交付验证\n所有集成测试均已通过",
            "thinking": "Now we formulate the final response and complete the workflow."
        });

        let updated = AntigravityWatcher::parse_entry(&entry, &mut track);
        assert!(updated);
        // 关键断言：即使带有 thinking 字段，也必须优先识别为 DONE，设置 last_model_finished_time
        assert!(track.last_model_finished_time.is_some());
        assert!(!track.final_summary_candidate.is_empty());
        assert!(track.final_summary_candidate.contains("已完成全部缺陷修复"));
        // 绝不能被拦截成“规划中”
        assert_ne!(track.last_action_desc, "规划中 (第 11 步) · 梳理方案...");
    }

    #[test]
    fn test_parse_entry_thinking_in_progress() {
        let mut track = make_test_track("test-sess-2", false);
        track.last_step_index = 5;

        // 模拟未完成状态下的中间思考
        let entry = json!({
            "step_index": 6,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "IN_PROGRESS",
            "thinking": "Thinking about next steps..."
        });

        let updated = AntigravityWatcher::parse_entry(&entry, &mut track);
        assert!(updated);
        assert!(track.last_model_finished_time.is_none());
        assert_eq!(track.last_action_desc, "规划中 (第 6 步) · 梳理方案...");
        assert!(track.is_working);
    }

    #[test]
    fn test_detect_project_fallback_to_wnoobar() {
        let temp_dir = std::env::temp_dir().join(format!("wnoobar_test_{}", std::process::id()));
        let _ = fs::create_dir_all(&temp_dir);
        // 创建一个干扰的 .md 文件
        let md_file = temp_dir.join("some_random_doc.md");
        let _ = fs::write(&md_file, "# Documentation");

        let (project, role) = AntigravityWatcher::detect_project_and_role(&temp_dir);
        let _ = fs::remove_dir_all(&temp_dir);

        // 即使目录下存在 .md 文件，也绝不把 .md 文件名当做项目名，必须默认回退到 "WNoobar"
        assert_eq!(project, "WNoobar");
        assert_eq!(role, "AI 结对助手");
    }

    #[test]
    fn test_parse_subagent_task_message() {
        let mut track = make_test_track("subagent-1", true);
        track.role_type = "子智能体 · 系统研发".into();

        // 真实 Parent Agent 委派任务给 Subagent
        let subagent_entry = json!({
            "step_index": 0,
            "source": "SYSTEM",
            "type": "SYSTEM_MESSAGE",
            "status": "DONE",
            "content": "<SYSTEM_MESSAGE>\n[Message] timestamp=2026-11-12T03:31:26Z sender=780605e5-6f13-4d42-89d8-5c16274ac781 priority=MESSAGE_PRIORITY_HIGH content=【需求追加工单 - 首席 Rust Windows 工程师】\n</SYSTEM_MESSAGE>"
        });

        assert!(AntigravityWatcher::parse_entry(&subagent_entry, &mut track));
        assert!(track.task_start_pending);
        assert!(track.is_working);
        assert_eq!(
            track.parent_session_id.as_deref(),
            Some("780605e5-6f13-4d42-89d8-5c16274ac781")
        );
        assert_eq!(track.last_action_desc, "子智能体已唤醒 · 准备中...");
    }

    #[test]
    fn test_subagent_task_finished_notification_ignored() {
        let mut track = make_test_track("subagent-1", true);
        track.role_type = "子智能体 · 系统研发".into();
        track.last_step_index = 10;
        track.last_action_desc = "执行中".into();
        track.is_working = true;
        track.has_reported_done = false;
        track.action_needed_pending = Some("旧问题".into());

        // 模拟后台工具执行完毕投递给子智能体的 SYSTEM_MESSAGE，包含 /task-xxx
        let task_finish_msg = json!({
            "step_index": 11,
            "source": "SYSTEM",
            "type": "SYSTEM_MESSAGE",
            "status": "DONE",
            "content": "<SYSTEM_MESSAGE>\n[Message] timestamp=2026-11-12T06:59:08Z sender=d733c5df-ff6a-4395-89ab-dd7aa2f9a979/task-906 priority=MESSAGE_PRIORITY_HIGH content=Task id \"task-906\" finished with result:\nCommand exited with code 0.\n</SYSTEM_MESSAGE>"
        });

        // 关键断言：后台命令完成绝不能被当作新任务唤醒！
        let updated = AntigravityWatcher::parse_entry(&task_finish_msg, &mut track);
        assert!(!updated);
        assert!(!track.task_start_pending);
        // 且由于属于工具响应，之前的 action_needed_pending 应该被清理
        assert!(track.action_needed_pending.is_none());
        assert_ne!(track.last_action_desc, "子智能体已唤醒 · 准备中...");
    }

    #[test]
    fn test_action_needed_cleared_by_generic_response_and_user_input() {
        let mut track = make_test_track("a0dbfa54-test", false);
        track.last_step_index = 37;
        track.last_action_desc = "执行中".into();
        track.is_working = true;
        track.has_reported_done = false;

        // Step 38: ask_question (with stringified JSON questions)
        let step_38 = json!({
            "step_index": 38,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "tool_calls": [{
                "name": "ask_question",
                "args": {
                    "questions": "[{\"question\":\"测试选项已生成，请选择本次要执行的测试方案：\"}]"
                }
            }]
        });
        assert!(AntigravityWatcher::parse_entry(&step_38, &mut track));
        assert!(track.action_needed_pending.is_some());
        assert_eq!(track.action_needed_pending.as_deref(), Some("测试选项已生成，请选择本次要执行的测试方案："));

        // Step 39: 工具/用户跳过返回 GENERIC
        let step_39 = json!({
            "step_index": 39,
            "source": "MODEL",
            "type": "GENERIC",
            "status": "DONE",
            "content": "A1: User Skipped"
        });
        AntigravityWatcher::parse_entry(&step_39, &mut track);
        // 关键断言：收到 GENERIC 响应后，action_needed_pending 必须被彻底清除！
        assert!(track.action_needed_pending.is_none());

        // 模拟即使残留了旧状态，在 Step 50 用户新指令到来时，也必须被无条件清除！
        track.action_needed_pending = Some("旧的残留问题".into());
        let step_50 = json!({
            "step_index": 50,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "status": "DONE",
            "content": "<USER_REQUEST>你好</USER_REQUEST>"
        });
        assert!(AntigravityWatcher::parse_entry(&step_50, &mut track));
        assert!(track.action_needed_pending.is_none());

        // Step 51: 模型回复收官
        let step_51 = json!({
            "step_index": 51,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "content": "你好"
        });
        assert!(AntigravityWatcher::parse_entry(&step_51, &mut track));
        assert!(track.action_needed_pending.is_none());
        assert!(track.last_model_finished_time.is_some());
    }

    #[test]
    fn test_long_running_tool_does_not_prematurely_complete() {
        let mut track = make_test_track("test-long-build", false);
        track.role_type = "系统研发".into();
        track.last_step_index = 10;
        track.last_action_desc = "执行".into();
        track.has_reported_done = false;

        // 用户输入任务
        let user_step = json!({
            "step_index": 1,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "content": "<USER_REQUEST>cargo build --release</USER_REQUEST>"
        });
        assert!(AntigravityWatcher::parse_entry(&user_step, &mut track));
        assert!(track.is_working);
        assert!(!track.has_reported_done);
        assert!(track.last_model_finished_time.is_none());

        // 发起长时间运行的构建工具调用
        let tool_step = json!({
            "step_index": 2,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "tool_calls": [{
                "name": "run_command",
                "args": {
                    "CommandLine": "cargo build --release",
                    "toolSummary": "Build release binary"
                }
            }]
        });
        assert!(AntigravityWatcher::parse_entry(&tool_step, &mut track));
        assert!(track.is_working);
        assert!(!track.has_reported_done);
        // 工具运行中，绝不能标记模型答复完成
        assert!(track.last_model_finished_time.is_none());
        assert_eq!(track.last_action_desc, "处理中 (第 2 步) · 执行 · Build release bina...");

        // 工具完成返回
        let tool_resp = json!({
            "step_index": 3,
            "source": "TOOL",
            "type": "TOOL_RESPONSE",
            "status": "DONE",
            "content": "Finished release target(s)"
        });
        assert!(!AntigravityWatcher::parse_entry(&tool_resp, &mut track));
        assert!(track.is_working);
        assert!(track.last_model_finished_time.is_none());

        // 最终收官总结
        let final_step = json!({
            "step_index": 4,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "content": "构建已成功完成！产物位于 target/release/wnoobar.exe"
        });
        assert!(AntigravityWatcher::parse_entry(&final_step, &mut track));
        assert!(track.is_working);
        assert!(track.last_model_finished_time.is_some());
        assert!(!track.final_summary_candidate.is_empty());
    }

    #[test]
    fn test_should_emit_working_by_filter_level() {
        let now = Instant::now();
        let recent = now - Duration::from_millis(200);
        let stale = now - Duration::from_millis(650);

        assert!(!AntigravityWatcher::should_emit_working(NoiseFilterLevel::Silent, stale, now));
        assert!(AntigravityWatcher::should_emit_working(NoiseFilterLevel::Verbose, recent, now));
        assert!(!AntigravityWatcher::should_emit_working(NoiseFilterLevel::Normal, recent, now));
        assert!(AntigravityWatcher::should_emit_working(NoiseFilterLevel::Normal, stale, now));
        // 子智能体心跳：Silent 下依然按 >= 600ms 节流保活
        assert!(AntigravityWatcher::should_emit_subagent_heartbeat(NoiseFilterLevel::Silent, stale, now));
        assert!(!AntigravityWatcher::should_emit_subagent_heartbeat(NoiseFilterLevel::Silent, recent, now));
    }

    #[test]
    fn test_subagent_metadata_and_swarm_wait_transitions() {
        let root = std::env::temp_dir().join(format!("wnoobar_sub_meta_{}", std::process::id()));
        let sess_dir = root.join("brain").join("sub-meta-1");
        let logs_dir = sess_dir.join(".system_generated").join("logs");
        let msg_dir = sess_dir.join(".system_generated").join("messages");
        fs::create_dir_all(&logs_dir).unwrap();
        fs::create_dir_all(&msg_dir).unwrap();
        fs::write(msg_dir.join("msg.json"), "{}").unwrap();

        let line0 = json!({
            "step_index": 0,
            "source": "SYSTEM",
            "type": "SYSTEM_MESSAGE",
            "status": "DONE",
            "content": "<SYSTEM_MESSAGE>\n[Message] timestamp=2026-11-13T09:35:41Z sender=11111111-2222-3333-4444-555555555555 priority=MESSAGE_PRIORITY_HIGH content=[swarm] new board message #10\n</SYSTEM_MESSAGE>"
        });
        let line1 = json!({
            "step_index": 1,
            "source": "SYSTEM",
            "type": "SYSTEM_MESSAGE",
            "status": "DONE",
            "content": "<SYSTEM_MESSAGE>\n[Message] timestamp=2026-11-13T09:35:41Z sender=35c66131-bf60-4586-a11f-2541f472a985 priority=MESSAGE_PRIORITY_HIGH content=You have just started. Your assignment (recorded on the board as #29 for @swarm_core_eng):\n</SYSTEM_MESSAGE>"
        });
        fs::write(logs_dir.join("transcript.jsonl"), format!("{}\n{}\n", line0, line1)).unwrap();

        assert!(AntigravityWatcher::is_subagent_session(&sess_dir, "sub-meta-1"));
        let (parent_id, worker_name) =
            AntigravityWatcher::detect_subagent_metadata(&sess_dir, "Lead Engineer");
        assert_eq!(
            parent_id.as_deref(),
            Some("35c66131-bf60-4586-a11f-2541f472a985")
        );
        assert_eq!(worker_name, "@swarm_core_eng");

        // 无 @<name> 时应回退到清理后的角色名
        let sess_dir_fallback = root.join("brain").join("sub-meta-fallback");
        fs::create_dir_all(sess_dir_fallback.join(".system_generated").join("logs")).unwrap();
        fs::write(
            sess_dir_fallback
                .join(".system_generated")
                .join("logs")
                .join("transcript.jsonl"),
            format!(
                "{}\n",
                json!({
                    "step_index": 0,
                    "source": "SYSTEM",
                    "type": "SYSTEM_MESSAGE",
                    "content": "[Message] sender=parent-abc content=Do work"
                })
            ),
        )
        .unwrap();
        let (parent_fb, name_fb) =
            AntigravityWatcher::detect_subagent_metadata(&sess_dir_fallback, "Lead Engineer");
        assert_eq!(parent_fb.as_deref(), Some("parent-abc"));
        assert_eq!(name_fb, "系统研发");

        // parse_entry 验证 post_message + swarm_wait 同轮调用 -> Waiting，以及唤醒 -> Working
        let mut track = make_test_track("sub-meta-1", true);
        let wait_entry = json!({
            "step_index": 5,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "tool_calls": [
                { "name": "post_message", "args": { "text": "done" } },
                { "name": "swarm_wait", "args": { "since": 29 } }
            ]
        });
        assert!(AntigravityWatcher::parse_entry(&wait_entry, &mut track));
        assert!(track.is_waiting);
        assert!(track.waiting_pending);
        assert!(!track.is_working);
        assert!(track.last_model_finished_time.is_none());

        let wake_entry = json!({
            "step_index": 6,
            "source": "SYSTEM",
            "type": "SYSTEM_MESSAGE",
            "status": "DONE",
            "content": "<SYSTEM_MESSAGE>\n[Message] timestamp=2026-11-13T09:40:00Z sender=35c66131-bf60-4586-a11f-2541f472a985 priority=MESSAGE_PRIORITY_HIGH content=[swarm] new board message #30\n</SYSTEM_MESSAGE>"
        });
        assert!(AntigravityWatcher::parse_entry(&wake_entry, &mut track));
        assert!(!track.is_waiting);
        assert!(!track.waiting_pending);
        assert!(track.is_working);
        assert!(track.task_start_pending);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_subagent_emits_only_subagent_status_events() {
        use std::io::Write;

        let root = std::env::temp_dir().join(format!(
            "wnoobar_subagent_events_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let brain_dir = root.join("brain");
        let sess_dir = brain_dir.join("sub-worker-alpha");
        let logs_dir = sess_dir.join(".system_generated").join("logs");
        let msg_dir = sess_dir.join(".system_generated").join("messages");
        fs::create_dir_all(&logs_dir).unwrap();
        fs::create_dir_all(&msg_dir).unwrap();
        fs::write(msg_dir.join("assign.json"), "{}").unwrap();

        let log_file = logs_dir.join("transcript.jsonl");
        let init_line = json!({
            "step_index": 1,
            "source": "SYSTEM",
            "type": "SYSTEM_MESSAGE",
            "status": "DONE",
            "content": "<SYSTEM_MESSAGE>\n[Message] timestamp=2026-11-13T09:35:41Z sender=parent-uuid-001 priority=MESSAGE_PRIORITY_HIGH content=You have just started. Your assignment (recorded on the board as #29 for @font_eng):\n</SYSTEM_MESSAGE>"
        });
        fs::write(&log_file, format!("{}\n", init_line)).unwrap();

        let (tx, rx) = crossbeam_channel::unbounded();
        let running = Arc::new(AtomicBool::new(true));
        let filter = SharedNoiseFilter::new(NoiseFilterLevel::Normal);

        let bg_brain = brain_dir.clone();
        let bg_running = running.clone();
        let bg_filter = filter.clone();
        let handle = thread::spawn(move || {
            AntigravityWatcher::run_loop_internal(bg_brain, tx, bg_running, bg_filter, None);
        });

        // 1. 启动仅发射 SubagentStatus::Working（不发射 TaskStarted）
        let ev1 = rx.recv_timeout(Duration::from_secs(5)).expect("Working SubagentStatus");
        match ev1 {
            AgentEvent::SubagentStatus {
                group_key,
                parent_session_id,
                worker_id,
                worker_name,
                status,
                ..
            } => {
                assert_eq!(group_key, "parent-uuid-001");
                assert_eq!(parent_session_id.as_deref(), Some("parent-uuid-001"));
                assert_eq!(worker_id, "sub-worker-alpha");
                assert_eq!(worker_name, "@font_eng");
                assert_eq!(status, SwarmWorkerStatus::Working);
            }
            other => panic!("Expected SubagentStatus::Working, got {:?}", other),
        }

        // 2. 调用 swarm_wait 后发射 SubagentStatus::Waiting
        let wait_line = json!({
            "step_index": 2,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "tool_calls": [{ "name": "swarm_wait", "args": { "since": 29 } }]
        });
        {
            let mut f = fs::OpenOptions::new().append(true).open(&log_file).unwrap();
            writeln!(f, "{}", wait_line).unwrap();
            f.flush().unwrap();
        }
        let ev2 = rx.recv_timeout(Duration::from_secs(5)).expect("Waiting SubagentStatus");
        match ev2 {
            AgentEvent::SubagentStatus { status, step_index, .. } => {
                assert_eq!(status, SwarmWorkerStatus::Waiting);
                assert_eq!(step_index, 2);
            }
            other => panic!("Expected SubagentStatus::Waiting, got {:?}", other),
        }

        // 3. 收到新 tool_calls 时从 Waiting 切回 Working
        let tool_line = json!({
            "step_index": 3,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "tool_calls": [{ "name": "edit_file", "args": { "Path": "src/core/swarm.rs" } }]
        });
        {
            let mut f = fs::OpenOptions::new().append(true).open(&log_file).unwrap();
            writeln!(f, "{}", tool_line).unwrap();
            f.flush().unwrap();
        }
        let ev3 = rx.recv_timeout(Duration::from_secs(5)).expect("Resumed Working SubagentStatus");
        match ev3 {
            AgentEvent::SubagentStatus { status, step_index, .. } => {
                assert_eq!(status, SwarmWorkerStatus::Working);
                assert_eq!(step_index, 3);
            }
            other => panic!("Expected SubagentStatus::Working, got {:?}", other),
        }

        // 4. 最终收官发射 SubagentStatus::Done（不发射独立 Done）
        let done_line = json!({
            "step_index": 4,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "content": "全部字形渲染修复完成"
        });
        {
            let mut f = fs::OpenOptions::new().append(true).open(&log_file).unwrap();
            writeln!(f, "{}", done_line).unwrap();
            f.flush().unwrap();
        }
        let ev4 = rx.recv_timeout(Duration::from_secs(5)).expect("Done SubagentStatus");
        match ev4 {
            AgentEvent::SubagentStatus { status, step_index, task_summary, .. } => {
                assert_eq!(status, SwarmWorkerStatus::Done);
                assert_eq!(step_index, 4);
                assert!(task_summary.contains("全部字形渲染修复完成"));
            }
            other => panic!("Expected SubagentStatus::Done, got {:?}", other),
        }

        running.store(false, Ordering::SeqCst);
        handle.join().unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_subagent_fallback_group_key_without_parent_uuid() {
        let root = std::env::temp_dir().join(format!(
            "wnoobar_subagent_fallback_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let brain_dir = root.join("brain");
        let sess_dir = brain_dir.join("sub-orphan-1");
        let logs_dir = sess_dir.join(".system_generated").join("logs");
        let msg_dir = sess_dir.join(".system_generated").join("messages");
        fs::create_dir_all(&logs_dir).unwrap();
        fs::create_dir_all(&msg_dir).unwrap();
        fs::write(msg_dir.join("task.json"), "{}").unwrap();

        let init_line = json!({
            "step_index": 1,
            "source": "SYSTEM",
            "type": "SYSTEM_MESSAGE",
            "status": "DONE",
            "content": "<SYSTEM_MESSAGE>\n[Message] timestamp=2026-11-13T09:35:41Z priority=MESSAGE_PRIORITY_HIGH content=Handle WNoobar task\n</SYSTEM_MESSAGE>"
        });
        fs::write(logs_dir.join("transcript.jsonl"), format!("{}\n", init_line)).unwrap();

        let (tx, rx) = crossbeam_channel::unbounded();
        let running = Arc::new(AtomicBool::new(true));
        let filter = SharedNoiseFilter::new(NoiseFilterLevel::Normal);

        let bg_brain = brain_dir.clone();
        let bg_running = running.clone();
        let handle = thread::spawn(move || {
            AntigravityWatcher::run_loop_internal(bg_brain, tx, bg_running, filter, None);
        });

        let ev = rx.recv_timeout(Duration::from_secs(5)).expect("fallback SubagentStatus");
        match ev {
            AgentEvent::SubagentStatus {
                group_key,
                parent_session_id,
                project_name,
                agent_name,
                status,
                ..
            } => {
                assert_eq!(parent_session_id, None);
                assert_eq!(project_name, "WNoobar");
                assert_eq!(agent_name, "Antigravity");
                assert_eq!(group_key, "Antigravity::WNoobar");
                assert_eq!(status, SwarmWorkerStatus::Working);
            }
            other => panic!("Expected SubagentStatus, got {:?}", other),
        }

        running.store(false, Ordering::SeqCst);
        handle.join().unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_watcher_shared_filter_wiring_and_live_switch() {
        use std::io::Write;

        static TEST_SEQ: AtomicUsize = AtomicUsize::new(0);
        let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);

        let (tx, rx) = crossbeam_channel::unbounded();
        if let Ok(mut watcher) = AntigravityWatcher::new(tx.clone()) {
            assert_eq!(watcher.shared_filter.get(), NoiseFilterLevel::Normal);

            watcher.set_filter_level(NoiseFilterLevel::Silent);
            assert_eq!(watcher.shared_filter.get(), NoiseFilterLevel::Silent);

            let external = SharedNoiseFilter::new(NoiseFilterLevel::Verbose);
            watcher.set_shared_filter(external.clone());
            assert_eq!(watcher.shared_filter.get(), NoiseFilterLevel::Verbose);

            external.set(NoiseFilterLevel::Normal);
            assert_eq!(watcher.shared_filter.get(), NoiseFilterLevel::Normal);
        }

        // 验证 run_loop 在运行期间无需重启即可响应 SharedNoiseFilter 的实时切换
        let root = std::env::temp_dir().join(format!(
            "wnoobar_live_filter_{}_{}",
            std::process::id(),
            seq
        ));
        let _ = fs::remove_dir_all(&root);
        let brain_dir = root.join("brain");
        let ann_dir = root.join("annotations");
        let sess_dir = brain_dir.join("main-live-filter");
        let logs_dir = sess_dir.join(".system_generated").join("logs");
        fs::create_dir_all(&logs_dir).expect("create logs dir");
        fs::create_dir_all(&ann_dir).expect("create annotations dir");
        fs::write(ann_dir.join("main-live-filter.pbtxt"), "title:\"LiveFilter\"")
            .expect("create pbtxt");

        let log_file = logs_dir.join("transcript.jsonl");
        fs::write(&log_file, "").expect("create empty initial log");

        let running = Arc::new(AtomicBool::new(true));
        let live_filter = SharedNoiseFilter::new(NoiseFilterLevel::Silent);
        let ticks = Arc::new(AtomicUsize::new(0));

        let bg_brain = brain_dir.clone();
        let bg_running = running.clone();
        let bg_filter = live_filter.clone();
        let bg_ticks = ticks.clone();
        let handle = thread::spawn(move || {
            AntigravityWatcher::run_loop_internal(
                bg_brain,
                tx,
                bg_running,
                bg_filter,
                Some(&bg_ticks),
            );
        });

        while ticks.load(Ordering::SeqCst) < 1 {
            thread::sleep(Duration::from_millis(10));
        }

        let line1 = json!({
            "step_index": 1,
            "source": "USER_EXPLICIT",
            "type": "USER_INPUT",
            "content": "<USER_REQUEST>live filter check</USER_REQUEST>"
        });
        {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(&log_file)
                .expect("open log for line1");
            writeln!(file, "{}", line1).expect("append line1");
            file.flush().expect("flush line1");
        }

        let ev1 = rx.recv_timeout(Duration::from_secs(5)).expect("should receive TaskStarted");
        assert!(matches!(ev1, AgentEvent::TaskStarted { .. }));

        // 在不重启 run_loop 的情况下，从 Silent 实时切换到 Verbose
        live_filter.set(NoiseFilterLevel::Verbose);

        let line2 = json!({
            "step_index": 2,
            "source": "MODEL",
            "type": "PLANNER_RESPONSE",
            "status": "DONE",
            "tool_calls": [{
                "name": "view_file",
                "args": { "AbsolutePath": "src/core/config.rs" }
            }]
        });
        {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(&log_file)
                .expect("open log for append");
            writeln!(file, "{}", line2).expect("append tool call line");
            file.flush().expect("flush log");
        }

        let ev2 = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("should immediately emit Working after live switch to Verbose");
        match ev2 {
            AgentEvent::Working { step_index, .. } => assert_eq!(step_index, 2),
            other => panic!("Expected Working event, got {:?}", other),
        }

        running.store(false, Ordering::SeqCst);
        handle.join().expect("run_loop thread should exit cleanly");
        let _ = fs::remove_dir_all(&root);
    }
}
