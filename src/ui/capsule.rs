#![allow(dead_code)]
use std::ffi::c_void;
use std::ptr::null_mut;
use std::time::Instant;

use crate::core::fsm::{EdgeDockMode, FlipTransition, IslandState};
use crate::core::swarm::{SwarmSummary, SwarmWorkerInfo, SwarmWorkerStatus};
use crate::ui::glyphs::{AgentBrand, GlyphManager};
use crate::ui::odometer::{ActiveSessionItem, OdometerCarousel};
use crate::win32::gdiplus::*;

/// 解析当前焦点 Swarm 在 `active_sessions` 中的下标（渲染与点击跳转共用的**唯一**优先级策略，所见即所点）：
/// 1. 轮播激活（`len >= 2` 且非 `ActionNeeded`/`Done`）时，先取 `active_sessions[odometer.active_index % len]`，
///    其含 `swarm` 则直接命中；不含时仅在 `SwarmExpanded` 下继续回退，其余模式返回 `None`；
/// 2. 否则按状态 `session_id`（含 `group_key` 与 `parent_session_id`）匹配，其次按 `(project_name, agent_name)` 匹配；
/// 3. 多会话时回退至当前轮播焦点；`Idle` 或 `SwarmExpanded` 最终回退至首个含 `swarm` 的条目。
pub fn resolve_focused_swarm_index(
    state: &IslandState,
    active_sessions: &[ActiveSessionItem],
    odometer: &OdometerCarousel,
    dock_mode: EdgeDockMode,
) -> Option<usize> {
    let len = active_sessions.len();
    if len == 0 {
        return None;
    }
    let odo_idx = odometer.active_index % len;
    let rotation_active = len >= 2
        && !matches!(
            state,
            IslandState::ActionNeeded { .. } | IslandState::Done { .. }
        );
    if rotation_active {
        if active_sessions[odo_idx].swarm.is_some() {
            return Some(odo_idx);
        }
        if dock_mode != EdgeDockMode::SwarmExpanded {
            return None;
        }
    }

    let state_meta = match state {
        IslandState::Started {
            session_id,
            project_name,
            agent_name,
            ..
        }
        | IslandState::Working {
            session_id,
            project_name,
            agent_name,
            ..
        }
        | IslandState::ActionNeeded {
            session_id,
            project_name,
            agent_name,
            ..
        }
        | IslandState::Done {
            session_id,
            project_name,
            agent_name,
            ..
        } => Some((
            session_id.as_str(),
            project_name.as_str(),
            agent_name.as_str(),
        )),
        IslandState::Idle => None,
    };

    if let Some((sid, project, agent)) = state_meta {
        if let Some(i) = active_sessions.iter().position(|item| {
            item.swarm.as_ref().is_some_and(|s| {
                item.session_id == sid
                    || s.group_key == sid
                    || s.parent_session_id.as_deref() == Some(sid)
            })
        }) {
            return Some(i);
        }
        if let Some(i) = active_sessions.iter().position(|item| {
            item.swarm.as_ref().is_some_and(|s| {
                (item.project_name == project && item.agent_name == agent)
                    || (s.project_name == project && s.agent_name == agent)
            })
        }) {
            return Some(i);
        }
    }

    if len >= 2 && active_sessions[odo_idx].swarm.is_some() {
        return Some(odo_idx);
    }
    if state_meta.is_none() || dock_mode == EdgeDockMode::SwarmExpanded {
        return active_sessions.iter().position(|item| item.swarm.is_some());
    }
    None
}

/// 解析当前焦点对应的活跃 Swarm 摘要（策略见 [`resolve_focused_swarm_index`]）。
pub fn resolve_focused_swarm<'a>(
    state: &IslandState,
    active_sessions: &'a [ActiveSessionItem],
    odometer: &OdometerCarousel,
    dock_mode: EdgeDockMode,
) -> Option<&'a SwarmSummary> {
    resolve_focused_swarm_index(state, active_sessions, odometer, dock_mode)
        .and_then(|i| active_sessions[i].swarm.as_ref())
}

pub struct CapsuleRenderer {
    font_family_ui: *mut c_void,
    font_title: *mut c_void,
    font_body: *mut c_void,
    font_role: *mut c_void,
    font_small: *mut c_void,
    format_single_line: *mut c_void,
    pulse_phase: f32,
    pub flip_anim: FlipTransition,
    pub glyph_manager: GlyphManager,
    pub hovered_deck_idx: Option<usize>,
    pub is_action_hovered: bool,
    pub is_silent_mode: bool,
    pub is_dnd_active: bool,
}

impl CapsuleRenderer {
    pub fn new() -> Self {
        unsafe {
            let mut font_family_ui: *mut c_void = null_mut();
            let mut font_title: *mut c_void = null_mut();
            let mut font_body: *mut c_void = null_mut();
            let mut font_role: *mut c_void = null_mut();
            let mut font_small: *mut c_void = null_mut();

            // 1. 字体链：优先加载 Microsoft YaHei UI（针对 Windows UI 深度优化的无衬线）
            let mut status;
            let font_candidates = [
                "Microsoft YaHei UI\0",
                "Segoe UI Variable Text\0",
                "Segoe UI\0",
                "Microsoft YaHei\0",
            ];

            for candidate in &font_candidates {
                let name_utf16: Vec<u16> = candidate.encode_utf16().collect();
                status = GdipCreateFontFamilyFromName(name_utf16.as_ptr(), null_mut(), &mut font_family_ui);
                if status == 0 && !font_family_ui.is_null() {
                    break;
                }
            }

            if !font_family_ui.is_null() {
                // Retina 级字阶与字重分级（统一 12px 黄金字阶，基线 100% 严密共线对齐）
                GdipCreateFont(font_family_ui, 12.0, FONT_STYLE_BOLD, UNIT_PIXEL, &mut font_title);
                GdipCreateFont(font_family_ui, 12.0, FONT_STYLE_REGULAR, UNIT_PIXEL, &mut font_body);
                GdipCreateFont(font_family_ui, 12.0, FONT_STYLE_REGULAR, UNIT_PIXEL, &mut font_role);
                GdipCreateFont(font_family_ui, 11.0, FONT_STYLE_REGULAR, UNIT_PIXEL, &mut font_small);
            }

            // 单行绝不折行 StringFormat
            let mut format_single_line: *mut c_void = null_mut();
            GdipCreateStringFormat(0, 0, &mut format_single_line);
            if !format_single_line.is_null() {
                GdipSetStringFormatLineAlign(format_single_line, 1); // 垂直严格居中
                GdipSetStringFormatFlags(format_single_line, STRING_FORMAT_FLAGS_NO_WRAP); // 严禁换行
                GdipSetStringFormatTrimming(format_single_line, STRING_TRIMMING_ELLIPSIS_CHARACTER); // 超出省略号
            }

            Self {
                font_family_ui,
                font_title,
                font_body,
                font_role,
                font_small,
                format_single_line,
                pulse_phase: 0.0,
                flip_anim: FlipTransition::new(),
                glyph_manager: GlyphManager::new(),
                hovered_deck_idx: None,
                is_action_hovered: false,
                is_silent_mode: false,
                is_dnd_active: false,
            }
        }
    }

