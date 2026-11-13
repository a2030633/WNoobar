#![allow(dead_code)]
use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Swarm 组内单个 Subagent 的纯态状态（按紧急度排序：ActionNeeded < Working < Done < Waiting）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SwarmWorkerStatus {
    ActionNeeded,
    Working,
    Done,
    Waiting,
}

impl SwarmWorkerStatus {
    /// 返回状态紧急度排名，数值越小越靠前展示
    pub fn priority_rank(self) -> u8 {
        match self {
            Self::ActionNeeded => 0,
            Self::Working => 1,
            Self::Done => 2,
            Self::Waiting => 3,
        }
    }

    /// 判断是否处于活跃阻塞或执行态（未收敛）
    pub fn is_active(self) -> bool {
        matches!(self, Self::ActionNeeded | Self::Working)
    }
}

/// 单个 Subagent 工作节点信息
#[derive(Debug, Clone, PartialEq)]
pub struct SwarmWorkerInfo {
    pub worker_id: String,
    pub display_name: String,
    pub role: String,
    pub status: SwarmWorkerStatus,
    pub task_summary: String,
    pub step_index: u64,
    pub started_at: Instant,
    pub last_update: Instant,
}

/// 同组 Swarm 聚合摘要快照（用于 Odometer / Deck / Swarm 面板纯态展示）
#[derive(Debug, Clone, PartialEq)]
pub struct SwarmSummary {
    pub group_key: String,
    pub parent_session_id: Option<String>,
    pub project_name: String,
    pub agent_name: String,
    pub total_workers: usize,
    pub working_count: usize,
    pub action_count: usize,
    pub done_count: usize,
    pub waiting_count: usize,
    pub workers: Vec<SwarmWorkerInfo>,
    pub aggregate_summary: String,
    pub elapsed_secs: u64,
}

#[derive(Debug, Clone)]
struct SwarmGroup {
    group_key: String,
    parent_session_id: Option<String>,
    project_name: String,
    agent_name: String,
    workers: HashMap<String, SwarmWorkerInfo>,
    started_at: Instant,
    last_update: Instant,
    parent_pending_done: bool,
    had_active_work: bool,
}

/// 追踪多个并发 Swarm 协同组的状态机与父子收官协调器
#[derive(Debug, Default)]
pub struct SwarmRegistry {
    groups: HashMap<String, SwarmGroup>,
    parent_sessions: HashMap<String, (String, String)>,
}

