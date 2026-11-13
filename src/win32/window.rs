#![allow(dead_code)]
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC,
    RGBQUAD,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NOTIFYICONDATAW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::core::bus::{AgentEvent, NoiseFilterLevel};
use crate::core::config::{AppConfig, MonitorMode, SharedNoiseFilter};
use crate::core::fsm::{EdgeDockMode, IslandState};
use crate::core::swarm::{SwarmRegistry, SwarmSummary, SwarmWorkerStatus};
use crate::ui::capsule::{resolve_focused_swarm_index, CapsuleRenderer};
use crate::ui::odometer::{ActiveSessionItem, OdometerCarousel};
use crate::ui::spring::SpringValue;
use crate::ui::tray_menu::TrayMenuSnapshot;
use crate::win32::focus;
use crate::win32::gdiplus::{
    create_bitmap_from_bytes, GdipCreateBitmapFromScan0, GdipCreateHICONFromBitmap,
    GdipDeleteGraphics, GdipDisposeImage, GdipGetImageGraphicsContext, GdipScaleWorldTransform,
    GdipSetSmoothingMode, GdipSetTextRenderingHint, MATRIX_ORDER_PREPEND,
    PIXEL_FORMAT_32BPP_PARGB, SMOOTHING_MODE_ANTIALIAS,
    TEXT_RENDERING_HINT_ANTIALIAS_GRID_FIT,
};
use crate::win32::system::{self, CueKind, MonitorPlacement};
use crate::win32::tray_popup::TrayPopupHost;

const WNOOBAR_PNG: &[u8] = include_bytes!("../../assets/icons/wnoobar.png");

const WM_TRAYICON: u32 = WM_USER + 101;
pub const WM_WNOOBAR_WAKE: u32 = WM_USER + 102;

pub(crate) const IDM_TRAY_TOGGLE: usize = 2001;
pub(crate) const IDM_TRAY_DECK: usize = 2002;
pub(crate) const IDM_TRAY_RESET: usize = 2003;
pub(crate) const IDM_TRAY_EXIT: usize = 2004;
pub(crate) const IDM_NOISE_SILENT: usize = 2010;
pub(crate) const IDM_NOISE_NORMAL: usize = 2011;
pub(crate) const IDM_NOISE_VERBOSE: usize = 2012;
pub(crate) const IDM_MONITOR_PRIMARY: usize = 2020;
pub(crate) const IDM_MONITOR_FOLLOW: usize = 2021;
pub(crate) const IDM_TOGGLE_DND: usize = 2030;
pub(crate) const IDM_TOGGLE_SOUND: usize = 2031;
pub(crate) const IDM_TOGGLE_AUTOSTART: usize = 2032;

#[allow(non_snake_case)]
#[repr(C)]
pub struct BLENDFUNCTION {
    pub BlendOp: u8,
    pub BlendFlags: u8,
    pub SourceConstantAlpha: u8,
    pub AlphaFormat: u8,
}
pub const AC_SRC_OVER: u8 = 0;
pub const AC_SRC_ALPHA: u8 = 1;
pub const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
pub const WM_MOUSELEAVE: u32 = 0x02A3;

unsafe extern "system" {
    fn ScreenToClient(hwnd: HWND, lpPoint: *mut POINT) -> i32;
    fn UpdateWindow(hwnd: HWND) -> i32;
    fn SetWindowLongPtrW(hWnd: HWND, nIndex: i32, dwNewLong: isize) -> isize;
    fn GetWindowLongPtrW(hWnd: HWND, nIndex: i32) -> isize;
    fn TrackMouseEvent(lpEventTrack: *mut TRACKMOUSEEVENT) -> i32;
}

#[allow(non_snake_case)]
#[repr(C)]
struct TRACKMOUSEEVENT {
    cbSize: u32,
    dwFlags: u32,
    hwndTrack: HWND,
    dwHoverTime: u32,
}
const TME_LEAVE: u32 = 0x00000002;