    unsafe fn measure_text_w(graphics: *mut c_void, font: *mut c_void, text: &str, format: *mut c_void) -> f32 {
        let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let layout = RectF { x: 0.0, y: 0.0, width: 2000.0, height: 100.0 };
        let mut bbox = RectF { x: 0.0, y: 0.0, width: 0.0, height: 0.0 };
        let mut fitted: i32 = 0;
        let mut lines: i32 = 0;
        let status = unsafe {
            GdipMeasureString(
                graphics,
                utf16.as_ptr(),
                utf16.len() as i32 - 1,
                font,
                &layout,
                format,
                &mut bbox,
                &mut fitted,
                &mut lines,
            )
        };
        if status == 0 && bbox.width > 0.0 {
            bbox.width.ceil()
        } else {
            (text.chars().count() as f32) * 12.0
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.pulse_phase = (self.pulse_phase + dt * 2.8) % (std::f32::consts::PI * 2.0);
        self.flip_anim.update(dt);
    }

    /// 判定逻辑坐标是否命中左侧品牌微标区域（用于展开/收起 Session Deck）。
    pub fn is_left_glyph_hit(cap_x: f32, cap_y: f32, cap_h: f32, log_x: f32, log_y: f32) -> bool {
        if cap_h < 20.0 {
            return false;
        }
        let hit_h = cap_h.min(38.0);
        log_x >= cap_x && log_x <= cap_x + 38.0 && log_y >= cap_y && log_y <= cap_y + hit_h
    }

    /// 根据 Session Deck 卡片几何坐标返回命中的会话索引（最多 3 张卡片）。
    pub fn deck_card_index_at(
        cap_x: f32,
        cap_y: f32,
        cap_w: f32,
        log_x: f32,
        log_y: f32,
        card_count: usize,
    ) -> Option<usize> {
        let visible = card_count.min(3);
        if visible == 0 || cap_w <= 28.0 {
            return None;
        }
        let card_left = cap_x + 14.0;
        let card_right = cap_x + cap_w - 14.0;
        if log_x < card_left || log_x > card_right {
            return None;
        }
        let start_y = cap_y + 40.0;
        let item_h = 42.0;
        let gap = 6.0;
        for i in 0..visible {
            let item_y = start_y + (i as f32) * (item_h + gap);
            if log_y >= item_y && log_y <= item_y + item_h {
                return Some(i);
            }
        }
        None
    }

    /// 判定在 `ActionNeeded` 卡片状态下是否命中目标窗口跳转有效热区（排除左侧品牌微标）。
    pub fn is_action_jump_hit(
        cap_x: f32,
        cap_y: f32,
        cap_w: f32,
        cap_h: f32,
        log_x: f32,
        log_y: f32,
    ) -> bool {
        if cap_w <= 0.0 || cap_h < 20.0 {
            return false;
        }
        let in_capsule =
            log_x >= cap_x && log_x <= cap_x + cap_w && log_y >= cap_y && log_y <= cap_y + cap_h;
        in_capsule && !Self::is_left_glyph_hit(cap_x, cap_y, cap_h, log_x, log_y)
    }

    /// 绘制紧凑圆角微胶囊标签（自动水平居中文本并保持整数像素对齐）。
    unsafe fn draw_pill_tag(
        &self,
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        text: &str,
        measured_text_w: f32,
        bg_color: u32,
        border_color: u32,
        text_color: u32,
    ) {
        unsafe {
            let mut path: *mut c_void = null_mut();
            GdipCreatePath(0, &mut path);
            if !path.is_null() {
                add_round_rect_to_path(path, x, y, w, h, radius);
                let mut bg_brush: *mut c_void = null_mut();
                GdipCreateSolidFill(bg_color, &mut bg_brush);
                GdipFillPath(graphics, bg_brush, path);
                GdipDeleteBrush(bg_brush);

                let mut border_pen: *mut c_void = null_mut();
                GdipCreatePen1(border_color, 1.0, UNIT_PIXEL, &mut border_pen);
                GdipDrawPath(graphics, border_pen, path);
                GdipDeletePen(border_pen);

                GdipDeletePath(path);
            }

            let text_utf16: Vec<u16> = format!("{}\0", text).encode_utf16().collect();
            let text_x = (x + ((w - measured_text_w) / 2.0).max(4.0)).round();
            let text_w = (x + w - text_x - 2.0).max(16.0);
            let text_rect = RectF {
                x: text_x,
                y,
                width: text_w,
                height: h,
            };
            let mut text_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(text_color, &mut text_brush);
            GdipDrawString(
                graphics,
                text_utf16.as_ptr(),
                text_utf16.len() as i32 - 1,
                self.font_small,
                &text_rect,
                self.format_single_line,
                text_brush,
            );
            GdipDeleteBrush(text_brush);
        }
    }

    pub unsafe fn render(
        &mut self,
        graphics: *mut c_void,
        canvas_w: f32,
        _canvas_h: f32,
        capsule_w: f32,
        capsule_h: f32,
        state: &IslandState,
        dock_mode: EdgeDockMode,
        recent_sessions: &[(String, String, String, String, String)], // id, project, role, desc, agent
        active_sessions: &[ActiveSessionItem],
        odometer: &OdometerCarousel,
    ) {
        unsafe {
            if graphics.is_null() {
                return;
            }

            // Retina 字体平滑抗锯齿：灰度抗锯齿 + 像素网格拟合，彻底消除模糊与彩色杂边
            GdipSetTextRenderingHint(graphics, TEXT_RENDERING_HINT_ANTIALIAS_GRID_FIT);
            GdipSetTextContrast(graphics, 0);

            // 1. 全连续形态演进进度因子 (0.0: 4px极顶微光条 -> 1.0: 32px及以上标准胶囊)
            let morph = ((capsule_h - 4.0) / 26.0).clamp(0.0, 1.0);
            let cap_x = ((canvas_w - capsule_w) / 2.0).round();
            // 贴顶平滑过渡：收拢时紧贴 y=0.0，展开时平滑下移至 cap_y=4.0 整数坐标
            let cap_y = (4.0 * morph).round();
            let cap_w = capsule_w.round();
            let cap_h = capsule_h.round();

            // 2. 关键防御：填充 Alpha=1 隐形热区底垫（彻底消灭 Win32 DWM Alpha==0 穿透断层）
            let pad_x = cap_x - 14.0;
            let pad_y = 0.0;
            let pad_w = cap_w + 28.0;
            let pad_h = cap_y + cap_h + 14.0;
            let mut pad_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(1, 0, 0, 0), &mut pad_brush);
            let mut pad_path: *mut c_void = null_mut();
            GdipCreatePath(0, &mut pad_path);
            add_round_rect_to_path(pad_path, pad_x, pad_y, pad_w, pad_h, 8.0);
            GdipFillPath(graphics, pad_brush, pad_path);
            GdipDeletePath(pad_path);
            GdipDeleteBrush(pad_brush);

            // 3. 几何圆角自适应
            let radius = if matches!(
                dock_mode,
                EdgeDockMode::DeckExpanded | EdgeDockMode::SwarmExpanded
            ) {
                22.0
            } else {
                (cap_h / 2.0).clamp(2.0, 20.0)
            };

            let expanded_swarm = if dock_mode == EdgeDockMode::SwarmExpanded {
                resolve_focused_swarm(state, active_sessions, odometer, dock_mode)
            } else {
                None
            };
            let normal_swarm = if dock_mode == EdgeDockMode::Normal {
                resolve_focused_swarm(state, active_sessions, odometer, dock_mode)
            } else {
                None
            };
            let active_swarm_has_action = expanded_swarm
                .or(normal_swarm)
                .is_some_and(|s| s.action_count > 0);

            // 4. 外部环境柔光 (Glow) - 展开时平滑增强
            if morph > 0.1 {
                let glow_base = match state {
                    IslandState::Idle => {
                        if active_swarm_has_action {
                            let a = (70.0 + 25.0 * self.pulse_phase.sin()) as u8;
                            (a, 255, 149, 0)
                        } else {
                            (20, 255, 255, 255)
                        }
                    }
                    IslandState::Started { .. } => {
                        if active_swarm_has_action {
                            let a = (70.0 + 25.0 * self.pulse_phase.sin()) as u8;
                            (a, 255, 149, 0)
                        } else {
                            (60, 0, 160, 255)
                        }
                    }
                    IslandState::Working { .. } => {
                        if active_swarm_has_action {
                            let a = (70.0 + 25.0 * self.pulse_phase.sin()) as u8;
                            (a, 255, 149, 0)
                        } else {
                            let a = (50.0 + 25.0 * self.pulse_phase.sin()) as u8;
                            (a, 0, 160, 255)
                        }
                    }
                    IslandState::Done { .. } => (55, 46, 204, 113),
                    IslandState::ActionNeeded { .. } => {
                        let a = (70.0 + 25.0 * self.pulse_phase.sin()) as u8;
                        (a, 255, 149, 0) // 琥珀暖橙专属警示辉光
                    }
                };
                let glow_a = ((glow_base.0 as f32) * morph) as u8;
                let glow_color = make_argb(glow_a, glow_base.1, glow_base.2, glow_base.3);

                let mut path: *mut c_void = null_mut();
                GdipCreatePath(0, &mut path);
                if !path.is_null() {
                    add_round_rect_to_path(path, cap_x - 2.5, cap_y - 2.5, cap_w + 5.0, cap_h + 5.0, radius + 2.5);
                    let mut glow_pen: *mut c_void = null_mut();
                    GdipCreatePen1(glow_color, 2.5, UNIT_PIXEL, &mut glow_pen);
                    GdipDrawPath(graphics, glow_pen, path);
                    GdipDeletePen(glow_pen);
                    GdipDeletePath(path);
                }
            }

            // 5. 绘制深黑晶石高对比磨砂主体 (Jet Black Frosted Glass - a=252 阻断背景穿透发虚)
            let mut path: *mut c_void = null_mut();
            GdipCreatePath(0, &mut path);
            if !path.is_null() {
                add_round_rect_to_path(path, cap_x, cap_y, cap_w, cap_h, radius);

                let body_color = if matches!(
                    dock_mode,
                    EdgeDockMode::DeckExpanded | EdgeDockMode::SwarmExpanded
                ) {
                    make_argb(248, 14, 14, 16)
                } else {
                    let a = (210.0 + 42.0 * morph) as u8;
                    let r = (16.0 + 4.0 * (1.0 - morph)) as u8;
                    let g = (16.0 + 6.0 * (1.0 - morph)) as u8;
                    let b = (20.0 + 8.0 * (1.0 - morph)) as u8;
                    make_argb(a, r, g, b)
                };
                let mut body_brush: *mut c_void = null_mut();
                GdipCreateSolidFill(body_color, &mut body_brush);
                GdipFillPath(graphics, body_brush, path);
                GdipDeleteBrush(body_brush);

                // 1px 精致纳米高光倒角边缘（随 morph 渐显）
                let border_a = (42.0 * morph).max(18.0) as u8;
                let border_color = make_argb(border_a, 255, 255, 255);
                let mut border_pen: *mut c_void = null_mut();
                GdipCreatePen1(border_color, 1.0, UNIT_PIXEL, &mut border_pen);
                GdipDrawPath(graphics, border_pen, path);
                GdipDeletePen(border_pen);

                GdipDeletePath(path);
            }

            // 6. 贴边微光条顶沿柔光（仅在微缩态呈现微光脉动）
            if morph < 0.3 {
                let strip_glow = ((1.0 - morph / 0.3) * (140.0 + 80.0 * self.pulse_phase.sin())) as u8;
                let strip_color = make_argb(strip_glow, 200, 225, 255);
                let mut strip_brush: *mut c_void = null_mut();
                GdipCreateSolidFill(strip_color, &mut strip_brush);
                let mut strip_path: *mut c_void = null_mut();
                GdipCreatePath(0, &mut strip_path);
                add_round_rect_to_path(strip_path, cap_x + 6.0, cap_y + 0.5, cap_w - 12.0, (cap_h - 1.0).max(1.5), 1.0);
                GdipFillPath(graphics, strip_brush, strip_path);
                GdipDeletePath(strip_path);
                GdipDeleteBrush(strip_brush);
            }

            // 7. 内容渐显（Fade-in）：仅当 morph > 0.35 时渲染内部文字与微标
            let content_alpha = ((morph - 0.35) / 0.65).clamp(0.0, 1.0);
            if content_alpha > 0.05 {
                // 启用胶囊圆角几何裁切：确保所有内部文字与元素 100% 严禁溢出圆角边缘！
                let mut clip_path: *mut c_void = null_mut();
                GdipCreatePath(0, &mut clip_path);
                if !clip_path.is_null() {
                    add_round_rect_to_path(clip_path, cap_x, cap_y, cap_w, cap_h, radius);
                    GdipSetClipPath(graphics, clip_path, 0);
                    GdipDeletePath(clip_path);
                }

                if dock_mode == EdgeDockMode::DeckExpanded {
                    self.render_deck_view(graphics, cap_x, cap_y, cap_w, cap_h, recent_sessions);
                } else if dock_mode == EdgeDockMode::SwarmExpanded {
                    if let Some(swarm) = expanded_swarm {
                        self.render_swarm_expanded_view(graphics, cap_x, cap_y, cap_w, cap_h, swarm);
                    } else {
                        self.render_deck_view(graphics, cap_x, cap_y, cap_w, cap_h, recent_sessions);
                    }
                } else if active_sessions.len() >= 2 && !matches!(state, IslandState::Done { .. } | IslandState::ActionNeeded { .. }) {
                    // 当处于多会话并发时，自动激活里程表式垂直滚轮换页！
                    self.render_odometer_view(graphics, cap_x, cap_y, cap_w, cap_h, active_sessions, odometer);
                } else {
                    match state {
                        IslandState::Idle => {
                            self.render_idle_state(graphics, cap_x, cap_y, cap_w, cap_h);
                        }
                        IslandState::Started { project_name, role_type, agent_name, task_summary, .. } => {
                            if let Some(swarm) = normal_swarm {
                                self.render_swarm_compact_row(
                                    graphics, cap_x, cap_y, cap_w, cap_h, project_name, agent_name, swarm, 1.0,
                                );
                            } else {
                                self.render_working_state(graphics, cap_x, cap_y, cap_w, cap_h, project_name, role_type, agent_name, task_summary, 0);
                            }
                        }
                        IslandState::Working { project_name, role_type, agent_name, task_summary, step_index, .. } => {
                            if let Some(swarm) = normal_swarm {
                                self.render_swarm_compact_row(
                                    graphics, cap_x, cap_y, cap_w, cap_h, project_name, agent_name, swarm, 1.0,
                                );
                            } else {
                                self.render_working_state(graphics, cap_x, cap_y, cap_w, cap_h, project_name, role_type, agent_name, task_summary, *step_index);
                            }
                        }
                        IslandState::Done { project_name, role_type, agent_name, summary, duration_secs, artifacts_count, .. } => {
                            self.render_done_state(graphics, cap_x, cap_y, cap_w, cap_h, project_name, role_type, agent_name, summary, *duration_secs, *artifacts_count);
                        }
                        IslandState::ActionNeeded { project_name, role_type, agent_name, prompt, .. } => {
                            self.render_action_state(graphics, cap_x, cap_y, cap_w, cap_h, project_name, role_type, agent_name, prompt);
                        }
                    }
                }

                GdipResetClip(graphics);
            }
        }
    }

    /// 待机状态（严格光学垂直居中对齐，严禁换行，纯整数像素对齐）
    unsafe fn render_idle_state(&self, graphics: *mut c_void, x: f32, y: f32, w: f32, h: f32) {
        unsafe {
            let (glyph_offset, title_offset, dot_gap, dot_w, right_pad) = if self.is_silent_mode {
                (16.0, 12.0, 3.0, 6.0, 10.0)
            } else {
                (18.0, 14.0, 6.0, 8.0, 14.0)
            };

            let glyph_x = (x + glyph_offset).round();
            let glyph_y = (y + h / 2.0).round();
            self.glyph_manager.draw(graphics, AgentBrand::WNoobar, glyph_x, glyph_y, 16.0, self.pulse_phase);

            // 统一垂直居中整数像素基准线
            let text_y = (y + (h - 18.0) / 2.0).round();
            let text_h = 18.0;

            // 1. 标题 "WNoobar"（精确测宽，绝不硬编码假定宽度）
            let title = "WNoobar";
            let title_utf16: Vec<u16> = format!("{}\0", title).encode_utf16().collect();
            let mut cur_x = (glyph_x + title_offset).round();
            let measured_title_w = Self::measure_text_w(graphics, self.font_title, title, self.format_single_line);
            let title_w = measured_title_w.clamp(20.0, 90.0).round();
            let title_rect = RectF { x: cur_x, y: text_y, width: title_w, height: text_h };
            let mut white_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(255, 255, 255, 255), &mut white_brush);
            GdipDrawString(graphics, title_utf16.as_ptr(), title_utf16.len() as i32 - 1, self.font_title, &title_rect, self.format_single_line, white_brush);
            GdipDeleteBrush(white_brush);
            cur_x = (cur_x + title_w + dot_gap).round();

            // 2. 分隔圆点 "·"（独立排版，左右严格对称，绝对垂直共线）
            let dot_str = "·\0";
            let dot_utf16: Vec<u16> = dot_str.encode_utf16().collect();
            let dot_rect = RectF { x: cur_x, y: text_y, width: dot_w, height: text_h };
            let mut dot_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(180, 165, 170, 180), &mut dot_brush);
            GdipDrawString(graphics, dot_utf16.as_ptr(), dot_utf16.len() as i32 - 1, self.font_body, &dot_rect, self.format_single_line, dot_brush);
            GdipDeleteBrush(dot_brush);
            cur_x = (cur_x + dot_w + dot_gap).round();

