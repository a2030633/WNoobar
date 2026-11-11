#![allow(dead_code)]
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub enum IslandState {
    /// 待机状态：精简胶囊
    Idle,
    /// 启动节拍：弹一下致意（1.8s后平滑贴边隐藏）
    Started {
        session_id: String,
        project_name: String,
        role_type: String,
        agent_name: String,
        task_summary: String,
        started_at: Instant,
        auto_hide_secs: f32,
    },
    /// 处理中：微胶囊，后台静默执行
    Working {
        session_id: String,
        project_name: String,
        role_type: String,
        agent_name: String,
        task_summary: String,
        step_index: u64,
        start_time: Instant,
    },
    /// 需要用户介入
    ActionNeeded {
        session_id: String,
        project_name: String,
        role_type: String,
        agent_name: String,
        prompt: String,
    },
    /// 任务完成：顺滑展开成果总结卡片，停留 3.5s 后贴边隐藏
    Done {
        session_id: String,
        project_name: String,
        role_type: String,
        agent_name: String,
        summary: String,
        duration_secs: u64,
        artifacts_count: usize,
        finished_at: Instant,
        auto_collapse_secs: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDockMode {
    Peeking,      // 贴边隐藏：只露出顶部 2.5px 极细微光条
    Normal,       // 正常浮动：标准胶囊位置
    DeckExpanded, // 展开会话抽屉（Session Deck）
}

impl IslandState {
    pub fn is_idle(&self) -> bool {
        matches!(self, IslandState::Idle)
    }

    pub fn target_dimensions(&self, dock_mode: EdgeDockMode) -> (f32, f32) {
        match dock_mode {
            EdgeDockMode::Peeking => (140.0, 4.0),
            EdgeDockMode::DeckExpanded => (440.0, 190.0),
            EdgeDockMode::Normal => match self {
                IslandState::Idle => (210.0, 32.0),
                IslandState::Started { .. } => (320.0, 38.0),
                IslandState::Working { project_name, role_type, task_summary, .. } => {
                    let total_chars = project_name.chars().count() + role_type.chars().count() + task_summary.chars().count();
                    if total_chars <= 16 {
                        (360.0, 38.0)
                    } else {
                        (420.0, 38.0)
                    }
                }
                IslandState::ActionNeeded { prompt, .. } => {
                    if prompt.chars().count() <= 24 {
                        (380.0, 60.0)
                    } else {
                        (440.0, 76.0)
                    }
                }
                IslandState::Done { summary, .. } => {
                    if summary.chars().count() <= 24 {
                        (380.0, 58.0)
                    } else {
                        (440.0, 72.0)
                    }
                }
            },
        }
    }
}

/// 3D 翻滚动画状态跟踪
#[derive(Debug, Clone)]
pub struct FlipTransition {
    pub progress: f32,
    pub is_animating: bool,
    pub previous_text: String,
    pub next_text: String,
}

impl FlipTransition {
    pub fn new() -> Self {
        Self {
            progress: 1.0,
            is_animating: false,
            previous_text: String::new(),
            next_text: String::new(),
        }
    }

    pub fn trigger(&mut self, prev: String, next: String) {
        self.previous_text = prev;
        self.next_text = next;
        self.progress = 0.0;
        self.is_animating = true;
    }

    pub fn update(&mut self, dt: f32) -> bool {
        if !self.is_animating {
            return false;
        }
        self.progress += dt * 3.5;
        if self.progress >= 1.0 {
            self.progress = 1.0;
            self.is_animating = false;
        }
        true
    }
}