impl SwarmRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记或刷新父会话元数据（用于无显式 parent_session_id 时的唯一组自动绑定）
    pub fn register_parent_session(
        &mut self,
        session_id: &str,
        project_name: &str,
        agent_name: &str,
    ) {
        self.parent_sessions.insert(
            session_id.to_string(),
            (project_name.to_string(), agent_name.to_string()),
        );
        if let Some(key) = self.find_bound_group_key(session_id) {
            if let Some(group) = self.groups.get_mut(&key) {
                group.parent_pending_done = false;
            }
        }
    }

    /// 按 group_key 归并或更新 Subagent 状态
    #[allow(clippy::too_many_arguments)]
    pub fn apply_subagent_status(
        &mut self,
        group_key: &str,
        parent_session_id: Option<&str>,
        worker_id: &str,
        worker_name: &str,
        project_name: &str,
        agent_name: &str,
        status: SwarmWorkerStatus,
        task_summary: &str,
        step_index: u64,
    ) {
        self.apply_subagent_status_at(
            group_key,
            parent_session_id,
            worker_id,
            worker_name,
            project_name,
            agent_name,
            status,
            task_summary,
            step_index,
            Instant::now(),
        );
    }

    /// 指定时间戳版本的状态更新（便于确定性单元测试）
    #[allow(clippy::too_many_arguments)]
    pub fn apply_subagent_status_at(
        &mut self,
        group_key: &str,
        parent_session_id: Option<&str>,
        worker_id: &str,
        worker_name: &str,
        project_name: &str,
        agent_name: &str,
        status: SwarmWorkerStatus,
        task_summary: &str,
        step_index: u64,
        now: Instant,
    ) {
        let mut migrated_worker_started_at: Option<Instant> = None;
        let mut migrated_group_started_at: Option<Instant> = None;

        // 若此前在未绑定回退组登记过同一 worker，迁出以免重复计数
        if let Some(pid) = parent_session_id {
            let mut empty_fallback_key: Option<String> = None;
            for (existing_key, group) in self.groups.iter_mut() {
                if existing_key != group_key
                    && (group.parent_session_id.is_none()
                        || group.parent_session_id.as_deref() == Some(pid))
                {
                    if let Some(old_worker) = group.workers.remove(worker_id) {
                        migrated_worker_started_at = Some(old_worker.started_at);
                        if group.workers.is_empty() {
                            migrated_group_started_at = Some(group.started_at);
                            empty_fallback_key = Some(existing_key.clone());
                        }
                        break;
                    }
                }
            }
            if let Some(old_key) = empty_fallback_key {
                self.groups.remove(&old_key);
            }
        }

        let target_key = if self.groups.contains_key(group_key) {
            group_key.to_string()
        } else if let Some(pid) = parent_session_id {
            self.find_bound_group_key(pid)
                .unwrap_or_else(|| group_key.to_string())
        } else {
            group_key.to_string()
        };

        let group = self.groups.entry(target_key.clone()).or_insert_with(|| SwarmGroup {
            group_key: target_key,
            parent_session_id: parent_session_id.map(|s| s.to_string()),
            project_name: project_name.to_string(),
            agent_name: agent_name.to_string(),
            workers: HashMap::new(),
            started_at: migrated_group_started_at.unwrap_or(now),
            last_update: now,
            parent_pending_done: false,
            had_active_work: false,
        });

        if group.parent_session_id.is_none() {
            if let Some(pid) = parent_session_id {
                group.parent_session_id = Some(pid.to_string());
            }
        }
        if !project_name.is_empty() && (group.project_name.is_empty() || group.project_name == "WNoobar") {
            group.project_name = project_name.to_string();
        }
        if group.agent_name.is_empty() && !agent_name.is_empty() {
            group.agent_name = agent_name.to_string();
        }
        group.last_update = now;

        let display_name = if !worker_name.trim().is_empty() {
            worker_name.trim().to_string()
        } else {
            worker_id.to_string()
        };
        let role = derive_worker_role(&display_name);

        let prev_status = group.workers.get(worker_id).map(|w| w.status);
        let is_new_worker = prev_status.is_none();

        if let Some(existing) = group.workers.get_mut(worker_id) {
            if !worker_name.trim().is_empty() {
                existing.display_name = display_name;
                existing.role = role;
            }
            existing.status = status;
            if !task_summary.is_empty() {
                existing.task_summary = task_summary.to_string();
            }
            existing.step_index = step_index;
            existing.last_update = now;
        } else {
            group.workers.insert(
                worker_id.to_string(),
                SwarmWorkerInfo {
                    worker_id: worker_id.to_string(),
                    display_name,
                    role,
                    status,
                    task_summary: task_summary.to_string(),
                    step_index,
                    started_at: migrated_worker_started_at.unwrap_or(now),
                    last_update: now,
                },
            );
        }

        if status.is_active() {
            group.had_active_work = true;
            // 当已收束（Done/Waiting）节点被重新唤醒、或刚收到新活跃节点时，若此前因同轮子代理尚未结束而挂起需重新观察
            let reactivated = matches!(
                prev_status,
                Some(SwarmWorkerStatus::Done | SwarmWorkerStatus::Waiting)
            );
            if reactivated && !is_new_worker {
                group.parent_pending_done = false;
            }
        }
    }

    /// 当父会话报告 Done 时判定是否应延迟收官：
    /// 若匹配到对应 Swarm 组且仍有 Working / ActionNeeded 节点，则标记 parent_pending_done 并返回 true。
    pub fn should_defer_parent_done(&mut self, session_id: &str) -> bool {
        let matched_key = self.find_or_bind_group_key_for_parent(session_id);
        let Some(key) = matched_key else {
            return false;
        };
        let Some(group) = self.groups.get_mut(&key) else {
            return false;
        };
        let has_active = group.workers.values().any(|w| w.status.is_active());
        // 父会话已报告 Done：无论是否延后，都记录“父侧已收尾”，
        // 这样活跃节点收敛后（或立即）才允许整组收官。
        group.parent_pending_done = true;
        has_active
    }

    /// 若指定组（按 group_key 或 parent_session_id）已整体收官，产出收官摘要元组并清理该组：
    /// 返回 `(session_id, project_name, agent_name, summary, elapsed_secs)`
    pub fn pop_completed_group_done(
        &mut self,
        group_key: &str,
    ) -> Option<(String, String, String, String, u64)> {
        self.pop_completed_group_done_at(group_key, Instant::now())
    }

    /// 指定时间戳版本的组收官弹出（便于确定性单元测试）
    pub fn pop_completed_group_done_at(
        &mut self,
        group_key: &str,
        now: Instant,
    ) -> Option<(String, String, String, String, u64)> {
        let resolved_key = self.find_bound_group_key(group_key)?;
        let should_pop = {
            let group = self.groups.get(&resolved_key)?;
            if group.workers.is_empty() {
                false
            } else {
                let (action_count, working_count, done_count, waiting_count) =
                    count_worker_statuses(&group.workers);
                let settled = working_count == 0 && action_count == 0;
                // 仅当父会话已报告 Done 才允许收官；
                // 唯一例外：确无父会话的孤立回退组（无已绑定/已登记父会话、无 Waiting 监控节点）且全员终态。
                let orphan_fallback = group.parent_session_id.is_none()
                    && !self
                        .parent_sessions
                        .values()
                        .any(|(p, a)| *p == group.project_name && *a == group.agent_name)
                    && waiting_count == 0
                    && done_count > 0
                    && group.had_active_work;
                settled && (group.parent_pending_done || orphan_fallback)
            }
        };

        if !should_pop {
            return None;
        }

        let group = self.groups.remove(&resolved_key)?;
        let (_, _, done_count, waiting_count) = count_worker_statuses(&group.workers);
        let non_waiting_total = group.workers.len().saturating_sub(waiting_count);
        let summary = if non_waiting_total == 0 {
            "Swarm 已收官".to_string()
        } else {
            format!("Swarm {}/{} 已收官", done_count, non_waiting_total)
        };
        let session_id = group
            .parent_session_id
            .unwrap_or_else(|| group.group_key.clone());
        let elapsed_secs = now.saturating_duration_since(group.started_at).as_secs().max(1);
        Some((
            session_id,
            group.project_name,
            group.agent_name,
            summary,
            elapsed_secs,
        ))
    }

    /// 扫描并弹出所有已具备收官条件的 Swarm 组
    pub fn pop_ready_group_dones(&mut self) -> Vec<(String, String, String, String, u64)> {
        let keys: Vec<String> = self.groups.keys().cloned().collect();
        let now = Instant::now();
        let mut results = Vec::new();
        for key in keys {
            if let Some(done) = self.pop_completed_group_done_at(&key, now) {
                results.push(done);
            }
        }
        results
    }

    /// 按 group_key 或 parent_session_id 获取当前 Swarm 聚合摘要
    pub fn summary_for_group(&self, key_or_session: &str) -> Option<SwarmSummary> {
        self.summary_for_group_at(key_or_session, Instant::now())
    }

    pub fn summary_for_group_at(
        &self,
        key_or_session: &str,
        now: Instant,
    ) -> Option<SwarmSummary> {
        let key = self.find_bound_group_key(key_or_session)?;
        self.groups.get(&key).map(|g| build_summary(g, now))
    }

    /// 按项目名与 Agent 名查找最紧急/最近的 Swarm 聚合摘要
    pub fn summary_for_project_agent(
        &self,
        project_name: &str,
        agent_name: &str,
    ) -> Option<SwarmSummary> {
        let now = Instant::now();
        self.groups
            .values()
            .filter(|g| g.project_name == project_name && g.agent_name == agent_name)
            .map(|g| (g, build_summary(g, now)))
            .max_by(|(ga, sa), (gb, sb)| {
                sa.action_count
                    .cmp(&sb.action_count)
                    .then_with(|| sa.working_count.cmp(&sb.working_count))
                    .then_with(|| ga.last_update.cmp(&gb.last_update))
                    .then_with(|| gb.group_key.cmp(&ga.group_key))
            })
            .map(|(_, s)| s)
    }

    /// 返回所有活跃 Swarm 组摘要（按紧急度排序）
    pub fn all_summaries(&self) -> Vec<SwarmSummary> {
        let now = Instant::now();
        let mut list: Vec<(&SwarmGroup, SwarmSummary)> = self
            .groups
            .values()
            .map(|g| (g, build_summary(g, now)))
            .collect();
        list.sort_by(|(ga, sa), (gb, sb)| {
            sb.action_count
                .cmp(&sa.action_count)
                .then_with(|| sb.working_count.cmp(&sa.working_count))
                .then_with(|| gb.last_update.cmp(&ga.last_update))
                .then_with(|| ga.group_key.cmp(&gb.group_key))
        });
        list.into_iter().map(|(_, s)| s).collect()
    }

    pub fn active_group_count(&self) -> usize {
        self.groups.len()
    }

    pub fn clear_group(&mut self, key_or_session: &str) {
        if let Some(key) = self.find_bound_group_key(key_or_session) {
            self.groups.remove(&key);
        }
    }

    /// 清理超时的工作节点与空/超时 Swarm 组；返回被整体淘汰的组 `(group_key, parent_session_id)`
    pub fn retire_stale(
        &mut self,
        stale_worker_secs: u64,
        stale_group_secs: u64,
    ) -> Vec<(String, Option<String>)> {
        self.retire_stale_at(Instant::now(), stale_worker_secs, stale_group_secs)
    }

    pub fn retire_stale_at(
        &mut self,
        now: Instant,
        stale_worker_secs: u64,
        stale_group_secs: u64,
    ) -> Vec<(String, Option<String>)> {
        let worker_limit = Duration::from_secs(stale_worker_secs);
        let group_limit = Duration::from_secs(stale_group_secs);

        for group in self.groups.values_mut() {
            let group_alive = now.saturating_duration_since(group.last_update) < group_limit;
            group.workers.retain(|_, w| {
                let fresh = now.saturating_duration_since(w.last_update) < worker_limit;
                // 组整体仍存活时，保留已收束（Done / Waiting）节点，避免汇总比例缩水；
                // 仅驱逐长期无心跳的 Working / ActionNeeded 节点。
                fresh || (group_alive && !w.status.is_active())
            });
        }

        let mut retired = Vec::new();
        self.groups.retain(|_, group| {
            let keep = !group.workers.is_empty()
                && now.saturating_duration_since(group.last_update) < group_limit;
            if !keep {
                retired.push((group.group_key.clone(), group.parent_session_id.clone()));
            }
            keep
        });
        retired
    }


    fn find_bound_group_key(&self, key_or_session: &str) -> Option<String> {
        if self.groups.contains_key(key_or_session) {
            return Some(key_or_session.to_string());
        }
        self.groups
            .iter()
            .find(|(_, g)| g.parent_session_id.as_deref() == Some(key_or_session))
            .map(|(k, _)| k.clone())
    }

    fn find_or_bind_group_key_for_parent(&mut self, session_id: &str) -> Option<String> {
        if let Some(key) = self.find_bound_group_key(session_id) {
            return Some(key);
        }
        let (proj, agent) = self.parent_sessions.get(session_id)?.clone();
        let candidates: Vec<String> = self
            .groups
            .iter()
            .filter(|(_, g)| {
                g.parent_session_id.is_none()
                    && g.project_name == proj
                    && g.agent_name == agent
            })
            .map(|(k, _)| k.clone())
            .collect();
        if candidates.len() == 1 {
            let key = candidates[0].clone();
            if let Some(group) = self.groups.get_mut(&key) {
                group.parent_session_id = Some(session_id.to_string());
            }
            Some(key)
        } else {
            None
        }
    }
}

