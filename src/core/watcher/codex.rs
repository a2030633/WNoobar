#![allow(dead_code)]
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::{Duration as ChronoDuration, Local};
use crossbeam_channel::Sender;
use serde_json::Value;

use crate::core::bus::AgentEvent;

pub struct CodexWatcher {
    codex_dir: PathBuf,
    sender: Sender<AgentEvent>,
    running: Arc<AtomicBool>,
}

struct CodexSessionTrackInfo {
    session_id: String,
    project_name: String,
    role_type: String,
    file_offset: u64,
    last_event_time: Instant,
    last_step_index: u64,
    last_action_desc: String,
    is_working: bool,
    has_reported_done: bool,
    task_start_pending: bool,
    task_summary_pending: String,
    action_needed_pending: Option<String>,
    done_pending: Option<(String, u64)>, // summary, duration_secs
    last_agent_message_candidate: String,
}

impl CodexWatcher {
    pub fn new(sender: Sender<AgentEvent>) -> Result<Self, String> {
        let user_profile = std::env::var("USERPROFILE")
            .map_err(|e| format!("Could not read USERPROFILE: {e}"))?;
        let codex_dir = PathBuf::from(user_profile).join(".codex");

        if !codex_dir.exists() {
            return Err(format!("Codex directory not found at {:?}", codex_dir));
        }

        Ok(Self {
            codex_dir,
            sender,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn start(&mut self) {
        self.running.store(true, Ordering::SeqCst);
        let codex_dir = self.codex_dir.clone();
        let sender = self.sender.clone();
        let running = self.running.clone();

        thread::Builder::new()
            .name("codex-watcher".into())
            .spawn(move || {
                Self::run_loop(codex_dir, sender, running);
            })
            .expect("Failed to spawn Codex watcher thread");
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    fn run_loop(codex_dir: PathBuf, sender: Sender<AgentEvent>, running: Arc<AtomicBool>) {
        println!("[Watcher] Codex Three-Beat Rhythm Engine active: {:?}", codex_dir);

        let mut tracked_sessions: HashMap<String, CodexSessionTrackInfo> = HashMap::new();

        while running.load(Ordering::SeqCst) {
            // 1. 扫描当前与近期活跃的 Codex Rollout 日志文件
            let active_files = Self::scan_recent_rollouts(&codex_dir);
            let thread_names = Self::read_session_index_names(&codex_dir);

            for (session_id, file_path) in active_files {
                let track = tracked_sessions.entry(session_id.clone()).or_insert_with(|| {
                    let len = file_path.metadata().map(|m| m.len()).unwrap_or(0);
                    let thread_name = thread_names.get(&session_id).cloned().unwrap_or_else(|| "Codex 编程助手".into());
                    let project_name = Self::detect_project_from_rollout(&file_path);

                    CodexSessionTrackInfo {
                        session_id: session_id.clone(),
                        project_name,
                        role_type: thread_name,
                        file_offset: len, // 初始历史文件不回放，从最新行开始监听
                        last_event_time: Instant::now() - Duration::from_secs(60),
                        last_step_index: 0,
                        last_action_desc: "待命就绪".into(),
                        is_working: false,
                        has_reported_done: true,
                        task_start_pending: false,
                        task_summary_pending: "收到任务 · 开始处理...".into(),
                        action_needed_pending: None,
                        done_pending: None,
                        last_agent_message_candidate: String::new(),
                    }
                });

                // 同步会话自定义名称（经过领域规范化清洗，防止整句提问塞满导致胶囊排版截断）
                if let Some(name) = thread_names.get(&session_id) {
                    if !name.trim().is_empty() && name != "待命就绪" {
                        track.role_type = Self::normalize_role_name(name);
                    }
                }

                // 读取新写入的增量行
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
                                    if Self::parse_codex_entry(&val, track) {
                                        step_updated = true;
                                    }
                                }
                            }
                            line.clear();
                        }

                        track.file_offset = current_len;

                        // 节拍 1：启动轻弹（Start Pop）
                        if track.task_start_pending {
                            track.task_start_pending = false;
                            track.is_working = true;
                            track.has_reported_done = false;
                            track.last_event_time = Instant::now();

                            println!("[Watcher] [Codex] TaskStarted: [{}] - {}", track.project_name, track.task_summary_pending);
                            let _ = sender.send(AgentEvent::TaskStarted {
                                session_id: track.session_id.clone(),
                                project_name: track.project_name.clone(),
                                role_type: track.role_type.clone(),
                                agent_name: "Codex".into(),
                                task_summary: track.task_summary_pending.clone(),
                            });
                        } else if let Some(prompt) = track.action_needed_pending.take() {
                            // 节拍 1.5：需要确认交互（ActionNeeded）
                            track.is_working = true;
                            track.has_reported_done = false;

                            println!("[Watcher] [Codex] ActionNeeded: [{}] - {}", track.project_name, prompt);
                            let _ = sender.send(AgentEvent::ActionNeeded {
                                session_id: track.session_id.clone(),
                                project_name: track.project_name.clone(),
                                role_type: track.role_type.clone(),
                                agent_name: "Codex".into(),
                                prompt,
                            });
                        } else if let Some((summary, duration_secs)) = track.done_pending.take() {
                            // 节拍 3：完成交付（Done Pop）
                            track.is_working = false;
                            track.has_reported_done = true;

                            println!("[Watcher] [Codex] Done: [{}] - {} ({}s)", track.project_name, summary, duration_secs);
                            let _ = sender.send(AgentEvent::Done {
                                session_id: track.session_id.clone(),
                                project_name: track.project_name.clone(),
                                role_type: track.role_type.clone(),
                                agent_name: "Codex".into(),
                                summary,
                                duration_secs,
                                artifacts_count: 0,
                            });
                        } else if step_updated && track.is_working {
                            // 节拍 2：执行静默期（Working）
                            let now = Instant::now();
                            if now.duration_since(track.last_event_time) >= Duration::from_millis(500) {
                                track.last_event_time = now;
                                let _ = sender.send(AgentEvent::Working {
                                    session_id: track.session_id.clone(),
                                    project_name: track.project_name.clone(),
                                    role_type: track.role_type.clone(),
                                    agent_name: "Codex".into(),
                                    task_summary: track.last_action_desc.clone(),
                                    step_index: track.last_step_index,
                                });
                            }
                        }
                    }
                }
            }

            thread::sleep(Duration::from_millis(150));
        }
    }

    /// 解析 Codex 单行 JSON 日志条目
    fn parse_codex_entry(val: &Value, track: &mut CodexSessionTrackInfo) -> bool {
        let entry_type = val.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let payload = val.get("payload");

        match entry_type {
            "session_meta" => {
                if let Some(p) = payload {
                    if let Some(cwd) = p.get("cwd").and_then(|c| c.as_str()) {
                        let path = Path::new(cwd);
                        if let Some(folder_name) = path.file_name().and_then(|n| n.to_str()) {
                            if !folder_name.is_empty() {
                                track.project_name = folder_name.to_string();
                            }
                        }
                    }
                }
                false
            }
            "item_completed" => {
                if let Some(p) = payload {
                    if let Some(item) = p.get("item") {
                        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        if item_type == "UserMessage" {
                            if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                                if let Some(first) = content.first() {
                                    if let Some(text) = first.get("text").and_then(|t| t.as_str()) {
                                        let clean_prompt = Self::clean_user_prompt(text);
                                        if !clean_prompt.is_empty() {
                                            track.task_summary_pending = clean_prompt;
                                            track.task_start_pending = true;
                                            track.is_working = true;
                                            track.has_reported_done = false;
                                            return true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                false
            }
            "event_msg" => {
                if let Some(p) = payload {
                    let msg_type = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match msg_type {
                        "task_started" => {
                            track.task_start_pending = true;
                            track.is_working = true;
                            track.has_reported_done = false;
                            track.last_step_index = 0;
                            true
                        }
                        "task_complete" => {
                            let duration_ms = p.get("duration_ms").and_then(|d| d.as_u64()).unwrap_or(0);
                            let duration_secs = (duration_ms / 1000).max(1);

                            let summary = if let Some(msg) = p.get("last_agent_message").and_then(|m| m.as_str()) {
                                Self::clean_summary(msg)
                            } else if !track.last_agent_message_candidate.is_empty() {
                                Self::clean_summary(&track.last_agent_message_candidate)
                            } else {
                                "任务执行完毕 · 结果已就绪".into()
                            };

                            track.done_pending = Some((summary, duration_secs));
                            true
                        }
                        _ => false,
                    }
                } else {
                    false
                }
            }
            "response_item" => {
                if let Some(p) = payload {
                    let resp_type = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match resp_type {
                        "function_call" => {
                            track.last_step_index += 1;
                            let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            let args_str = p.get("arguments").and_then(|a| a.as_str()).unwrap_or("");

                            if name == "request_user_input" {
                                let prompt = Self::extract_user_input_prompt(args_str);
                                track.action_needed_pending = Some(prompt);
                                return true;
                            }

                            let desc = match name {
                                "exec_command" => {
                                    let cmd = Self::extract_arg_field(args_str, "cmd");
                                    format!("终端命令 · {}", Self::truncate_str(&cmd, 32))
                                }
                                "apply_patch" => {
                                    let path = Self::extract_arg_field(args_str, "path");
                                    format!("修改代码 · {}", Self::short_filename(&path))
                                }
                                "write_file" => {
                                    let path = Self::extract_arg_field(args_str, "path");
                                    format!("写入文件 · {}", Self::short_filename(&path))
                                }
                                "view_file" | "read_file" => {
                                    let path = Self::extract_arg_field(args_str, "path");
                                    format!("查阅代码 · {}", Self::short_filename(&path))
                                }
                                "grep" => "全局检索 · 扫描代码".into(),
                                "find_files" => "文件查找 · 定位资源".into(),
                                other => format!("执行 · {other}"),
                            };

                            track.last_action_desc = format!("处理中 (第 {} 步) · {}", track.last_step_index, desc);
                            track.is_working = true;
                            track.has_reported_done = false;
                            true
                        }
                        "reasoning" => {
                            track.last_action_desc = format!("深度思考 (第 {} 步) · 规划执行步骤...", track.last_step_index);
                            track.is_working = true;
                            true
                        }
                        "message" => {
                            if p.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                                if let Some(content) = p.get("content").and_then(|c| c.as_array()) {
                                    for item in content {
                                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                            track.last_agent_message_candidate = text.to_string();
                                        }
                                    }
                                }
                            }
                            false
                        }
                        _ => false,
                    }
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// 扫描近期活跃的 Codex Rollout 文件
    fn scan_recent_rollouts(codex_dir: &Path) -> Vec<(String, PathBuf)> {
        let mut results = Vec::new();
        let sessions_root = codex_dir.join("sessions");
        if !sessions_root.exists() {
            return results;
        }

        let now = Local::now();
        let dates_to_check = [
            now.format("%Y/%m/%d").to_string(),
            (now - ChronoDuration::days(1)).format("%Y/%m/%d").to_string(),
        ];

        for date_str in &dates_to_check {
            let day_dir = sessions_root.join(date_str.replace('/', "\\"));
            if let Ok(entries) = fs::read_dir(&day_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                                // 提取 session_id: rollout-<date>-<session_id>.jsonl
                                if let Some(session_id) = Self::extract_session_id_from_filename(name) {
                                    results.push((session_id, path));
                                }
                            }
                        }
                    }
                }
            }
        }

        // 按修改时间降序排序
        results.sort_by(|a, b| {
            let time_a = a.1.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let time_b = b.1.metadata().and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            time_b.cmp(&time_a)
        });

        results.truncate(5);
        results
    }

    fn extract_session_id_from_filename(filename: &str) -> Option<String> {
        let trimmed = filename.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
        // 查找最后一个连字符后面的 UUID 格式
        if let Some(pos) = trimmed.rfind('-') {
            let candidate = &trimmed[pos + 1..];
            if candidate.len() >= 8 {
                // 如果是完整的完整UUID格式: 01a1c4ea-eee5-7d72-a10a-076aeb38b642
                // 会有 5 个部分，查找前缀包含 4 个连字符的倒数部分
                let parts: Vec<&str> = trimmed.split('-').collect();
                if parts.len() >= 5 {
                    let uuid = parts[parts.len() - 5..].join("-");
                    return Some(uuid);
                }
                return Some(candidate.to_string());
            }
        }
        None
    }

    /// 读取 session_index.jsonl 建立 session_id -> thread_name 映射
    fn read_session_index_names(codex_dir: &Path) -> HashMap<String, String> {
        let mut map = HashMap::new();
        let index_file = codex_dir.join("session_index.jsonl");
        if let Ok(file) = File::open(&index_file) {
            let reader = BufReader::new(file);
            for line in reader.lines().flatten() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                        if let (Some(id), Some(name)) = (
                            val.get("id").and_then(|i| i.as_str()),
                            val.get("thread_name").and_then(|n| n.as_str()),
                        ) {
                            if !name.trim().is_empty() {
                                map.insert(id.to_string(), Self::normalize_role_name(name));
                            }
                        }
                    }
                }
            }
        }
        map
    }

    fn detect_project_from_rollout(path: &Path) -> String {
        if let Ok(file) = File::open(path) {
            let reader = BufReader::new(file);
            for line in reader.lines().take(10).flatten() {
                if let Ok(val) = serde_json::from_str::<Value>(line.trim()) {
                    if val.get("type").and_then(|t| t.as_str()) == Some("session_meta") {
                        if let Some(cwd) = val["payload"]["cwd"].as_str() {
                            let p = Path::new(cwd);
                            if let Some(folder) = p.file_name().and_then(|n| n.to_str()) {
                                if !folder.is_empty() {
                                    return folder.to_string();
                                }
                            }
                        }
                    }
                }
            }
        }
        "Codex".into()
    }

    fn clean_user_prompt(prompt: &str) -> String {
        for line in prompt.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('<') {
                continue;
            }
            let s: String = trimmed.chars().take(36).collect();
            return s;
        }
        "处理用户新指令...".into()
    }

    fn clean_summary(content: &str) -> String {
        let mut lines = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim()
                .trim_start_matches(|c| c == '#' || c == '*' || c == '-' || c == '>' || c == '`' || c == ' ');
            if !trimmed.is_empty() && trimmed.len() > 2 {
                if trimmed.starts_with("---") || trimmed.starts_with("===") {
                    continue;
                }
                lines.push(trimmed);
                if lines.len() >= 2 {
                    break;
                }
            }
        }

        if !lines.is_empty() {
            let joined = lines.join(" · ").replace('`', "").replace("**", "");
            if joined.chars().count() > 48 {
                let s: String = joined.chars().take(48).collect();
                format!("{}...", s)
            } else {
                joined
            }
        } else {
            "已完成本次任务交互".into()
        }
    }

    fn extract_user_input_prompt(args_str: &str) -> String {
        if let Ok(val) = serde_json::from_str::<Value>(args_str) {
            if let Some(prompt) = val.get("prompt").or_else(|| val.get("question")).and_then(|p| p.as_str()) {
                return prompt.chars().take(40).collect();
            }
        }
        "需要用户提供进一步指示或授权确认".into()
    }

    fn extract_arg_field(args_str: &str, field: &str) -> String {
        if let Ok(val) = serde_json::from_str::<Value>(args_str) {
            if let Some(s) = val.get(field).and_then(|v| v.as_str()) {
                return s.to_string();
            }
        }
        String::new()
    }

    fn short_filename(path_str: &str) -> String {
        let p = Path::new(path_str);
        p.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path_str)
            .to_string()
    }

    fn truncate_str(s: &str, max_chars: usize) -> String {
        let clean = s.replace('\n', " ").replace('\r', "");
        if clean.chars().count() > max_chars {
            let t: String = clean.chars().take(max_chars).collect();
            format!("{}...", t)
        } else {
            clean
        }
    }

    /// 规范化清洗 Codex 会话/角色名称
    /// 解决用户提问长句（例如“帮我编写一个测试脚本验证多线程性能”）直接塞满天蓝色角色位导致右侧截断的硬伤
    pub fn normalize_role_name(name: &str) -> String {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return "功能研发".into();
        }

        // 若已经是 6 个字符以内的干练短语且无明显句末标点，直接保留
        let char_count = trimmed.chars().count();
        let has_punctuation = trimmed.contains('。')
            || trimmed.contains('？')
            || trimmed.contains('！')
            || trimmed.contains('，')
            || trimmed.contains('?')
            || trimmed.contains('!')
            || trimmed.contains(',')
            || trimmed.contains(':')
            || trimmed.contains('：');

        if char_count <= 6 && !has_punctuation {
            return trimmed.to_string();
        }

        let lower = trimmed.to_lowercase();

        // 领域关键词匹配
        if lower.contains("test")
            || lower.contains("测试")
            || lower.contains("验证")
            || lower.contains("unittest")
            || lower.contains("单测")
            || lower.contains("bench")
        {
            return "测试验证".into();
        }

        if lower.contains("fix")
            || lower.contains("bug")
            || lower.contains("修复")
            || lower.contains("缺陷")
            || lower.contains("报错")
            || lower.contains("issue")
            || lower.contains("error")
            || lower.contains("crash")
            || lower.contains("排查")
        {
            return "缺陷修复".into();
        }

        if lower.contains("refactor")
            || lower.contains("重构")
            || lower.contains("架构")
            || lower.contains("architecture")
            || lower.contains("拆分")
        {
            return "架构重构".into();
        }

        if lower.contains("optimi")
            || lower.contains("性能")
            || lower.contains("加速")
            || lower.contains("内存")
            || lower.contains("perf")
            || lower.contains("speed")
            || lower.contains("缩容")
        {
            return "性能优化".into();
        }

        if lower.contains("deploy")
            || lower.contains("发布")
            || lower.contains("ci")
            || lower.contains("cd")
            || lower.contains("pipeline")
            || lower.contains("docker")
            || lower.contains("构建")
            || lower.contains("build")
            || lower.contains("编译")
        {
            return "构建部署".into();
        }

        if lower.contains("docs")
            || lower.contains("document")
            || lower.contains("文档")
            || lower.contains("readme")
            || lower.contains("注释")
            || lower.contains("说明")
        {
            return "文档编写".into();
        }

        if lower.contains("review")
            || lower.contains("评审")
            || lower.contains("审查")
            || lower.contains("audit")
            || lower.contains("检查")
        {
            return "代码审查".into();
        }

        if lower.contains("ui")
            || lower.contains("界面")
            || lower.contains("样式")
            || lower.contains("css")
            || lower.contains("frontend")
            || lower.contains("前端")
            || lower.contains("组件")
            || lower.contains("弹窗")
        {
            return "界面研发".into();
        }

        if lower.contains("api")
            || lower.contains("后端")
            || lower.contains("服务")
            || lower.contains("server")
            || lower.contains("route")
            || lower.contains("proxy")
            || lower.contains("代理")
            || lower.contains("网关")
        {
            return "后端服务".into();
        }

        if lower.contains("subagent")
            || lower.contains("subtask")
            || lower.contains("子任务")
            || lower.contains("子智能体")
        {
            return "协同处理".into();
        }

        if lower.contains("feat")
            || lower.contains("功能")
            || lower.contains("新增")
            || lower.contains("开发")
            || lower.contains("实现")
            || lower.contains("add")
            || lower.contains("implement")
            || lower.contains("编写")
        {
            return "功能研发".into();
        }

        // 若均未命中，提取标点前的核心前缀（至多 6 字符）
        let prefix = trimmed.split(&['。', '？', '！', '，', '?', '!', ',', ':', '：', '\n'][..]).next().unwrap_or(trimmed);
        let cleaned: String = prefix.chars().filter(|c| !c.is_ascii_punctuation()).take(6).collect();
        if !cleaned.trim().is_empty() {
            cleaned.trim().to_string()
        } else {
            "功能研发".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codex_normalize_role_name() {
        assert_eq!(CodexWatcher::normalize_role_name("帮我写一个测试脚本验证多线程性能"), "测试验证");
        assert_eq!(CodexWatcher::normalize_role_name("修复窗口渲染黑边和崩溃Bug"), "缺陷修复");
        assert!(CodexWatcher::normalize_role_name("重构 watcher 模块代码结构") == "架构重构");
        assert_eq!(CodexWatcher::normalize_role_name("优化内存占用与渲染帧率性能"), "性能优化");
        assert_eq!(CodexWatcher::normalize_role_name("编写项目使用文档和README"), "文档编写");
        assert_eq!(CodexWatcher::normalize_role_name("代码审查"), "代码审查");
        assert_eq!(CodexWatcher::normalize_role_name("Docker 构建与发布流水线"), "构建部署");
        assert_eq!(CodexWatcher::normalize_role_name(""), "功能研发");
    }
}