            // 3. 状态文本（Silent 降噪态显示 "待命 · 极简模式"，常规态显示 "待命就绪"）
            let sub_text = if self.is_silent_mode {
                "待命 · 极简模式"
            } else {
                "待命就绪"
            };
            let sub_utf16: Vec<u16> = format!("{}\0", sub_text).encode_utf16().collect();
            let remain_w = (x + w - cur_x - right_pad).max(10.0).round();
            let sub_rect = RectF { x: cur_x, y: text_y, width: remain_w, height: text_h };
            let sub_color = if self.is_silent_mode {
                make_argb(235, 175, 195, 220)
            } else {
                make_argb(235, 195, 200, 210) // 高对比度清晰银灰
            };
            let mut gray_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(sub_color, &mut gray_brush);
            GdipDrawString(graphics, sub_utf16.as_ptr(), sub_utf16.len() as i32 - 1, self.font_role, &sub_rect, self.format_single_line, gray_brush);
            GdipDeleteBrush(gray_brush);
        }
    }

    /// 执行中：结构化 [项目名] · [角色] · [指令/进度摘要]（单行流式排版，严格测宽，纯整数像素对齐，严禁折行）
    unsafe fn render_working_state(
        &self,
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        project: &str,
        role: &str,
        agent: &str,
        task_summary: &str,
        _step: u64,
    ) {
        unsafe {
            let brand = AgentBrand::from_name(agent);
            let glyph_x = (x + 18.0).round();
            let glyph_y = (y + h / 2.0).round();

            // 绘制专属品牌微标（Antigravity/Claude/Codex/DeepSeek）
            self.glyph_manager.draw(graphics, brand, glyph_x, glyph_y, 18.0, self.pulse_phase);

            let text_y = (y + (h - 18.0) / 2.0).round();
            let text_h = 18.0;

            // 1. 信息层级第一段：[项目名]
            let proj_name = if project.is_empty() { "WNoobar" } else { project };
            let proj_utf16: Vec<u16> = format!("{}\0", proj_name).encode_utf16().collect();
            let mut cur_x = (glyph_x + 14.0).round();
            let measured_proj_w = Self::measure_text_w(graphics, self.font_title, proj_name, self.format_single_line);
            let proj_w = measured_proj_w.clamp(20.0, 85.0).round();
            let proj_rect = RectF { x: cur_x, y: text_y, width: proj_w, height: text_h };
            let mut proj_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(255, 255, 255, 255), &mut proj_brush);
            GdipDrawString(graphics, proj_utf16.as_ptr(), proj_utf16.len() as i32 - 1, self.font_title, &proj_rect, self.format_single_line, proj_brush);
            GdipDeleteBrush(proj_brush);
            cur_x = (cur_x + proj_w + 6.0).round();

            // 分隔圆点 1
            let dot_str = "·\0";
            let dot_utf16: Vec<u16> = dot_str.encode_utf16().collect();
            let dot_w = 8.0;
            let dot_rect1 = RectF { x: cur_x, y: text_y, width: dot_w, height: text_h };
            let mut dot_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(180, 165, 170, 180), &mut dot_brush);
            GdipDrawString(graphics, dot_utf16.as_ptr(), dot_utf16.len() as i32 - 1, self.font_body, &dot_rect1, self.format_single_line, dot_brush);
            cur_x = (cur_x + dot_w + 6.0).round();

            // 2. 信息层级第二段：[角色/会话名]（天蓝色，测宽自适应，上限控在 105px 严防长句挤压右侧摘要）
            let role_clean = role.trim();
            let role_utf16: Vec<u16> = format!("{}\0", role_clean).encode_utf16().collect();
            let measured_role_w = Self::measure_text_w(graphics, self.font_role, role_clean, self.format_single_line);
            let role_w = measured_role_w.clamp(24.0, 105.0).round();
            let role_rect = RectF { x: cur_x, y: text_y, width: role_w, height: text_h };
            let mut role_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(255, 10, 185, 255), &mut role_brush); // 纯粹高明度天蓝，字体清晰明快
            GdipDrawString(graphics, role_utf16.as_ptr(), role_utf16.len() as i32 - 1, self.font_role, &role_rect, self.format_single_line, role_brush);
            GdipDeleteBrush(role_brush);
            cur_x = (cur_x + role_w + 6.0).round();

            // 分隔圆点 2
            let dot_rect2 = RectF { x: cur_x, y: text_y, width: dot_w, height: text_h };
            GdipDrawString(graphics, dot_utf16.as_ptr(), dot_utf16.len() as i32 - 1, self.font_body, &dot_rect2, self.format_single_line, dot_brush);
            GdipDeleteBrush(dot_brush);
            cur_x = (cur_x + dot_w + 6.0).round();

            // 3. 当前指令/进度摘要（占据剩余全部空间，单行无折行，末尾自动省略截断）
            let desc_utf16: Vec<u16> = format!("{}\0", task_summary).encode_utf16().collect();
            let remain_w = (x + w - cur_x - 18.0).max(20.0).round();
            let desc_rect = RectF { x: cur_x, y: text_y, width: remain_w, height: text_h };
            let mut desc_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(245, 225, 230, 240), &mut desc_brush); // 纯净高对比，告别模糊发虚
            GdipDrawString(graphics, desc_utf16.as_ptr(), desc_utf16.len() as i32 - 1, self.font_body, &desc_rect, self.format_single_line, desc_brush);
            GdipDeleteBrush(desc_brush);
        }
    }

    /// 里程表式多会话垂直滚轮轮播主入口
    unsafe fn render_odometer_view(
        &self,
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        sessions: &[ActiveSessionItem],
        odometer: &OdometerCarousel,
    ) {
        unsafe {
            if sessions.is_empty() {
                return;
            }

            let count = sessions.len();
            let curr_item = &sessions[odometer.active_index % count];
            let row_height = 24.0; // 垂直滚轮行高

            if odometer.is_rolling {
                let (prev_offset, curr_offset) = odometer.offsets(row_height);
                let prev_item = &sessions[odometer.prev_index % count];

                // 1. 向上滚出胶囊顶部的上一会话 (translateY = prev_offset)
                self.render_odometer_item(graphics, x, y + prev_offset, w, h, prev_item, 1.0);

                // 2. 从胶囊底部向上滚入的当前会话 (translateY = curr_offset)
                self.render_odometer_item(graphics, x, y + curr_offset, w, h, curr_item, 1.0);
            } else {
                // 3. 驻留态与卡槽落地微弹强调 (Landing Scale Bounce)
                let scale = odometer.landing_scale();
                self.render_odometer_item(graphics, x, y, w, h, curr_item, scale);
            }
        }
    }

    unsafe fn render_odometer_item(
        &self,
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        item: &ActiveSessionItem,
        scale: f32,
    ) {
        unsafe {
            if let Some(swarm) = item.swarm.as_ref() {
                self.render_swarm_compact_row(
                    graphics,
                    x,
                    y,
                    w,
                    h,
                    &item.project_name,
                    &item.agent_name,
                    swarm,
                    scale,
                );
            } else {
                self.render_odometer_single_row(
                    graphics,
                    x,
                    y,
                    w,
                    h,
                    &item.project_name,
                    &item.role_type,
                    &item.agent_name,
                    &item.task_summary,
                    item.step_index,
                    scale,
                );
            }
        }
    }

    /// 单行会话内容排版与渲染（支持落地微弹强调）
    unsafe fn render_odometer_single_row(
        &self,
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        project: &str,
        role: &str,
        agent: &str,
        task_summary: &str,
        step: u64,
        scale: f32,
    ) {
        unsafe {
            let mut g_state: u32 = 0;
            let need_scale = (scale - 1.0).abs() > 0.001;

            if need_scale {
                GdipSaveGraphics(graphics, &mut g_state);
                let cx = x + w / 2.0;
                let cy = y + h / 2.0;
                GdipTranslateWorldTransform(graphics, cx, cy, MATRIX_ORDER_PREPEND);
                GdipScaleWorldTransform(graphics, scale, scale, MATRIX_ORDER_PREPEND);
                GdipTranslateWorldTransform(graphics, -cx, -cy, MATRIX_ORDER_PREPEND);
            }

            let brand = AgentBrand::from_name(agent);
            let glyph_x = (x + 18.0).round();
            let glyph_y = (y + h / 2.0).round();

            // 绘制专属品牌微标
            self.glyph_manager.draw(graphics, brand, glyph_x, glyph_y, 18.0, self.pulse_phase);

            let text_y = (y + (h - 18.0) / 2.0).round();
            let text_h = 18.0;

            // 1. 项目名 (粗体白字，例如 "WNoobar" 或 "QQ-BOT")
            let proj_name = if project.is_empty() { "WNoobar" } else { project };
            let proj_utf16: Vec<u16> = format!("{}\0", proj_name).encode_utf16().collect();
            let mut cur_x = (glyph_x + 14.0).round();
            let measured_proj_w = Self::measure_text_w(graphics, self.font_title, proj_name, self.format_single_line);
            let proj_w = measured_proj_w.clamp(20.0, 85.0).round();
            let proj_rect = RectF { x: cur_x, y: text_y, width: proj_w, height: text_h };
            let mut proj_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(255, 255, 255, 255), &mut proj_brush);
            GdipDrawString(graphics, proj_utf16.as_ptr(), proj_utf16.len() as i32 - 1, self.font_title, &proj_rect, self.format_single_line, proj_brush);
            GdipDeleteBrush(proj_brush);
            cur_x = (cur_x + proj_w + 6.0).round();

            // 分隔圆点 1
            let dot_str = "·\0";
            let dot_utf16: Vec<u16> = dot_str.encode_utf16().collect();
            let dot_w = 8.0;
            let dot_rect1 = RectF { x: cur_x, y: text_y, width: dot_w, height: text_h };
            let mut dot_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(180, 165, 170, 180), &mut dot_brush);
            GdipDrawString(graphics, dot_utf16.as_ptr(), dot_utf16.len() as i32 - 1, self.font_body, &dot_rect1, self.format_single_line, dot_brush);
            cur_x = (cur_x + dot_w + 6.0).round();

            // 2. 角色/会话名 (天蓝色，上限控制在 105px)
            let role_clean = role.trim();
            let role_utf16: Vec<u16> = format!("{}\0", role_clean).encode_utf16().collect();
            let measured_role_w = Self::measure_text_w(graphics, self.font_role, role_clean, self.format_single_line);
            let role_w = measured_role_w.clamp(24.0, 105.0).round();
            let role_rect = RectF { x: cur_x, y: text_y, width: role_w, height: text_h };
            let mut role_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(255, 10, 185, 255), &mut role_brush);
            GdipDrawString(graphics, role_utf16.as_ptr(), role_utf16.len() as i32 - 1, self.font_role, &role_rect, self.format_single_line, role_brush);
            GdipDeleteBrush(role_brush);
            cur_x = (cur_x + role_w + 6.0).round();

            // 分隔圆点 2
            let dot_rect2 = RectF { x: cur_x, y: text_y, width: dot_w, height: text_h };
            GdipDrawString(graphics, dot_utf16.as_ptr(), dot_utf16.len() as i32 - 1, self.font_body, &dot_rect2, self.format_single_line, dot_brush);
            cur_x = (cur_x + dot_w + 6.0).round();

            // 3. 步数展示（如果有步数，例如 "第 222 步"，采用等宽数字平稳排版）
            if step > 0 {
                let step_str = format!("第 {} 步\0", step);
                let step_utf16: Vec<u16> = step_str.encode_utf16().collect();
                let measured_step_w = Self::measure_text_w(graphics, self.font_small, &step_str[..step_str.len() - 1], self.format_single_line);
                let step_w = measured_step_w.clamp(36.0, 70.0).round();
                let step_rect = RectF { x: cur_x, y: text_y, width: step_w, height: text_h };
                let mut step_brush: *mut c_void = null_mut();
                GdipCreateSolidFill(make_argb(230, 52, 211, 153), &mut step_brush); // 翡翠绿等宽数字
                GdipDrawString(graphics, step_utf16.as_ptr(), step_utf16.len() as i32 - 1, self.font_small, &step_rect, self.format_single_line, step_brush);
                GdipDeleteBrush(step_brush);
                cur_x = (cur_x + step_w + 6.0).round();

                // 分隔圆点 3
                let dot_rect3 = RectF { x: cur_x, y: text_y, width: dot_w, height: text_h };
                GdipDrawString(graphics, dot_utf16.as_ptr(), dot_utf16.len() as i32 - 1, self.font_body, &dot_rect3, self.format_single_line, dot_brush);
                cur_x = (cur_x + dot_w + 6.0).round();
            }
            GdipDeleteBrush(dot_brush);

            // 4. 当前指令/进度摘要 (高对比银灰)
            let desc_utf16: Vec<u16> = format!("{}\0", task_summary).encode_utf16().collect();
            let remain_w = (x + w - cur_x - 18.0).max(20.0).round();
            let desc_rect = RectF { x: cur_x, y: text_y, width: remain_w, height: text_h };
            let mut desc_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(245, 225, 230, 240), &mut desc_brush);
            GdipDrawString(graphics, desc_utf16.as_ptr(), desc_utf16.len() as i32 - 1, self.font_body, &desc_rect, self.format_single_line, desc_brush);
            GdipDeleteBrush(desc_brush);

            if need_scale {
                GdipRestoreGraphics(graphics, g_state);
            }
        }
    }

    /// 完成交付展开卡片（Apple 级双层模块化自适应排版，整数像素对齐）
    unsafe fn render_done_state(
        &self,
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        _h: f32,
        project: &str,
        _role: &str,
        agent: &str,
        summary: &str,
        duration: u64,
        artifacts: usize,
    ) {
        unsafe {
            let brand = AgentBrand::from_name(agent);
            let icon_center_x = (x + 24.0).round();
            let row1_y = (y + 11.0).round();

            // 1. 左侧：Agent 品牌图标 (18px)
            self.glyph_manager.draw(graphics, brand, icon_center_x, row1_y + 8.0, 18.0, self.pulse_phase);

            // 2. 左侧核心标识：醒目的绿色 "✓ 任务已完成" (13.0px Bold)
            let check_title = "✓ 任务已完成\0";
            let check_utf16: Vec<u16> = check_title.encode_utf16().collect();
            let check_rect = RectF {
                x: (x + 40.0).round(),
                y: row1_y,
                width: 98.0,
                height: 18.0,
            };
            let mut check_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(255, 46, 204, 113), &mut check_brush);
            GdipDrawString(graphics, check_utf16.as_ptr(), check_utf16.len() as i32 - 1, self.font_title, &check_rect, self.format_single_line, check_brush);
            GdipDeleteBrush(check_brush);

            // 3. 左侧工程胶囊徽章：[v10] (圆角深灰底垫，白字)
            let proj_name = if project.is_empty() { "WNoobar" } else { project };
            let proj_badge_text = format!("{}\0", proj_name);
            let proj_badge_utf16: Vec<u16> = proj_badge_text.encode_utf16().collect();
            let badge_x = (x + 142.0).round();
            let badge_w = (proj_name.chars().count() as f32 * 8.0 + 12.0).clamp(24.0, 110.0).round();
            let badge_h = 16.0;

            let mut badge_path: *mut c_void = null_mut();
            GdipCreatePath(0, &mut badge_path);
            add_round_rect_to_path(badge_path, badge_x, row1_y + 1.0, badge_w, badge_h, 4.0);
            let mut badge_bg: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(50, 255, 255, 255), &mut badge_bg);
            GdipFillPath(graphics, badge_bg, badge_path);
            GdipDeleteBrush(badge_bg);
            GdipDeletePath(badge_path);

            let proj_rect = RectF {
                x: badge_x + 6.0,
                y: row1_y + 1.0,
                width: badge_w - 6.0,
                height: badge_h,
            };
            let mut proj_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(235, 230, 235, 245), &mut proj_brush);
            GdipDrawString(graphics, proj_badge_utf16.as_ptr(), proj_badge_utf16.len() as i32 - 1, self.font_small, &proj_rect, self.format_single_line, proj_brush);
            GdipDeleteBrush(proj_brush);

            // 4. 右侧：耗时与产物胶囊 (右对齐，绝不撞车)
            let meta_str = if artifacts > 0 {
                format!("+{} 产物 · {}秒\0", artifacts, duration)
            } else {
                format!("{}秒\0", duration)
            };
            let meta_utf16: Vec<u16> = meta_str.encode_utf16().collect();
            let meta_w = if artifacts > 0 { 92.0 } else { 48.0 };
            let meta_x = (x + w - meta_w - 18.0).round();

            let mut meta_path: *mut c_void = null_mut();
            GdipCreatePath(0, &mut meta_path);
            add_round_rect_to_path(meta_path, meta_x, row1_y + 1.0, meta_w, 16.0, 4.0);
            let mut meta_bg: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(40, 46, 204, 113), &mut meta_bg);
            GdipFillPath(graphics, meta_bg, meta_path);
            GdipDeleteBrush(meta_bg);
            GdipDeletePath(meta_path);

            let meta_rect = RectF {
                x: meta_x + 4.0,
                y: row1_y + 1.0,
                width: meta_w - 6.0,
                height: 16.0,
            };
            let mut meta_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(255, 46, 204, 113), &mut meta_brush);
            GdipDrawString(graphics, meta_utf16.as_ptr(), meta_utf16.len() as i32 - 1, self.font_small, &meta_rect, self.format_single_line, meta_brush);
            GdipDeleteBrush(meta_brush);

            // 5. 第二行（内容摘要栏，紧凑垂直对称，消除大片留白）
            let sum_utf16: Vec<u16> = format!("{}\0", summary).encode_utf16().collect();
            let is_multi = summary.chars().count() > 24;
            let sum_rect = RectF {
                x: (x + 40.0).round(),
                y: (y + 31.0).round(),
                width: (w - 58.0).round(),
                height: if is_multi { 30.0 } else { 18.0 },
            };
            let mut sum_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(240, 225, 230, 238), &mut sum_brush);
            GdipDrawString(graphics, sum_utf16.as_ptr(), sum_utf16.len() as i32 - 1, self.font_body, &sum_rect, if is_multi { null_mut() } else { self.format_single_line }, sum_brush);
            GdipDeleteBrush(sum_brush);
        }
    }

    /// 任务交互态展开卡片（Apple 级琥珀暖橙警示排版，整数像素对齐）
    unsafe fn render_action_state(
        &self,
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        _h: f32,
        project: &str,
        role: &str,
        agent: &str,
        prompt: &str,
    ) {
        unsafe {
            let brand = AgentBrand::from_name(agent);
            let icon_center_x = (x + 24.0).round();
            let row1_y = (y + 11.0).round();

            // 1. 左侧：Agent 品牌图标 (18px)
            self.glyph_manager.draw(graphics, brand, icon_center_x, row1_y + 8.0, 18.0, self.pulse_phase);

            // 2. 核心交互警示文字：亮橙色 "⚡ 需要确认交互" (13.0px Bold)
            let action_title = "⚡ 需要确认交互";
            let measured_action_w = Self::measure_text_w(graphics, self.font_title, action_title, self.format_single_line);
            let action_w = measured_action_w.clamp(80.0, 112.0).round();
            let action_utf16: Vec<u16> = format!("{}\0", action_title).encode_utf16().collect();
            let action_rect = RectF {
                x: (x + 40.0).round(),
                y: row1_y,
                width: action_w,
                height: 18.0,
            };
            let mut action_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(255, 255, 149, 0), &mut action_brush);
            GdipDrawString(graphics, action_utf16.as_ptr(), action_utf16.len() as i32 - 1, self.font_title, &action_rect, self.format_single_line, action_brush);
            GdipDeleteBrush(action_brush);

            // 3. 右侧交互引导微标签："↗ 跳转窗口"（预先测量宽度，严格防止与左侧徽章碰撞）
            let jump_text = "↗ 跳转窗口";
            let measured_jump_w = Self::measure_text_w(graphics, self.font_small, jump_text, self.format_single_line);
            let jump_w = (measured_jump_w + 12.0).clamp(62.0, 96.0).round();
            let jump_h = 16.0;
            let jump_x = (x + w - jump_w - 18.0).round();
            let jump_y = row1_y + 1.0;
            let (jump_bg, jump_border, jump_fg) = if self.is_action_hovered {
                (
                    make_argb(125, 255, 159, 20),
                    make_argb(235, 255, 215, 120),
                    make_argb(255, 255, 250, 235),
                )
            } else {
                (
                    make_argb(65, 255, 149, 0),
                    make_argb(145, 255, 175, 50),
                    make_argb(255, 255, 215, 145),
                )
            };
            self.draw_pill_tag(
                graphics,
                jump_x,
                jump_y,
                jump_w,
                jump_h,
                4.0,
                jump_text,
                measured_jump_w,
                jump_bg,
                jump_border,
                jump_fg,
            );

            // 4. 中间项目与会话徽章：[项目名 · 角色]（动态测量并裁切至 jump_x - 8px 之前）
            let proj_name = if project.is_empty() { "WNoobar" } else { project };
            let badge_label = if !role.is_empty() && role != "AI 结对助手" {
                format!("{} · {}", proj_name, role)
            } else {
                proj_name.to_string()
            };
            let badge_x = (x + 40.0 + action_w + 6.0).round();
            let max_badge_w = (jump_x - 8.0 - badge_x).min(160.0).round();
            if max_badge_w >= 24.0 {
                let measured_badge_w = Self::measure_text_w(graphics, self.font_small, &badge_label, self.format_single_line);
                let badge_w = (measured_badge_w + 12.0).clamp(24.0, max_badge_w).round();
                let badge_h = 16.0;
                let badge_utf16: Vec<u16> = format!("{}\0", badge_label).encode_utf16().collect();

                let mut badge_path: *mut c_void = null_mut();
                GdipCreatePath(0, &mut badge_path);
                add_round_rect_to_path(badge_path, badge_x, row1_y + 1.0, badge_w, badge_h, 4.0);
                let mut badge_bg: *mut c_void = null_mut();
                GdipCreateSolidFill(make_argb(45, 255, 149, 0), &mut badge_bg);
                GdipFillPath(graphics, badge_bg, badge_path);
                GdipDeleteBrush(badge_bg);
                GdipDeletePath(badge_path);

                let proj_rect = RectF {
                    x: badge_x + 6.0,
                    y: row1_y + 1.0,
                    width: (badge_w - 10.0).max(8.0),
                    height: badge_h,
                };
                let mut proj_brush: *mut c_void = null_mut();
                GdipCreateSolidFill(make_argb(255, 255, 205, 130), &mut proj_brush);
                GdipDrawString(graphics, badge_utf16.as_ptr(), badge_utf16.len() as i32 - 1, self.font_small, &proj_rect, self.format_single_line, proj_brush);
                GdipDeleteBrush(proj_brush);
            }

            // 5. 第二行：具体的提问内容/待确认提示
            let prompt_utf16: Vec<u16> = format!("{}\0", prompt).encode_utf16().collect();
            let is_multi = prompt.chars().count() > 24;
            let prompt_rect = RectF {
                x: (x + 40.0).round(),
                y: (y + 31.0).round(),
                width: (w - 58.0).round(),
                height: if is_multi { 32.0 } else { 18.0 },
            };
            let mut p_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(255, 250, 250, 255), &mut p_brush);
            GdipDrawString(graphics, prompt_utf16.as_ptr(), prompt_utf16.len() as i32 - 1, self.font_body, &prompt_rect, if is_multi { null_mut() } else { self.format_single_line }, p_brush);
            GdipDeleteBrush(p_brush);
        }
    }

    /// 多会话抽屉（Session Deck）列表视图
    unsafe fn render_deck_view(
        &self,
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        _h: f32,
        recent_sessions: &[(String, String, String, String, String)],
    ) {
        unsafe {
            // 抽屉顶部标题
            let header_str = "活跃会话抽屉 (Session Deck) · 点击切换聚焦\0";
            let header_utf16: Vec<u16> = header_str.encode_utf16().collect();
            let header_rect = RectF {
                x: (x + 20.0).round(),
                y: (y + 12.0).round(),
                width: (w - 40.0).round(),
                height: 16.0,
            };
            let mut h_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(225, 185, 190, 205), &mut h_brush);
            GdipDrawString(graphics, header_utf16.as_ptr(), header_utf16.len() as i32 - 1, self.font_small, &header_rect, self.format_single_line, h_brush);
            GdipDeleteBrush(h_brush);

            // 分隔线
            let mut line_pen: *mut c_void = null_mut();
            GdipCreatePen1(make_argb(35, 255, 255, 255), 1.0, UNIT_PIXEL, &mut line_pen);
            GdipDrawLine(graphics, line_pen, (x + 16.0).round(), (y + 32.0).round(), (x + w - 16.0).round(), (y + 32.0).round());
            GdipDeletePen(line_pen);

            // 卡片列表条目
            let start_y = (y + 40.0).round();
            let item_h = 42.0;

            let jump_text = "↗ 跳转";
            let measured_jump_w = Self::measure_text_w(graphics, self.font_small, jump_text, self.format_single_line);
            let jump_w = (measured_jump_w + 12.0).clamp(48.0, 72.0).round();
            let jump_h = 18.0;
            let jump_x = (x + w - 14.0 - 10.0 - jump_w).round();

            for (i, (_, project, role, desc, agent)) in recent_sessions.iter().take(3).enumerate() {
                let item_y = (start_y + (i as f32) * (item_h + 6.0)).round();
                let is_hovered = self.hovered_deck_idx == Some(i);

                // 卡片底板与悬停高亮反馈
                let mut path: *mut c_void = null_mut();
                GdipCreatePath(0, &mut path);
                add_round_rect_to_path(path, (x + 14.0).round(), item_y, (w - 28.0).round(), item_h, 10.0);

                let bg_color = match (is_hovered, i == 0) {
                    (true, true) => make_argb(95, 20, 145, 255),
                    (true, false) => make_argb(60, 220, 235, 255),
                    (false, true) => make_argb(55, 10, 132, 255), // 当前聚焦高亮
                    (false, false) => make_argb(25, 255, 255, 255),
                };
                let mut brush: *mut c_void = null_mut();
                GdipCreateSolidFill(bg_color, &mut brush);
                GdipFillPath(graphics, brush, path);
                GdipDeleteBrush(brush);

                let stroke_color = match (is_hovered, i == 0) {
                    (true, true) => make_argb(190, 90, 185, 255),
                    (true, false) => make_argb(150, 255, 255, 255),
                    (false, true) => make_argb(65, 40, 150, 255),
                    (false, false) => make_argb(30, 255, 255, 255),
                };
                let mut stroke_pen: *mut c_void = null_mut();
                GdipCreatePen1(stroke_color, 1.0, UNIT_PIXEL, &mut stroke_pen);
                GdipDrawPath(graphics, stroke_pen, path);
                GdipDeletePen(stroke_pen);
                GdipDeletePath(path);

                // 绘制该项目的 Agent 专属微标（支持 Antigravity / Claude / Codex / DeepSeek 等）
                let brand = AgentBrand::from_name(agent);
                self.glyph_manager.draw(graphics, brand, (x + 30.0).round(), (item_y + item_h / 2.0).round(), 16.0, self.pulse_phase);

                // 右侧引导微缩标签 "↗ 跳转"（悬停态更亮）
                let jump_y = (item_y + (item_h - jump_h) / 2.0).round();
                let (jump_bg, jump_border, jump_fg) = if is_hovered {
                    (
                        make_argb(95, 10, 165, 255),
                        make_argb(190, 80, 200, 255),
                        make_argb(255, 240, 250, 255),
                    )
                } else {
                    (
                        make_argb(32, 255, 255, 255),
                        make_argb(45, 255, 255, 255),
                        make_argb(185, 195, 210, 230),
                    )
                };
                self.draw_pill_tag(
                    graphics,
                    jump_x,
                    jump_y,
                    jump_w,
                    jump_h,
                    5.0,
                    jump_text,
                    measured_jump_w,
                    jump_bg,
                    jump_border,
                    jump_fg,
                );

                // 左侧文本区域严格裁切至 jump_x - 8px 之前，严禁与右侧标签重叠
                let text_left = (x + 44.0).round();
                let text_max_w = (jump_x - 8.0 - text_left).max(40.0).round();

                // 项目与角色
                let row_title = format!("{}  ·  {}\0", project, role);
                let row_utf16: Vec<u16> = row_title.encode_utf16().collect();
                let row_rect = RectF { x: text_left, y: item_y + 6.0, width: text_max_w, height: 16.0 };
                let mut row_brush: *mut c_void = null_mut();
                GdipCreateSolidFill(make_argb(255, 255, 255, 255), &mut row_brush);
                GdipDrawString(graphics, row_utf16.as_ptr(), row_utf16.len() as i32 - 1, self.font_title, &row_rect, self.format_single_line, row_brush);
                GdipDeleteBrush(row_brush);

                // 描述
                let d_utf16: Vec<u16> = format!("{}\0", desc).encode_utf16().collect();
                let d_rect = RectF { x: text_left, y: item_y + 24.0, width: text_max_w, height: 16.0 };
                let mut d_brush: *mut c_void = null_mut();
                GdipCreateSolidFill(make_argb(230, 205, 210, 220), &mut d_brush);
                GdipDrawString(graphics, d_utf16.as_ptr(), d_utf16.len() as i32 - 1, self.font_body, &d_rect, self.format_single_line, d_brush);
                GdipDeleteBrush(d_brush);
            }
        }
    }

    unsafe fn draw_text_line(
        &self,
        graphics: *mut c_void,
        font: *mut c_void,
        text: &str,
        rect: RectF,
        color: u32,
    ) {
        unsafe {
            if rect.width <= 1.0 || rect.height <= 1.0 || font.is_null() {
                return;
            }
            let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let mut brush: *mut c_void = null_mut();
            GdipCreateSolidFill(color, &mut brush);
            if !brush.is_null() {
                GdipDrawString(
                    graphics,
                    utf16.as_ptr(),
                    utf16.len() as i32 - 1,
                    font,
                    &rect,
                    self.format_single_line,
                    brush,
                );
                GdipDeleteBrush(brush);
            }
        }
    }

    fn format_swarm_elapsed(secs: u64) -> String {
        if secs >= 3600 {
            format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
        } else {
            format!("{:02}:{:02}", secs / 60, secs % 60)
        }
    }

    fn swarm_status_color(&self, status: SwarmWorkerStatus, animated_dot: bool) -> u32 {
        match status {
            SwarmWorkerStatus::ActionNeeded => {
                if animated_dot {
                    let a = (195.0 + 60.0 * self.pulse_phase.sin())
                        .round()
                        .clamp(120.0, 255.0) as u8;
                    make_argb(a, 255, 149, 0)
                } else {
                    make_argb(245, 255, 149, 0)
                }
            }
            SwarmWorkerStatus::Working => {
                if animated_dot {
                    let a = (175.0 + 80.0 * self.pulse_phase.sin())
                        .round()
                        .clamp(95.0, 255.0) as u8;
                    make_argb(a, 10, 185, 255)
                } else {
                    make_argb(235, 10, 185, 255)
                }
            }
            SwarmWorkerStatus::Done => make_argb(245, 52, 211, 153),
            SwarmWorkerStatus::Waiting => make_argb(120, 165, 175, 190),
        }
    }

    /// 绘制单颗圆形状态指示灯
    unsafe fn draw_dot(&self, graphics: *mut c_void, x: f32, y: f32, size: f32, color: u32) {
        unsafe {
            let mut brush: *mut c_void = null_mut();
            GdipCreateSolidFill(color, &mut brush);
            if !brush.is_null() {
                GdipFillEllipse(graphics, brush, x, y, size, size);
                GdipDeleteBrush(brush);
            }
        }
    }

    /// 迷你状态点阵：圆角微底垫 `rgba(255,255,255,0.08)` + 最多 4 颗 `4px` 状态圆点 + 超 4 节点 `+N` 紧凑微标
    unsafe fn draw_swarm_dot_matrix(
        &self,
        graphics: *mut c_void,
        x: f32,
        row_y: f32,
        row_h: f32,
        swarm: &SwarmSummary,
    ) -> f32 {
        unsafe {
            let mut dot_statuses: Vec<SwarmWorkerStatus> =
                swarm.workers.iter().take(4).map(|w| w.status).collect();
            if dot_statuses.is_empty() && swarm.total_workers > 0 {
                for _ in 0..swarm.action_count {
                    if dot_statuses.len() < 4 {
                        dot_statuses.push(SwarmWorkerStatus::ActionNeeded);
                    }
                }
                for _ in 0..swarm.working_count {
                    if dot_statuses.len() < 4 {
                        dot_statuses.push(SwarmWorkerStatus::Working);
                    }
                }
                for _ in 0..swarm.done_count {
                    if dot_statuses.len() < 4 {
                        dot_statuses.push(SwarmWorkerStatus::Done);
                    }
                }
                for _ in 0..swarm.waiting_count {
                    if dot_statuses.len() < 4 {
                        dot_statuses.push(SwarmWorkerStatus::Waiting);
                    }
                }
            }

            let dot_count = dot_statuses.len();
            if dot_count == 0 {
                return 0.0;
            }

            let pill_h = 16.0;
            let pill_y = (row_y + (row_h - pill_h) / 2.0).round();
            let dot_size = 4.0;
            let dot_stride = 7.0;
            let dots_w =
                (dot_count as f32) * dot_size + ((dot_count.saturating_sub(1)) as f32) * 3.0;
            let pill_w = (6.0 + dots_w + 6.0).round();

            let mut pill_path: *mut c_void = null_mut();
            GdipCreatePath(0, &mut pill_path);
            if !pill_path.is_null() {
                add_round_rect_to_path(pill_path, x, pill_y, pill_w, pill_h, 8.0);
                let mut bg_brush: *mut c_void = null_mut();
                GdipCreateSolidFill(make_argb(20, 255, 255, 255), &mut bg_brush);
                if !bg_brush.is_null() {
                    GdipFillPath(graphics, bg_brush, pill_path);
                    GdipDeleteBrush(bg_brush);
                }
                GdipDeletePath(pill_path);
            }

            let dot_y = (pill_y + (pill_h - dot_size) / 2.0).round();
            for (i, status) in dot_statuses.iter().enumerate() {
                let dot_x = (x + 6.0 + (i as f32) * dot_stride).round();
                let color = self.swarm_status_color(*status, true);
                self.draw_dot(graphics, dot_x, dot_y, dot_size, color);
            }

            let total = swarm.total_workers.max(swarm.workers.len());
            if total > 4 {
                let plus_text = format!("+{}", total - 4);
                let measured_plus_w = Self::measure_text_w(
                    graphics,
                    self.font_small,
                    &plus_text,
                    self.format_single_line,
                );
                let badge_w = (measured_plus_w + 8.0).clamp(20.0, 40.0).round();
                let badge_x = (x + pill_w + 4.0).round();
                self.draw_pill_tag(
                    graphics,
                    badge_x,
                    pill_y,
                    badge_w,
                    pill_h,
                    5.0,
                    &plus_text,
                    measured_plus_w,
                    make_argb(24, 255, 255, 255),
                    make_argb(42, 255, 255, 255),
                    make_argb(230, 200, 212, 230),
                );
                (pill_w + 4.0 + badge_w).round()
            } else {
                pill_w
            }
        }
    }

    /// 按状态占比绘制底部贴边 `2px` 分段聚合微进度条
    unsafe fn draw_swarm_ratio_bar(
        &self,
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        swarm: &SwarmSummary,
    ) {
        unsafe {
            let bar_x = (x + 12.0).round();
            let bar_w = (w - 24.0).max(16.0).round();
            let bar_y = (y + h - 2.0).round();
            self.draw_swarm_ratio_bar_at(graphics, bar_x, bar_y, bar_w, 2.0, swarm);
        }
    }

    unsafe fn draw_swarm_ratio_bar_at(
        &self,
        graphics: *mut c_void,
        bar_x: f32,
        bar_y: f32,
        bar_w: f32,
        bar_h: f32,
        swarm: &SwarmSummary,
    ) {
        unsafe {
            let mut track_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(24, 255, 255, 255), &mut track_brush);
            if !track_brush.is_null() {
                GdipFillRectangle(graphics, track_brush, bar_x, bar_y, bar_w, bar_h);
                GdipDeleteBrush(track_brush);
            }

            let buckets = [
                (
                    swarm.action_count,
                    self.swarm_status_color(SwarmWorkerStatus::ActionNeeded, false),
                ),
                (
                    swarm.working_count,
                    self.swarm_status_color(SwarmWorkerStatus::Working, false),
                ),
                (
                    swarm.done_count,
                    self.swarm_status_color(SwarmWorkerStatus::Done, false),
                ),
                (
                    swarm.waiting_count,
                    self.swarm_status_color(SwarmWorkerStatus::Waiting, false),
                ),
            ];
            let active_buckets: Vec<(usize, u32)> =
                buckets.into_iter().filter(|(c, _)| *c > 0).collect();
            let total: usize = active_buckets.iter().map(|(c, _)| *c).sum();
            if total == 0 || active_buckets.is_empty() {
                return;
            }

            let gap = if active_buckets.len() > 1 && bar_w > 24.0 {
                2.0
            } else {
                0.0
            };
            let usable_w =
                (bar_w - gap * (active_buckets.len().saturating_sub(1) as f32)).max(4.0);
            let mut cur_x = bar_x;
            for (idx, (count, color)) in active_buckets.iter().enumerate() {
                let seg_w = if idx + 1 == active_buckets.len() {
                    (bar_x + bar_w - cur_x).max(2.0)
                } else {
                    ((*count as f32 / total as f32) * usable_w)
                        .round()
                        .max(3.0)
                        .min((bar_x + bar_w - cur_x).max(0.0))
                };
                if seg_w <= 0.0 {
                    continue;
                }
                let mut seg_brush: *mut c_void = null_mut();
                GdipCreateSolidFill(*color, &mut seg_brush);
                if !seg_brush.is_null() {
                    GdipFillRectangle(graphics, seg_brush, cur_x, bar_y, seg_w, bar_h);
                    GdipDeleteBrush(seg_brush);
                }
                cur_x = (cur_x + seg_w + gap).round();
            }
        }
    }

    /// 绘制 SwarmExpanded 顶栏下方 `3px` 分段总进度条（每个 worker 按状态着色）
    unsafe fn draw_swarm_worker_bar(
        &self,
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        swarm: &SwarmSummary,
    ) {
        unsafe {
            let bar_x = (x + 16.0).round();
            let bar_w = (w - 32.0).max(20.0).round();
            let bar_y = (y + 30.0).round();
            let bar_h = 3.0;

            let n = swarm.workers.len();
            if n == 0 {
                self.draw_swarm_ratio_bar_at(graphics, bar_x, bar_y, bar_w, bar_h, swarm);
                return;
            }

            let mut track_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(26, 255, 255, 255), &mut track_brush);
            if !track_brush.is_null() {
                GdipFillRectangle(graphics, track_brush, bar_x, bar_y, bar_w, bar_h);
                GdipDeleteBrush(track_brush);
            }

            let per_cell = bar_w / (n as f32);
            let gap = if n <= 1 {
                0.0
            } else if per_cell >= 12.0 {
                2.0
            } else if per_cell >= 5.0 {
                1.0
            } else {
                0.0
            };
            let usable_w = (bar_w - gap * ((n - 1) as f32)).max(n as f32);
            let step = usable_w / (n as f32) + gap;

            for (i, worker) in swarm.workers.iter().enumerate() {
                let seg_x = if i == 0 {
                    bar_x
                } else {
                    (bar_x + (i as f32) * step).round()
                };
                let next_x = if i + 1 == n {
                    bar_x + bar_w
                } else {
                    (bar_x + ((i + 1) as f32) * step - gap).round()
                };
                let seg_w = (next_x - seg_x).max(1.0);
                let color = self.swarm_status_color(worker.status, false);
                let mut seg_brush: *mut c_void = null_mut();
                GdipCreateSolidFill(color, &mut seg_brush);
                if !seg_brush.is_null() {
                    GdipFillRectangle(graphics, seg_brush, seg_x, bar_y, seg_w, bar_h);
                    GdipDeleteBrush(seg_brush);
                }
            }
        }
    }

    /// 折叠胶囊态紧凑 Swarm 聚合行（复用在 Normal 与 Odometer 滚轮视图中）
    #[allow(clippy::too_many_arguments)]
    unsafe fn render_swarm_compact_row(
        &self,
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        project: &str,
        agent: &str,
        swarm: &SwarmSummary,
        scale: f32,
    ) {
        unsafe {
            let mut g_state: u32 = 0;
            let need_scale = (scale - 1.0).abs() > 0.001;
            if need_scale {
                GdipSaveGraphics(graphics, &mut g_state);
                let cx = x + w / 2.0;
                let cy = y + h / 2.0;
                GdipTranslateWorldTransform(graphics, cx, cy, MATRIX_ORDER_PREPEND);
                GdipScaleWorldTransform(graphics, scale, scale, MATRIX_ORDER_PREPEND);
                GdipTranslateWorldTransform(graphics, -cx, -cy, MATRIX_ORDER_PREPEND);
            }

            let agent_name = if !agent.is_empty() {
                agent
            } else {
                &swarm.agent_name
            };
            let brand = AgentBrand::from_name(agent_name);
            let glyph_x = (x + 18.0).round();
            let glyph_y = (y + h / 2.0).round();
            self.glyph_manager
                .draw(graphics, brand, glyph_x, glyph_y, 18.0, self.pulse_phase);

            let text_y = (y + (h - 18.0) / 2.0).round();
            let text_h = 18.0;

            // 第一段：[项目名] · Swarm
            let proj_name = if !project.is_empty() {
                project
            } else if !swarm.project_name.is_empty() {
                &swarm.project_name
            } else {
                "WNoobar"
            };
            let mut cur_x = (glyph_x + 14.0).round();
            let measured_proj_w = Self::measure_text_w(
                graphics,
                self.font_title,
                proj_name,
                self.format_single_line,
            );
            let proj_w = measured_proj_w.clamp(20.0, 74.0).round();
            self.draw_text_line(
                graphics,
                self.font_title,
                proj_name,
                RectF {
                    x: cur_x,
                    y: text_y,
                    width: proj_w,
                    height: text_h,
                },
                make_argb(255, 255, 255, 255),
            );
            cur_x = (cur_x + proj_w + 5.0).round();

            let dot_w = 6.0;
            self.draw_text_line(
                graphics,
                self.font_body,
                "·",
                RectF {
                    x: cur_x,
                    y: text_y,
                    width: dot_w,
                    height: text_h,
                },
                make_argb(180, 165, 170, 180),
            );
            cur_x = (cur_x + dot_w + 5.0).round();

            let swarm_label = "Swarm";
            let swarm_color = if swarm.action_count > 0 {
                make_argb(255, 255, 149, 0)
            } else {
                make_argb(255, 10, 185, 255)
            };
            let measured_swarm_w = Self::measure_text_w(
                graphics,
                self.font_title,
                swarm_label,
                self.format_single_line,
            );
            let swarm_w = measured_swarm_w.clamp(36.0, 52.0).round();
            self.draw_text_line(
                graphics,
                self.font_title,
                swarm_label,
                RectF {
                    x: cur_x,
                    y: text_y,
                    width: swarm_w,
                    height: text_h,
                },
                swarm_color,
            );
            cur_x = (cur_x + swarm_w + 6.0).round();

            // 迷你状态点阵
            let matrix_w = self.draw_swarm_dot_matrix(graphics, cur_x, y, h, swarm);
            if matrix_w > 0.0 {
                cur_x = (cur_x + matrix_w + 8.0).round();
            }

            // 第二段：聚合摘要 + 重点子代理单行摘要（严格防止右侧溢出）
            let avail_w = (x + w - cur_x - 16.0).max(0.0).round();
            if avail_w >= 18.0 {
                let agg_text = swarm.aggregate_summary.trim();
                let agg_color = if swarm.action_count > 0 {
                    make_argb(255, 255, 179, 64)
                } else {
                    make_argb(245, 225, 232, 245)
                };
                let measured_agg_w = Self::measure_text_w(
                    graphics,
                    self.font_body,
                    agg_text,
                    self.format_single_line,
                );
                let agg_w = measured_agg_w.round();

                if let Some(focus) = swarm.workers.first() {
                    if avail_w > agg_w + 54.0 {
                        self.draw_text_line(
                            graphics,
                            self.font_body,
                            agg_text,
                            RectF {
                                x: cur_x,
                                y: text_y,
                                width: agg_w,
                                height: text_h,
                            },
                            agg_color,
                        );
                        cur_x = (cur_x + agg_w + 5.0).round();

                        self.draw_text_line(
                            graphics,
                            self.font_body,
                            "·",
                            RectF {
                                x: cur_x,
                                y: text_y,
                                width: dot_w,
                                height: text_h,
                            },
                            make_argb(165, 165, 175, 190),
                        );
                        cur_x = (cur_x + dot_w + 5.0).round();

                        let focus_name = if focus.display_name.trim().is_empty() {
                            "@worker".to_string()
                        } else if focus.display_name.trim().starts_with('@') {
                            focus.display_name.trim().to_string()
                        } else {
                            format!("@{}", focus.display_name.trim())
                        };
                        let focus_summary = focus.task_summary.trim();
                        let focus_text = if focus_summary.is_empty() {
                            focus_name
                        } else {
                            format!("{} {}", focus_name, focus_summary)
                        };
                        let focus_w = (x + w - cur_x - 16.0).max(16.0).round();
                        self.draw_text_line(
                            graphics,
                            self.font_body,
                            &focus_text,
                            RectF {
                                x: cur_x,
                                y: text_y,
                                width: focus_w,
                                height: text_h,
                            },
                            make_argb(215, 188, 198, 214),
                        );
                    } else {
                        self.draw_text_line(
                            graphics,
                            self.font_body,
                            agg_text,
                            RectF {
                                x: cur_x,
                                y: text_y,
                                width: avail_w,
                                height: text_h,
                            },
                            agg_color,
                        );
                    }
                } else {
                    self.draw_text_line(
                        graphics,
                        self.font_body,
                        agg_text,
                        RectF {
                            x: cur_x,
                            y: text_y,
                            width: avail_w,
                            height: text_h,
                        },
                        agg_color,
                    );
                }
            }

            // 底部贴边 2px 分段聚合微进度条
            self.draw_swarm_ratio_bar(graphics, x, y, w, h, swarm);

            if need_scale {
                GdipRestoreGraphics(graphics, g_state);
            }
        }
    }

    fn format_worker_meta(worker: &SwarmWorkerInfo, now: Instant) -> String {
        let elapsed_secs = match worker.status {
            SwarmWorkerStatus::Done => worker
                .last_update
                .saturating_duration_since(worker.started_at)
                .as_secs(),
            SwarmWorkerStatus::Waiting => 0,
            SwarmWorkerStatus::Working | SwarmWorkerStatus::ActionNeeded => now
                .saturating_duration_since(worker.started_at)
                .max(worker.last_update.saturating_duration_since(worker.started_at))
                .as_secs(),
        };
        let dur = Self::format_swarm_elapsed(elapsed_secs);
        match worker.status {
            SwarmWorkerStatus::Waiting => "待命中".to_string(),
            SwarmWorkerStatus::Done => {
                if worker.step_index > 0 {
                    format!("第 {} 步 · {} · 完成", worker.step_index, dur)
                } else {
                    format!("已完成 · {}", dur)
                }
            }
            SwarmWorkerStatus::ActionNeeded => {
                if worker.step_index > 0 {
                    format!("⚠ 第 {} 步 · {}", worker.step_index, dur)
                } else {
                    format!("⚠ 待确认 · {}", dur)
                }
            }
            SwarmWorkerStatus::Working => {
                if worker.step_index > 0 {
                    format!("第 {} 步 · {}", worker.step_index, dur)
                } else {
                    format!("运行中 · {}", dur)
                }
            }
        }
    }

    /// 悬停展开雷达抽屉（SwarmExpanded, 440.0 × 190.0，纯只读 + 右上角唯一切回窗口微胶囊）
    unsafe fn render_swarm_expanded_view(
        &self,
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        _h: f32,
        swarm: &SwarmSummary,
    ) {
        unsafe {
            let now = Instant::now();
            let top_y = (y + 9.0).round();
            let top_h = 18.0;
            let jump_y = (y + 10.0).round();

            // 1. 左侧 16px 品牌图标
            let brand = AgentBrand::from_name(&swarm.agent_name);
            self.glyph_manager.draw(
                graphics,
                brand,
                (x + 24.0).round(),
                (y + 18.0).round(),
                16.0,
                self.pulse_phase,
            );

            // 2. 右上角唯一 [↗ 切回窗口] 微标签
            let jump_text = "↗ 切回窗口";
            let measured_jump_w = Self::measure_text_w(
                graphics,
                self.font_small,
                jump_text,
                self.format_single_line,
            );
            let jump_w = (measured_jump_w + 12.0).clamp(68.0, 96.0).round();
            let jump_h = 16.0;
            let jump_x = (x + w - jump_w - 16.0).round();
            let (jump_bg, jump_border, jump_fg) = match (self.is_action_hovered, swarm.action_count > 0) {
                (true, true) => (
                    make_argb(125, 255, 159, 20),
                    make_argb(235, 255, 215, 120),
                    make_argb(255, 255, 250, 235),
                ),
                (true, false) => (
                    make_argb(110, 10, 165, 255),
                    make_argb(220, 100, 215, 255),
                    make_argb(255, 245, 252, 255),
                ),
                (false, true) => (
                    make_argb(65, 255, 149, 0),
                    make_argb(145, 255, 175, 50),
                    make_argb(255, 255, 215, 145),
                ),
                (false, false) => (
                    make_argb(42, 10, 165, 255),
                    make_argb(95, 70, 190, 255),
                    make_argb(235, 215, 238, 255),
                ),
            };
            self.draw_pill_tag(
                graphics,
                jump_x,
                jump_y,
                jump_w,
                jump_h,
                4.0,
                jump_text,
                measured_jump_w,
                jump_bg,
                jump_border,
                jump_fg,
            );

            // 3. 顶栏标题 + 简洁计数/总耗时
            let title_x = (x + 38.0).round();
            let avail_left_w = (jump_x - 8.0 - title_x).max(40.0).round();
            let proj_name = if swarm.project_name.is_empty() {
                "WNoobar"
            } else {
                &swarm.project_name
            };
            let title_text = format!("{} · Swarm 雷达", proj_name);

            let total = swarm.total_workers.max(swarm.workers.len());
            let active = swarm.working_count + swarm.action_count;
            let primary_count = if active > 0 { active } else { swarm.done_count };
            let dur = Self::format_swarm_elapsed(swarm.elapsed_secs);
            let mut meta_text = if swarm.action_count > 0 {
                format!("⚠ {}/{} · {}", primary_count, total, dur)
            } else {
                format!("{}/{} · {}", primary_count, total, dur)
            };
            if total > 4 {
                meta_text.push_str(&format!(" · +{} 节点", total - 4));
            }

            let measured_title_w = Self::measure_text_w(
                graphics,
                self.font_title,
                &title_text,
                self.format_single_line,
            );
            let measured_meta_w = Self::measure_text_w(
                graphics,
                self.font_small,
                &meta_text,
                self.format_single_line,
            );
            let (title_w, meta_w) = if measured_title_w + 8.0 + measured_meta_w <= avail_left_w {
                (measured_title_w.round(), measured_meta_w.round())
            } else {
                let capped_meta = measured_meta_w.min((avail_left_w * 0.48).max(48.0)).round();
                let remain_title = (avail_left_w - capped_meta - 8.0).max(36.0).round();
                (remain_title, capped_meta)
            };

            self.draw_text_line(
                graphics,
                self.font_title,
                &title_text,
                RectF {
                    x: title_x,
                    y: top_y,
                    width: title_w,
                    height: top_h,
                },
                make_argb(255, 255, 255, 255),
            );

            let meta_x = (title_x + title_w + 8.0).round();
            if meta_x + 20.0 <= jump_x - 8.0 {
                let meta_color = if swarm.action_count > 0 {
                    make_argb(255, 255, 179, 64)
                } else {
                    make_argb(225, 170, 192, 220)
                };
                self.draw_text_line(
                    graphics,
                    self.font_small,
                    &meta_text,
                    RectF {
                        x: meta_x,
                        y: top_y,
                        width: (jump_x - 8.0 - meta_x).min(meta_w + 2.0).max(16.0),
                        height: top_h,
                    },
                    meta_color,
                );
            }

            // 4. 分段总进度条（y + 30, 高 3px）
            self.draw_swarm_worker_bar(graphics, x, y, w, swarm);

            // 5. 纯只读子代理轨道列表（y + 39 起，最多 4 行，row_h = 30.0, gap = 5.0，严禁放置任何按钮）
            let start_y = (y + 39.0).round();
            let row_h = 30.0;
            let row_gap = 5.0;
            let row_x = (x + 14.0).round();
            let row_w = (w - 28.0).round();

            for (i, worker) in swarm.workers.iter().take(4).enumerate() {
                let item_y = (start_y + (i as f32) * (row_h + row_gap)).round();
                let is_action = worker.status == SwarmWorkerStatus::ActionNeeded;

                let mut row_path: *mut c_void = null_mut();
                GdipCreatePath(0, &mut row_path);
                if !row_path.is_null() {
                    add_round_rect_to_path(row_path, row_x, item_y, row_w, row_h, 8.0);
                    let bg_color = if is_action {
                        make_argb(36, 255, 149, 0)
                    } else {
                        make_argb(22, 255, 255, 255)
                    };
                    let mut bg_brush: *mut c_void = null_mut();
                    GdipCreateSolidFill(bg_color, &mut bg_brush);
                    if !bg_brush.is_null() {
                        GdipFillPath(graphics, bg_brush, row_path);
                        GdipDeleteBrush(bg_brush);
                    }

                    let stroke_color = if is_action {
                        make_argb(145, 255, 159, 20)
                    } else {
                        make_argb(32, 255, 255, 255)
                    };
                    let mut stroke_pen: *mut c_void = null_mut();
                    GdipCreatePen1(stroke_color, 1.0, UNIT_PIXEL, &mut stroke_pen);
                    if !stroke_pen.is_null() {
                        GdipDrawPath(graphics, stroke_pen, row_path);
                        GdipDeletePen(stroke_pen);
                    }
                    GdipDeletePath(row_path);
                }

                // 左侧 6px 彩色状态灯
                let dot_x = (row_x + 10.0).round();
                let dot_y = (item_y + (row_h - 6.0) / 2.0).round();
                let dot_color = self.swarm_status_color(worker.status, true);
                self.draw_dot(graphics, dot_x, dot_y, 6.0, dot_color);

                // 右侧对齐步骤/耗时信息
                let row_meta = Self::format_worker_meta(worker, now);
                let measured_row_meta_w = Self::measure_text_w(
                    graphics,
                    self.font_small,
                    &row_meta,
                    self.format_single_line,
                );
                let row_meta_w = measured_row_meta_w.clamp(28.0, 132.0).round();
                let row_meta_x = (row_x + row_w - 10.0 - row_meta_w).round();
                let text_y = (item_y + (row_h - 18.0) / 2.0).round();
                let row_meta_color = match worker.status {
                    SwarmWorkerStatus::ActionNeeded => make_argb(255, 255, 188, 92),
                    SwarmWorkerStatus::Working => make_argb(225, 145, 208, 245),
                    SwarmWorkerStatus::Done => make_argb(225, 110, 225, 175),
                    SwarmWorkerStatus::Waiting => make_argb(165, 165, 175, 190),
                };
                self.draw_text_line(
                    graphics,
                    self.font_small,
                    &row_meta,
                    RectF {
                        x: row_meta_x,
                        y: text_y,
                        width: row_meta_w,
                        height: 18.0,
                    },
                    row_meta_color,
                );

                // 中间区域：@display_name + 单行 task_summary（右侧严格裁剪至 row_meta_x - 8px）
                let name_x = (dot_x + 6.0 + 8.0).round();
                let max_mid_w = (row_meta_x - 8.0 - name_x).max(24.0).round();
                let raw_name = worker.display_name.trim();
                let name_label = if raw_name.is_empty() {
                    "@worker".to_string()
                } else if raw_name.starts_with('@') {
                    raw_name.to_string()
                } else {
                    format!("@{}", raw_name)
                };
                let measured_name_w = Self::measure_text_w(
                    graphics,
                    self.font_title,
                    &name_label,
                    self.format_single_line,
                );
                let name_w = measured_name_w
                    .clamp(24.0, (max_mid_w * 0.45).max(44.0).min(115.0))
                    .round();
                let name_color = match worker.status {
                    SwarmWorkerStatus::ActionNeeded => make_argb(255, 255, 195, 110),
                    SwarmWorkerStatus::Working => make_argb(255, 245, 248, 255),
                    SwarmWorkerStatus::Done => make_argb(235, 185, 235, 210),
                    SwarmWorkerStatus::Waiting => make_argb(185, 175, 185, 200),
                };
                self.draw_text_line(
                    graphics,
                    self.font_title,
                    &name_label,
                    RectF {
                        x: name_x,
                        y: text_y,
                        width: name_w,
                        height: 18.0,
                    },
                    name_color,
                );

                let task_x = (name_x + name_w + 8.0).round();
                let task_w = (row_meta_x - 8.0 - task_x).round();
                if task_w >= 16.0 {
                    let task_text = if worker.task_summary.trim().is_empty() {
                        match worker.status {
                            SwarmWorkerStatus::Waiting => "等待消息唤醒",
                            SwarmWorkerStatus::Done => "任务已收官",
                            SwarmWorkerStatus::ActionNeeded => "等待确认",
                            SwarmWorkerStatus::Working => "执行中...",
                        }
                    } else {
                        worker.task_summary.trim()
                    };
                    let task_color = match worker.status {
                        SwarmWorkerStatus::ActionNeeded => make_argb(245, 255, 236, 205),
                        SwarmWorkerStatus::Waiting => make_argb(165, 175, 185, 200),
                        _ => make_argb(230, 212, 220, 232),
                    };
                    self.draw_text_line(
                        graphics,
                        self.font_body,
                        task_text,
                        RectF {
                            x: task_x,
                            y: text_y,
                            width: task_w,
                            height: 18.0,
                        },
                        task_color,
                    );
                }
            }
        }
    }
}