fn derive_worker_role(display_name: &str) -> String {
    let trimmed = display_name.trim();
    if let Some(stripped) = trimmed.strip_prefix('@') {
        if !stripped.is_empty() {
            return stripped.to_string();
        }
    }
    trimmed.to_string()
}

fn count_worker_statuses(workers: &HashMap<String, SwarmWorkerInfo>) -> (usize, usize, usize, usize) {
    let mut action_count = 0;
    let mut working_count = 0;
    let mut done_count = 0;
    let mut waiting_count = 0;
    for w in workers.values() {
        match w.status {
            SwarmWorkerStatus::ActionNeeded => action_count += 1,
            SwarmWorkerStatus::Working => working_count += 1,
            SwarmWorkerStatus::Done => done_count += 1,
            SwarmWorkerStatus::Waiting => waiting_count += 1,
        }
    }
    (action_count, working_count, done_count, waiting_count)
}

fn format_aggregate_summary(
    total: usize,
    action_count: usize,
    working_count: usize,
    done_count: usize,
    waiting_count: usize,
) -> String {
    if action_count > 0 {
        let mut s = format!("⚠ {} 待确认", action_count);
        if working_count > 0 {
            s.push_str(&format!(" · {} 运行中", working_count));
        } else if waiting_count > 0 {
            s.push_str(&format!(" · {} 待命", waiting_count));
        } else if done_count > 0 {
            s.push_str(&format!(" · {} 完成", done_count));
        }
        return s;
    }

    if working_count > 0 {
        let mut s = format!("{}/{} 运行中", working_count, total);
        if done_count > 0 {
            s.push_str(&format!(" · {} 完成", done_count));
        }
        if waiting_count > 0 {
            s.push_str(&format!(" · {} 待命", waiting_count));
        }
        return s;
    }

    if waiting_count > 0 {
        if done_count > 0 {
            return format!("{} 待命 · {} 完成", waiting_count, done_count);
        }
        return format!("{}/{} 待命监控", waiting_count, total);
    }

    if done_count > 0 {
        return format!("{}/{} 已完成", done_count, total);
    }

    "Swarm 就绪".to_string()
}