pub struct IslandWindow {
    _hwnd: HWND,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventEffects {
    pub tray_tip: Option<String>,
    pub sound_cue: Option<CueKind>,
    pub switch_session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapsuleClickAction {
    None,
    SetDockMode(EdgeDockMode),
    FocusSession(String),
    CollapseToPeeking,
}

const SWARM_HOVER_EXPAND_DELAY: Duration = Duration::from_millis(260);
const HOVER_LEAVE_COLLAPSE_DELAY: Duration = Duration::from_millis(120);
const DONE_PROTECTION_WINDOW: Duration = Duration::from_millis(2500);

/// 纯 Rust 灵动岛状态机与 Swarm 归并控制器（便于 Win32 主循环与集成测试共享同一状态机实现）。
pub struct IslandRuntime {
    pub width_spring: SpringValue,
    pub height_spring: SpringValue,
    pub current_state: IslandState,
    pub dock_mode: EdgeDockMode,
    pub config: AppConfig,
    pub noise_filter: SharedNoiseFilter,
    pub is_dnd_hidden: bool,
    pub is_mouse_over: bool,
    pub mouse_leave_instant: Option<Instant>,
    pub hover_suppress_until: Option<Instant>,
    pub swarm_hover_since: Option<Instant>,
    pub started_auto_hide_at: Option<Instant>,
    pub recent_sessions: Vec<(String, String, String, String, String)>, // id, project, role, desc, agent
    pub active_sessions_pool: Vec<ActiveSessionItem>,
    pub odometer: OdometerCarousel,
    pub swarm_registry: SwarmRegistry,
    /// 记录已播报提示音/弹出的 (group_key, worker_id) -> prompt，防止子代理心跳重复叮咚响
    alerted_swarm_actions: HashMap<(String, String), String>,
}

impl IslandRuntime {
    pub fn new(config: AppConfig, noise_filter: SharedNoiseFilter) -> Self {
        let dock_mode = EdgeDockMode::Peeking;
        let (target_w, target_h) = IslandState::Idle.target_dimensions(dock_mode);
        Self {
            width_spring: SpringValue::new(target_w),
            height_spring: SpringValue::new(target_h),
            current_state: IslandState::Idle,
            dock_mode,
            config,
            noise_filter,
            is_dnd_hidden: false,
            is_mouse_over: false,
            mouse_leave_instant: None,
            hover_suppress_until: None,
            swarm_hover_since: None,
            started_auto_hide_at: None,
            recent_sessions: Vec::new(),
            active_sessions_pool: Vec::new(),
            odometer: OdometerCarousel::new(),
            swarm_registry: SwarmRegistry::new(),
            alerted_swarm_actions: HashMap::new(),
        }
    }

    /// 统一切换边缘停靠模式并同步宽度/高度弹簧目标尺寸。
    pub fn set_dock_mode(&mut self, mode: EdgeDockMode) {
        self.dock_mode = mode;
        if mode != EdgeDockMode::Normal {
            self.swarm_hover_since = None;
            self.started_auto_hide_at = None;
        }
        let (tw, th) = self.current_state.target_dimensions(self.dock_mode);
        self.width_spring.set_target(tw);
        self.height_spring.set_target(th);
    }

    /// 收起胶囊至贴边微缩态并设置短时悬停抑制，防止指针尚在顶部热区时下一帧立刻弹回。
    pub fn collapse_to_peeking_suppressed(&mut self) {
        self.collapse_to_peeking_suppressed_at(Instant::now());
    }

    pub fn collapse_to_peeking_suppressed_at(&mut self, now: Instant) {
        self.is_mouse_over = false;
        self.mouse_leave_instant = None;
        self.swarm_hover_since = None;
        self.started_auto_hide_at = None;
        self.hover_suppress_until = Some(now + Duration::from_millis(450));
        self.set_dock_mode(EdgeDockMode::Peeking);
    }

    /// 抽屉展开（DeckExpanded / SwarmExpanded）期间暂停 Odometer 自动翻滚，避免切换焦点覆盖面板。
    pub fn should_update_odometer(&self) -> bool {
        !matches!(
            self.dock_mode,
            EdgeDockMode::DeckExpanded | EdgeDockMode::SwarmExpanded
        )
    }

    /// 解析当前聚焦的 Swarm 会话条目（供渲染、悬停展开与点击跳转共用；
    /// 与 `CapsuleRenderer::render` 使用同一个 `resolve_focused_swarm_index` 优先级策略，确保所见即所点）。
    pub fn focused_swarm_item(&self) -> Option<&ActiveSessionItem> {
        resolve_focused_swarm_index(
            &self.current_state,
            &self.active_sessions_pool,
            &self.odometer,
            self.dock_mode,
        )
        .map(|i| &self.active_sessions_pool[i])
    }

    pub fn focused_swarm_session_id(&self) -> Option<String> {
        self.focused_swarm_item().map(|item| item.session_id.clone())
    }

    fn has_expandable_focused_swarm(&self) -> bool {
        !matches!(self.current_state, IslandState::Idle | IslandState::Done { .. })
            && self.focused_swarm_item().is_some()
    }

    fn in_done_protection_at(&self, now: Instant) -> bool {
        match &self.current_state {
            IslandState::Done { finished_at, .. } => {
                now.saturating_duration_since(*finished_at) < DONE_PROTECTION_WINDOW
            }
            _ => false,
        }
    }

    /// 悬停与离开防抖状态机（260ms 悬停触发 SwarmExpanded，120ms 离盒收缩回 Peeking）。
    pub fn update_hover_state_at(
        &mut self,
        cap_x: f32,
        cap_y: f32,
        cap_w: f32,
        cap_h: f32,
        log_x: f32,
        log_y: f32,
        logical_w: f32,
        now: Instant,
    ) {
        let center_x = logical_w / 2.0;
        let hover_suppressed = self
            .hover_suppress_until
            .is_some_and(|until| now < until);
        if !hover_suppressed {
            self.hover_suppress_until = None;
        }

        // 阈值 A：唤醒盒 (Enter Box - 紧凑精准)：处于贴边时，只要鼠标进入顶部核心条立即唤醒
        let in_enter_box = log_x >= center_x - 90.0
            && log_x <= center_x + 90.0
            && log_y >= 0.0
            && log_y <= 14.0;

        // 阈值 B：保持盒 (Keep-Alive Box - 宽裕包裹)：处于展开态时，外扩 18px 几何防抖保护
        let in_keep_box = log_x >= cap_x - 18.0
            && log_x <= cap_x + cap_w + 18.0
            && log_y >= -5.0
            && log_y <= cap_y + cap_h + 18.0;

        // 严格胶囊命中盒（用于 Normal 悬停 260ms 自动展开 Swarm 名单）
        let in_capsule_box = cap_w > 0.0
            && cap_h >= 20.0
            && log_x >= cap_x
            && log_x <= cap_x + cap_w
            && log_y >= cap_y
            && log_y <= cap_y + cap_h;

        if self.dock_mode == EdgeDockMode::Peeking {
            self.swarm_hover_since = None;
            if in_enter_box && !hover_suppressed && !self.is_dnd_hidden {
                self.is_mouse_over = true;
                self.mouse_leave_instant = None;
                self.started_auto_hide_at = None;
                self.set_dock_mode(EdgeDockMode::Normal);
            }
        } else {
            if in_keep_box {
                self.is_mouse_over = true;
                self.mouse_leave_instant = None;
                self.started_auto_hide_at = None;
            } else {
                self.swarm_hover_since = None;
                if self.is_mouse_over {
                    self.is_mouse_over = false;
                    self.mouse_leave_instant = Some(now);
                }

                if let Some(leave_time) = self.mouse_leave_instant {
                    if now.saturating_duration_since(leave_time) >= HOVER_LEAVE_COLLAPSE_DELAY {
                        self.mouse_leave_instant = None;
                        self.swarm_hover_since = None;
                        if self.dock_mode == EdgeDockMode::SwarmExpanded {
                            let target = if matches!(self.current_state, IslandState::ActionNeeded { .. }) {
                                EdgeDockMode::Normal
                            } else {
                                EdgeDockMode::Peeking
                            };
                            self.set_dock_mode(target);
                        } else if !matches!(self.current_state, IslandState::ActionNeeded { .. }) {
                            self.set_dock_mode(EdgeDockMode::Peeking);
                        }
                    }
                }
            }

            if self.dock_mode == EdgeDockMode::Normal {
                let can_expand_swarm = in_capsule_box
                    && !hover_suppressed
                    && !self.is_dnd_hidden
                    && self.has_expandable_focused_swarm();
                if can_expand_swarm {
                    match self.swarm_hover_since {
                        None => {
                            self.swarm_hover_since = Some(now);
                        }
                        Some(since) => {
                            if now.saturating_duration_since(since) >= SWARM_HOVER_EXPAND_DELAY {
                                self.set_dock_mode(EdgeDockMode::SwarmExpanded);
                            }
                        }
                    }
                } else {
                    self.swarm_hover_since = None;
                }
            } else {
                self.swarm_hover_since = None;
            }
        }
    }

    /// 定时看门狗与超时推进（Started/Done 自动收起 + Swarm 超时清理与延迟收官队列同步）。
    pub fn tick_timers_at(&mut self, now: Instant) -> EventEffects {
        let mut effects = EventEffects::default();

        // 2. 检查 Started 状态超时（按配置 start_pop_secs 致意后一次性迁移并按需贴边隐藏）
        if let IslandState::Started {
            session_id,
            project_name,
            role_type,
            agent_name,
            task_summary,
            started_at,
            auto_hide_secs,
        } = &self.current_state
        {
            if now.saturating_duration_since(*started_at).as_secs_f32() > *auto_hide_secs {
                if self.active_sessions_pool.is_empty() {
                    self.current_state = IslandState::Idle;
                    effects.tray_tip = Some("WNoobar - 智能灵动岛 (待命就绪)".to_string());
                } else {
                    let pool_summary = self
                        .active_sessions_pool
                        .iter()
                        .find(|i| i.session_id == *session_id)
                        .map(|i| i.task_summary.clone())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| task_summary.clone());
                    self.current_state = IslandState::Working {
                        session_id: session_id.clone(),
                        project_name: project_name.clone(),
                        role_type: role_type.clone(),
                        agent_name: agent_name.clone(),
                        task_summary: pool_summary,
                        step_index: 0,
                        start_time: *started_at,
                    };
                }
                self.started_auto_hide_at = None;
                let target_mode =
                    if !self.is_mouse_over && self.dock_mode == EdgeDockMode::Normal {
                        EdgeDockMode::Peeking
                    } else {
                        self.dock_mode
                    };
                self.set_dock_mode(target_mode);
            }
        } else if let Some(deadline) = self.started_auto_hide_at {
            if now >= deadline {
                self.started_auto_hide_at = None;
                if !self.is_mouse_over
                    && self.dock_mode == EdgeDockMode::Normal
                    && matches!(self.current_state, IslandState::Working { .. })
                {
                    self.set_dock_mode(EdgeDockMode::Peeking);
                }
            }
        }

        // 2.5 活跃会话池与 Swarm 看门狗：清理超时 Worker (300s) 与僵尸组 (600s) 并同步延迟收官
        let retired_groups = self.swarm_registry.retire_stale_at(now, 300, 600);
        for (group_key, parent_sid) in &retired_groups {
            self.clear_alerts_for_group(group_key);
            // 被淘汰组的合成占位条目（role_type == "Swarm"）直接移除；真实父会话条目仅在下方清空 swarm。
            self.active_sessions_pool.retain(|item| {
                let belongs = item.session_id == *group_key
                    || parent_sid.as_deref() == Some(item.session_id.as_str())
                    || item
                        .swarm
                        .as_ref()
                        .is_some_and(|s| s.group_key == *group_key);
                !(belongs && is_synthetic_swarm_item(item))
            });
        }
        for item in &mut self.active_sessions_pool {
            if let Some(prev_swarm) = &item.swarm {
                let refreshed = self
                    .swarm_registry
                    .summary_for_group_at(&item.session_id, now)
                    .or_else(|| {
                        self.swarm_registry
                            .summary_for_group_at(&prev_swarm.group_key, now)
                    });
                if let Some(fresh) = refreshed {
                    item.task_summary = fresh.aggregate_summary.clone();
                    item.swarm = Some(fresh);
                } else {
                    item.swarm = None;
                }
            }
        }
        // 兜底：不再挂载任何 Swarm 的合成占位条目不应占坑（真实父会话不受影响）
        self.active_sessions_pool
            .retain(|item| !(item.swarm.is_none() && is_synthetic_swarm_item(item)));
        self.prune_stale_alerts(now);

        let ready_dones = self.swarm_registry.pop_ready_group_dones();
        self.apply_ready_group_dones(ready_dones, None, now, &mut effects);

        let pool_len_before = self.active_sessions_pool.len();
        self.active_sessions_pool.retain(|item| {
            now.saturating_duration_since(item.last_update_time) < Duration::from_secs(600)
        });
        if self.active_sessions_pool.is_empty() && pool_len_before > 0 {
            if matches!(
                self.current_state,
                IslandState::Working { .. } | IslandState::Started { .. }
            ) {
                self.current_state = IslandState::Idle;
                let target_mode = if !self.is_mouse_over {
                    EdgeDockMode::Peeking
                } else {
                    self.dock_mode
                };
                self.set_dock_mode(target_mode);
                effects.tray_tip = Some("WNoobar - 智能灵动岛 (待命就绪)".to_string());
            }
        }

        if self.dock_mode == EdgeDockMode::SwarmExpanded && self.focused_swarm_item().is_none() {
            let fallback_mode = if self.is_mouse_over {
                EdgeDockMode::Normal
            } else {
                EdgeDockMode::Peeking
            };
            self.set_dock_mode(fallback_mode);
        }

        // 3. 检查 Done 状态超时（按配置 done_pop_secs 展示后平滑贴边隐藏）
        if let IslandState::Done {
            finished_at,
            auto_collapse_secs,
            ..
        } = &self.current_state
        {
            if now.saturating_duration_since(*finished_at).as_secs_f32() > *auto_collapse_secs {
                self.current_state = IslandState::Idle;
                let target_mode = if !self.is_mouse_over {
                    EdgeDockMode::Peeking
                } else {
                    self.dock_mode
                };
                self.set_dock_mode(target_mode);
                effects.tray_tip = Some("WNoobar - 智能灵动岛 (待命就绪)".to_string());
            }
        }

        effects
    }

    /// 消费单条 AgentEvent 并更新状态机、活跃池、Swarm 注册表与近期会话。
    pub fn handle_event(&mut self, event: AgentEvent) -> EventEffects {
        self.handle_event_at(event, Instant::now())
    }

    pub fn handle_event_at(&mut self, event: AgentEvent, now: Instant) -> EventEffects {
        let mut effects = EventEffects::default();
        match event {
            AgentEvent::TaskStarted {
                session_id,
                project_name,
                role_type,
                agent_name,
                task_summary,
            } => {
                effects.tray_tip = Some(format!("WNoobar - 启动: {}", project_name));
                self.swarm_registry
                    .register_parent_session(&session_id, &project_name, &agent_name);

                let swarm_summary = self.lookup_swarm_for_parent(
                    &session_id,
                    &project_name,
                    &agent_name,
                    now,
                );
                if let Some(ref s) = swarm_summary {
                    self.promote_fallback_session_id(&s.group_key, &session_id);
                }

                let is_silent = self.noise_filter.get() == NoiseFilterLevel::Silent;
                self.current_state = IslandState::Started {
                    session_id: session_id.clone(),
                    project_name: project_name.clone(),
                    role_type: role_type.clone(),
                    agent_name: agent_name.clone(),
                    task_summary: task_summary.clone(),
                    started_at: now,
                    auto_hide_secs: self.config.start_pop_secs,
                };

                let next_mode = if is_silent {
                    self.dock_mode
                } else {
                    EdgeDockMode::Normal
                };
                self.set_dock_mode(next_mode);
                self.started_auto_hide_at = if is_silent {
                    None
                } else {
                    Some(now + Duration::from_secs_f32(self.config.start_pop_secs))
                };

                upsert_active_session(
                    &mut self.active_sessions_pool,
                    &session_id,
                    &project_name,
                    &role_type,
                    &agent_name,
                    &task_summary,
                    0,
                    swarm_summary,
                    now,
                );
                upsert_recent_session(
                    &mut self.recent_sessions,
                    &session_id,
                    &project_name,
                    &role_type,
                    &task_summary,
                    &agent_name,
                );
            }
            AgentEvent::Working {
                session_id,
                project_name,
                role_type,
                agent_name,
                task_summary,
                step_index,
            } => {
                effects.tray_tip = Some(format!("WNoobar - 处理中: {}", project_name));
                self.swarm_registry
                    .register_parent_session(&session_id, &project_name, &agent_name);

                let swarm_summary = self.lookup_swarm_for_parent(
                    &session_id,
                    &project_name,
                    &agent_name,
                    now,
                );
                if let Some(ref s) = swarm_summary {
                    self.promote_fallback_session_id(&s.group_key, &session_id);
                }

                let in_done_protection = self.in_done_protection_at(now);
                let preserving_swarm_action = swarm_summary
                    .as_ref()
                    .is_some_and(|s| s.action_count > 0)
                    && matches!(
                        &self.current_state,
                        IslandState::ActionNeeded { session_id: cur, .. } if cur == &session_id
                    );

                if !in_done_protection && !preserving_swarm_action {
                    let start_time = match &self.current_state {
                        IslandState::Working {
                            session_id: cur,
                            start_time,
                            ..
                        } if cur == &session_id => *start_time,
                        _ => now,
                    };
                    self.current_state = IslandState::Working {
                        session_id: session_id.clone(),
                        project_name: project_name.clone(),
                        role_type: role_type.clone(),
                        agent_name: agent_name.clone(),
                        task_summary: task_summary.clone(),
                        step_index,
                        start_time,
                    };
                    self.set_dock_mode(self.dock_mode);
                }

                upsert_active_session(
                    &mut self.active_sessions_pool,
                    &session_id,
                    &project_name,
                    &role_type,
                    &agent_name,
                    &task_summary,
                    step_index,
                    swarm_summary,
                    now,
                );
                upsert_recent_session(
                    &mut self.recent_sessions,
                    &session_id,
                    &project_name,
                    &role_type,
                    &task_summary,
                    &agent_name,
                );
            }
            AgentEvent::Done {
                session_id,
                project_name,
                role_type,
                agent_name,
                summary,
                duration_secs,
                artifacts_count,
            } => {
                if self.swarm_registry.should_defer_parent_done(&session_id) {
                    let bound_summary = self.swarm_registry.summary_for_group_at(&session_id, now);
                    if let Some(ref s) = bound_summary {
                        self.upsert_swarm_pool_and_recent(
                            &s.group_key.clone(),
                            Some(&session_id),
                            s,
                            0,
                            now,
                        );
                        effects.tray_tip =
                            Some(format!("WNoobar - Swarm 执行中: {}", s.project_name));

                        let in_done_protection = self.in_done_protection_at(now);
                        let is_action = matches!(
                            &self.current_state,
                            IslandState::ActionNeeded { session_id: cur, .. } if cur == &session_id
                        ) && s.action_count > 0;

                        if !in_done_protection && !is_action {
                            let start_time = match &self.current_state {
                                IslandState::Working {
                                    session_id: cur,
                                    start_time,
                                    ..
                                } if cur == &session_id => *start_time,
                                _ => now,
                            };
                            self.current_state = IslandState::Working {
                                session_id: session_id.clone(),
                                project_name: if s.project_name.is_empty() {
                                    project_name.clone()
                                } else {
                                    s.project_name.clone()
                                },
                                role_type: "Swarm".to_string(),
                                agent_name: if s.agent_name.is_empty() {
                                    agent_name.clone()
                                } else {
                                    s.agent_name.clone()
                                },
                                task_summary: s.aggregate_summary.clone(),
                                step_index: s
                                    .workers
                                    .iter()
                                    .map(|w| w.step_index)
                                    .max()
                                    .unwrap_or(0),
                                start_time,
                            };
                            self.set_dock_mode(self.dock_mode);
                        }
                    }
                } else {
                    let group_done = self
                        .swarm_registry
                        .pop_completed_group_done_at(&session_id, now);
                    let (final_proj, final_role, final_agent, final_summary, final_duration) =
                        if let Some((
                            done_sid,
                            done_proj,
                            done_agent,
                            group_summary,
                            group_elapsed,
                        )) = group_done
                        {
                            self.clear_alerts_for_group(&done_sid);
                            self.clear_alerts_for_group(&session_id);
                            self.remove_group_from_pools(&done_sid, Some(&session_id));

                            let merged = if summary.trim().is_empty() {
                                group_summary
                            } else if summary.contains(&group_summary) {
                                summary
                            } else {
                                format!("{} · {}", group_summary, summary.trim())
                            };
                            let proj = if project_name.is_empty() {
                                done_proj
                            } else {
                                project_name
                            };
                            let agent = if agent_name.is_empty() {
                                done_agent
                            } else {
                                agent_name
                            };
                            (
                                proj,
                                "Swarm".to_string(),
                                agent,
                                merged,
                                duration_secs.max(group_elapsed),
                            )
                        } else {
                            self.swarm_registry.clear_group(&session_id);
                            self.clear_alerts_for_group(&session_id);
                            self.remove_group_from_pools(&session_id, None);
                            (
                                project_name,
                                role_type,
                                agent_name,
                                summary,
                                duration_secs,
                            )
                        };

                    effects.tray_tip = Some(format!("WNoobar - 任务完成: {}", final_proj));
                    self.started_auto_hide_at = None;
                    self.current_state = IslandState::Done {
                        session_id: session_id.clone(),
                        project_name: final_proj.clone(),
                        role_type: final_role.clone(),
                        agent_name: final_agent.clone(),
                        summary: final_summary.clone(),
                        duration_secs: final_duration,
                        artifacts_count,
                        finished_at: now,
                        auto_collapse_secs: self.config.done_pop_secs,
                    };
                    self.set_dock_mode(EdgeDockMode::Normal);

                    upsert_recent_session(
                        &mut self.recent_sessions,
                        &session_id,
                        &final_proj,
                        &final_role,
                        &final_summary,
                        &final_agent,
                    );
                    effects.sound_cue = Some(CueKind::Done);
                }
            }
            AgentEvent::ActionNeeded {
                session_id,
                project_name,
                role_type,
                agent_name,
                prompt,
            } => {
                effects.tray_tip = Some(format!("WNoobar - 需确认: {}", project_name));
                self.started_auto_hide_at = None;
                self.current_state = IslandState::ActionNeeded {
                    session_id: session_id.clone(),
                    project_name: project_name.clone(),
                    role_type: role_type.clone(),
                    agent_name: agent_name.clone(),
                    prompt: prompt.clone(),
                };
                self.set_dock_mode(EdgeDockMode::Normal);

                upsert_recent_session(
                    &mut self.recent_sessions,
                    &session_id,
                    &project_name,
                    &role_type,
                    &prompt,
                    &agent_name,
                );
                effects.sound_cue = Some(CueKind::ActionNeeded);
            }
            AgentEvent::SubagentStatus {
                group_key,
                parent_session_id,
                worker_id,
                worker_name,
                project_name,
                agent_name,
                status,
                task_summary,
                step_index,
            } => {
                self.swarm_registry.apply_subagent_status_at(
                    &group_key,
                    parent_session_id.as_deref(),
                    &worker_id,
                    &worker_name,
                    &project_name,
                    &agent_name,
                    status,
                    &task_summary,
                    step_index,
                    now,
                );

                let summary = parent_session_id
                    .as_deref()
                    .and_then(|pid| self.swarm_registry.summary_for_group_at(pid, now))
                    .or_else(|| self.swarm_registry.summary_for_group_at(&group_key, now));

                let mut tracked_group_key = group_key.clone();
                if let Some(summary) = summary {
                    tracked_group_key = summary.group_key.clone();

                    // 若节点脱离 ActionNeeded 或组内已无阻塞，清理历史告警去重记录，便于下次再阻塞时重新提示
                    if status != SwarmWorkerStatus::ActionNeeded {
                        self.alerted_swarm_actions
                            .remove(&(summary.group_key.clone(), worker_id.clone()));
                        self.alerted_swarm_actions
                            .remove(&(group_key.clone(), worker_id.clone()));
                    }
                    if summary.action_count == 0 {
                        self.clear_alerts_for_group(&summary.group_key);
                        self.clear_alerts_for_group(&group_key);
                    }

                    let rep_sid = self.upsert_swarm_pool_and_recent(
                        &group_key,
                        parent_session_id.as_deref(),
                        &summary,
                        step_index,
                        now,
                    );

                    let in_done_protection = self.in_done_protection_at(now);
                    if !in_done_protection {
                        if summary.action_count > 0 {
                            if let Some(top_worker) = summary
                                .workers
                                .iter()
                                .find(|w| w.status == SwarmWorkerStatus::ActionNeeded)
                            {
                                let worker_tag = if top_worker.display_name.trim().is_empty() {
                                    if top_worker.worker_id.trim().is_empty() {
                                        "@worker".to_string()
                                    } else {
                                        format!("@{}", top_worker.worker_id.trim())
                                    }
                                } else if top_worker.display_name.trim().starts_with('@') {
                                    top_worker.display_name.trim().to_string()
                                } else {
                                    format!("@{}", top_worker.display_name.trim())
                                };

                                let raw_body = top_worker.task_summary.trim();
                                let prompt = if raw_body.is_empty() {
                                    format!("{}: 需要人工确认", worker_tag)
                                } else if raw_body.starts_with(&worker_tag) {
                                    raw_body.to_string()
                                } else {
                                    format!("{}: {}", worker_tag, raw_body)
                                };

                                let alert_key =
                                    (summary.group_key.clone(), top_worker.worker_id.clone());
                                let is_first_block = self
                                    .alerted_swarm_actions
                                    .get(&alert_key)
                                    .is_none_or(|prev| prev != &prompt);

                                self.current_state = IslandState::ActionNeeded {
                                    session_id: rep_sid,
                                    project_name: summary.project_name.clone(),
                                    role_type: worker_tag,
                                    agent_name: summary.agent_name.clone(),
                                    prompt: prompt.clone(),
                                };
                                effects.tray_tip = Some(format!(
                                    "WNoobar - 需确认: {} (Swarm)",
                                    summary.project_name
                                ));

                                if is_first_block {
                                    self.alerted_swarm_actions.insert(alert_key, prompt);
                                    self.started_auto_hide_at = None;
                                    self.set_dock_mode(EdgeDockMode::Normal);
                                    effects.sound_cue = Some(CueKind::ActionNeeded);
                                } else {
                                    self.set_dock_mode(self.dock_mode);
                                }
                            }
                        } else if summary.working_count > 0 {
                            let top_step = summary
                                .workers
                                .iter()
                                .find(|w| w.status == SwarmWorkerStatus::Working)
                                .map_or(step_index, |w| w.step_index);
                            let start_time = match &self.current_state {
                                IslandState::Working {
                                    session_id: cur,
                                    start_time,
                                    ..
                                } if cur == &rep_sid => *start_time,
                                _ => now,
                            };
                            self.current_state = IslandState::Working {
                                session_id: rep_sid,
                                project_name: summary.project_name.clone(),
                                role_type: "Swarm".to_string(),
                                agent_name: summary.agent_name.clone(),
                                task_summary: summary.aggregate_summary.clone(),
                                step_index: top_step,
                                start_time,
                            };
                            self.set_dock_mode(self.dock_mode);
                            effects.tray_tip = Some(format!(
                                "WNoobar - Swarm 运行中: {} ({})",
                                summary.project_name, summary.aggregate_summary
                            ));
                        } else if matches!(
                            &self.current_state,
                            IslandState::ActionNeeded { session_id: cur, .. } if cur == &rep_sid
                        ) {
                            self.current_state = IslandState::Working {
                                session_id: rep_sid,
                                project_name: summary.project_name.clone(),
                                role_type: "Swarm".to_string(),
                                agent_name: summary.agent_name.clone(),
                                task_summary: summary.aggregate_summary.clone(),
                                step_index,
                                start_time: now,
                            };
                            self.set_dock_mode(self.dock_mode);
                        }
                    }
                }

                // 若父会话此前已提前触发过 Done 并被延迟，此时最后一个活跃 Worker 收官则立即统一收官
                let ready_dones = self.swarm_registry.pop_ready_group_dones();
                self.apply_ready_group_dones(
                    ready_dones,
                    Some(&tracked_group_key),
                    now,
                    &mut effects,
                );
            }
            AgentEvent::SwitchSession(session_id) => {
                effects.switch_session = Some(session_id);
            }
            AgentEvent::ResetIdle => {
                effects.tray_tip = Some("WNoobar - 智能灵动岛 (待命就绪)".to_string());
                self.started_auto_hide_at = None;
                self.current_state = IslandState::Idle;
                self.set_dock_mode(EdgeDockMode::Peeking);
            }
        }
        effects
    }

    /// 判定胶囊面板左键点击行为（抽屉切换 / Swarm 直跳主窗 / ActionNeeded 跳转 / 避让收起）。
    pub fn resolve_capsule_click(
        &self,
        cap_x: f32,
        cap_y: f32,
        cap_w: f32,
        cap_h: f32,
        log_x: f32,
        log_y: f32,
    ) -> CapsuleClickAction {
        if CapsuleRenderer::is_left_glyph_hit(cap_x, cap_y, cap_h, log_x, log_y) {
            let next_mode = match self.dock_mode {
                EdgeDockMode::DeckExpanded => EdgeDockMode::Normal,
                _ => EdgeDockMode::DeckExpanded,
            };
            return CapsuleClickAction::SetDockMode(next_mode);
        }

        if self.dock_mode == EdgeDockMode::DeckExpanded {
            if let Some(idx) = CapsuleRenderer::deck_card_index_at(
                cap_x,
                cap_y,
                cap_w,
                log_x,
                log_y,
                self.recent_sessions.len(),
            ) {
                return CapsuleClickAction::FocusSession(self.recent_sessions[idx].0.clone());
            }
            return CapsuleClickAction::SetDockMode(EdgeDockMode::Normal);
        }

        if self.dock_mode == EdgeDockMode::SwarmExpanded
            && CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, log_x, log_y)
        {
            if let Some(sid) = self.focused_swarm_session_id() {
                return CapsuleClickAction::FocusSession(sid);
            }
            return CapsuleClickAction::CollapseToPeeking;
        }

        if matches!(self.current_state, IslandState::ActionNeeded { .. })
            && CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, log_x, log_y)
        {
            if let IslandState::ActionNeeded { session_id, .. } = &self.current_state {
                return CapsuleClickAction::FocusSession(session_id.clone());
            }
        }

