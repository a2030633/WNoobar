#![allow(dead_code)]
use std::time::Instant;
use crossbeam_channel::{unbounded, Receiver, Sender};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NoiseFilterLevel {
    Silent,   // 极简：仅关键节点与完成提示
    #[default]
    Normal,   // 默认：聚合防抖（600-1000ms），展示关键里程碑
    Verbose,  // 详细：所有具体子动作
}

#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub project_name: String,
    pub role_type: String,
    pub agent_name: String,
    pub instruction: String,
    pub step_index: u64,
    pub is_working: bool,
    pub last_active: Instant,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// 任务启动：轻盈弹一下致意（1.8s后自动贴边隐藏）
    TaskStarted {
        session_id: String,
        project_name: String,
        role_type: String,
        agent_name: String,
        task_summary: String,
    },
    /// 执行中：静默更新内部进度，严禁打扰用户
    Working {
        session_id: String,
        project_name: String,
        role_type: String,
        agent_name: String,
        task_summary: String,
        step_index: u64,
    },
    /// 任务彻底完成：再次顺滑弹出大卡片，展示成果
    Done {
        session_id: String,
        project_name: String,
        role_type: String,
        agent_name: String,
        summary: String,
        duration_secs: u64,
        artifacts_count: usize,
    },
    ActionNeeded {
        session_id: String,
        project_name: String,
        role_type: String,
        agent_name: String,
        prompt: String,
    },
    SwitchSession(String),
    ResetIdle,
}

#[derive(Clone)]
pub struct EventBus {
    sender: Sender<AgentEvent>,
    receiver: Receiver<AgentEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, receiver) = unbounded();
        Self { sender, receiver }
    }

    pub fn sender(&self) -> Sender<AgentEvent> {
        self.sender.clone()
    }

    pub fn receiver(&self) -> &Receiver<AgentEvent> {
        &self.receiver
    }
}