fn build_summary(group: &SwarmGroup, now: Instant) -> SwarmSummary {
    let (action_count, working_count, done_count, waiting_count) =
        count_worker_statuses(&group.workers);
    let mut workers: Vec<SwarmWorkerInfo> = group.workers.values().cloned().collect();
    workers.sort_by(|a, b| {
        a.status
            .priority_rank()
            .cmp(&b.status.priority_rank())
            .then_with(|| Reverse(a.last_update).cmp(&Reverse(b.last_update)))
            .then_with(|| a.display_name.cmp(&b.display_name))
            .then_with(|| a.worker_id.cmp(&b.worker_id))
    });
    let total_workers = workers.len();
    let aggregate_summary = format_aggregate_summary(
        total_workers,
        action_count,
        working_count,
        done_count,
        waiting_count,
    );

    SwarmSummary {
        group_key: group.group_key.clone(),
        parent_session_id: group.parent_session_id.clone(),
        project_name: group.project_name.clone(),
        agent_name: group.agent_name.clone(),
        total_workers,
        working_count,
        action_count,
        done_count,
        waiting_count,
        workers,
        aggregate_summary,
        elapsed_secs: now.saturating_duration_since(group.started_at).as_secs(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_status_counts_priority_sorting_and_summary() {
        let mut reg = SwarmRegistry::new();
        let t0 = Instant::now();

        reg.apply_subagent_status_at(
            "parent-1",
            Some("parent-1"),
            "w-wait",
            "@monitor",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Waiting,
            "待命监控",
            3,
            t0,
        );
        reg.apply_subagent_status_at(
            "parent-1",
            Some("parent-1"),
            "w-done",
            "@font_eng",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Done,
            "字体修复完成",
            8,
            t0 + Duration::from_secs(1),
        );
        reg.apply_subagent_status_at(
            "parent-1",
            Some("parent-1"),
            "w-work-1",
            "@core_eng",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "编写 swarm.rs",
            5,
            t0 + Duration::from_secs(2),
        );
        reg.apply_subagent_status_at(
            "parent-1",
            Some("parent-1"),
            "w-work-2",
            "@ui_eng",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "调整排版",
            6,
            t0 + Duration::from_secs(3),
        );

        let sum = reg
            .summary_for_group_at("parent-1", t0 + Duration::from_secs(10))
            .expect("summary should exist");
        assert_eq!(sum.total_workers, 4);
        assert_eq!(sum.working_count, 2);
        assert_eq!(sum.action_count, 0);
        assert_eq!(sum.done_count, 1);
        assert_eq!(sum.waiting_count, 1);
        assert_eq!(sum.elapsed_secs, 10);
        assert_eq!(sum.aggregate_summary, "2/4 运行中 · 1 完成 · 1 待命");

        // 排序验证：Working 在前，其次 Done，最后 Waiting
        assert_eq!(sum.workers[0].worker_id, "w-work-2");
        assert_eq!(sum.workers[1].worker_id, "w-work-1");
        assert_eq!(sum.workers[2].worker_id, "w-done");
        assert_eq!(sum.workers[3].worker_id, "w-wait");
        assert_eq!(sum.workers[2].display_name, "@font_eng");
        assert_eq!(sum.workers[2].role, "font_eng");
    }

    #[test]
    fn test_single_subagent_done_does_not_trigger_group_done() {
        let mut reg = SwarmRegistry::new();
        let t0 = Instant::now();

        reg.apply_subagent_status_at(
            "parent-2",
            Some("parent-2"),
            "w1",
            "@w1",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "任务 1",
            1,
            t0,
        );
        reg.apply_subagent_status_at(
            "parent-2",
            Some("parent-2"),
            "w2",
            "@w2",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "任务 2",
            1,
            t0,
        );

        // 仅 w1 完成时，w2 仍在运行，绝不可误报整组 Done
        reg.apply_subagent_status_at(
            "parent-2",
            Some("parent-2"),
            "w1",
            "@w1",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Done,
            "w1 完成",
            4,
            t0 + Duration::from_secs(2),
        );
        assert!(reg
            .pop_completed_group_done_at("parent-2", t0 + Duration::from_secs(2))
            .is_none());

        reg.apply_subagent_status_at(
            "parent-2",
            Some("parent-2"),
            "w2",
            "@w2",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Done,
            "w2 完成",
            6,
            t0 + Duration::from_secs(4),
        );
        // w2 也完成后，父会话（parent-2）尚未 Done，仍不得抢跑整组收官
        assert!(reg
            .pop_completed_group_done_at("parent-2", t0 + Duration::from_secs(4))
            .is_none());
        assert!(reg.pop_ready_group_dones().is_empty());

        // 父会话报告 Done（无活跃节点，不延后）后才产出整组收官并清理该组
        assert!(!reg.should_defer_parent_done("parent-2"));
        let done = reg
            .pop_completed_group_done_at("parent-2", t0 + Duration::from_secs(4))
            .expect("group should now be done");
        assert_eq!(done.0, "parent-2");
        assert_eq!(done.1, "WNoobar");
        assert_eq!(done.2, "Antigravity");
        assert_eq!(done.3, "Swarm 2/2 已收官");
        assert_eq!(done.4, 4);
        assert!(reg.summary_for_group("parent-2").is_none());
    }

    #[test]
    fn test_parent_deferral_and_fallback_autobind() {
        let mut reg = SwarmRegistry::new();
        let t0 = Instant::now();

        reg.register_parent_session("sess-coord", "WNoobar", "Antigravity");

        // Subagent 初始没有显式 parent_session_id，落入 Antigravity::WNoobar 回退组
        reg.apply_subagent_status_at(
            "Antigravity::WNoobar",
            None,
            "w1",
            "@core",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "实现核心逻辑",
            1,
            t0,
        );

        // Parent 尝试 Done：应通过 project+agent 唯一匹配自动绑定，并延迟收官
        assert!(reg.should_defer_parent_done("sess-coord"));
        assert!(reg.pop_completed_group_done_at("sess-coord", t0 + Duration::from_secs(1)).is_none());

        // 通过 parent_session_id 也可查询到绑定后的 SwarmSummary
        let sum = reg
            .summary_for_group("sess-coord")
            .expect("auto-bound group should be queryable by parent session_id");
        assert_eq!(sum.parent_session_id.as_deref(), Some("sess-coord"));

        // 当 worker 结束进入 swarm_wait (Waiting) 且不再有活跃节点时，因 parent_pending_done=true 触发整组收官
        reg.apply_subagent_status_at(
            "Antigravity::WNoobar",
            None,
            "w1",
            "@core",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Waiting,
            "待命监控",
            5,
            t0 + Duration::from_secs(5),
        );
        let done = reg
            .pop_completed_group_done_at("sess-coord", t0 + Duration::from_secs(5))
            .expect("pending parent done should settle once workers leave Working");
        assert_eq!(done.0, "sess-coord");
        assert_eq!(done.3, "Swarm 已收官");
    }

    #[test]
    fn test_action_needed_bubbles_and_blocks_done() {
        let mut reg = SwarmRegistry::new();
        let t0 = Instant::now();

        reg.apply_subagent_status_at(
            "g-action",
            Some("g-action"),
            "w1",
            "@eng1",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "构建中",
            2,
            t0,
        );
        reg.apply_subagent_status_at(
            "g-action",
            Some("g-action"),
            "w2",
            "@eng2",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "测试中",
            3,
            t0,
        );
        reg.apply_subagent_status_at(
            "g-action",
            Some("g-action"),
            "w3",
            "@eng3",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::ActionNeeded,
            "请选择部署环境",
            4,
            t0 + Duration::from_secs(1),
        );

        let sum = reg.summary_for_group("g-action").unwrap();
        assert_eq!(sum.action_count, 1);
        assert_eq!(sum.working_count, 2);
        assert_eq!(sum.workers[0].status, SwarmWorkerStatus::ActionNeeded);
        assert_eq!(sum.workers[0].worker_id, "w3");
        assert_eq!(sum.aggregate_summary, "⚠ 1 待确认 · 2 运行中");

        // 存在 ActionNeeded 时必须阻止 Parent 过早 Done 且不可弹出收官
        assert!(reg.should_defer_parent_done("g-action"));
        assert!(reg.pop_completed_group_done("g-action").is_none());

        // 用户确认并完成所有节点后，ActionNeeded 解除并正常收官
        reg.apply_subagent_status_at(
            "g-action",
            Some("g-action"),
            "w1",
            "@eng1",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Done,
            "完成",
            5,
            t0 + Duration::from_secs(2),
        );
        reg.apply_subagent_status_at(
            "g-action",
            Some("g-action"),
            "w2",
            "@eng2",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Done,
            "完成",
            5,
            t0 + Duration::from_secs(2),
        );
        reg.apply_subagent_status_at(
            "g-action",
            Some("g-action"),
            "w3",
            "@eng3",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Done,
            "完成",
            6,
            t0 + Duration::from_secs(3),
        );

        let done = reg
            .pop_completed_group_done_at("g-action", t0 + Duration::from_secs(3))
            .expect("should complete after ActionNeeded resolves");
        assert_eq!(done.3, "Swarm 3/3 已收官");
    }

    #[test]
    fn test_worker_reactivation_cancels_parent_pending_done() {
        let mut reg = SwarmRegistry::new();
        let t0 = Instant::now();

        reg.apply_subagent_status_at(
            "g-reactivate",
            Some("g-reactivate"),
            "w1",
            "@w1",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "轮次1",
            1,
            t0,
        );
        assert!(reg.should_defer_parent_done("g-reactivate"));

        // 模拟 w1 先停在 Waiting 后又收到新消息被唤醒为 Working，应取消此前 parent_pending_done
        reg.apply_subagent_status_at(
            "g-reactivate",
            Some("g-reactivate"),
            "w1",
            "@w1",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Waiting,
            "待命中",
            2,
            t0 + Duration::from_secs(1),
        );
        // 先别 pop，模拟此时 w1 在等待期内被新任务重新唤醒
        reg.apply_subagent_status_at(
            "g-reactivate",
            Some("g-reactivate"),
            "w1",
            "@w1",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "轮次2",
            3,
            t0 + Duration::from_secs(2),
        );
        assert!(reg.pop_completed_group_done("g-reactivate").is_none());

        // 若再次进入 Waiting（且 parent 未重新完成），此时不再因旧 parent_pending_done 误弹出
        reg.apply_subagent_status_at(
            "g-reactivate",
            Some("g-reactivate"),
            "w1",
            "@w1",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Waiting,
            "继续待命",
            4,
            t0 + Duration::from_secs(3),
        );
        assert!(reg.pop_completed_group_done("g-reactivate").is_none());
    }

    #[test]
    fn test_retire_stale_workers_and_groups() {
        let mut reg = SwarmRegistry::new();
        let t0 = Instant::now();

        reg.apply_subagent_status_at(
            "g-active",
            Some("g-active"),
            "w-old",
            "@old",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "旧任务",
            1,
            t0,
        );
        reg.apply_subagent_status_at(
            "g-active",
            Some("g-active"),
            "w-fresh",
            "@fresh",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "新任务",
            2,
            t0 + Duration::from_secs(50),
        );
        reg.apply_subagent_status_at(
            "g-stale",
            Some("g-stale"),
            "w-dead",
            "@dead",
            "Other",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "僵死任务",
            1,
            t0,
        );

        reg.apply_subagent_status_at(
            "g-active",
            Some("g-active"),
            "w-done",
            "@done",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Done,
            "早先已完成",
            9,
            t0,
        );

        // 在 t0 + 70s 执行清理：worker 超时阈值 60s，group 超时阈值 120s
        let retired = reg.retire_stale_at(t0 + Duration::from_secs(70), 60, 120);

        // g-active 保留 w-fresh，驱逐 w-old；先期 Done 的兄弟节点在组仍存活时保留，汇总比例不缩水
        let active_sum = reg.summary_for_group("g-active").expect("g-active should remain");
        assert_eq!(active_sum.total_workers, 2);
        assert_eq!(active_sum.done_count, 1);
        assert_eq!(active_sum.working_count, 1);
        assert!(active_sum.workers.iter().any(|w| w.worker_id == "w-fresh"));
        assert!(active_sum.workers.iter().all(|w| w.worker_id != "w-old"));
        // g-stale 因唯一 worker 被驱逐而整体清理，并回传被淘汰组标识
        assert!(reg.summary_for_group("g-stale").is_none());
        assert_eq!(reg.active_group_count(), 1);
        assert_eq!(
            retired,
            vec![("g-stale".to_string(), Some("g-stale".to_string()))]
        );
    }

    #[test]
    fn test_pop_requires_parent_done_and_excludes_waiting_from_denominator() {
        let mut reg = SwarmRegistry::new();
        let t0 = Instant::now();
        reg.register_parent_session("p-9", "WNoobar", "Antigravity");
        for (id, st) in [
            ("a", SwarmWorkerStatus::Done),
            ("b", SwarmWorkerStatus::Done),
            ("m", SwarmWorkerStatus::Waiting),
        ] {
            reg.apply_subagent_status_at(
                "p-9",
                Some("p-9"),
                id,
                &format!("@{id}"),
                "WNoobar",
                "Antigravity",
                st,
                "t",
                1,
                t0,
            );
        }
        // 父会话尚未 Done：即便无活跃节点也不可抢跑收官
        assert!(reg.pop_ready_group_dones().is_empty());
        // 父会话 Done，且无活跃节点：不延后，随后收官，分母排除 Waiting 监控节点
        assert!(!reg.should_defer_parent_done("p-9"));
        let done = reg.pop_completed_group_done("p-9").expect("group done");
        assert_eq!(done.3, "Swarm 2/2 已收官");
    }

    #[test]
    fn test_orphan_fallback_group_pops_only_when_fully_terminal() {
        let mut reg = SwarmRegistry::new();
        let t0 = Instant::now();
        reg.apply_subagent_status_at(
            "Antigravity::Orphan",
            None,
            "w1",
            "@w1",
            "Orphan",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "t",
            1,
            t0,
        );
        reg.apply_subagent_status_at(
            "Antigravity::Orphan",
            None,
            "w1",
            "@w1",
            "Orphan",
            "Antigravity",
            SwarmWorkerStatus::Done,
            "t",
            2,
            t0,
        );
        reg.apply_subagent_status_at(
            "Antigravity::Orphan",
            None,
            "w2",
            "@w2",
            "Orphan",
            "Antigravity",
            SwarmWorkerStatus::Waiting,
            "t",
            1,
            t0,
        );
        // 仍有 Waiting 节点：不收官
        assert!(reg.pop_ready_group_dones().is_empty());
        reg.apply_subagent_status_at(
            "Antigravity::Orphan",
            None,
            "w2",
            "@w2",
            "Orphan",
            "Antigravity",
            SwarmWorkerStatus::Done,
            "t",
            2,
            t0,
        );
        let done = reg.pop_ready_group_dones();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].3, "Swarm 2/2 已收官");
    }


    #[test]
    fn test_worker_migration_from_fallback_to_parent_group() {
        let mut reg = SwarmRegistry::new();
        let t0 = Instant::now();

        reg.apply_subagent_status_at(
            "Antigravity::WNoobar",
            None,
            "sub-1",
            "系统研发",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "初始化",
            0,
            t0,
        );
        reg.apply_subagent_status_at(
            "parent-uuid",
            Some("parent-uuid"),
            "sub-1",
            "@core_eng",
            "WNoobar",
            "Antigravity",
            SwarmWorkerStatus::Working,
            "解析就绪",
            1,
            t0 + Duration::from_secs(2),
        );

        assert_eq!(reg.active_group_count(), 1);
        let sum = reg
            .summary_for_group_at("parent-uuid", t0 + Duration::from_secs(5))
            .unwrap();
        assert_eq!(sum.total_workers, 1);
        assert_eq!(sum.workers[0].display_name, "@core_eng");
        assert_eq!(sum.elapsed_secs, 5);
    }
}