        if !self.current_state.is_idle() {
            return CapsuleClickAction::CollapseToPeeking;
        }

        CapsuleClickAction::None
    }

    fn lookup_swarm_for_parent(
        &self,
        session_id: &str,
        project_name: &str,
        agent_name: &str,
        now: Instant,
    ) -> Option<SwarmSummary> {
        self.swarm_registry
            .summary_for_group_at(session_id, now)
            .or_else(|| {
                self.swarm_registry
                    .summary_for_project_agent(project_name, agent_name)
                    .filter(|s| {
                        s.parent_session_id.is_none()
                            || s.parent_session_id.as_deref() == Some(session_id)
                    })
            })
    }

    /// 若此前仅以回退键 `agent::project` 建过代表条目，在识别出真实父 session_id 后原地升格并去重。
    fn promote_fallback_session_id(&mut self, fallback_key: &str, parent_sid: &str) {
        if fallback_key == parent_sid {
            return;
        }
        let has_parent_pool = self
            .active_sessions_pool
            .iter()
            .any(|i| i.session_id == parent_sid);
        if has_parent_pool {
            self.active_sessions_pool
                .retain(|i| !(i.session_id == fallback_key && i.swarm.is_some()));
        } else if let Some(item) = self
            .active_sessions_pool
            .iter_mut()
            .find(|i| i.session_id == fallback_key && i.swarm.is_some())
        {
            item.session_id = parent_sid.to_string();
        }

        let has_parent_recent = self
            .recent_sessions
            .iter()
            .any(|(id, _, _, _, _)| id == parent_sid);
        if has_parent_recent {
            self.recent_sessions
                .retain(|(id, _, _, _, _)| id != fallback_key);
        } else if let Some(rec) = self
            .recent_sessions
            .iter_mut()
            .find(|(id, _, _, _, _)| id == fallback_key)
        {
            rec.0 = parent_sid.to_string();
        }
    }

    /// 在活跃池与近期会话中最多维护 1 条 Swarm 组代表条目，并返回当前有效组 session_id。
    fn upsert_swarm_pool_and_recent(
        &mut self,
        raw_group_key: &str,
        parent_session_id: Option<&str>,
        summary: &SwarmSummary,
        step_index: u64,
        now: Instant,
    ) -> String {
        let canonical_sid = summary
            .parent_session_id
            .as_deref()
            .unwrap_or(&summary.group_key);
        let fallback_key = format!("{}::{}", summary.agent_name, summary.project_name);

        let matches_group = |item: &ActiveSessionItem| -> bool {
            item.session_id == canonical_sid
                || item.session_id == summary.group_key
                || item.session_id == raw_group_key
                || item.swarm.as_ref().is_some_and(|s| {
                    s.group_key == summary.group_key || s.group_key == raw_group_key
                })
                || (parent_session_id.is_some()
                    && item.session_id == fallback_key
                    && item.swarm.is_some())
        };

        let matched_idx = self
            .active_sessions_pool
            .iter()
            .position(|i| i.session_id == canonical_sid)
            .or_else(|| self.active_sessions_pool.iter().position(matches_group))
            .or_else(|| {
                // parent_session_id 缺失时回退聚合：升级同 (project, agent) 且尚未挂载 swarm 的真实父会话条目，避免池内分裂为 2 条
                if summary.parent_session_id.is_some() {
                    return None;
                }
                self.active_sessions_pool
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| {
                        i.swarm.is_none()
                            && !is_synthetic_swarm_item(i)
                            && i.project_name == summary.project_name
                            && i.agent_name == summary.agent_name
                    })
                    .max_by_key(|(_, i)| i.last_update_time)
                    .map(|(idx, _)| idx)
            });

        let max_step = summary
            .workers
            .iter()
            .map(|w| w.step_index)
            .max()
            .unwrap_or(step_index)
            .max(step_index);

        let effective_sid = if let Some(idx) = matched_idx {
            let sid_to_keep = if summary.parent_session_id.is_some() {
                canonical_sid.to_string()
            } else {
                let existing_sid = &self.active_sessions_pool[idx].session_id;
                if existing_sid != &summary.group_key
                    && existing_sid != raw_group_key
                    && existing_sid != &fallback_key
                {
                    existing_sid.clone()
                } else {
                    canonical_sid.to_string()
                }
            };

            {
                let item = &mut self.active_sessions_pool[idx];
                item.session_id = sid_to_keep.clone();
                if !summary.project_name.is_empty() {
                    item.project_name = summary.project_name.clone();
                }
                if !summary.agent_name.is_empty() {
                    item.agent_name = summary.agent_name.clone();
                }
                if item.role_type.trim().is_empty() {
                    item.role_type = "Swarm".to_string();
                }
                item.task_summary = summary.aggregate_summary.clone();
                item.step_index = max_step;
                item.last_update_time = now;
                item.swarm = Some(summary.clone());
            }

            // 清除历史迁徙留下的多余同组条目，确保池中严格最多 1 条
            let mut cursor = 0usize;
            self.active_sessions_pool.retain(|item| {
                let keep = cursor == idx || !matches_group(item);
                cursor += 1;
                keep
            });

            sid_to_keep
        } else {
            self.active_sessions_pool.push(ActiveSessionItem {
                session_id: canonical_sid.to_string(),
                project_name: summary.project_name.clone(),
                role_type: "Swarm".to_string(),
                task_summary: summary.aggregate_summary.clone(),
                agent_name: summary.agent_name.clone(),
                step_index: max_step,
                start_time: now,
                last_update_time: now,
                swarm: Some(summary.clone()),
            });
            canonical_sid.to_string()
        };

        // 同步更新 recent_sessions，同样最多只保留/刷新 1 条所属父会话/项目条目
        let matches_recent_alias = |id: &str| -> bool {
            id == effective_sid
                || id == canonical_sid
                || id == summary.group_key
                || id == raw_group_key
                || (parent_session_id.is_some() && id == fallback_key)
        };
        let rec_idx = self
            .recent_sessions
            .iter()
            .position(|(id, _, _, _, _)| id == &effective_sid)
            .or_else(|| {
                self.recent_sessions
                    .iter()
                    .position(|(id, _, _, _, _)| matches_recent_alias(id))
            });

        if let Some(r_idx) = rec_idx {
            {
                let rec = &mut self.recent_sessions[r_idx];
                rec.0 = effective_sid.clone();
                rec.1 = summary.project_name.clone();
                if rec.2.trim().is_empty() {
                    rec.2 = "Swarm".to_string();
                }
                rec.3 = summary.aggregate_summary.clone();
                rec.4 = summary.agent_name.clone();
            }
            let mut r_cursor = 0usize;
            self.recent_sessions.retain(|(id, _, _, _, _)| {
                let keep = r_cursor == r_idx || !matches_recent_alias(id);
                r_cursor += 1;
                keep
            });
        } else {
            upsert_recent_session(
                &mut self.recent_sessions,
                &effective_sid,
                &summary.project_name,
                "Swarm",
                &summary.aggregate_summary,
                &summary.agent_name,
            );
        }

        effective_sid
    }

    fn apply_ready_group_dones(
        &mut self,
        ready_dones: Vec<(String, String, String, String, u64)>,
        extra_group_key: Option<&str>,
        now: Instant,
        effects: &mut EventEffects,
    ) {
        for (done_sid, done_proj, done_agent, done_summary, elapsed_secs) in ready_dones {
            self.clear_alerts_for_group(&done_sid);
            if let Some(gk) = extra_group_key {
                self.clear_alerts_for_group(gk);
            }
            self.remove_group_from_pools(&done_sid, extra_group_key);
            upsert_recent_session(
                &mut self.recent_sessions,
                &done_sid,
                &done_proj,
                "Swarm",
                &done_summary,
                &done_agent,
            );
            self.started_auto_hide_at = None;
            self.current_state = IslandState::Done {
                session_id: done_sid,
                project_name: done_proj.clone(),
                role_type: "Swarm".to_string(),
                agent_name: done_agent,
                summary: done_summary,
                duration_secs: elapsed_secs,
                artifacts_count: 0,
                finished_at: now,
                auto_collapse_secs: self.config.done_pop_secs,
            };
            self.set_dock_mode(EdgeDockMode::Normal);
            effects.tray_tip = Some(format!("WNoobar - 任务完成: {}", done_proj));
            effects.sound_cue = Some(CueKind::Done);
        }
    }

    fn remove_group_from_pools(&mut self, sid_or_group: &str, alias_key: Option<&str>) {
        self.active_sessions_pool.retain(|i| {
            if i.session_id == sid_or_group {
                return false;
            }
            if let Some(alias) = alias_key {
                if i.session_id == alias {
                    return false;
                }
            }
            if let Some(s) = &i.swarm {
                if s.group_key == sid_or_group
                    || s.parent_session_id.as_deref() == Some(sid_or_group)
                {
                    return false;
                }
                if let Some(alias) = alias_key {
                    if s.group_key == alias || s.parent_session_id.as_deref() == Some(alias) {
                        return false;
                    }
                }
            }
            true
        });
    }

    fn clear_alerts_for_group(&mut self, group_key_or_sid: &str) {
        self.alerted_swarm_actions
            .retain(|(g, _), _| g != group_key_or_sid);
    }

    fn prune_stale_alerts(&mut self, now: Instant) {
        let registry = &self.swarm_registry;
        self.alerted_swarm_actions.retain(|(g, w_id), _| {
            registry
                .summary_for_group_at(g, now)
                .is_some_and(|sum| {
                    sum.workers
                        .iter()
                        .any(|w| w.worker_id == *w_id && w.status == SwarmWorkerStatus::ActionNeeded)
                })
        });
    }
}