impl Drop for CapsuleRenderer {
    fn drop(&mut self) {
        unsafe {
            if !self.font_title.is_null() { GdipDeleteFont(self.font_title); }
            if !self.font_body.is_null() { GdipDeleteFont(self.font_body); }
            if !self.font_role.is_null() { GdipDeleteFont(self.font_role); }
            if !self.font_small.is_null() { GdipDeleteFont(self.font_small); }
            if !self.font_family_ui.is_null() { GdipDeleteFontFamily(self.font_family_ui); }
            if !self.format_single_line.is_null() { GdipDeleteStringFormat(self.format_single_line); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capsule_renderer_default_state_and_render_smoke() {
        let _gdip = GdiplusContext::init().expect("GdiplusContext::init should succeed on Windows");
        let mut renderer = CapsuleRenderer::new();
        assert_eq!(renderer.hovered_deck_idx, None);
        assert!(!renderer.is_action_hovered);
        assert!(!renderer.is_silent_mode);
        assert!(!renderer.is_dnd_active);

        let width = 480i32;
        let height = 210i32;
        let mut buf = vec![0u8; (width * height * 4) as usize];
        let mut bmp: *mut c_void = null_mut();
        let mut gfx: *mut c_void = null_mut();
        unsafe {
            assert_eq!(
                GdipCreateBitmapFromScan0(
                    width,
                    height,
                    width * 4,
                    PIXEL_FORMAT_32BPP_PARGB,
                    buf.as_mut_ptr(),
                    &mut bmp,
                ),
                0
            );
            assert_eq!(GdipGetImageGraphicsContext(bmp, &mut gfx), 0);

            let odo = OdometerCarousel::new();
            let recent = vec![
                ("s1".into(), "WNoobar".into(), "核心研发".into(), "正在构建".into(), "Antigravity".into()),
                ("s2".into(), "QQ-BOT".into(), "Claude".into(), "消息流处理".into(), "Claude".into()),
                ("s3".into(), "VeryLongProjectName".into(), "SuperLongRoleNameForTruncation".into(), "长描述文本测试截断与右侧跳转标签防重叠".into(), "Codex".into()),
            ];

            // 1. Idle (常规 & Silent 极简模式)
            renderer.render(gfx, 480.0, 210.0, 210.0, 32.0, &IslandState::Idle, EdgeDockMode::Normal, &recent, &[], &odo);
            renderer.is_silent_mode = true;
            renderer.render(gfx, 480.0, 210.0, 210.0, 32.0, &IslandState::Idle, EdgeDockMode::Normal, &recent, &[], &odo);

            // 验证 Silent 模式文本宽度在 210px 胶囊内不触发截断
            let title_w = CapsuleRenderer::measure_text_w(gfx, renderer.font_title, "WNoobar", renderer.format_single_line).clamp(20.0, 90.0).round();
            let silent_sub_w = CapsuleRenderer::measure_text_w(gfx, renderer.font_role, "待命 · 极简模式", renderer.format_single_line);
            let idle_used_before_sub = 16.0 + 12.0 + title_w + 3.0 + 6.0 + 3.0;
            let idle_remain_w = 210.0 - idle_used_before_sub - 10.0;
            assert!(
                silent_sub_w <= idle_remain_w,
                "Silent subtitle width ({}) should fit inside remaining width ({})",
                silent_sub_w,
                idle_remain_w
            );

            // 2. ActionNeeded (常态 & 悬停高亮)
            let action_state = IslandState::ActionNeeded {
                session_id: "s1".into(),
                project_name: "WNoobarLongProject".into(),
                role_type: "核心架构师".into(),
                agent_name: "Antigravity".into(),
                prompt: "请确认是否执行数据库迁移？".into(),
            };
            renderer.render(gfx, 480.0, 210.0, 380.0, 60.0, &action_state, EdgeDockMode::Normal, &recent, &[], &odo);
            renderer.is_action_hovered = true;
            renderer.render(gfx, 480.0, 210.0, 380.0, 60.0, &action_state, EdgeDockMode::Normal, &recent, &[], &odo);

            // 3. Session Deck (无悬停 & 第 2 项悬停高亮)
            renderer.render(gfx, 480.0, 210.0, 440.0, 190.0, &IslandState::Idle, EdgeDockMode::DeckExpanded, &recent, &[], &odo);
            renderer.hovered_deck_idx = Some(1);
            renderer.render(gfx, 480.0, 210.0, 440.0, 190.0, &IslandState::Idle, EdgeDockMode::DeckExpanded, &recent, &[], &odo);

            assert!(buf.iter().any(|&b| b != 0), "GDI+ render should produce non-empty pixels");

            GdipDeleteGraphics(gfx);
            GdipDisposeImage(bmp);
        }
    }

    #[test]
    fn test_is_left_glyph_hit_boundaries() {
        // Idle 胶囊 (cap_x=135, cap_y=4, cap_h=32)
        assert!(CapsuleRenderer::is_left_glyph_hit(135.0, 4.0, 32.0, 135.0, 4.0));
        assert!(CapsuleRenderer::is_left_glyph_hit(135.0, 4.0, 32.0, 173.0, 36.0));
        assert!(CapsuleRenderer::is_left_glyph_hit(135.0, 4.0, 32.0, 153.0, 20.0));
        assert!(!CapsuleRenderer::is_left_glyph_hit(135.0, 4.0, 32.0, 134.9, 20.0));
        assert!(!CapsuleRenderer::is_left_glyph_hit(135.0, 4.0, 32.0, 173.1, 20.0));
        assert!(!CapsuleRenderer::is_left_glyph_hit(135.0, 4.0, 32.0, 153.0, 3.9));
        assert!(!CapsuleRenderer::is_left_glyph_hit(135.0, 4.0, 32.0, 153.0, 36.1));

        // Working 胶囊 (cap_x=20, cap_y=4, cap_h=38)
        assert!(CapsuleRenderer::is_left_glyph_hit(20.0, 4.0, 38.0, 58.0, 42.0));
        assert!(!CapsuleRenderer::is_left_glyph_hit(20.0, 4.0, 38.0, 58.1, 42.0));
        assert!(!CapsuleRenderer::is_left_glyph_hit(20.0, 4.0, 38.0, 35.0, 42.1));

        // DeckExpanded / ActionNeeded 高度 (> 38px) 垂直热区严格限制在顶部 38px 内，避免与 Deck 首张卡片重叠
        assert!(CapsuleRenderer::is_left_glyph_hit(20.0, 4.0, 190.0, 35.0, 42.0));
        assert!(!CapsuleRenderer::is_left_glyph_hit(20.0, 4.0, 190.0, 35.0, 42.1));
        assert!(!CapsuleRenderer::is_left_glyph_hit(20.0, 4.0, 190.0, 35.0, 44.0));

        // Peeking 微光条状态不可点击左微标
        assert!(!CapsuleRenderer::is_left_glyph_hit(170.0, 0.0, 4.0, 180.0, 2.0));
    }

    #[test]
    fn test_deck_card_index_at_boundaries() {
        let cap_x = 20.0;
        let cap_y = 4.0;
        let cap_w = 440.0;

        // X 边界：card_left = 34.0, card_right = 446.0
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 33.9, 60.0, 3), None);
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 446.1, 60.0, 3), None);

