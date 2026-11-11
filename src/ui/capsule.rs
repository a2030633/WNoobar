#![allow(dead_code)]
use std::ffi::c_void;
use std::ptr::null_mut;

use crate::core::fsm::{EdgeDockMode, FlipTransition, IslandState};
use crate::ui::glyphs::{AgentBrand, GlyphManager};
use crate::win32::gdiplus::*;

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
            let radius = if dock_mode == EdgeDockMode::DeckExpanded {
                22.0
            } else {
                (cap_h / 2.0).clamp(2.0, 20.0)
            };

            // 4. 外部环境柔光 (Glow) - 展开时平滑增强
            if morph > 0.1 {
                let glow_base = match state {
                    IslandState::Idle => (20, 255, 255, 255),
                    IslandState::Started { .. } => (60, 0, 160, 255),
                    IslandState::Working { .. } => {
                        let a = (50.0 + 25.0 * self.pulse_phase.sin()) as u8;
                        (a, 0, 160, 255)
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

                let body_color = if dock_mode == EdgeDockMode::DeckExpanded {
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
                } else {
                    match state {
                        IslandState::Idle => {
                            self.render_idle_state(graphics, cap_x, cap_y, cap_w, cap_h);
                        }
                        IslandState::Started { project_name, role_type, agent_name, task_summary, .. } => {
                            self.render_working_state(graphics, cap_x, cap_y, cap_w, cap_h, project_name, role_type, agent_name, task_summary, 0);
                        }
                        IslandState::Working { project_name, role_type, agent_name, task_summary, step_index, .. } => {
                            self.render_working_state(graphics, cap_x, cap_y, cap_w, cap_h, project_name, role_type, agent_name, task_summary, *step_index);
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
            let glyph_x = (x + 18.0).round();
            let glyph_y = (y + h / 2.0).round();
            self.glyph_manager.draw(graphics, AgentBrand::WNoobar, glyph_x, glyph_y, 16.0, self.pulse_phase);

            // 统一垂直居中整数像素基准线
            let text_y = (y + (h - 18.0) / 2.0).round();
            let text_h = 18.0;

            // 1. 标题 "WNoobar"（精确测宽，绝不硬编码假定宽度）
            let title = "WNoobar";
            let title_utf16: Vec<u16> = format!("{}\0", title).encode_utf16().collect();
            let mut cur_x = (glyph_x + 14.0).round();
            let measured_title_w = Self::measure_text_w(graphics, self.font_title, title, self.format_single_line);
            let title_w = measured_title_w.clamp(20.0, 90.0).round();
            let title_rect = RectF { x: cur_x, y: text_y, width: title_w, height: text_h };
            let mut white_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(255, 255, 255, 255), &mut white_brush);
            GdipDrawString(graphics, title_utf16.as_ptr(), title_utf16.len() as i32 - 1, self.font_title, &title_rect, self.format_single_line, white_brush);
            GdipDeleteBrush(white_brush);
            cur_x = (cur_x + title_w + 6.0).round();

            // 2. 分隔圆点 "·"（独立排版，左右严格对称 6px，绝对垂直共线）
            let dot_str = "·\0";
            let dot_utf16: Vec<u16> = dot_str.encode_utf16().collect();
            let dot_w = 8.0;
            let dot_rect = RectF { x: cur_x, y: text_y, width: dot_w, height: text_h };
            let mut dot_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(180, 165, 170, 180), &mut dot_brush);
            GdipDrawString(graphics, dot_utf16.as_ptr(), dot_utf16.len() as i32 - 1, self.font_body, &dot_rect, self.format_single_line, dot_brush);
            GdipDeleteBrush(dot_brush);
            cur_x = (cur_x + dot_w + 6.0).round();

            // 3. 状态 "待命就绪"（纯净中文，与 WNoobar 共享统一 12px 黄金字阶与基线，彻底消灭高低错位偏差）
            let sub_str = "待命就绪\0";
            let sub_utf16: Vec<u16> = sub_str.encode_utf16().collect();
            let remain_w = (x + w - cur_x - 14.0).max(10.0).round();
            let sub_rect = RectF { x: cur_x, y: text_y, width: remain_w, height: text_h };
            let mut gray_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(235, 195, 200, 210), &mut gray_brush); // 高对比度清晰银灰
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
            let proj_w = measured_proj_w.clamp(20.0, 95.0).round();
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

            // 2. 信息层级第二段：[角色/会话名]（天蓝色，测宽自适应，绝不折行）
            let role_clean = role.trim();
            let role_utf16: Vec<u16> = format!("{}\0", role_clean).encode_utf16().collect();
            let measured_role_w = Self::measure_text_w(graphics, self.font_role, role_clean, self.format_single_line);
            let role_w = measured_role_w.clamp(24.0, 130.0).round();
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
            let action_title = "⚡ 需要确认交互\0";
            let action_utf16: Vec<u16> = action_title.encode_utf16().collect();
            let action_rect = RectF {
                x: (x + 40.0).round(),
                y: row1_y,
                width: 110.0,
                height: 18.0,
            };
            let mut action_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(255, 255, 149, 0), &mut action_brush);
            GdipDrawString(graphics, action_utf16.as_ptr(), action_utf16.len() as i32 - 1, self.font_title, &action_rect, self.format_single_line, action_brush);
            GdipDeleteBrush(action_brush);

            // 3. 项目与会话徽章：[项目名]
            let proj_name = if project.is_empty() { "WNoobar" } else { project };
            let badge_text = if !role.is_empty() && role != "AI 结对助手" {
                format!("{} · {}\0", proj_name, role)
            } else {
                format!("{}\0", proj_name)
            };
            let badge_utf16: Vec<u16> = badge_text.encode_utf16().collect();
            let badge_x = (x + 154.0).round();
            let badge_w = (badge_text.chars().count() as f32 * 7.5 + 14.0).clamp(24.0, 160.0).round();
            let badge_h = 16.0;

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
                width: badge_w - 6.0,
                height: badge_h,
            };
            let mut proj_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(make_argb(255, 255, 205, 130), &mut proj_brush);
            GdipDrawString(graphics, badge_utf16.as_ptr(), badge_utf16.len() as i32 - 1, self.font_small, &proj_rect, self.format_single_line, proj_brush);
            GdipDeleteBrush(proj_brush);

            // 4. 第二行：具体的提问内容/待确认提示
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

            for (i, (_, project, role, desc, agent)) in recent_sessions.iter().take(3).enumerate() {
                let item_y = (start_y + (i as f32) * (item_h + 6.0)).round();

                // 卡片底板
                let mut path: *mut c_void = null_mut();
                GdipCreatePath(0, &mut path);
                add_round_rect_to_path(path, (x + 14.0).round(), item_y, (w - 28.0).round(), item_h, 10.0);

                let bg_color = if i == 0 {
                    make_argb(55, 10, 132, 255) // 当前聚焦高亮
                } else {
                    make_argb(25, 255, 255, 255)
                };
                let mut brush: *mut c_void = null_mut();
                GdipCreateSolidFill(bg_color, &mut brush);
                GdipFillPath(graphics, brush, path);
                GdipDeleteBrush(brush);

                let mut stroke_pen: *mut c_void = null_mut();
                GdipCreatePen1(make_argb(30, 255, 255, 255), 1.0, UNIT_PIXEL, &mut stroke_pen);
                GdipDrawPath(graphics, stroke_pen, path);
                GdipDeletePen(stroke_pen);
                GdipDeletePath(path);

                // 绘制该项目的 Agent 专属微标（支持 Antigravity / Claude / Codex / DeepSeek 等）
                let brand = AgentBrand::from_name(agent);
                self.glyph_manager.draw(graphics, brand, (x + 30.0).round(), (item_y + item_h / 2.0).round(), 16.0, self.pulse_phase);

                // 项目与角色
                let row_title = format!("{}  ·  {}\0", project, role);
                let row_utf16: Vec<u16> = row_title.encode_utf16().collect();
                let row_rect = RectF { x: (x + 44.0).round(), y: item_y + 6.0, width: (w - 80.0).round(), height: 16.0 };
                let mut row_brush: *mut c_void = null_mut();
                GdipCreateSolidFill(make_argb(255, 255, 255, 255), &mut row_brush);
                GdipDrawString(graphics, row_utf16.as_ptr(), row_utf16.len() as i32 - 1, self.font_title, &row_rect, self.format_single_line, row_brush);
                GdipDeleteBrush(row_brush);

                // 描述
                let d_utf16: Vec<u16> = format!("{}\0", desc).encode_utf16().collect();
                let d_rect = RectF { x: (x + 44.0).round(), y: item_y + 24.0, width: (w - 90.0).round(), height: 16.0 };
                let mut d_brush: *mut c_void = null_mut();
                GdipCreateSolidFill(make_argb(230, 205, 210, 220), &mut d_brush);
                GdipDrawString(graphics, d_utf16.as_ptr(), d_utf16.len() as i32 - 1, self.font_body, &d_rect, self.format_single_line, d_brush);
                GdipDeleteBrush(d_brush);
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