struct WindowState {
    runtime: IslandRuntime,
    renderer: CapsuleRenderer,
    event_rx: Receiver<AgentEvent>,
    last_env_check: Instant,
    last_placement: MonitorPlacement,
    logical_w: f32,
    logical_h: f32,
    phys_w: i32,
    phys_h: i32,
    dpi_scale: f32,
    mem_dc: HDC,
    dib_bitmap: HBITMAP,
    bits_ptr: *mut u8,
    gp_bitmap: *mut c_void,
    gp_graphics: *mut c_void,
    last_frame: Instant,
    tray_icon: HICON,
    tray_popup: TrayPopupHost,
    taskbar_created_msg: u32,
}

impl WindowState {
    /// 统一切换边缘停靠模式并同步渲染悬停态。
    fn set_dock_mode(&mut self, mode: EdgeDockMode) {
        self.runtime.set_dock_mode(mode);
        self.sync_renderer_for_dock_mode();
    }

    fn sync_renderer_for_dock_mode(&mut self) {
        if self.runtime.dock_mode != EdgeDockMode::DeckExpanded {
            self.renderer.hovered_deck_idx = None;
        }
        if !matches!(
            self.runtime.dock_mode,
            EdgeDockMode::Normal | EdgeDockMode::SwarmExpanded
        ) {
            self.renderer.is_action_hovered = false;
        }
    }

    /// 收起胶囊至贴边微缩态并设置短时悬停抑制，防止指针尚在顶部热区时下一帧立刻弹回。
    fn collapse_to_peeking_suppressed(&mut self) {
        self.runtime.collapse_to_peeking_suppressed();
        self.sync_renderer_for_dock_mode();
    }

    /// 释放当前 DIBSection 与 GDI+ 渲染表面，确保跨 DPI 切换与退出时无句柄泄漏。
    unsafe fn release_render_surface(&mut self) {
        unsafe {
            if !self.gp_graphics.is_null() {
                GdipDeleteGraphics(self.gp_graphics);
                self.gp_graphics = null_mut();
            }
            if !self.gp_bitmap.is_null() {
                GdipDisposeImage(self.gp_bitmap);
                self.gp_bitmap = null_mut();
            }
            // 先销毁兼容 DC，解除位图选中状态，再安全删除 DIBSection
            if !self.mem_dc.is_null() {
                DeleteDC(self.mem_dc);
                self.mem_dc = 0 as _;
            }
            if !self.dib_bitmap.is_null() {
                DeleteObject(self.dib_bitmap as _);
                self.dib_bitmap = 0 as _;
            }
            self.bits_ptr = null_mut();
        }
    }

    /// 在跨显示器切换且 DPI 变化时安全重建 DIBSection 与 GDI+ 画布。
    unsafe fn rebuild_render_surface(&mut self, hwnd: HWND, new_dpi_scale: f32) {
        unsafe {
            self.release_render_surface();
            let safe_scale = if new_dpi_scale > 0.1 { new_dpi_scale } else { 1.0 };
            let surface = create_render_surface(hwnd, self.logical_w, self.logical_h, safe_scale);
            self.phys_w = surface.phys_w;
            self.phys_h = surface.phys_h;
            self.dpi_scale = surface.dpi_scale;
            self.mem_dc = surface.mem_dc;
            self.dib_bitmap = surface.dib_bitmap;
            self.bits_ptr = surface.bits_ptr;
            self.gp_bitmap = surface.gp_bitmap;
            self.gp_graphics = surface.gp_graphics;
        }
    }
}

impl Drop for WindowState {
    fn drop(&mut self) {
        unsafe {
            self.release_render_surface();
            if !self.tray_icon.is_null() {
                DestroyIcon(self.tray_icon);
                self.tray_icon = 0 as _;
            }
        }
    }
}

struct RenderSurface {
    phys_w: i32,
    phys_h: i32,
    dpi_scale: f32,
    mem_dc: HDC,
    dib_bitmap: HBITMAP,
    bits_ptr: *mut u8,
    gp_bitmap: *mut c_void,
    gp_graphics: *mut c_void,
}

unsafe fn create_render_surface(
    hwnd: HWND,
    logical_w: f32,
    logical_h: f32,
    dpi_scale: f32,
) -> RenderSurface {
    unsafe {
        let phys_w = (logical_w * dpi_scale).round().max(1.0) as i32;
        let phys_h = (logical_h * dpi_scale).round().max(1.0) as i32;

        let screen_dc = GetDC(hwnd);
        let mem_dc = CreateCompatibleDC(screen_dc);

        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: phys_w,
                biHeight: -phys_h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: (phys_w * phys_h * 4) as u32,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }],
        };

        let mut bits_ptr: *mut c_void = null_mut();
        let dib_bitmap = CreateDIBSection(
            mem_dc,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits_ptr,
            null_mut(),
            0,
        );
        SelectObject(mem_dc, dib_bitmap as _);
        ReleaseDC(hwnd, screen_dc);

        let stride = phys_w * 4;
        let mut gp_bitmap: *mut c_void = null_mut();
        GdipCreateBitmapFromScan0(
            phys_w,
            phys_h,
            stride,
            PIXEL_FORMAT_32BPP_PARGB,
            bits_ptr as *mut u8,
            &mut gp_bitmap,
        );

        let mut gp_graphics: *mut c_void = null_mut();
        GdipGetImageGraphicsContext(gp_bitmap, &mut gp_graphics);
        GdipSetSmoothingMode(gp_graphics, SMOOTHING_MODE_ANTIALIAS);
        GdipSetTextRenderingHint(gp_graphics, TEXT_RENDERING_HINT_ANTIALIAS_GRID_FIT);
        GdipScaleWorldTransform(gp_graphics, dpi_scale, dpi_scale, MATRIX_ORDER_PREPEND);

        RenderSurface {
            phys_w,
            phys_h,
            dpi_scale,
            mem_dc,
            dib_bitmap,
            bits_ptr: bits_ptr as *mut u8,
            gp_bitmap,
            gp_graphics,
        }
    }
}

impl IslandWindow {
    pub fn new(event_rx: Receiver<AgentEvent>) -> Result<Self, String> {
        let config = AppConfig::default();
        let filter = SharedNoiseFilter::new(config.noise_level);
        Self::new_with_config(event_rx, config, filter)
    }