        // 标题区域（Card 0 上方）不可命中
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 200.0, 43.9, 3), None);

        // Card 0: [44.0, 86.0]
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 34.0, 44.0, 3), Some(0));
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 446.0, 86.0, 3), Some(0));

        // Gap 0-1: (86.0, 92.0)
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 200.0, 86.1, 3), None);
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 200.0, 89.0, 3), None);
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 200.0, 91.9, 3), None);

        // Card 1: [92.0, 134.0]
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 34.0, 92.0, 3), Some(1));
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 446.0, 134.0, 3), Some(1));

        // Gap 1-2: (134.0, 140.0)
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 200.0, 137.0, 3), None);

        // Card 2: [140.0, 182.0]
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 34.0, 140.0, 3), Some(2));
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 446.0, 182.0, 3), Some(2));

        // Card 2 下方空白
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 200.0, 182.1, 3), None);

        // card_count 边界与最多 3 张限制
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 200.0, 60.0, 0), None);
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 200.0, 110.0, 1), None);
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 200.0, 150.0, 2), None);
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 200.0, 150.0, 5), Some(2));
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, cap_w, 200.0, 200.0, 5), None);

        // 过窄胶囊防护
        assert_eq!(CapsuleRenderer::deck_card_index_at(cap_x, cap_y, 28.0, 34.0, 60.0, 3), None);
    }

    #[test]
    fn test_is_action_jump_hit_boundaries() {
        let cap_x = 50.0;
        let cap_y = 4.0;
        let cap_w = 380.0;
        let cap_h = 60.0;

        // 左侧品牌微标区域 [50.0, 88.0] x [4.0, 42.0] 应排除在跳转热区之外
        assert!(!CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, 50.0, 4.0));
        assert!(!CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, 70.0, 20.0));
        assert!(!CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, 88.0, 42.0));

        // 未命中左侧微标且在 ActionNeeded 卡片内 -> 有效跳转热区
        assert!(CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, 88.1, 20.0));
        assert!(CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, 70.0, 42.1));
        assert!(CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, 390.0, 20.0));
        assert!(CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, 430.0, 64.0));

        // 卡片外部不可命中
        assert!(!CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, 49.9, 50.0));
        assert!(!CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, 430.1, 30.0));
        assert!(!CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, 200.0, 3.9));
        assert!(!CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, cap_w, cap_h, 200.0, 64.1));

        // Peeking / 无效尺寸防护
        assert!(!CapsuleRenderer::is_action_jump_hit(cap_x, 0.0, cap_w, 4.0, 200.0, 2.0));
        assert!(!CapsuleRenderer::is_action_jump_hit(cap_x, cap_y, 0.0, cap_h, 50.0, 20.0));
    }

    fn make_test_swarm(group_key: &str, parent_sid: Option<&str>, project: &str, agent: &str, statuses: &[SwarmWorkerStatus]) -> SwarmSummary {
        let now = Instant::now();
        let mut workers: Vec<SwarmWorkerInfo> = statuses
            .iter()
            .enumerate()
            .map(|(i, &st)| SwarmWorkerInfo {
                worker_id: format!("w-{}", i),
                display_name: format!("@worker_{}", i),
                role: format!("worker_{}", i),
                status: st,
                task_summary: match st {
                    SwarmWorkerStatus::ActionNeeded => format!("需要确认数据库写入步骤 #{}", i),
                    SwarmWorkerStatus::Working => format!("正在构建模块 {} 并验证核心单元测试", i),
                    SwarmWorkerStatus::Done => format!("任务 {} 已完成", i),
                    SwarmWorkerStatus::Waiting => "待命监控中".to_string(),
                },
                step_index: (i as u64 + 1) * 7,
                started_at: now - std::time::Duration::from_secs(38),
                last_update: now,
            })
            .collect();
        workers.sort_by_key(|w| w.status.priority_rank());

        let action_count = workers.iter().filter(|w| w.status == SwarmWorkerStatus::ActionNeeded).count();
        let working_count = workers.iter().filter(|w| w.status == SwarmWorkerStatus::Working).count();
        let done_count = workers.iter().filter(|w| w.status == SwarmWorkerStatus::Done).count();
        let waiting_count = workers.iter().filter(|w| w.status == SwarmWorkerStatus::Waiting).count();
        let aggregate_summary = if action_count > 0 {
            format!("⚠ {} 待确认 · {} 运行中", action_count, working_count)
        } else if working_count > 0 {
            format!("{}/{} 运行中", working_count, workers.len())
        } else {
            format!("{}/{} 已完成", done_count, workers.len())
        };

        SwarmSummary {
            group_key: group_key.to_string(),
            parent_session_id: parent_sid.map(|s| s.to_string()),
            project_name: project.to_string(),
            agent_name: agent.to_string(),
            total_workers: workers.len(),
            working_count,
            action_count,
            done_count,
            waiting_count,
            workers,
            aggregate_summary,
            elapsed_secs: 102,
        }
    }

    fn make_test_active_item(session_id: &str, project: &str, agent: &str, swarm: Option<SwarmSummary>) -> ActiveSessionItem {
        let now = Instant::now();
        ActiveSessionItem {
            session_id: session_id.to_string(),
            project_name: project.to_string(),
            role_type: "AI 结对助手".to_string(),
            task_summary: "并发处理任务".to_string(),
            agent_name: agent.to_string(),
            step_index: 12,
            start_time: now,
            last_update_time: now,
            swarm,
        }
    }

    #[test]
    fn test_resolve_focused_swarm_priority_and_fallback() {
        let swarm_a = make_test_swarm("grp-a", Some("parent-a"), "ProjA", "Antigravity", &[SwarmWorkerStatus::Working, SwarmWorkerStatus::Done]);
        let swarm_b = make_test_swarm("grp-b", None, "ProjB", "Claude", &[SwarmWorkerStatus::ActionNeeded, SwarmWorkerStatus::Working]);
        let sessions = vec![
            make_test_active_item("parent-a", "ProjA", "Antigravity", Some(swarm_a)),
            make_test_active_item("plain-c", "ProjC", "Codex", None),
            make_test_active_item("grp-b", "ProjB", "Claude", Some(swarm_b)),
        ];
        let mut odo = OdometerCarousel::new();
        odo.active_index = 2;

        // 1. session_id 优先匹配（即使 odometer 当前指向 index 2）
        let state_sid = IslandState::Working {
            session_id: "parent-a".into(),
            project_name: "ProjB".into(),
            role_type: "Swarm".into(),
            agent_name: "Claude".into(),
            task_summary: "...".into(),
            step_index: 1,
            start_time: Instant::now(),
        };
        let matched = resolve_focused_swarm(&state_sid, &sessions, &odo, EdgeDockMode::Normal).expect("rotation active: odometer focus wins over session_id");
        assert_eq!(matched.group_key, "grp-b");
        let matched_exp = resolve_focused_swarm(&state_sid, &sessions, &odo, EdgeDockMode::SwarmExpanded).expect("expanded keeps odometer focus");
        assert_eq!(matched_exp.group_key, "grp-b");
        // 轮播当前页无 Swarm：Normal 不命中，SwarmExpanded 回退 session_id 匹配
        let mut odo_plain = OdometerCarousel::new();
        odo_plain.active_index = 1;
        assert!(resolve_focused_swarm(&state_sid, &sessions, &odo_plain, EdgeDockMode::Normal).is_none());
        let fb = resolve_focused_swarm(&state_sid, &sessions, &odo_plain, EdgeDockMode::SwarmExpanded).expect("fallback by session_id");
        assert_eq!(fb.group_key, "grp-a");

        // 2. 其次按 (project_name, agent_name) 匹配
        let state_proj = IslandState::Started {
            session_id: "unknown-sid".into(),
            project_name: "ProjB".into(),
            role_type: "Swarm".into(),
            agent_name: "Claude".into(),
            task_summary: "...".into(),
            started_at: Instant::now(),
            auto_hide_secs: 1.8,
        };
        let matched_proj = resolve_focused_swarm(&state_proj, &sessions, &odo, EdgeDockMode::Normal).expect("rotation active: odometer focus");
        assert_eq!(matched_proj.group_key, "grp-b");

        // 3. 无匹配单会话不应误命中无关 Swarm；多会话轮播可跟随当前 odometer 焦点
        let single_unrelated = vec![sessions[0].clone()];
        let state_other = IslandState::Working {
            session_id: "other-sid".into(),
            project_name: "OtherProj".into(),
            role_type: "Dev".into(),
            agent_name: "Codex".into(),
            task_summary: "...".into(),
            step_index: 1,
            start_time: Instant::now(),
        };
        assert!(resolve_focused_swarm(&state_other, &single_unrelated, &odo, EdgeDockMode::Normal).is_none());
        let matched_odo = resolve_focused_swarm(&state_other, &sessions, &odo, EdgeDockMode::Normal).expect("multi-session falls back to odometer focus");
        assert_eq!(matched_odo.group_key, "grp-b");

        // 4. Idle 状态优先当前轮播焦点或首个含 Swarm 条目
        odo.active_index = 1; // plain-c has swarm: None
        assert!(resolve_focused_swarm(&IslandState::Idle, &sessions, &odo, EdgeDockMode::Normal).is_none());
        let matched_idle = resolve_focused_swarm(&IslandState::Idle, &sessions, &odo, EdgeDockMode::SwarmExpanded).expect("expanded idle falls back to first swarm");
        assert_eq!(matched_idle.group_key, "grp-a");
    }

    #[test]
    fn test_capsule_renderer_swarm_normal_and_expanded_smoke() {
        let _gdip = GdiplusContext::init().expect("GdiplusContext::init should succeed on Windows");
        let mut renderer = CapsuleRenderer::new();

        let width = 480i32;
        let height = 210i32;
        let mut buf = vec![0u8; (width * height * 4) as usize];
        let mut bmp: *mut c_void = null_mut();
        let mut gfx: *mut c_void = null_mut();

        unsafe {
            assert_eq!(
                GdipCreateBitmapFromScan0(
                    width,
                    height,
                    width * 4,
                    PIXEL_FORMAT_32BPP_PARGB,
                    buf.as_mut_ptr(),
                    &mut bmp,
                ),
                0
            );
            assert_eq!(GdipGetImageGraphicsContext(bmp, &mut gfx), 0);

            let mut odo = OdometerCarousel::new();
            let recent = vec![
                ("s1".into(), "WNoobar".into(), "Swarm".into(), "聚合雷达".into(), "Antigravity".into()),
            ];

            // Case 1: Normal Swarm (2 workers)
            let swarm_2 = make_test_swarm(
                "parent-2",
                Some("parent-2"),
                "WNoobar",
                "Antigravity",
                &[SwarmWorkerStatus::Working, SwarmWorkerStatus::Done],
            );
            let sessions_2 = vec![make_test_active_item("parent-2", "WNoobar", "Antigravity", Some(swarm_2))];
            let working_2 = IslandState::Working {
                session_id: "parent-2".into(),
                project_name: "WNoobar".into(),
                role_type: "Swarm".into(),
                agent_name: "Antigravity".into(),
                task_summary: "running".into(),
                step_index: 5,
                start_time: Instant::now(),
            };
            renderer.render(gfx, 480.0, 210.0, 400.0, 38.0, &working_2, EdgeDockMode::Normal, &recent, &sessions_2, &odo);
            assert!(buf.iter().any(|&b| b != 0), "2-worker Normal Swarm should render non-empty pixels");

            // Case 2: Normal Swarm (6 workers -> >4 dots +2 badge) + Started + Odometer rolling
            let swarm_6 = make_test_swarm(
                "parent-6",
                Some("parent-6"),
                "LongProjectAlpha",
                "Claude",
                &[
                    SwarmWorkerStatus::Working,
                    SwarmWorkerStatus::Working,
                    SwarmWorkerStatus::Done,
                    SwarmWorkerStatus::Done,
                    SwarmWorkerStatus::Waiting,
                    SwarmWorkerStatus::Waiting,
                ],
            );
            let sessions_multi = vec![
                make_test_active_item("parent-6", "LongProjectAlpha", "Claude", Some(swarm_6.clone())),
                make_test_active_item("solo-1", "SideProject", "Codex", None),
            ];
            let started_6 = IslandState::Started {
                session_id: "parent-6".into(),
                project_name: "LongProjectAlpha".into(),
                role_type: "Swarm".into(),
                agent_name: "Claude".into(),
                task_summary: "started".into(),
                started_at: Instant::now(),
                auto_hide_secs: 1.8,
            };
            renderer.render(gfx, 480.0, 210.0, 440.0, 38.0, &started_6, EdgeDockMode::Normal, &recent, &sessions_multi[..1], &odo);
            odo.is_rolling = true;
            odo.roll_progress = 0.45;
            odo.prev_index = 1;
            odo.active_index = 0;
            renderer.render(gfx, 480.0, 210.0, 440.0, 38.0, &working_2, EdgeDockMode::Normal, &recent, &sessions_multi, &odo);
            odo.is_rolling = false;

            // Case 3: Normal Swarm with ActionNeeded (orange glow + amber badge)
            let swarm_action = make_test_swarm(
                "parent-act",
                Some("parent-act"),
                "WNoobar",
                "Antigravity",
                &[
                    SwarmWorkerStatus::ActionNeeded,
                    SwarmWorkerStatus::Working,
                    SwarmWorkerStatus::Done,
                    SwarmWorkerStatus::Waiting,
                    SwarmWorkerStatus::Working,
                    SwarmWorkerStatus::Done,
                ],
            );
            let sessions_act = vec![make_test_active_item("parent-act", "WNoobar", "Antigravity", Some(swarm_action))];
            let working_act = IslandState::Working {
                session_id: "parent-act".into(),
                project_name: "WNoobar".into(),
                role_type: "Swarm".into(),
                agent_name: "Antigravity".into(),
                task_summary: "action".into(),
                step_index: 9,
                start_time: Instant::now(),
            };
            buf.fill(0);
            renderer.render(gfx, 480.0, 210.0, 440.0, 38.0, &working_act, EdgeDockMode::Normal, &recent, &sessions_act, &odo);
            // 32bpp PARGB (B, G, R, A) 应包含琥珀橙色像素 (R > 160, G in 60..200, B < 60)
            let has_amber_pixel = buf.chunks_exact(4).any(|px| px[3] > 20 && px[2] > 160 && px[1] > 60 && px[0] < 60);
            assert!(has_amber_pixel, "ActionNeeded Normal Swarm should emit amber glow/dot pixels");

            // Case 4: EdgeDockMode::SwarmExpanded (440 x 190) normal & hovered jump pill
            buf.fill(0);
            renderer.is_action_hovered = false;
            renderer.render(gfx, 480.0, 210.0, 440.0, 190.0, &working_act, EdgeDockMode::SwarmExpanded, &recent, &sessions_act, &odo);
            assert!(buf.iter().any(|&b| b != 0), "SwarmExpanded should render non-empty pixels");
            renderer.is_action_hovered = true;
            renderer.render(gfx, 480.0, 210.0, 440.0, 190.0, &working_act, EdgeDockMode::SwarmExpanded, &recent, &sessions_act, &odo);

            // Case 5: EdgeDockMode::SwarmExpanded graceful fallback to render_deck_view when no SwarmSummary
            buf.fill(0);
            renderer.render(gfx, 480.0, 210.0, 440.0, 190.0, &IslandState::Idle, EdgeDockMode::SwarmExpanded, &recent, &[], &odo);
            assert!(buf.iter().any(|&b| b != 0), "SwarmExpanded fallback to DeckView should still render cleanly");

            GdipDeleteGraphics(gfx);
            GdipDisposeImage(bmp);
        }
    }
}