    pub fn new_with_config(
        event_rx: Receiver<AgentEvent>,
        config: AppConfig,
        noise_filter: SharedNoiseFilter,
    ) -> Result<Self, String> {
        unsafe {
            // 1. 设置 Per-Monitor V2 高 DPI 感知，彻底消灭拉伸模糊
            Self::enable_per_monitor_dpi_v2();

            let instance = GetModuleHandleW(null_mut());
            let class_name = encode_wide("WNoobarIslandWindowClass");

            // 创建状态栏专属高清图标（从内嵌 PNG 直接生成 32bpp Alpha 图标）
            let mut tray_icon: HICON = 0 as _;
            let bmp = create_bitmap_from_bytes(WNOOBAR_PNG);
            if !bmp.is_null() {
                GdipCreateHICONFromBitmap(bmp, &mut tray_icon);
                GdipDisposeImage(bmp);
            }
            if tray_icon.is_null() {
                tray_icon = LoadIconW(0 as _, IDI_APPLICATION);
            }

            let wnd_class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: tray_icon,
                hCursor: LoadCursorW(0 as _, IDC_ARROW),
                hbrBackground: 0 as _,
                lpszMenuName: null_mut(),
                lpszClassName: class_name.as_ptr(),
                hIconSm: tray_icon,
            };

            RegisterClassExW(&wnd_class);

            let follow_cursor = config.monitor_mode == MonitorMode::FollowCursor;
            let placement = system::get_target_monitor_placement(follow_cursor);
            let dpi_scale = if placement.dpi_scale > 0.1 {
                placement.dpi_scale
            } else {
                Self::get_system_dpi_scale()
            };

            let logical_w = 480.0;
            let logical_h = 210.0;
            let init_phys_w = (logical_w * dpi_scale).round().max(1.0) as i32;
            let init_phys_h = (logical_h * dpi_scale).round().max(1.0) as i32;
            let pos_x = placement.centered_window_x(init_phys_w);
            let pos_y = placement.origin_y;

            let ex_style = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED;
            let style = WS_POPUP;

            let title = encode_wide("WNoobar");
            let hwnd = CreateWindowExW(
                ex_style,
                class_name.as_ptr(),
                title.as_ptr(),
                style,
                pos_x,
                pos_y,
                init_phys_w,
                init_phys_h,
                0 as _,
                0 as _,
                instance,
                null_mut(),
            );

            if hwnd.is_null() {
                let err = windows_sys::Win32::Foundation::GetLastError();
                return Err(format!("Failed to create Win32 layered window, GetLastError={err}"));
            }

            // 注入 Windows 11 深色模式
            let dark_mode: i32 = 1;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE as _,
                &dark_mode as *const _ as _,
                size_of::<i32>() as u32,
            );

            // 分配 Retina 物理像素 DIBSection 内存画布
            let surface = create_render_surface(hwnd, logical_w, logical_h, dpi_scale);

            let mut renderer = CapsuleRenderer::new();
            noise_filter.set(config.noise_level);
            renderer.is_silent_mode = noise_filter.get() == NoiseFilterLevel::Silent;

            let runtime = IslandRuntime::new(config, noise_filter);

            let taskbar_created_msg = RegisterWindowMessageW(encode_wide("TaskbarCreated").as_ptr());

            let tray_popup = match TrayPopupHost::new(hwnd) {
                Ok(host) => host,
                Err(e) => {
                    if !surface.gp_graphics.is_null() {
                        GdipDeleteGraphics(surface.gp_graphics);
                    }
                    if !surface.gp_bitmap.is_null() {
                        GdipDisposeImage(surface.gp_bitmap);
                    }
                    if !surface.mem_dc.is_null() {
                        DeleteDC(surface.mem_dc);
                    }
                    if !surface.dib_bitmap.is_null() {
                        DeleteObject(surface.dib_bitmap as _);
                    }
                    DestroyWindow(hwnd);
                    if !tray_icon.is_null() {
                        DestroyIcon(tray_icon);
                    }
                    return Err(e);
                }
            };

            // 注册到 Windows 任务栏系统托盘（状态栏通知区）
            add_tray_icon(hwnd, tray_icon, "WNoobar - 智能灵动岛 (待命就绪)");

            let state = Box::new(WindowState {
                runtime,
                renderer,
                event_rx,
                last_env_check: Instant::now(),
                last_placement: placement,
                logical_w,
                logical_h,
                phys_w: surface.phys_w,
                phys_h: surface.phys_h,
                dpi_scale: surface.dpi_scale,
                mem_dc: surface.mem_dc,
                dib_bitmap: surface.dib_bitmap,
                bits_ptr: surface.bits_ptr,
                gp_bitmap: surface.gp_bitmap,
                gp_graphics: surface.gp_graphics,
                last_frame: Instant::now(),
                tray_icon,
                tray_popup,
                taskbar_created_msg,
            });

            let state_raw = Box::into_raw(state);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_raw as isize);

            // 启动 16ms 渲染循环
            SetTimer(hwnd, 1, 16, None);

            render_and_update_layered(hwnd, &mut *state_raw);
            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);

            Ok(Self { _hwnd: hwnd })
        }
    }

    unsafe fn enable_per_monitor_dpi_v2() {
        unsafe {
            let user32_name = encode_wide("user32.dll");
            let user32 = GetModuleHandleW(user32_name.as_ptr());
            if !user32.is_null() {
                let func = GetProcAddress(user32, b"SetProcessDpiAwarenessContext\0".as_ptr());
                if let Some(f) = func {
                    let set_dpi: unsafe extern "system" fn(isize) -> i32 = std::mem::transmute(f);
                    set_dpi(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
                }
            }
        }
    }

    unsafe fn get_system_dpi_scale() -> f32 {
        unsafe {
            let user32_name = encode_wide("user32.dll");
            let user32 = GetModuleHandleW(user32_name.as_ptr());
            if !user32.is_null() {
                let func = GetProcAddress(user32, b"GetDpiForSystem\0".as_ptr());
                if let Some(f) = func {
                    let get_dpi: unsafe extern "system" fn() -> u32 = std::mem::transmute(f);
                    let dpi = get_dpi();
                    if dpi > 0 {
                        return dpi as f32 / 96.0;
                    }
                }
            }
            1.0
        }
    }

    pub fn run_message_loop(&self) {
        unsafe {
            let mut msg = MSG {
                hwnd: 0 as _,
                message: 0,
                wParam: 0,
                lParam: 0,
                time: 0,
                pt: POINT { x: 0, y: 0 },
            };

            while GetMessageW(&mut msg, 0 as _, 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

fn capsule_bounds(state: &WindowState) -> (f32, f32, f32, f32) {
    let cap_w = state.runtime.width_spring.current.round();
    let cap_h = state.runtime.height_spring.current.round();
    let cap_x = ((state.logical_w - state.runtime.width_spring.current) / 2.0).round();
    let morph = ((state.runtime.height_spring.current - 4.0) / 26.0).clamp(0.0, 1.0);
    let cap_y = (4.0 * morph).round();
    (cap_x, cap_y, cap_w, cap_h)
}

#[allow(clippy::too_many_arguments)]
/// 活跃池条目是否为 Swarm 聚合自动生成的合成占位（真实会话事件会覆写 `role_type`）。
fn is_synthetic_swarm_item(item: &ActiveSessionItem) -> bool {
    item.role_type == "Swarm"
}

fn upsert_active_session(
    pool: &mut Vec<ActiveSessionItem>,
    session_id: &str,
    project_name: &str,
    role_type: &str,
    agent_name: &str,
    task_summary: &str,
    step_index: u64,
    swarm: Option<SwarmSummary>,
    now: Instant,
) {
    if let Some(item) = pool.iter_mut().find(|i| i.session_id == session_id) {
        item.project_name = project_name.to_string();
        item.role_type = role_type.to_string();
        item.task_summary = task_summary.to_string();
        item.agent_name = agent_name.to_string();
        item.step_index = step_index;
        item.last_update_time = now;
        if let Some(s) = swarm {
            item.swarm = Some(s);
        }
    } else {
        pool.push(ActiveSessionItem {
            session_id: session_id.to_string(),
            project_name: project_name.to_string(),
            role_type: role_type.to_string(),
            task_summary: task_summary.to_string(),
            agent_name: agent_name.to_string(),
            step_index,
            start_time: now,
            last_update_time: now,
            swarm,
        });
    }
}

fn upsert_recent_session(
    recent: &mut Vec<(String, String, String, String, String)>,
    session_id: &str,
    project_name: &str,
    role_type: &str,
    desc: &str,
    agent_name: &str,
) {
    if let Some(item) = recent.iter_mut().find(|(id, _, _, _, _)| id == session_id) {
        item.1 = project_name.to_string();
        item.2 = role_type.to_string();
        item.3 = desc.to_string();
        item.4 = agent_name.to_string();
    } else {
        recent.insert(
            0,
            (
                session_id.to_string(),
                project_name.to_string(),
                role_type.to_string(),
                desc.to_string(),
                agent_name.to_string(),
            ),
        );
        if recent.len() > 5 {
            recent.pop();
        }
    }
}

fn persist_config_and_sync(state: &mut WindowState) {
    state.runtime.noise_filter.set(state.runtime.config.noise_level);
    state.renderer.is_silent_mode = state.runtime.noise_filter.get() == NoiseFilterLevel::Silent;
    if let Err(e) = state.runtime.config.save() {
        println!("[Config] Failed to save config: {e}");
    }
}

fn focus_and_switch_session(hwnd: HWND, state: &mut WindowState, session_id: &str) {
    let mut target_info: Option<(String, String, String)> = None;

    if let Some(pool_idx) = state
        .runtime
        .active_sessions_pool
        .iter()
        .position(|item| item.session_id == session_id)
    {
        let item = &state.runtime.active_sessions_pool[pool_idx];
        target_info = Some((
            item.project_name.clone(),
            item.agent_name.clone(),
            item.role_type.clone(),
        ));
        state.runtime.odometer.active_index = pool_idx;
        state.runtime.odometer.prev_index = pool_idx;
        state.runtime.odometer.dwell_timer = 0.0;
        state.runtime.odometer.is_rolling = false;
        state.runtime.odometer.roll_progress = 0.0;
        state.runtime.odometer.landing_bounce_time = 0.0;
    }

    if let Some(rec_idx) = state
        .runtime
        .recent_sessions
        .iter()
        .position(|(id, _, _, _, _)| id == session_id)
    {
        let entry = state.runtime.recent_sessions.remove(rec_idx);
        if target_info.is_none() {
            target_info = Some((entry.1.clone(), entry.4.clone(), entry.2.clone()));
        }
        state.runtime.recent_sessions.insert(0, entry);
    }

    if target_info.is_none() {
        match &state.runtime.current_state {
            IslandState::Started {
                session_id: sid,
                project_name,
                agent_name,
                role_type,
                ..
            }
            | IslandState::Working {
                session_id: sid,
                project_name,
                agent_name,
                role_type,
                ..
            }
            | IslandState::ActionNeeded {
                session_id: sid,
                project_name,
                agent_name,
                role_type,
                ..
            }
            | IslandState::Done {
                session_id: sid,
                project_name,
                agent_name,
                role_type,
                ..
            } if sid == session_id => {
                target_info = Some((project_name.clone(), agent_name.clone(), role_type.clone()));
            }
            _ => {}
        }
    }

    if target_info.is_none() {
        if let Some(s) = state.runtime.swarm_registry.summary_for_group(session_id) {
            target_info = Some((s.project_name, s.agent_name, "Swarm".to_string()));
        }
    }

    let Some((project_name, agent_name, role_type)) = target_info else {
        return;
    };

    focus::focus_session_window(hwnd, &project_name, &agent_name, &role_type);
    state.collapse_to_peeking_suppressed();
}

fn placement_changed(a: MonitorPlacement, b: MonitorPlacement) -> bool {
    a.origin_x != b.origin_x
        || a.origin_y != b.origin_y
        || a.monitor_w != b.monitor_w
        || (a.dpi_scale - b.dpi_scale).abs() > 0.01
}

unsafe fn sync_monitor_placement(hwnd: HWND, state: &mut WindowState) {
    let follow_cursor = state.runtime.config.monitor_mode == MonitorMode::FollowCursor;
    let placement = system::get_target_monitor_placement(follow_cursor);
    if placement_changed(placement, state.last_placement) {
        let dpi_changed = (placement.dpi_scale - state.dpi_scale).abs() > 0.01;
        state.last_placement = placement;
        unsafe {
            if dpi_changed {
                state.rebuild_render_surface(hwnd, placement.dpi_scale);
            }
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                placement.centered_window_x(state.phys_w),
                placement.origin_y,
                state.phys_w,
                state.phys_h,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
            if !state.runtime.is_dnd_hidden {
                render_and_update_layered(hwnd, state);
            }
        }
    }
}

unsafe fn show_tray_context_menu(_hwnd: HWND, state: &mut WindowState) {
    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);

        // 同步真实开机自启注册表状态，确保与系统实际一致
        state.runtime.config.auto_start = system::is_autostart_enabled();

        let snap = TrayMenuSnapshot {
            is_expanded: state.runtime.dock_mode != EdgeDockMode::Peeking,
            active_sessions_count: state.runtime.active_sessions_pool.len(),
            noise_level: state.runtime.config.noise_level,
            monitor_mode: state.runtime.config.monitor_mode,
            dnd_fullscreen: state.runtime.config.dnd_fullscreen,
            sound_enabled: state.runtime.config.sound_enabled,
            auto_start: state.runtime.config.auto_start,
        };
        state.tray_popup.open_at(pt.x, pt.y, snap);
    }
}

unsafe fn apply_event_effects(hwnd: HWND, state: &mut WindowState, effects: EventEffects) {
    unsafe {
        if let Some(tip) = effects.tray_tip {
            update_tray_tip(hwnd, &tip);
        }
        if let Some(cue) = effects.sound_cue {
            if state.runtime.config.sound_enabled && !state.runtime.is_dnd_hidden {
                system::play_cue_sound(cue);
            }
        }
        if let Some(sid) = effects.switch_session {
            focus_and_switch_session(hwnd, state, &sid);
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut WindowState;

        match msg {
            WM_NCHITTEST => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    if state.runtime.is_dnd_hidden {
                        return HTTRANSPARENT as LRESULT;
                    }
                    let screen_x = (lparam & 0xFFFF) as i16 as i32;
                    let screen_y = ((lparam >> 16) & 0xFFFF) as i16 as i32;

                    let mut pt = POINT { x: screen_x, y: screen_y };
                    ScreenToClient(hwnd, &mut pt);

                    let log_x = pt.x as f32 / state.dpi_scale;
                    let log_y = pt.y as f32 / state.dpi_scale;
                    let (cap_x, cap_y, cap_w, cap_h) = capsule_bounds(state);

                    // 连续几何命中测试（包含底垫缓冲）
                    let in_hit = if state.runtime.dock_mode == EdgeDockMode::Peeking {
                        // 贴顶极细微光条核心感应带（180px 宽，14px 高，触碰即开）
                        log_x >= (state.logical_w - 180.0) / 2.0
                            && log_x <= (state.logical_w + 180.0) / 2.0
                            && log_y >= 0.0
                            && log_y <= 14.0
                    } else {
                        log_x >= cap_x - 14.0
                            && log_x <= cap_x + cap_w + 14.0
                            && log_y >= 0.0
                            && log_y <= cap_y + cap_h + 14.0
                    };

                    if in_hit {
                        return HTCLIENT as LRESULT;
                    }
                }
                HTTRANSPARENT as LRESULT
            }
            WM_MOUSEMOVE => 0,
            WM_MOUSELEAVE => 0,
            WM_WNOOBAR_WAKE => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    if !state.runtime.is_dnd_hidden {
                        sync_monitor_placement(hwnd, state);
                        state.runtime.started_auto_hide_at = None;
                        state.set_dock_mode(EdgeDockMode::Normal);
                        render_and_update_layered(hwnd, state);
                    }
                }
                0
            }
            WM_MOUSEWHEEL => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    let len = state.runtime.active_sessions_pool.len();
                    if len >= 2
                        && state.runtime.dock_mode == EdgeDockMode::Normal
                        && !matches!(
                            state.runtime.current_state,
                            IslandState::Done { .. } | IslandState::ActionNeeded { .. }
                        )
                    {
                        let delta = ((wparam >> 16) & 0xFFFF) as i16;
                        if delta > 0 {
                            state.runtime.odometer.is_rolling = true;
                            state.runtime.odometer.roll_progress = 0.0;
                            state.runtime.odometer.prev_index = state.runtime.odometer.active_index;
                            state.runtime.odometer.active_index =
                                (state.runtime.odometer.active_index + len - 1) % len;
                            state.runtime.odometer.dwell_timer = 0.0;
                            state.runtime.odometer.landing_bounce_time = 0.0;
                        } else if delta < 0 {
                            state.runtime.odometer.trigger_next(len);
                        }
                        if delta != 0 && !state.runtime.is_dnd_hidden {
                            render_and_update_layered(hwnd, state);
                        }
                    }
                }
                0
            }
            WM_TIMER => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;

                    // 0. 同步降噪静默渲染标识
                    state.renderer.is_silent_mode =
                        state.runtime.noise_filter.get() == NoiseFilterLevel::Silent;

                    // 0.5 全屏免打扰 (DND) 与多显示器定位（每 ~250ms 节流检查）
                    let mut force_render = false;
                    if state.last_env_check.elapsed() >= Duration::from_millis(250) {
                        state.last_env_check = Instant::now();

                        let should_hide_dnd = state.runtime.config.dnd_fullscreen
                            && system::is_fullscreen_or_dnd_active(hwnd);
                        if should_hide_dnd && !state.runtime.is_dnd_hidden {
                            state.runtime.is_dnd_hidden = true;
                            state.renderer.is_dnd_active = true;
                            ShowWindow(hwnd, SW_HIDE);
                        } else if !should_hide_dnd && state.runtime.is_dnd_hidden {
                            state.runtime.is_dnd_hidden = false;
                            state.renderer.is_dnd_active = false;
                            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                            force_render = true;
                        }

                        if !state.runtime.is_dnd_hidden
                            && state.runtime.dock_mode == EdgeDockMode::Peeking
                            && !state.runtime.is_mouse_over
                        {
                            sync_monitor_placement(hwnd, state);
                        }
                    }

                    // 1. 全局硬件光标检测与双阈值施密特触发器 + 260ms Swarm 悬停展开状态机
                    let mut cur_pt = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut cur_pt);
                    ScreenToClient(hwnd, &mut cur_pt);
                    let log_x = cur_pt.x as f32 / state.dpi_scale;
                    let log_y = cur_pt.y as f32 / state.dpi_scale;

                    let (cap_x, cap_y, cap_w, cap_h) = capsule_bounds(state);
                    let tick_now = Instant::now();
                    state.runtime.update_hover_state_at(
                        cap_x,
                        cap_y,
                        cap_w,
                        cap_h,
                        log_x,
                        log_y,
                        state.logical_w,
                        tick_now,
                    );
                    state.sync_renderer_for_dock_mode();

                    // 2 & 2.5 & 3. 定时超时、活跃池看门狗、Swarm 超时清理与延迟收官队列同步
                    let timer_effects = state.runtime.tick_timers_at(tick_now);
                    apply_event_effects(hwnd, state, timer_effects);
                    state.sync_renderer_for_dock_mode();

                    // 4. 处理核心事件（含 SubagentStatus 归并防污染与延迟收官）
                    while let Ok(event) = state.event_rx.try_recv() {
                        let ev_effects = state.runtime.handle_event_at(event, Instant::now());
                        apply_event_effects(hwnd, state, ev_effects);
                        state.sync_renderer_for_dock_mode();
                    }

                    // 4.5 实时更新 Deck 卡片 / ActionNeeded / SwarmExpanded 跳转热区悬停高亮
                    let (cap_x_now, cap_y_now, cap_w_now, cap_h_now) = capsule_bounds(state);
                    let next_hovered_deck = if state.runtime.dock_mode == EdgeDockMode::DeckExpanded {
                        CapsuleRenderer::deck_card_index_at(
                            cap_x_now,
                            cap_y_now,
                            cap_w_now,
                            log_x,
                            log_y,
                            state.runtime.recent_sessions.len(),
                        )
                    } else {
                        None
                    };
                    let next_action_hovered = if state.runtime.dock_mode
                        == EdgeDockMode::SwarmExpanded
                        || (state.runtime.dock_mode == EdgeDockMode::Normal
                            && matches!(
                                state.runtime.current_state,
                                IslandState::ActionNeeded { .. }
                            )) {
                        CapsuleRenderer::is_action_jump_hit(
                            cap_x_now, cap_y_now, cap_w_now, cap_h_now, log_x, log_y,
                        )
                    } else {
                        false
                    };
                    let hover_changed = state.renderer.hovered_deck_idx != next_hovered_deck
                        || state.renderer.is_action_hovered != next_action_hovered;
                    state.renderer.hovered_deck_idx = next_hovered_deck;
                    state.renderer.is_action_hovered = next_action_hovered;

                    // 5. 动画更新与重绘
                    let now = Instant::now();
                    let dt = now.duration_since(state.last_frame).as_secs_f32();
                    state.last_frame = now;

                    let odo_anim = if state.runtime.should_update_odometer() {
                        state
                            .runtime
                            .odometer
                            .update(dt, state.runtime.active_sessions_pool.len())
                    } else {
                        false
                    };
                    let w_anim = state.runtime.width_spring.update(dt);
                    let h_anim = state.runtime.height_spring.update(dt);
                    state.renderer.update(dt);

                    // 多会话轮播时，自适应调整胶囊目标尺寸（放宽至 450px 保证各会话角色与进度摘要无截断展示）
                    if state.runtime.dock_mode == EdgeDockMode::Normal
                        && state.runtime.active_sessions_pool.len() >= 2
                        && !matches!(
                            state.runtime.current_state,
                            IslandState::Done { .. } | IslandState::ActionNeeded { .. }
                        )
                    {
                        state.runtime.width_spring.set_target(450.0);
                    }

                    // 自适应帧率：处于 DND 隐藏时跳过绘制；处于贴边且完全静止未悬停时跳过重绘
                    let is_animating = w_anim || h_anim || odo_anim;
                    let should_render = !state.runtime.is_dnd_hidden
                        && (force_render
                            || hover_changed
                            || is_animating
                            || state.runtime.dock_mode != EdgeDockMode::Peeking
                            || state.runtime.is_mouse_over);

                    if should_render {
                        render_and_update_layered(hwnd, state);
                    }
                }
                0
            }
            WM_LBUTTONDOWN => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    let x_pos = (lparam & 0xFFFF) as i16 as f32 / state.dpi_scale;
                    let y_pos = ((lparam >> 16) & 0xFFFF) as i16 as f32 / state.dpi_scale;
                    let (cap_x, cap_y, cap_w, cap_h) = capsule_bounds(state);

                    match state
                        .runtime
                        .resolve_capsule_click(cap_x, cap_y, cap_w, cap_h, x_pos, y_pos)
                    {
                        CapsuleClickAction::SetDockMode(next_mode) => {
                            state.runtime.started_auto_hide_at = None;
                            state.set_dock_mode(next_mode);
                            state.renderer.hovered_deck_idx = None;
                            state.renderer.is_action_hovered = false;
                        }
                        CapsuleClickAction::FocusSession(sid) => {
                            focus_and_switch_session(hwnd, state, &sid);
                        }
                        CapsuleClickAction::CollapseToPeeking => {
                            // 用户点击正在展示的卡片：仅做避让收缩回贴边微缩并短时抑制顶部悬停弹回，严禁修改 current_state！
                            state.collapse_to_peeking_suppressed();
                        }
                        CapsuleClickAction::None => {}
                    }

                    if !state.runtime.is_dnd_hidden {
                        render_and_update_layered(hwnd, state);
                    }
                }
                0
            }
            WM_TRAYICON => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    let event = lparam as u32;
                    match event {
                        WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                            if state.tray_popup.is_visible() {
                                state.tray_popup.close_all();
                            }
                            // 左键单击或双击托盘图标：切换灵动岛展开 / 贴边隐藏
                            let next_mode = match state.runtime.dock_mode {
                                EdgeDockMode::Peeking => EdgeDockMode::Normal,
                                _ => EdgeDockMode::Peeking,
                            };
                            state.runtime.started_auto_hide_at = None;
                            state.set_dock_mode(next_mode);
                            if !state.runtime.is_dnd_hidden {
                                render_and_update_layered(hwnd, state);
                            }
                        }
                        WM_RBUTTONUP => {
                            show_tray_context_menu(hwnd, state);
                        }
                        _ => {}
                    }
                }
                0
            }
            WM_COMMAND => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;
                    let cmd_id = (wparam & 0xFFFF) as usize;
                    match cmd_id {
                        IDM_TRAY_TOGGLE => {
                            let next_mode = match state.runtime.dock_mode {
                                EdgeDockMode::Peeking => EdgeDockMode::Normal,
                                _ => EdgeDockMode::Peeking,
                            };
                            state.runtime.started_auto_hide_at = None;
                            state.set_dock_mode(next_mode);
                        }
                        IDM_TRAY_DECK => {
                            let next_mode = match state.runtime.dock_mode {
                                EdgeDockMode::DeckExpanded => EdgeDockMode::Normal,
                                _ => EdgeDockMode::DeckExpanded,
                            };
                            state.runtime.started_auto_hide_at = None;
                            state.set_dock_mode(next_mode);
                        }
                        IDM_NOISE_SILENT => {
                            state.runtime.config.noise_level = NoiseFilterLevel::Silent;
                            persist_config_and_sync(state);
                            if !state.runtime.is_dnd_hidden {
                                render_and_update_layered(hwnd, state);
                            }
                        }
                        IDM_NOISE_NORMAL => {
                            state.runtime.config.noise_level = NoiseFilterLevel::Normal;
                            persist_config_and_sync(state);
                            if !state.runtime.is_dnd_hidden {
                                render_and_update_layered(hwnd, state);
                            }
                        }
                        IDM_NOISE_VERBOSE => {
                            state.runtime.config.noise_level = NoiseFilterLevel::Verbose;
                            persist_config_and_sync(state);
                            if !state.runtime.is_dnd_hidden {
                                render_and_update_layered(hwnd, state);
                            }
                        }
                        IDM_MONITOR_PRIMARY => {
                            state.runtime.config.monitor_mode = MonitorMode::Primary;
                            persist_config_and_sync(state);
                            sync_monitor_placement(hwnd, state);
                        }
                        IDM_MONITOR_FOLLOW => {
                            state.runtime.config.monitor_mode = MonitorMode::FollowCursor;
                            persist_config_and_sync(state);
                            sync_monitor_placement(hwnd, state);
                        }
                        IDM_TOGGLE_DND => {
                            state.runtime.config.dnd_fullscreen =
                                !state.runtime.config.dnd_fullscreen;
                            persist_config_and_sync(state);
                            if !state.runtime.config.dnd_fullscreen && state.runtime.is_dnd_hidden
                            {
                                state.runtime.is_dnd_hidden = false;
                                state.renderer.is_dnd_active = false;
                                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                                render_and_update_layered(hwnd, state);
                            }
                        }
                        IDM_TOGGLE_SOUND => {
                            state.runtime.config.sound_enabled =
                                !state.runtime.config.sound_enabled;
                            persist_config_and_sync(state);
                        }
                        IDM_TOGGLE_AUTOSTART => {
                            let next = !system::is_autostart_enabled();
                            match system::set_autostart_enabled(next) {
                                Ok(()) => {
                                    state.runtime.config.auto_start = next;
                                    persist_config_and_sync(state);
                                }
                                Err(e) => {
                                    println!("[System] Failed to update autostart: {e}");
                                }
                            }
                        }
                        IDM_TRAY_RESET => {
                            state.runtime.started_auto_hide_at = None;
                            state.runtime.current_state = IslandState::Idle;
                            state.set_dock_mode(EdgeDockMode::Peeking);
                            update_tray_tip(hwnd, "WNoobar - 智能灵动岛 (待命就绪)");
                        }
                        IDM_TRAY_EXIT => {
                            DestroyWindow(hwnd);
                        }
                        _ => {}
                    }
                }
                0
            }
            WM_DESTROY => {
                KillTimer(hwnd, 1);
                remove_tray_icon(hwnd);
                let state_ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut WindowState;
                if !state_ptr.is_null() {
                    let mut state = Box::from_raw(state_ptr);
                    state.tray_popup.destroy();
                }
                PostQuitMessage(0);
                0
            }
            msg if !state_ptr.is_null() && (*state_ptr).taskbar_created_msg != 0 && msg == (*state_ptr).taskbar_created_msg => {
                let state = &*state_ptr;
                add_tray_icon(hwnd, state.tray_icon, "WNoobar - 智能灵动岛 (待命就绪)");
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe fn render_and_update_layered(hwnd: HWND, state: &mut WindowState) {
    unsafe {
        let total_bytes = (state.phys_w * state.phys_h * 4) as usize;
        std::ptr::write_bytes(state.bits_ptr, 0, total_bytes);

        state.renderer.render(
            state.gp_graphics,
            state.logical_w,
            state.logical_h,
            state.runtime.width_spring.current,
            state.runtime.height_spring.current,
            &state.runtime.current_state,
            state.runtime.dock_mode,
            &state.runtime.recent_sessions,
            &state.runtime.active_sessions_pool,
            &state.runtime.odometer,
        );

        let screen_dc = GetDC(hwnd);
        let mut pt_src = POINT { x: 0, y: 0 };
        let mut size_wnd = SIZE {
            cx: state.phys_w,
            cy: state.phys_h,
        };
        let mut blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA,
        };

        UpdateLayeredWindow(
            hwnd,
            screen_dc,
            null_mut(),
            &mut size_wnd,
            state.mem_dc,
            &mut pt_src,
            0,
            &mut blend as *mut _ as _,
            ULW_ALPHA,
        );

        ReleaseDC(hwnd, screen_dc);
    }
}

fn encode_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn add_tray_icon(hwnd: HWND, hicon: HICON, tip: &str) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAYICON;
        nid.hIcon = hicon;

        let tip_utf16: Vec<u16> = tip.encode_utf16().collect();
        let max_len = nid.szTip.len() - 1;
        for (i, &c) in tip_utf16.iter().enumerate().take(max_len) {
            nid.szTip[i] = c;
        }

        Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

unsafe fn update_tray_tip(hwnd: HWND, tip: &str) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_TIP;

        let tip_utf16: Vec<u16> = tip.encode_utf16().collect();
        let max_len = nid.szTip.len() - 1;
        for (i, &c) in tip_utf16.iter().enumerate().take(max_len) {
            nid.szTip[i] = c;
        }

        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

unsafe fn remove_tray_icon(hwnd: HWND) {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_runtime() -> IslandRuntime {
        let cfg = AppConfig::default();
        let filter = SharedNoiseFilter::new(cfg.noise_level);
        IslandRuntime::new(cfg, filter)
    }

    fn subagent_ev(
        group_key: &str,
        parent_session_id: Option<&str>,
        worker_id: &str,
        worker_name: &str,
        status: SwarmWorkerStatus,
        task_summary: &str,
        step_index: u64,
    ) -> AgentEvent {
        AgentEvent::SubagentStatus {
            group_key: group_key.to_string(),
            parent_session_id: parent_session_id.map(|s| s.to_string()),
            worker_id: worker_id.to_string(),
            worker_name: worker_name.to_string(),
            project_name: "WNoobar".to_string(),
            agent_name: "Antigravity".to_string(),
            status,
            task_summary: task_summary.to_string(),
            step_index,
        }
    }

    #[test]
    fn test_swarm_expanded_dimensions_and_odometer_pause() {
        let mut rt = make_runtime();
        assert_eq!(
            IslandState::Idle.target_dimensions(EdgeDockMode::SwarmExpanded),
            (440.0, 190.0)
        );
        rt.set_dock_mode(EdgeDockMode::Normal);
        assert!(rt.should_update_odometer());

        rt.set_dock_mode(EdgeDockMode::SwarmExpanded);
        assert!(!rt.should_update_odometer());
        assert_eq!(rt.width_spring.target, 440.0);
        assert_eq!(rt.height_spring.target, 190.0);
        assert!(rt.swarm_hover_since.is_none());

        rt.set_dock_mode(EdgeDockMode::DeckExpanded);
        assert!(!rt.should_update_odometer());
    }

    #[test]
    fn test_subagent_aggregation_no_pollution_and_deferred_parent_done() {
        let mut rt = make_runtime();
        let t0 = Instant::now();

        // 预置两个已有项目的近期记录，校验不会被 4 个子代理挤出
        rt.recent_sessions.push((
            "other-1".into(),
            "ProjAlpha".into(),
            "助手".into(),
            "完成构建".into(),
            "Cursor".into(),
        ));
        rt.recent_sessions.push((
            "other-2".into(),
            "ProjBeta".into(),
            "助手".into(),
            "测试中".into(),
            "Claude".into(),
        ));

        rt.handle_event_at(
            AgentEvent::TaskStarted {
                session_id: "parent-1".into(),
                project_name: "WNoobar".into(),
                role_type: "Coordinator".into(),
                agent_name: "Antigravity".into(),
                task_summary: "启动 Swarm".into(),
            },
            t0,
        );

        for (idx, name) in ["@core", "@ui", "@win32", "@qa"].iter().enumerate() {
            rt.handle_event_at(
                subagent_ev(
                    "parent-1",
                    Some("parent-1"),
                    &format!("w-{}", idx + 1),
                    name,
                    SwarmWorkerStatus::Working,
                    &format!("任务 {}", idx + 1),
                    (idx as u64) + 1,
                ),
                t0 + Duration::from_millis(20 + idx as u64),
            );
        }

        // active_sessions_pool 必须严格聚合为 1 条，recent_sessions 保留原有两个项目
        assert_eq!(rt.active_sessions_pool.len(), 1);
        assert_eq!(rt.active_sessions_pool[0].session_id, "parent-1");
        let swarm = rt.active_sessions_pool[0]
            .swarm
            .as_ref()
            .expect("swarm summary attached");
        assert_eq!(swarm.total_workers, 4);
        assert_eq!(swarm.working_count, 4);
        assert_eq!(rt.recent_sessions.len(), 3);
        assert!(rt.recent_sessions.iter().any(|(id, ..)| id == "other-1"));
        assert!(rt.recent_sessions.iter().any(|(id, ..)| id == "other-2"));

        // 父会话 Working 事件不可覆盖回 swarm = None
        rt.handle_event_at(
            AgentEvent::Working {
                session_id: "parent-1".into(),
                project_name: "WNoobar".into(),
                role_type: "Coordinator".into(),
                agent_name: "Antigravity".into(),
                task_summary: "协调各路子代理".into(),
                step_index: 10,
            },
            t0 + Duration::from_millis(50),
        );
        assert_eq!(rt.active_sessions_pool.len(), 1);
        assert!(rt.active_sessions_pool[0].swarm.is_some());

        // 父会话过早 Done：仍有 4 个子代理在 Working，必须延迟抑制！
        let early_done = rt.handle_event_at(
            AgentEvent::Done {
                session_id: "parent-1".into(),
                project_name: "WNoobar".into(),
                role_type: "Coordinator".into(),
                agent_name: "Antigravity".into(),
                summary: "主线程等待".into(),
                duration_secs: 5,
                artifacts_count: 0,
            },
            t0 + Duration::from_millis(100),
        );
        assert_eq!(early_done.sound_cue, None);
        assert!(matches!(rt.current_state, IslandState::Working { .. }));
        assert_eq!(rt.active_sessions_pool.len(), 1);

        // 前 3 个 Worker 完结：仍不得提前收官
        for idx in 1..=3 {
            let eff = rt.handle_event_at(
                subagent_ev(
                    "parent-1",
                    Some("parent-1"),
                    &format!("w-{}", idx),
                    &format!("@w{}", idx),
                    SwarmWorkerStatus::Done,
                    "已完成",
                    12,
                ),
                t0 + Duration::from_millis(200 + idx as u64),
            );
            assert_eq!(eff.sound_cue, None);
            assert!(matches!(rt.current_state, IslandState::Working { .. }));
            assert_eq!(rt.active_sessions_pool.len(), 1);
        }

        // 第 4 个 Worker 完结：立即触发统一收官 Done，清空活跃池并播报 CueKind::Done
        let final_eff = rt.handle_event_at(
            subagent_ev(
                "parent-1",
                Some("parent-1"),
                "w-4",
                "@qa",
                SwarmWorkerStatus::Done,
                "验收完成",
                15,
            ),
            t0 + Duration::from_millis(500),
        );
        assert_eq!(final_eff.sound_cue, Some(CueKind::Done));
        assert!(rt.active_sessions_pool.is_empty());
        assert_eq!(rt.dock_mode, EdgeDockMode::Normal);
        match &rt.current_state {
            IslandState::Done {
                session_id,
                summary,
                ..
            } => {
                assert_eq!(session_id, "parent-1");
                assert_eq!(summary, "Swarm 4/4 已收官");
            }
            other => panic!("Expected unified Done, got {:?}", other),
        }
    }

    #[test]
    fn test_swarm_action_needed_alert_dedup_and_recovery() {
        let mut rt = make_runtime();
        let t0 = Instant::now();

        rt.handle_event_at(
            subagent_ev(
                "parent-a",
                Some("parent-a"),
                "w-1",
                "@win32_eng",
                SwarmWorkerStatus::Working,
                "编码中",
                1,
            ),
            t0,
        );
        rt.set_dock_mode(EdgeDockMode::Peeking);

        // 首次出现 ActionNeeded：切 Normal 并响提示音
        let first_block = rt.handle_event_at(
            subagent_ev(
                "parent-a",
                Some("parent-a"),
                "w-1",
                "@win32_eng",
                SwarmWorkerStatus::ActionNeeded,
                "需要审批命令",
                2,
            ),
            t0 + Duration::from_millis(100),
        );
        assert_eq!(first_block.sound_cue, Some(CueKind::ActionNeeded));
        assert_eq!(rt.dock_mode, EdgeDockMode::Normal);
        assert!(matches!(rt.current_state, IslandState::ActionNeeded { .. }));

        // 用户点击收起
        rt.collapse_to_peeking_suppressed_at(t0 + Duration::from_millis(150));
        assert_eq!(rt.dock_mode, EdgeDockMode::Peeking);

        // 其他 Worker 心跳或同节点重复心跳：禁止重复叮咚响，且不得强行弹窗打断 Peeking
        let heartbeat_other = rt.handle_event_at(
            subagent_ev(
                "parent-a",
                Some("parent-a"),
                "w-2",
                "@ui_eng",
                SwarmWorkerStatus::Working,
                "绘制进度条",
                4,
            ),
            t0 + Duration::from_millis(300),
        );
        assert_eq!(heartbeat_other.sound_cue, None);
        assert_eq!(rt.dock_mode, EdgeDockMode::Peeking);

        let heartbeat_same = rt.handle_event_at(
            subagent_ev(
                "parent-a",
                Some("parent-a"),
                "w-1",
                "@win32_eng",
                SwarmWorkerStatus::ActionNeeded,
                "需要审批命令",
                2,
            ),
            t0 + Duration::from_millis(400),
        );
        assert_eq!(heartbeat_same.sound_cue, None);
        assert_eq!(rt.dock_mode, EdgeDockMode::Peeking);

        // 阻塞解除恢复 Working，之后再次遇到阻塞应重新提示
        rt.handle_event_at(
            subagent_ev(
                "parent-a",
                Some("parent-a"),
                "w-1",
                "@win32_eng",
                SwarmWorkerStatus::Working,
                "继续执行",
                5,
            ),
            t0 + Duration::from_millis(500),
        );
        assert!(matches!(rt.current_state, IslandState::Working { .. }));

        let second_block = rt.handle_event_at(
            subagent_ev(
                "parent-a",
                Some("parent-a"),
                "w-1",
                "@win32_eng",
                SwarmWorkerStatus::ActionNeeded,
                "需要审批命令",
                6,
            ),
            t0 + Duration::from_millis(700),
        );
        assert_eq!(second_block.sound_cue, Some(CueKind::ActionNeeded));
        assert_eq!(rt.dock_mode, EdgeDockMode::Normal);
    }

    #[test]
    fn test_swarm_hover_260ms_expand_120ms_collapse_and_click_jump() {
        let mut rt = make_runtime();
        let t0 = Instant::now();

        rt.handle_event_at(
            subagent_ev(
                "parent-h",
                Some("parent-h"),
                "w-1",
                "@core",
                SwarmWorkerStatus::Working,
                "运行中",
                3,
            ),
            t0,
        );
        rt.set_dock_mode(EdgeDockMode::Normal);

        let (cap_x, cap_y, cap_w, cap_h) = (40.0, 4.0, 400.0, 38.0);
        // 悬停进入胶囊内部
        rt.update_hover_state_at(
            cap_x,
            cap_y,
            cap_w,
            cap_h,
            240.0,
            20.0,
            480.0,
            t0 + Duration::from_millis(10),
        );
        assert!(rt.swarm_hover_since.is_some());
        assert_eq!(rt.dock_mode, EdgeDockMode::Normal);

        // 200ms (<260ms) 尚未展开
        rt.update_hover_state_at(
            cap_x,
            cap_y,
            cap_w,
            cap_h,
            240.0,
            20.0,
            480.0,
            t0 + Duration::from_millis(210),
        );
        assert_eq!(rt.dock_mode, EdgeDockMode::Normal);

        // 鼠标移出胶囊框（虽仍在 keep_box 内）立即清零 swarm_hover_since
        rt.update_hover_state_at(
            cap_x,
            cap_y,
            cap_w,
            cap_h,
            240.0,
            48.0,
            480.0,
            t0 + Duration::from_millis(230),
        );
        assert!(rt.swarm_hover_since.is_none());
        assert_eq!(rt.dock_mode, EdgeDockMode::Normal);

        // 重新悬停超过 260ms -> 自动切换为 EdgeDockMode::SwarmExpanded
        let t_reenter = t0 + Duration::from_millis(300);
        rt.update_hover_state_at(cap_x, cap_y, cap_w, cap_h, 240.0, 20.0, 480.0, t_reenter);
        rt.update_hover_state_at(
            cap_x,
            cap_y,
            cap_w,
            cap_h,
            240.0,
            20.0,
            480.0,
            t_reenter + Duration::from_millis(270),
        );
        assert_eq!(rt.dock_mode, EdgeDockMode::SwarmExpanded);
        assert!(rt.swarm_hover_since.is_none());
        assert_eq!(rt.width_spring.target, 440.0);
        assert_eq!(rt.height_spring.target, 190.0);

        // 点击 SwarmExpanded 面板主体（非左上角图标）-> 直接返回 FocusSession("parent-h")
        let (exp_x, exp_y, exp_w, exp_h) = (20.0, 4.0, 440.0, 190.0);
        assert_eq!(
            rt.resolve_capsule_click(exp_x, exp_y, exp_w, exp_h, 300.0, 100.0),
            CapsuleClickAction::FocusSession("parent-h".to_string())
        );
        // 点击左上角图标 -> 切换至 DeckExpanded
        assert_eq!(
            rt.resolve_capsule_click(exp_x, exp_y, exp_w, exp_h, exp_x + 16.0, exp_y + 16.0),
            CapsuleClickAction::SetDockMode(EdgeDockMode::DeckExpanded)
        );

        // 鼠标保持在 SwarmExpanded 的 in_keep_box 内持续展示；移出超过 120ms 收缩回 Peeking
        let t_leave = t_reenter + Duration::from_millis(300);
        rt.update_hover_state_at(exp_x, exp_y, exp_w, exp_h, 10.0, 260.0, 480.0, t_leave);
        assert_eq!(rt.dock_mode, EdgeDockMode::SwarmExpanded);

        rt.update_hover_state_at(
            exp_x,
            exp_y,
            exp_w,
            exp_h,
            10.0,
            260.0,
            480.0,
            t_leave + Duration::from_millis(130),
        );
        assert_eq!(rt.dock_mode, EdgeDockMode::Peeking);
        assert!(rt.swarm_hover_since.is_none());
    }

    #[test]
    fn test_swarm_fallback_promotion_and_watchdog_retire_stale() {
        let mut rt = make_runtime();
        let t0 = Instant::now();

        // 无 parent_session_id 回退组先到达，随后父会话 TaskStarted 到达并升格去重
        rt.handle_event_at(
            subagent_ev(
                "Antigravity::WNoobar",
                None,
                "w-1",
                "@core",
                SwarmWorkerStatus::Working,
                "初始化",
                1,
            ),
            t0,
        );
        assert_eq!(rt.active_sessions_pool.len(), 1);
        assert_eq!(rt.active_sessions_pool[0].session_id, "Antigravity::WNoobar");

        rt.handle_event_at(
            AgentEvent::TaskStarted {
                session_id: "parent-real".into(),
                project_name: "WNoobar".into(),
                role_type: "Coordinator".into(),
                agent_name: "Antigravity".into(),
                task_summary: "主线任务".into(),
            },
            t0 + Duration::from_millis(20),
        );
        assert_eq!(rt.active_sessions_pool.len(), 1);
        assert_eq!(rt.active_sessions_pool[0].session_id, "parent-real");
        assert!(rt.active_sessions_pool[0].swarm.is_some());

        // 后续无 parent_id 子代理更新仍保持单条 parent-real 不分裂
        rt.handle_event_at(
            subagent_ev(
                "Antigravity::WNoobar",
                None,
                "w-2",
                "@ui",
                SwarmWorkerStatus::Done,
                "完成",
                2,
            ),
            t0 + Duration::from_millis(40),
        );
        assert_eq!(rt.active_sessions_pool.len(), 1);
        assert_eq!(rt.active_sessions_pool[0].session_id, "parent-real");

        // 父会话 Done 被挂起，305s 后看门狗清理掉卡死超时的 w-1，仅剩 Done 的 w-2 触发统一收官
        rt.handle_event_at(
            AgentEvent::Done {
                session_id: "parent-real".into(),
                project_name: "WNoobar".into(),
                role_type: "Coordinator".into(),
                agent_name: "Antigravity".into(),
                summary: "等待收官".into(),
                duration_secs: 2,
                artifacts_count: 0,
            },
            t0 + Duration::from_millis(60),
        );
        assert!(matches!(rt.current_state, IslandState::Working { .. }));

        // 在 250s 刷新 w-2 保活，而 w-1 停留在 t0
        rt.handle_event_at(
            subagent_ev(
                "Antigravity::WNoobar",
                None,
                "w-2",
                "@ui",
                SwarmWorkerStatus::Done,
                "完成",
                2,
            ),
            t0 + Duration::from_secs(250),
        );
        let wd_eff = rt.tick_timers_at(t0 + Duration::from_secs(305));
        assert_eq!(wd_eff.sound_cue, Some(CueKind::Done));
        assert!(matches!(rt.current_state, IslandState::Done { .. }));
        assert!(rt.active_sessions_pool.is_empty());
    }

    fn task_started_ev(sid: &str, project: &str, agent: &str) -> AgentEvent {
        AgentEvent::TaskStarted {
            session_id: sid.into(),
            project_name: project.into(),
            role_type: "Coordinator".into(),
            agent_name: agent.into(),
            task_summary: "主线任务".into(),
        }
    }

    #[test]
    fn test_dual_swarm_render_and_click_target_are_identical() {
        let mut rt = make_runtime();
        let t0 = Instant::now();
        rt.handle_event_at(
            subagent_ev("parent-a", Some("parent-a"), "a-1", "@a", SwarmWorkerStatus::Working, "A 任务", 1),
            t0,
        );
        rt.handle_event_at(
            subagent_ev("parent-b", Some("parent-b"), "b-1", "@b", SwarmWorkerStatus::Working, "B 任务", 1),
            t0 + Duration::from_millis(10),
        );
        assert_eq!(rt.active_sessions_pool.len(), 2);

        // Odometer 指向第 2 项（Swarm B），随后后台 Swarm A 刷新 Working 事件，current_state 被切到 A
        let b_idx = rt
            .active_sessions_pool
            .iter()
            .position(|i| i.session_id == "parent-b")
            .unwrap();
        rt.odometer.active_index = b_idx;
        rt.handle_event_at(
            subagent_ev("parent-a", Some("parent-a"), "a-1", "@a", SwarmWorkerStatus::Working, "A 刷新", 2),
            t0 + Duration::from_millis(30),
        );
        rt.odometer.active_index = b_idx;
        assert!(matches!(
            &rt.current_state,
            IslandState::Working { session_id, .. } if session_id == "parent-a"
        ));

        for mode in [EdgeDockMode::Normal, EdgeDockMode::SwarmExpanded] {
            rt.dock_mode = mode;
            let rendered = crate::ui::capsule::resolve_focused_swarm(
                &rt.current_state,
                &rt.active_sessions_pool,
                &rt.odometer,
                mode,
            )
            .expect("rendered swarm");
            let clicked = rt.focused_swarm_session_id().expect("click target");
            assert_eq!(rendered.group_key, "parent-b", "mode {:?}", mode);
            assert_eq!(clicked, "parent-b", "mode {:?}", mode);
            assert_eq!(
                rendered.parent_session_id.as_deref(),
                Some(clicked.as_str())
            );
        }
    }

    #[test]
    fn test_watchdog_removes_synthetic_swarm_shell_but_keeps_real_parent() {
        let mut rt = make_runtime();
        let t0 = Instant::now();

        // 合成占位：无父会话的回退组，仅有 Swarm 事件
        rt.handle_event_at(
            subagent_ev("Codex::Synth", None, "s-1", "@s", SwarmWorkerStatus::Working, "合成", 1),
            t0,
        );
        // 真实父会话 + Swarm
        rt.handle_event_at(task_started_ev("parent-real", "Real", "Antigravity"), t0);
        rt.handle_event_at(
            subagent_ev("parent-real", Some("parent-real"), "r-1", "@r", SwarmWorkerStatus::Working, "真实", 1),
            t0,
        );
        assert!(rt.active_sessions_pool.iter().any(|i| i.session_id == "Codex::Synth"));
        assert!(rt.active_sessions_pool.iter().any(|i| i.session_id == "parent-real"));

        // 305s 后子代理心跳超时，组被淘汰：合成条目被移除，真实父会话仅清空 swarm
        rt.tick_timers_at(t0 + Duration::from_secs(305));
        assert!(!rt.active_sessions_pool.iter().any(|i| i.session_id == "Codex::Synth"));
        let real = rt
            .active_sessions_pool
            .iter()
            .find(|i| i.session_id == "parent-real")
            .expect("real parent kept");
        assert!(real.swarm.is_none());
    }

    #[test]
    fn test_subagent_without_parent_id_upgrades_existing_parent_entry() {
        let mut rt = make_runtime();
        let t0 = Instant::now();
        rt.handle_event_at(task_started_ev("parent-x", "WNoobar", "Antigravity"), t0);
        assert_eq!(rt.active_sessions_pool.len(), 1);
        let recent_before = rt.recent_sessions.len();

        rt.handle_event_at(
            subagent_ev("Antigravity::WNoobar", None, "w-1", "@core", SwarmWorkerStatus::Working, "执行", 1),
            t0 + Duration::from_millis(20),
        );
        assert_eq!(rt.active_sessions_pool.len(), 1);
        assert_eq!(rt.active_sessions_pool[0].session_id, "parent-x");
        assert!(rt.active_sessions_pool[0].swarm.is_some());
        assert_eq!(rt.recent_sessions.len(), recent_before);

        rt.handle_event_at(
            subagent_ev("Antigravity::WNoobar", None, "w-2", "@ui", SwarmWorkerStatus::Working, "绘制", 2),
            t0 + Duration::from_millis(40),
        );
        assert_eq!(rt.active_sessions_pool.len(), 1);
        assert_eq!(rt.recent_sessions.len(), recent_before);
    }
}