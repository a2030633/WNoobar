#![allow(dead_code)]
use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, DrawTextW, GdiFlush, SelectObject, SetBkMode, SetTextColor,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CALCRECT,
    DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, FF_DONTCARE,
    FW_NORMAL, HDC, HFONT, OUT_DEFAULT_PRECIS, TRANSPARENT,
};

use crate::core::bus::NoiseFilterLevel;
use crate::core::config::MonitorMode;
use crate::win32::gdiplus::*;

pub const SHADOW_PAD: f32 = 10.0;
pub const MAIN_CARD_W: f32 = 244.0;
pub const SUBMENU_CARD_W: f32 = 198.0;
pub const CARD_PAD_Y: f32 = 6.0;
pub const ROW_H: f32 = 32.0;
pub const SEP_H: f32 = 9.0;

const LINE_CAP_ROUND: i32 = 2;
const LINE_JOIN_ROUND: i32 = 2;

#[link(name = "gdiplus")]
unsafe extern "system" {
    fn GdipStartPathFigure(path: *mut c_void) -> i32;
    fn GdipAddPathEllipse(path: *mut c_void, x: f32, y: f32, width: f32, height: f32) -> i32;
    fn GdipSetPenStartCap(pen: *mut c_void, startCap: i32) -> i32;
    fn GdipSetPenEndCap(pen: *mut c_void, endCap: i32) -> i32;
    fn GdipSetPenLineJoin(pen: *mut c_void, lineJoin: i32) -> i32;
    fn GdipFlush(graphics: *mut c_void, intention: i32) -> i32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmenuKind {
    Noise,
    Monitor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuIcon {
    ToggleIsland,
    SessionDeck,
    NoiseDot(NoiseFilterLevel),
    Monitor,
    MonitorCursor,
    Dnd,
    Sound,
    Autostart,
    Reset,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCommand {
    ToggleIsland,
    OpenDeck,
    SetNoise(NoiseFilterLevel),
    SetMonitor(MonitorMode),
    ToggleDnd,
    ToggleSound,
    ToggleAutostart,
    ResetIdle,
    Exit,
}

#[derive(Debug, Clone)]
pub struct TrayMenuSnapshot {
    pub is_expanded: bool,
    pub active_sessions_count: usize,
    pub noise_level: NoiseFilterLevel,
    pub monitor_mode: MonitorMode,
    pub dnd_fullscreen: bool,
    pub sound_enabled: bool,
    pub auto_start: bool,
}

#[derive(Debug, Clone)]
pub enum MenuRowKind {
    Separator,
    Item {
        label: String,
        right_hint: Option<String>,
        icon: MenuIcon,
        command: Option<MenuCommand>,
        submenu: Option<SubmenuKind>,
        checked: bool,
        switch_on: Option<bool>,
        destructive: bool,
    },
}

#[derive(Debug, Clone)]
pub struct MenuRow {
    pub kind: MenuRowKind,
    pub height: f32,
}

fn separator_row() -> MenuRow {
    MenuRow {
        kind: MenuRowKind::Separator,
        height: SEP_H,
    }
}

fn item_row(
    label: impl Into<String>,
    right_hint: Option<String>,
    icon: MenuIcon,
    command: Option<MenuCommand>,
    submenu: Option<SubmenuKind>,
    checked: bool,
    switch_on: Option<bool>,
    destructive: bool,
) -> MenuRow {
    MenuRow {
        kind: MenuRowKind::Item {
            label: label.into(),
            right_hint,
            icon,
            command,
            submenu,
            checked,
            switch_on,
            destructive,
        },
        height: ROW_H,
    }
}

pub fn build_main_rows(snap: &TrayMenuSnapshot) -> Vec<MenuRow> {
    let (toggle_label, toggle_hint) = if snap.is_expanded {
        ("收起灵动岛", "贴边")
    } else {
        ("展开灵动岛", "居顶")
    };

    let deck_hint = if snap.active_sessions_count > 0 {
        format!("{} 活跃", snap.active_sessions_count)
    } else {
        "Deck".to_string()
    };

    let noise_hint = match snap.noise_level {
        NoiseFilterLevel::Silent => "极简",
        NoiseFilterLevel::Normal => "标准",
        NoiseFilterLevel::Verbose => "详细",
    };

    let monitor_hint = match snap.monitor_mode {
        MonitorMode::Primary => "主屏",
        MonitorMode::FollowCursor => "跟随鼠标",
    };

    vec![
        item_row(
            toggle_label,
            Some(toggle_hint.to_string()),
            MenuIcon::ToggleIsland,
            Some(MenuCommand::ToggleIsland),
            None,
            false,
            None,
            false,
        ),
        item_row(
            "活跃会话抽屉",
            Some(deck_hint),
            MenuIcon::SessionDeck,
            Some(MenuCommand::OpenDeck),
            None,
            false,
            None,
            false,
        ),
        separator_row(),
        item_row(
            "通知降噪",
            Some(noise_hint.to_string()),
            MenuIcon::NoiseDot(snap.noise_level),
            None,
            Some(SubmenuKind::Noise),
            false,
            None,
            false,
        ),
        item_row(
            "屏幕位置",
            Some(monitor_hint.to_string()),
            MenuIcon::Monitor,
            None,
            Some(SubmenuKind::Monitor),
            false,
            None,
            false,
        ),
        separator_row(),
        item_row(
            "全屏 / 投屏免打扰",
            None,
            MenuIcon::Dnd,
            Some(MenuCommand::ToggleDnd),
            None,
            false,
            Some(snap.dnd_fullscreen),
            false,
        ),
        item_row(
            "轻量状态提示音",
            None,
            MenuIcon::Sound,
            Some(MenuCommand::ToggleSound),
            None,
            false,
            Some(snap.sound_enabled),
            false,
        ),
        item_row(
            "开机自动启动",
            None,
            MenuIcon::Autostart,
            Some(MenuCommand::ToggleAutostart),
            None,
            false,
            Some(snap.auto_start),
            false,
        ),
        separator_row(),
        item_row(
            "还原待命就绪状态",
            None,
            MenuIcon::Reset,
            Some(MenuCommand::ResetIdle),
            None,
            false,
            None,
            false,
        ),
        item_row(
            "退出 WNoobar",
            None,
            MenuIcon::Exit,
            Some(MenuCommand::Exit),
            None,
            false,
            None,
            true,
        ),
    ]
}

pub fn build_submenu_rows(kind: SubmenuKind, snap: &TrayMenuSnapshot) -> Vec<MenuRow> {
    match kind {
        SubmenuKind::Noise => vec![
            item_row(
                "极简模式",
                Some("Silent".to_string()),
                MenuIcon::NoiseDot(NoiseFilterLevel::Silent),
                Some(MenuCommand::SetNoise(NoiseFilterLevel::Silent)),
                None,
                snap.noise_level == NoiseFilterLevel::Silent,
                None,
                false,
            ),
            item_row(
                "标准模式",
                Some("Normal".to_string()),
                MenuIcon::NoiseDot(NoiseFilterLevel::Normal),
                Some(MenuCommand::SetNoise(NoiseFilterLevel::Normal)),
                None,
                snap.noise_level == NoiseFilterLevel::Normal,
                None,
                false,
            ),
            item_row(
                "详细模式",
                Some("Verbose".to_string()),
                MenuIcon::NoiseDot(NoiseFilterLevel::Verbose),
                Some(MenuCommand::SetNoise(NoiseFilterLevel::Verbose)),
                None,
                snap.noise_level == NoiseFilterLevel::Verbose,
                None,
                false,
            ),
        ],
        SubmenuKind::Monitor => vec![
            item_row(
                "固定主显示器",
                None,
                MenuIcon::Monitor,
                Some(MenuCommand::SetMonitor(MonitorMode::Primary)),
                None,
                snap.monitor_mode == MonitorMode::Primary,
                None,
                false,
            ),
            item_row(
                "跟随鼠标所在屏幕",
                None,
                MenuIcon::MonitorCursor,
                Some(MenuCommand::SetMonitor(MonitorMode::FollowCursor)),
                None,
                snap.monitor_mode == MonitorMode::FollowCursor,
                None,
                false,
            ),
        ],
    }
}

pub fn measure_panel_size(rows: &[MenuRow], card_w: f32) -> (f32, f32) {
    let rows_h: f32 = rows.iter().map(|r| r.height).sum();
    let canvas_w = card_w + SHADOW_PAD * 2.0;
    let canvas_h = CARD_PAD_Y * 2.0 + rows_h + SHADOW_PAD * 2.0;
    (canvas_w, canvas_h)
}

pub fn row_top_y(rows: &[MenuRow], index: usize) -> f32 {
    let prefix_h: f32 = rows.iter().take(index).map(|r| r.height).sum();
    SHADOW_PAD + CARD_PAD_Y + prefix_h
}

pub fn hit_test_row(rows: &[MenuRow], card_w: f32, log_x: f32, log_y: f32) -> Option<usize> {
    let min_x = SHADOW_PAD + 4.0;
    let max_x = SHADOW_PAD + card_w - 4.0;
    if max_x < min_x || log_x < min_x || log_x > max_x {
        return None;
    }

    let mut y = SHADOW_PAD + CARD_PAD_Y;
    if log_y < y {
        return None;
    }

    for (idx, row) in rows.iter().enumerate() {
        let next_y = y + row.height;
        if log_y >= y && log_y < next_y {
            return match row.kind {
                MenuRowKind::Separator => None,
                MenuRowKind::Item { .. } => Some(idx),
            };
        }
        y = next_y;
    }
    None
}

#[inline]
fn scaled_alpha(base_a: u8, global_alpha: f32) -> u8 {
    ((base_a as f32) * global_alpha.clamp(0.0, 1.0))
        .round()
        .clamp(0.0, 255.0) as u8
}

#[inline]
fn argb_alpha(base_a: u8, r: u8, g: u8, b: u8, global_alpha: f32) -> u32 {
    make_argb(scaled_alpha(base_a, global_alpha), r, g, b)
}

#[inline]
fn rgb_to_colorref(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16)
}

#[inline]
fn sanitize_dpi_scale(dpi_scale: f32) -> f32 {
    if dpi_scale.is_finite() && dpi_scale > 0.1 {
        dpi_scale
    } else {
        1.0
    }
}

#[inline]
fn item_label_rgba(destructive: bool, is_hovered: bool, is_active: bool) -> (u8, u8, u8, u8) {
    if destructive {
        if is_hovered {
            (255, 252, 165, 165)
        } else {
            (245, 248, 113, 113)
        }
    } else if is_active {
        (255, 255, 255, 255)
    } else {
        (242, 244, 244, 245)
    }
}

#[inline]
fn item_hint_rgba(command: Option<MenuCommand>, hint: &str) -> (u8, u8, u8, u8) {
    if command == Some(MenuCommand::OpenDeck) && hint != "Deck" {
        (255, 90, 180, 255)
    } else {
        (225, 156, 163, 175)
    }
}

enum RightSlot<'a> {
    Hint {
        hint_right_log: f32,
        hint: Option<&'a str>,
        pad_if_no_hint: f32,
        color_rgba: (u8, u8, u8, u8),
    },
    Switch {
        right_limit_log: f32,
    },
    None,
}

fn resolve_right_slot<'a>(
    right_edge: f32,
    submenu: Option<SubmenuKind>,
    switch_on: Option<bool>,
    checked: bool,
    has_any_checked: bool,
    command: Option<MenuCommand>,
    right_hint: Option<&'a str>,
) -> RightSlot<'a> {
    if submenu.is_some() {
        let hint_right_log = right_edge - 14.0;
        let color_rgba = right_hint
            .map(|h| item_hint_rgba(command, h))
            .unwrap_or((225, 156, 163, 175));
        RightSlot::Hint {
            hint_right_log,
            hint: right_hint,
            pad_if_no_hint: 4.0,
            color_rgba,
        }
    } else if switch_on.is_some() {
        RightSlot::Switch {
            right_limit_log: right_edge - 28.0 - 8.0,
        }
    } else if checked || has_any_checked {
        let hint_right_log = right_edge - 18.0;
        let color_rgba = right_hint
            .map(|h| item_hint_rgba(command, h))
            .unwrap_or((225, 156, 163, 175));
        RightSlot::Hint {
            hint_right_log,
            hint: right_hint,
            pad_if_no_hint: 4.0,
            color_rgba,
        }
    } else if let Some(hint) = right_hint {
        RightSlot::Hint {
            hint_right_log: right_edge,
            hint: Some(hint),
            pad_if_no_hint: 0.0,
            color_rgba: item_hint_rgba(command, hint),
        }
    } else {
        RightSlot::None
    }
}

pub struct TrayMenuRenderer {
    font_family_ui: *mut c_void,
    font_label: *mut c_void,
    font_hint: *mut c_void,
    format_left: *mut c_void,
    format_right: *mut c_void,
    gdi_dpi_scale: f32,
    hfont_label: HFONT,
    hfont_hint: HFONT,
}

impl Default for TrayMenuRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TrayMenuRenderer {
    pub fn new() -> Self {
        unsafe {
            let mut font_family_ui: *mut c_void = null_mut();
            let mut font_label: *mut c_void = null_mut();
            let mut font_hint: *mut c_void = null_mut();

            let font_candidates = [
                "Microsoft YaHei UI\0",
                "Segoe UI Variable Text\0",
                "Segoe UI\0",
                "Microsoft YaHei\0",
            ];
            for candidate in &font_candidates {
                let name_utf16: Vec<u16> = candidate.encode_utf16().collect();
                let status = GdipCreateFontFamilyFromName(
                    name_utf16.as_ptr(),
                    null_mut(),
                    &mut font_family_ui,
                );
                if status == 0 && !font_family_ui.is_null() {
                    break;
                }
            }

            if !font_family_ui.is_null() {
                GdipCreateFont(
                    font_family_ui,
                    13.0,
                    FONT_STYLE_REGULAR,
                    UNIT_PIXEL,
                    &mut font_label,
                );
                GdipCreateFont(
                    font_family_ui,
                    11.0,
                    FONT_STYLE_REGULAR,
                    UNIT_PIXEL,
                    &mut font_hint,
                );
            }

            let mut format_left: *mut c_void = null_mut();
            GdipCreateStringFormat(0, 0, &mut format_left);
            if !format_left.is_null() {
                GdipSetStringFormatAlign(format_left, 0);
                GdipSetStringFormatLineAlign(format_left, 1);
                GdipSetStringFormatFlags(format_left, STRING_FORMAT_FLAGS_NO_WRAP);
                GdipSetStringFormatTrimming(format_left, STRING_TRIMMING_ELLIPSIS_CHARACTER);
            }

            let mut format_right: *mut c_void = null_mut();
            GdipCreateStringFormat(0, 0, &mut format_right);
            if !format_right.is_null() {
                GdipSetStringFormatAlign(format_right, 2);
                GdipSetStringFormatLineAlign(format_right, 1);
                GdipSetStringFormatFlags(format_right, STRING_FORMAT_FLAGS_NO_WRAP);
                GdipSetStringFormatTrimming(format_right, STRING_TRIMMING_NONE);
            }

            Self {
                font_family_ui,
                font_label,
                font_hint,
                format_left,
                format_right,
                gdi_dpi_scale: 0.0,
                hfont_label: null_mut(),
                hfont_hint: null_mut(),
            }
        }
    }

    unsafe fn create_gdi_font(px_height: i32) -> HFONT {
        let candidates = ["Microsoft YaHei UI\0", "Microsoft YaHei\0", "Segoe UI\0"];
        unsafe {
            for face in &candidates {
                let face_w: Vec<u16> = face.encode_utf16().collect();
                let hf = CreateFontW(
                    px_height,
                    0,
                    0,
                    0,
                    FW_NORMAL as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET as u32,
                    OUT_DEFAULT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32,
                    CLEARTYPE_QUALITY as u32,
                    (DEFAULT_PITCH | FF_DONTCARE) as u32,
                    face_w.as_ptr(),
                );
                if !hf.is_null() {
                    return hf;
                }
            }
            null_mut()
        }
    }

    unsafe fn ensure_gdi_fonts(&mut self, dpi_scale: f32) {
        let scale = sanitize_dpi_scale(dpi_scale);
        if !self.hfont_label.is_null()
            && !self.hfont_hint.is_null()
            && (self.gdi_dpi_scale - scale).abs() <= 0.005
        {
            return;
        }
        unsafe {
            self.release_gdi_fonts();
            let label_h = -((13.0 * scale).round().max(11.0) as i32);
            let hint_h = -((11.5 * scale).round().max(10.0) as i32);
            self.hfont_label = Self::create_gdi_font(label_h);
            self.hfont_hint = Self::create_gdi_font(hint_h);
            self.gdi_dpi_scale = scale;
        }
    }

    unsafe fn release_gdi_fonts(&mut self) {
        unsafe {
            if !self.hfont_label.is_null() {
                DeleteObject(self.hfont_label as _);
                self.hfont_label = null_mut();
            }
            if !self.hfont_hint.is_null() {
                DeleteObject(self.hfont_hint as _);
                self.hfont_hint = null_mut();
            }
            self.gdi_dpi_scale = 0.0;
        }
    }

    unsafe fn apply_card_clip(
        graphics: *mut c_void,
        card_x: f32,
        card_y: f32,
        card_w: f32,
        card_h: f32,
        card_radius: f32,
    ) {
        unsafe {
            let mut clip_path: *mut c_void = null_mut();
            GdipCreatePath(0, &mut clip_path);
            if !clip_path.is_null() {
                add_round_rect_to_path(clip_path, card_x, card_y, card_w, card_h, card_radius);
                GdipSetClipPath(graphics, clip_path, 0);
                GdipDeletePath(clip_path);
            }
        }
    }

    unsafe fn draw_panel_chrome(
        graphics: *mut c_void,
        rows: &[MenuRow],
        card_w: f32,
        hovered_idx: Option<usize>,
        open_submenu_row: Option<usize>,
        alpha: f32,
        snap_scale: Option<f32>,
    ) {
        unsafe {
            GdipSetSmoothingMode(graphics, SMOOTHING_MODE_ANTIALIAS);

            let card_x = SHADOW_PAD;
            let card_y = SHADOW_PAD;
            let rows_h: f32 = rows.iter().map(|r| r.height).sum();
            let card_h = CARD_PAD_Y * 2.0 + rows_h;
            let card_radius = 10.0;

            // 1. 卡片外围 3 层柔和外扩阴影（严格留在 SHADOW_PAD 范围之内）
            let shadow_layers: [(f32, f32, f32, f32, u8); 3] = [
                (-6.0, -4.0, 12.0, 12.0, 14),
                (-4.0, -2.5, 8.0, 7.5, 26),
                (-1.5, -0.5, 3.0, 3.0, 42),
            ];
            for (dx, dy, dw, dh, base_a) in shadow_layers {
                Self::fill_round_rect(
                    graphics,
                    card_x + dx,
                    card_y + dy,
                    card_w + dw,
                    card_h + dh,
                    card_radius + (-dx),
                    argb_alpha(base_a, 0, 0, 0, alpha),
                );
            }

            // 2. 深黑磨砂主体 #1F1F23 (纯不透明 a=255，确保 ClearType 亚像素抗锯齿无彩边)
            //    + 10px 圆角 + 1px 内描边 rgba(255,255,255,0.10)
            Self::fill_round_rect(
                graphics,
                card_x,
                card_y,
                card_w,
                card_h,
                card_radius,
                argb_alpha(255, 31, 31, 35, alpha),
            );
            Self::stroke_round_rect(
                graphics,
                card_x + 0.5,
                card_y + 0.5,
                (card_w - 1.0).max(1.0),
                (card_h - 1.0).max(1.0),
                card_radius - 0.5,
                argb_alpha(26, 255, 255, 255, alpha),
                1.0,
            );

            // 3. 启用圆角裁切，保证所有内部高亮胶囊与分割线严禁溢出边界
            Self::apply_card_clip(graphics, card_x, card_y, card_w, card_h, card_radius);

            let mut row_y = card_y + CARD_PAD_Y;
            for (idx, row) in rows.iter().enumerate() {
                match &row.kind {
                    MenuRowKind::Separator => {
                        let raw_sep_y = row_y + (row.height - 1.0) * 0.5;
                        let (sep_y, sep_h) = if let Some(s) = snap_scale {
                            (
                                (raw_sep_y * s).round() / s,
                                (1.0 * s).round().max(1.0) / s,
                            )
                        } else {
                            (raw_sep_y.round(), 1.0)
                        };
                        let mut sep_brush: *mut c_void = null_mut();
                        GdipCreateSolidFill(argb_alpha(20, 255, 255, 255, alpha), &mut sep_brush);
                        if !sep_brush.is_null() {
                            GdipFillRectangle(
                                graphics,
                                sep_brush,
                                card_x + 12.0,
                                sep_y,
                                (card_w - 24.0).max(1.0),
                                sep_h,
                            );
                            GdipDeleteBrush(sep_brush);
                        }
                    }
                    MenuRowKind::Item {
                        icon,
                        submenu,
                        checked,
                        switch_on,
                        destructive,
                        ..
                    } => {
                        let is_hovered = hovered_idx == Some(idx);
                        let is_sub_open = open_submenu_row == Some(idx);
                        let is_active = is_hovered || is_sub_open;

                        if is_active {
                            let hi_color = if *destructive && is_hovered {
                                argb_alpha(41, 239, 68, 68, alpha)
                            } else {
                                argb_alpha(20, 255, 255, 255, alpha)
                            };
                            let (pill_y, pill_h) = if let Some(s) = snap_scale {
                                let top = ((row_y + 1.0) * s).round() / s;
                                let bot = ((row_y + row.height - 1.0) * s).round() / s;
                                (top, (bot - top).max(4.0))
                            } else {
                                (row_y + 1.0, (row.height - 2.0).max(4.0))
                            };
                            Self::fill_round_rect(
                                graphics,
                                card_x + 5.0,
                                pill_y,
                                (card_w - 10.0).max(4.0),
                                pill_h,
                                6.0,
                                hi_color,
                            );
                        }

                        let icon_cx = card_x + 22.0;
                        let icon_cy = if let Some(s) = snap_scale {
                            ((row_y + row.height * 0.5) * s).round() / s
                        } else {
                            row_y + row.height * 0.5
                        };
                        Self::draw_icon(
                            graphics,
                            *icon,
                            icon_cx,
                            icon_cy,
                            is_active,
                            *switch_on == Some(true),
                            alpha,
                        );

                        let right_edge = card_x + card_w - 14.0;
                        if submenu.is_some() {
                            let chev_cx = right_edge - 3.5;
                            let chev_color = if is_active {
                                argb_alpha(235, 228, 228, 231, alpha)
                            } else {
                                argb_alpha(210, 156, 163, 175, alpha)
                            };
                            Self::draw_chevron_right(graphics, chev_cx, icon_cy, chev_color);
                        } else if let Some(on) = switch_on {
                            let sw_w = 28.0;
                            let sw_h = 15.0;
                            let sw_x = right_edge - sw_w;
                            let sw_y = if let Some(s) = snap_scale {
                                ((row_y + (row.height - sw_h) * 0.5) * s).round() / s
                            } else {
                                (row_y + (row.height - sw_h) * 0.5).round()
                            };
                            Self::draw_switch(graphics, sw_x, sw_y, sw_w, sw_h, *on, alpha);
                        } else if *checked {
                            let check_cx = right_edge - 6.0;
                            Self::draw_checkmark(
                                graphics,
                                check_cx,
                                icon_cy,
                                argb_alpha(255, 90, 180, 255, alpha),
                            );
                        }
                    }
                }
                row_y += row.height;
            }

            GdipResetClip(graphics);
        }
    }

    pub unsafe fn render_panel(
        &self,
        graphics: *mut c_void,
        rows: &[MenuRow],
        card_w: f32,
        hovered_idx: Option<usize>,
        open_submenu_row: Option<usize>,
        global_alpha: f32,
    ) {
        unsafe {
            if graphics.is_null() {
                return;
            }
            let alpha = global_alpha.clamp(0.0, 1.0);
            if alpha <= 0.004 {
                return;
            }

            Self::draw_panel_chrome(
                graphics,
                rows,
                card_w,
                hovered_idx,
                open_submenu_row,
                alpha,
                None,
            );

            GdipSetTextRenderingHint(graphics, TEXT_RENDERING_HINT_ANTIALIAS_GRID_FIT);

            let card_x = SHADOW_PAD;
            let card_y = SHADOW_PAD;
            let rows_h: f32 = rows.iter().map(|r| r.height).sum();
            let card_h = CARD_PAD_Y * 2.0 + rows_h;
            Self::apply_card_clip(graphics, card_x, card_y, card_w, card_h, 10.0);

            let has_any_checked = rows
                .iter()
                .any(|r| matches!(r.kind, MenuRowKind::Item { checked: true, .. }));

            let mut row_y = card_y + CARD_PAD_Y;
            for (idx, row) in rows.iter().enumerate() {
                if let MenuRowKind::Item {
                    label,
                    right_hint,
                    command,
                    submenu,
                    checked,
                    switch_on,
                    destructive,
                    ..
                } = &row.kind
                {
                    let is_hovered = hovered_idx == Some(idx);
                    let is_sub_open = open_submenu_row == Some(idx);
                    let is_active = is_hovered || is_sub_open;

                    let right_edge = card_x + card_w - 14.0;
                    let text_x = card_x + 40.0;
                    let mut text_right_limit = right_edge;

                    match resolve_right_slot(
                        right_edge,
                        *submenu,
                        *switch_on,
                        *checked,
                        has_any_checked,
                        *command,
                        right_hint.as_deref(),
                    ) {
                        RightSlot::Hint {
                            hint_right_log,
                            hint: Some(hint),
                            color_rgba: (ha, hr, hg, hb),
                            ..
                        } => {
                            let hint_w = Self::measure_text_w(
                                graphics,
                                self.font_hint,
                                hint,
                                self.format_left,
                            )
                            .min((hint_right_log - text_x - 24.0).max(20.0));
                            let hint_rect = RectF {
                                x: (hint_right_log - hint_w - 4.0).round(),
                                y: row_y.round(),
                                width: (hint_w + 4.0).round(),
                                height: row.height.round(),
                            };
                            Self::draw_text_str(
                                graphics,
                                hint,
                                self.font_hint,
                                &hint_rect,
                                self.format_right,
                                argb_alpha(ha, hr, hg, hb, alpha),
                            );
                            text_right_limit = hint_right_log - hint_w - 8.0;
                        }
                        RightSlot::Hint {
                            hint_right_log,
                            hint: None,
                            pad_if_no_hint,
                            ..
                        } => {
                            text_right_limit = hint_right_log - pad_if_no_hint;
                        }
                        RightSlot::Switch { right_limit_log } => {
                            text_right_limit = right_limit_log;
                        }
                        RightSlot::None => {}
                    }

                    let label_w = (text_right_limit - text_x).max(20.0);
                    let label_rect = RectF {
                        x: text_x.round(),
                        y: row_y.round(),
                        width: label_w.round(),
                        height: row.height.round(),
                    };
                    let (la, lr, lg, lb) = item_label_rgba(*destructive, is_hovered, is_active);
                    Self::draw_text_str(
                        graphics,
                        label,
                        self.font_label,
                        &label_rect,
                        self.format_left,
                        argb_alpha(la, lr, lg, lb, alpha),
                    );
                }
                row_y += row.height;
            }

            GdipResetClip(graphics);
        }
    }

    pub unsafe fn render_panel_to_dib(
        &mut self,
        graphics: *mut c_void,
        mem_dc: HDC,
        bits_ptr: *mut u8,
        phys_w: i32,
        phys_h: i32,
        dpi_scale: f32,
        rows: &[MenuRow],
        card_w: f32,
        hovered_idx: Option<usize>,
        open_submenu_row: Option<usize>,
        global_alpha: f32,
    ) {
        unsafe {
            if graphics.is_null()
                || mem_dc.is_null()
                || bits_ptr.is_null()
                || phys_w <= 0
                || phys_h <= 0
            {
                return;
            }
            let alpha = global_alpha.clamp(0.0, 1.0);
            if alpha <= 0.004 {
                return;
            }

            let scale = sanitize_dpi_scale(dpi_scale);

            // Step A: GDI+ 绘制投影、不透明 #1F1F23 卡片、内描边、悬停胶囊、分割线与矢量图标，
            //         并立即刷新确保像素落盘到 bits_ptr。
            Self::draw_panel_chrome(
                graphics,
                rows,
                card_w,
                hovered_idx,
                open_submenu_row,
                alpha,
                Some(scale),
            );
            GdipFlush(graphics, 1);

            // Step B: 按物理像素高度创建/复用 ClearType HFONT
            self.ensure_gdi_fonts(scale);
            if self.hfont_label.is_null() || self.hfont_hint.is_null() {
                return;
            }

            // Step C: 在 1:1 物理像素坐标系的 mem_dc 上调用原生 GDI DrawTextW 绘制 ClearType 文本
            let old_bk_mode = SetBkMode(mem_dc, TRANSPARENT as i32);
            let old_text_color = SetTextColor(mem_dc, rgb_to_colorref(244, 244, 245));

            let card_x = SHADOW_PAD;
            let card_y = SHADOW_PAD;
            let rows_h: f32 = rows.iter().map(|r| r.height).sum();
            let card_h = CARD_PAD_Y * 2.0 + rows_h;
            let card_radius = 10.0_f32;

            let has_any_checked = rows
                .iter()
                .any(|r| matches!(r.kind, MenuRowKind::Item { checked: true, .. }));

            let mut drawn_rects: Vec<RECT> = Vec::with_capacity(rows.len() * 2);
            let pad_px = (4.0 * scale).round().max(2.0) as i32;
            let gap_px = (8.0 * scale).round().max(4.0) as i32;
            let min_hint_w_px = (20.0 * scale).round() as i32;
            let min_label_w_px = (20.0 * scale).round() as i32;
            let text_x = card_x + 40.0;
            let text_x_px = (text_x * scale).round() as i32;
            let right_edge = card_x + card_w - 14.0;

            let mut row_y = card_y + CARD_PAD_Y;
            for (idx, row) in rows.iter().enumerate() {
                if let MenuRowKind::Item {
                    label,
                    right_hint,
                    command,
                    submenu,
                    checked,
                    switch_on,
                    destructive,
                    ..
                } = &row.kind
                {
                    let is_hovered = hovered_idx == Some(idx);
                    let is_sub_open = open_submenu_row == Some(idx);
                    let is_active = is_hovered || is_sub_open;

                    let row_top_px = (row_y * scale).round() as i32;
                    let row_bot_px = ((row_y + row.height) * scale).round() as i32;
                    let mut text_right_limit_px = (right_edge * scale).round() as i32;

                    match resolve_right_slot(
                        right_edge,
                        *submenu,
                        *switch_on,
                        *checked,
                        has_any_checked,
                        *command,
                        right_hint.as_deref(),
                    ) {
                        RightSlot::Hint {
                            hint_right_log,
                            hint: Some(hint),
                            color_rgba: (_, hr, hg, hb),
                            ..
                        } => {
                            let hint_right_px = (hint_right_log * scale).round() as i32;
                            let max_hint_w_px = ((hint_right_log - text_x - 24.0).max(20.0)
                                * scale)
                                .round() as i32;
                            let measured_w_px =
                                Self::measure_gdi_text_w(mem_dc, self.hfont_hint, hint);
                            let hint_w_px =
                                measured_w_px.min(max_hint_w_px.max(min_hint_w_px));
                            let mut hint_rc = RECT {
                                left: hint_right_px - hint_w_px - pad_px,
                                top: row_top_px,
                                right: hint_right_px,
                                bottom: row_bot_px,
                            };
                            if Self::draw_gdi_text(
                                mem_dc,
                                self.hfont_hint,
                                hint,
                                &mut hint_rc,
                                rgb_to_colorref(hr, hg, hb),
                                true,
                            ) {
                                drawn_rects.push(hint_rc);
                            }
                            text_right_limit_px = hint_right_px - hint_w_px - gap_px;
                        }
                        RightSlot::Hint {
                            hint_right_log,
                            hint: None,
                            pad_if_no_hint,
                            ..
                        } => {
                            text_right_limit_px =
                                ((hint_right_log - pad_if_no_hint) * scale).round() as i32;
                        }
                        RightSlot::Switch { right_limit_log } => {
                            text_right_limit_px = (right_limit_log * scale).round() as i32;
                        }
                        RightSlot::None => {}
                    }

                    let label_right_px = text_right_limit_px.max(text_x_px + min_label_w_px);
                    let mut label_rc = RECT {
                        left: text_x_px,
                        top: row_top_px,
                        right: label_right_px,
                        bottom: row_bot_px,
                    };
                    let (_, lr, lg, lb) = item_label_rgba(*destructive, is_hovered, is_active);
                    if Self::draw_gdi_text(
                        mem_dc,
                        self.hfont_label,
                        label,
                        &mut label_rc,
                        rgb_to_colorref(lr, lg, lb),
                        false,
                    ) {
                        drawn_rects.push(label_rc);
                    }
                }
                row_y += row.height;
            }

            SetTextColor(mem_dc, old_text_color);
            SetBkMode(mem_dc, old_bk_mode);

            // Step D: 刷新 GDI 后将卡片内部文字区域的 Alpha 字节恢复为 255，
            //         双重安全钳位至圆角内部矩形，绝不触碰 10px 圆角边缘或外侧阴影。
            GdiFlush();

            let corner_safe_px = ((card_radius * scale).ceil() as i32).max(2);
            let safe_left = ((card_x * scale).ceil() as i32 + corner_safe_px).clamp(0, phys_w);
            let safe_right = (((card_x + card_w) * scale).floor() as i32 - corner_safe_px)
                .clamp(safe_left, phys_w);
            let safe_top = ((card_y * scale).ceil() as i32 + 2).clamp(0, phys_h);
            let safe_bottom =
                (((card_y + card_h) * scale).floor() as i32 - 2).clamp(safe_top, phys_h);

            for rc in drawn_rects {
                let x0 = rc.left.clamp(safe_left, safe_right);
                let x1 = rc.right.clamp(safe_left, safe_right);
                let y0 = rc.top.clamp(safe_top, safe_bottom);
                let y1 = rc.bottom.clamp(safe_top, safe_bottom);
                for y in y0..y1 {
                    let row_base = (y as usize) * (phys_w as usize) * 4;
                    for x in x0..x1 {
                        *bits_ptr.add(row_base + (x as usize) * 4 + 3) = 255;
                    }
                }
            }
        }
    }

    unsafe fn measure_gdi_text_w(mem_dc: HDC, hfont: HFONT, text: &str) -> i32 {
        if mem_dc.is_null() || hfont.is_null() || text.is_empty() {
            return 0;
        }
        unsafe {
            let old_font = SelectObject(mem_dc, hfont as _);
            let utf16: Vec<u16> = text.encode_utf16().collect();
            let mut rc = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            DrawTextW(
                mem_dc,
                utf16.as_ptr(),
                utf16.len() as i32,
                &mut rc,
                DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(mem_dc, old_font);
            rc.right.max(0)
        }
    }

    unsafe fn draw_gdi_text(
        mem_dc: HDC,
        hfont: HFONT,
        text: &str,
        rect: &mut RECT,
        color: u32,
        align_right: bool,
    ) -> bool {
        if mem_dc.is_null()
            || hfont.is_null()
            || text.is_empty()
            || rect.right <= rect.left
            || rect.bottom <= rect.top
        {
            return false;
        }
        unsafe {
            let old_font = SelectObject(mem_dc, hfont as _);
            SetTextColor(mem_dc, color);
            let utf16: Vec<u16> = text.encode_utf16().collect();
            let align_flag = if align_right { DT_RIGHT } else { DT_LEFT };
            let mut draw_rc = *rect;
            DrawTextW(
                mem_dc,
                utf16.as_ptr(),
                utf16.len() as i32,
                &mut draw_rc,
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX | align_flag,
            );
            SelectObject(mem_dc, old_font);
            true
        }
    }

    unsafe fn fill_round_rect(
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: u32,
    ) {
        unsafe {
            let mut path: *mut c_void = null_mut();
            GdipCreatePath(0, &mut path);
            if path.is_null() {
                return;
            }
            add_round_rect_to_path(path, x, y, w, h, radius);
            let mut brush: *mut c_void = null_mut();
            GdipCreateSolidFill(color, &mut brush);
            if !brush.is_null() {
                GdipFillPath(graphics, brush, path);
                GdipDeleteBrush(brush);
            }
            GdipDeletePath(path);
        }
    }

    unsafe fn stroke_round_rect(
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: u32,
        stroke_w: f32,
    ) {
        unsafe {
            let mut path: *mut c_void = null_mut();
            GdipCreatePath(0, &mut path);
            if path.is_null() {
                return;
            }
            add_round_rect_to_path(path, x, y, w, h, radius);
            let pen = Self::create_round_pen(color, stroke_w);
            if !pen.is_null() {
                GdipDrawPath(graphics, pen, path);
                GdipDeletePen(pen);
            }
            GdipDeletePath(path);
        }
    }

    unsafe fn create_round_pen(color: u32, width: f32) -> *mut c_void {
        unsafe {
            let mut pen: *mut c_void = null_mut();
            GdipCreatePen1(color, width, UNIT_PIXEL, &mut pen);
            if !pen.is_null() {
                GdipSetPenStartCap(pen, LINE_CAP_ROUND);
                GdipSetPenEndCap(pen, LINE_CAP_ROUND);
                GdipSetPenLineJoin(pen, LINE_JOIN_ROUND);
            }
            pen
        }
    }

    unsafe fn measure_text_w(
        graphics: *mut c_void,
        font: *mut c_void,
        text: &str,
        format: *mut c_void,
    ) -> f32 {
        unsafe {
            if graphics.is_null() || font.is_null() {
                return (text.chars().count() as f32) * 11.0;
            }
            let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let layout = RectF {
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 60.0,
            };
            let mut bbox = RectF {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            };
            let mut fitted: i32 = 0;
            let mut lines: i32 = 0;
            let status = GdipMeasureString(
                graphics,
                utf16.as_ptr(),
                utf16.len() as i32 - 1,
                font,
                &layout,
                format,
                &mut bbox,
                &mut fitted,
                &mut lines,
            );
            if status == 0 && bbox.width > 0.0 {
                bbox.width.ceil()
            } else {
                (text.chars().count() as f32) * 11.0
            }
        }
    }

    unsafe fn draw_text_str(
        graphics: *mut c_void,
        text: &str,
        font: *mut c_void,
        rect: &RectF,
        format: *mut c_void,
        color: u32,
    ) {
        unsafe {
            if font.is_null() {
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
                    rect,
                    format,
                    brush,
                );
                GdipDeleteBrush(brush);
            }
        }
    }

    unsafe fn draw_switch(
        graphics: *mut c_void,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        on: bool,
        alpha: f32,
    ) {
        unsafe {
            let radius = h * 0.5;
            if on {
                Self::fill_round_rect(
                    graphics,
                    x,
                    y,
                    w,
                    h,
                    radius,
                    argb_alpha(255, 59, 130, 246, alpha),
                );
            } else {
                Self::fill_round_rect(
                    graphics,
                    x,
                    y,
                    w,
                    h,
                    radius,
                    argb_alpha(255, 63, 63, 70, alpha),
                );
                Self::stroke_round_rect(
                    graphics,
                    x + 0.5,
                    y + 0.5,
                    w - 1.0,
                    h - 1.0,
                    radius - 0.5,
                    argb_alpha(36, 255, 255, 255, alpha),
                    1.0,
                );
            }

            let knob_d = 11.0;
            let knob_y = y + (h - knob_d) * 0.5;
            let knob_x = if on { x + w - knob_d - 2.0 } else { x + 2.0 };
            let knob_color = if on {
                argb_alpha(255, 255, 255, 255, alpha)
            } else {
                argb_alpha(255, 161, 161, 170, alpha)
            };
            let mut knob_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(knob_color, &mut knob_brush);
            if !knob_brush.is_null() {
                GdipFillEllipse(graphics, knob_brush, knob_x, knob_y, knob_d, knob_d);
                GdipDeleteBrush(knob_brush);
            }
        }
    }

    unsafe fn draw_chevron_right(graphics: *mut c_void, cx: f32, cy: f32, color: u32) {
        unsafe {
            let mut path: *mut c_void = null_mut();
            GdipCreatePath(0, &mut path);
            if path.is_null() {
                return;
            }
            GdipAddPathLine(path, cx - 1.8, cy - 3.6, cx + 1.8, cy);
            GdipAddPathLine(path, cx + 1.8, cy, cx - 1.8, cy + 3.6);
            let pen = Self::create_round_pen(color, 1.35);
            if !pen.is_null() {
                GdipDrawPath(graphics, pen, path);
                GdipDeletePen(pen);
            }
            GdipDeletePath(path);
        }
    }

    unsafe fn draw_checkmark(graphics: *mut c_void, cx: f32, cy: f32, color: u32) {
        unsafe {
            let mut path: *mut c_void = null_mut();
            GdipCreatePath(0, &mut path);
            if path.is_null() {
                return;
            }
            GdipAddPathLine(path, cx - 4.4, cy + 0.2, cx - 1.2, cy + 3.4);
            GdipAddPathLine(path, cx - 1.2, cy + 3.4, cx + 4.6, cy - 3.0);
            let pen = Self::create_round_pen(color, 1.6);
            if !pen.is_null() {
                GdipDrawPath(graphics, pen, path);
                GdipDeletePen(pen);
            }
            GdipDeletePath(path);
        }
    }

    unsafe fn draw_icon(
        graphics: *mut c_void,
        icon: MenuIcon,
        cx: f32,
        cy: f32,
        is_active: bool,
        is_switch_on: bool,
        alpha: f32,
    ) {
        unsafe {
            let base_fg = if is_active {
                argb_alpha(255, 244, 244, 245, alpha)
            } else {
                argb_alpha(240, 212, 212, 216, alpha)
            };

            match icon {
                MenuIcon::NoiseDot(level) => {
                    let (r, g, b) = match level {
                        NoiseFilterLevel::Silent => (52, 211, 153),  // #34D399
                        NoiseFilterLevel::Normal => (90, 180, 255),  // #5AB4FF
                        NoiseFilterLevel::Verbose => (245, 158, 11), // #F59E0B
                    };
                    let mut halo_brush: *mut c_void = null_mut();
                    GdipCreateSolidFill(argb_alpha(48, r, g, b, alpha), &mut halo_brush);
                    if !halo_brush.is_null() {
                        GdipFillEllipse(graphics, halo_brush, cx - 6.0, cy - 6.0, 12.0, 12.0);
                        GdipDeleteBrush(halo_brush);
                    }
                    let mut dot_brush: *mut c_void = null_mut();
                    GdipCreateSolidFill(argb_alpha(255, r, g, b, alpha), &mut dot_brush);
                    if !dot_brush.is_null() {
                        GdipFillEllipse(graphics, dot_brush, cx - 3.3, cy - 3.3, 6.6, 6.6);
                        GdipDeleteBrush(dot_brush);
                    }
                }
                MenuIcon::ToggleIsland => {
                    Self::stroke_round_rect(
                        graphics,
                        cx - 6.5,
                        cy - 3.7,
                        13.0,
                        7.4,
                        3.7,
                        base_fg,
                        1.35,
                    );
                    let mut dot_brush: *mut c_void = null_mut();
                    GdipCreateSolidFill(argb_alpha(255, 90, 180, 255, alpha), &mut dot_brush);
                    if !dot_brush.is_null() {
                        GdipFillEllipse(graphics, dot_brush, cx - 4.1, cy - 1.3, 2.6, 2.6);
                        GdipDeleteBrush(dot_brush);
                    }
                    let pen = Self::create_round_pen(base_fg, 1.25);
                    if !pen.is_null() {
                        GdipDrawLine(graphics, pen, cx - 0.2, cy, cx + 3.6, cy);
                        GdipDeletePen(pen);
                    }
                }
                MenuIcon::SessionDeck => {
                    // 后方卡片右上角探头轮廓 + 前方主卡片
                    let pen = Self::create_round_pen(base_fg, 1.3);
                    if !pen.is_null() {
                        let mut back_path: *mut c_void = null_mut();
                        GdipCreatePath(0, &mut back_path);
                        if !back_path.is_null() {
                            GdipAddPathLine(back_path, cx - 2.6, cy - 4.8, cx + 4.3, cy - 4.8);
                            GdipAddPathArc(back_path, cx + 2.8, cy - 4.8, 3.0, 3.0, 270.0, 90.0);
                            GdipAddPathLine(back_path, cx + 5.8, cy - 3.3, cx + 5.8, cy + 2.2);
                            GdipDrawPath(graphics, pen, back_path);
                            GdipDeletePath(back_path);
                        }
                        GdipDrawLine(graphics, pen, cx - 3.4, cy + 1.5, cx + 1.4, cy + 1.5);
                        GdipDeletePen(pen);
                    }
                    Self::stroke_round_rect(
                        graphics,
                        cx - 5.8,
                        cy - 2.2,
                        9.6,
                        7.4,
                        1.8,
                        base_fg,
                        1.3,
                    );
                }
                MenuIcon::Monitor => {
                    Self::stroke_round_rect(
                        graphics,
                        cx - 6.0,
                        cy - 5.0,
                        12.0,
                        8.0,
                        1.6,
                        base_fg,
                        1.3,
                    );
                    let pen = Self::create_round_pen(base_fg, 1.3);
                    if !pen.is_null() {
                        GdipDrawLine(graphics, pen, cx, cy + 3.0, cx, cy + 5.3);
                        GdipDrawLine(graphics, pen, cx - 3.2, cy + 5.3, cx + 3.2, cy + 5.3);
                        GdipDeletePen(pen);
                    }
                    let pill_pen = Self::create_round_pen(argb_alpha(230, 90, 180, 255, alpha), 1.2);
                    if !pill_pen.is_null() {
                        GdipDrawLine(graphics, pill_pen, cx - 1.8, cy - 3.2, cx + 1.8, cy - 3.2);
                        GdipDeletePen(pill_pen);
                    }
                }
                MenuIcon::MonitorCursor => {
                    Self::stroke_round_rect(
                        graphics,
                        cx - 6.2,
                        cy - 5.2,
                        11.2,
                        7.6,
                        1.5,
                        base_fg,
                        1.25,
                    );
                    let pen = Self::create_round_pen(base_fg, 1.25);
                    if !pen.is_null() {
                        GdipDrawLine(graphics, pen, cx - 0.8, cy + 2.4, cx - 0.8, cy + 4.8);
                        GdipDrawLine(graphics, pen, cx - 3.4, cy + 4.8, cx + 1.4, cy + 4.8);
                        GdipDeletePen(pen);
                    }
                    // 指针箭头
                    let mut cur_path: *mut c_void = null_mut();
                    GdipCreatePath(0, &mut cur_path);
                    if !cur_path.is_null() {
                        GdipAddPathLine(cur_path, cx + 1.8, cy - 0.8, cx + 1.8, cy + 5.2);
                        GdipAddPathLine(cur_path, cx + 1.8, cy + 5.2, cx + 3.5, cy + 3.7);
                        GdipAddPathLine(cur_path, cx + 3.5, cy + 3.7, cx + 6.0, cy + 3.7);
                        GdipClosePathFigure(cur_path);
                        let mut cur_brush: *mut c_void = null_mut();
                        GdipCreateSolidFill(argb_alpha(255, 90, 180, 255, alpha), &mut cur_brush);
                        if !cur_brush.is_null() {
                            GdipFillPath(graphics, cur_brush, cur_path);
                            GdipDeleteBrush(cur_brush);
                        }
                        GdipDeletePath(cur_path);
                    }
                }
                MenuIcon::Dnd => {
                    let dnd_color = if is_switch_on {
                        argb_alpha(255, 248, 113, 113, alpha) // #F87171 醒目珊瑚红
                    } else {
                        base_fg
                    };
                    let pen = Self::create_round_pen(dnd_color, 1.35);
                    if !pen.is_null() {
                        GdipDrawEllipse(graphics, pen, cx - 5.6, cy - 5.6, 11.2, 11.2);
                        GdipDrawLine(graphics, pen, cx - 3.8, cy - 3.8, cx + 3.8, cy + 3.8);
                        GdipDeletePen(pen);
                    }
                }
                MenuIcon::Sound => {
                    let bell_color = if is_switch_on {
                        argb_alpha(255, 244, 244, 245, alpha)
                    } else {
                        base_fg
                    };
                    let pen = Self::create_round_pen(bell_color, 1.3);
                    if !pen.is_null() {
                        let mut path: *mut c_void = null_mut();
                        GdipCreatePath(0, &mut path);
                        if !path.is_null() {
                            GdipAddPathArc(path, cx - 3.8, cy - 4.6, 7.6, 7.6, 180.0, 180.0);
                            GdipAddPathLine(path, cx + 3.8, cy - 0.8, cx + 5.1, cy + 2.6);
                            GdipAddPathLine(path, cx + 5.1, cy + 2.6, cx - 5.1, cy + 2.6);
                            GdipAddPathLine(path, cx - 5.1, cy + 2.6, cx - 3.8, cy - 0.8);
                            GdipClosePathFigure(path);

                            GdipStartPathFigure(path);
                            GdipAddPathLine(path, cx, cy - 5.8, cx, cy - 4.6);

                            GdipStartPathFigure(path);
                            GdipAddPathArc(path, cx - 1.5, cy + 1.3, 3.0, 3.0, 0.0, 180.0);

                            GdipDrawPath(graphics, pen, path);
                            GdipDeletePath(path);
                        }
                        GdipDeletePen(pen);
                    }
                }
                MenuIcon::Autostart => {
                    let pen = Self::create_round_pen(base_fg, 1.3);
                    if !pen.is_null() {
                        let mut path: *mut c_void = null_mut();
                        GdipCreatePath(0, &mut path);
                        if !path.is_null() {
                            // 火箭箭头主体
                            GdipAddPathLine(path, cx, cy - 6.0, cx + 2.6, cy - 2.3);
                            GdipAddPathLine(path, cx + 2.6, cy - 2.3, cx + 2.3, cy + 2.0);
                            GdipAddPathLine(path, cx + 2.3, cy + 2.0, cx - 2.3, cy + 2.0);
                            GdipAddPathLine(path, cx - 2.3, cy + 2.0, cx - 2.6, cy - 2.3);
                            GdipClosePathFigure(path);

                            // 左右尾翼
                            GdipStartPathFigure(path);
                            GdipAddPathLine(path, cx - 2.4, cy - 0.3, cx - 4.9, cy + 2.7);
                            GdipAddPathLine(path, cx - 4.9, cy + 2.7, cx - 2.3, cy + 2.0);

                            GdipStartPathFigure(path);
                            GdipAddPathLine(path, cx + 2.4, cy - 0.3, cx + 4.9, cy + 2.7);
                            GdipAddPathLine(path, cx + 4.9, cy + 2.7, cx + 2.3, cy + 2.0);

                            // 推进尾焰
                            GdipStartPathFigure(path);
                            GdipAddPathLine(path, cx, cy + 3.4, cx, cy + 5.7);

                            GdipDrawPath(graphics, pen, path);
                            GdipDeletePath(path);
                        }
                        GdipDeletePen(pen);
                    }
                    let mut win_brush: *mut c_void = null_mut();
                    GdipCreateSolidFill(
                        if is_switch_on {
                            argb_alpha(255, 90, 180, 255, alpha)
                        } else {
                            base_fg
                        },
                        &mut win_brush,
                    );
                    if !win_brush.is_null() {
                        GdipFillEllipse(graphics, win_brush, cx - 1.0, cy - 2.4, 2.0, 2.0);
                        GdipDeleteBrush(win_brush);
                    }
                }
                MenuIcon::Reset => {
                    let pen = Self::create_round_pen(base_fg, 1.35);
                    if !pen.is_null() {
                        let mut path: *mut c_void = null_mut();
                        GdipCreatePath(0, &mut path);
                        if !path.is_null() {
                            GdipAddPathArc(path, cx - 5.0, cy - 5.0, 10.0, 10.0, 30.0, 285.0);
                            GdipStartPathFigure(path);
                            GdipAddPathLine(path, cx + 3.5, cy - 6.3, cx + 3.5, cy - 3.5);
                            GdipAddPathLine(path, cx + 3.5, cy - 3.5, cx + 0.7, cy - 3.5);
                            GdipDrawPath(graphics, pen, path);
                            GdipDeletePath(path);
                        }
                        GdipDeletePen(pen);
                    }
                }
                MenuIcon::Exit => {
                    let exit_color = if is_active {
                        argb_alpha(255, 252, 165, 165, alpha)
                    } else {
                        argb_alpha(245, 248, 113, 113, alpha)
                    };
                    let pen = Self::create_round_pen(exit_color, 1.4);
                    if !pen.is_null() {
                        let mut path: *mut c_void = null_mut();
                        GdipCreatePath(0, &mut path);
                        if !path.is_null() {
                            GdipAddPathArc(path, cx - 5.0, cy - 4.4, 10.0, 10.0, -52.0, 284.0);
                            GdipStartPathFigure(path);
                            GdipAddPathLine(path, cx, cy - 6.0, cx, cy - 0.5);
                            GdipDrawPath(graphics, pen, path);
                            GdipDeletePath(path);
                        }
                        GdipDeletePen(pen);
                    }
                }
            }
        }
    }
}

impl Drop for TrayMenuRenderer {
    fn drop(&mut self) {
        unsafe {
            self.release_gdi_fonts();
            if !self.font_label.is_null() {
                GdipDeleteFont(self.font_label);
            }
            if !self.font_hint.is_null() {
                GdipDeleteFont(self.font_hint);
            }
            if !self.font_family_ui.is_null() {
                GdipDeleteFontFamily(self.font_family_ui);
            }
            if !self.format_left.is_null() {
                GdipDeleteStringFormat(self.format_left);
            }
            if !self.format_right.is_null() {
                GdipDeleteStringFormat(self.format_right);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> TrayMenuSnapshot {
        TrayMenuSnapshot {
            is_expanded: false,
            active_sessions_count: 2,
            noise_level: NoiseFilterLevel::Normal,
            monitor_mode: MonitorMode::Primary,
            dnd_fullscreen: true,
            sound_enabled: false,
            auto_start: true,
        }
    }

    fn contains_emoji(s: &str) -> bool {
        s.chars().any(|c| {
            let cp = c as u32;
            (0x1F300..=0x1FAFF).contains(&cp)
                || (0x2600..=0x27BF).contains(&cp)
                || cp == 0xFE0F
        })
    }

    #[test]
    fn test_build_main_rows_variants() {
        let snap_collapsed = sample_snapshot();
        let rows = build_main_rows(&snap_collapsed);
        assert_eq!(rows.len(), 12);

        let sep_count = rows
            .iter()
            .filter(|r| matches!(r.kind, MenuRowKind::Separator))
            .count();
        assert_eq!(sep_count, 3);

        // 严禁出现任何 Unicode Emoji
        for r in &rows {
            if let MenuRowKind::Item {
                label, right_hint, ..
            } = &r.kind
            {
                assert!(!contains_emoji(label), "label contains emoji: {label}");
                if let Some(h) = right_hint {
                    assert!(!contains_emoji(h), "hint contains emoji: {h}");
                }
                assert!((r.height - ROW_H).abs() < f32::EPSILON);
            } else {
                assert!((r.height - SEP_H).abs() < f32::EPSILON);
            }
        }

        // Row 0: ToggleIsland (未展开)
        match &rows[0].kind {
            MenuRowKind::Item {
                label,
                right_hint,
                icon,
                command,
                ..
            } => {
                assert_eq!(label, "展开灵动岛");
                assert_eq!(right_hint.as_deref(), Some("居顶"));
                assert_eq!(*icon, MenuIcon::ToggleIsland);
                assert_eq!(*command, Some(MenuCommand::ToggleIsland));
            }
            _ => panic!("expected ToggleIsland item"),
        }

        // Row 1: OpenDeck (2 活跃)
        match &rows[1].kind {
            MenuRowKind::Item {
                label,
                right_hint,
                command,
                ..
            } => {
                assert_eq!(label, "活跃会话抽屉");
                assert_eq!(right_hint.as_deref(), Some("2 活跃"));
                assert_eq!(*command, Some(MenuCommand::OpenDeck));
            }
            _ => panic!("expected OpenDeck item"),
        }

        // Row 3: Noise submenu
        match &rows[3].kind {
            MenuRowKind::Item {
                label,
                right_hint,
                icon,
                submenu,
                command,
                ..
            } => {
                assert_eq!(label, "通知降噪");
                assert_eq!(right_hint.as_deref(), Some("标准"));
                assert_eq!(*icon, MenuIcon::NoiseDot(NoiseFilterLevel::Normal));
                assert_eq!(*submenu, Some(SubmenuKind::Noise));
                assert_eq!(*command, None);
            }
            _ => panic!("expected Noise submenu item"),
        }

        // Row 4: Monitor submenu
        match &rows[4].kind {
            MenuRowKind::Item {
                label,
                right_hint,
                icon,
                submenu,
                ..
            } => {
                assert_eq!(label, "屏幕位置");
                assert_eq!(right_hint.as_deref(), Some("主屏"));
                assert_eq!(*icon, MenuIcon::Monitor);
                assert_eq!(*submenu, Some(SubmenuKind::Monitor));
            }
            _ => panic!("expected Monitor submenu item"),
        }

        // Rows 6, 7, 8: Switches
        match &rows[6].kind {
            MenuRowKind::Item {
                switch_on, command, ..
            } => {
                assert_eq!(*switch_on, Some(true));
                assert_eq!(*command, Some(MenuCommand::ToggleDnd));
            }
            _ => panic!("expected ToggleDnd"),
        }
        match &rows[7].kind {
            MenuRowKind::Item {
                switch_on, command, ..
            } => {
                assert_eq!(*switch_on, Some(false));
                assert_eq!(*command, Some(MenuCommand::ToggleSound));
            }
            _ => panic!("expected ToggleSound"),
        }
        match &rows[8].kind {
            MenuRowKind::Item {
                switch_on, command, ..
            } => {
                assert_eq!(*switch_on, Some(true));
                assert_eq!(*command, Some(MenuCommand::ToggleAutostart));
            }
            _ => panic!("expected ToggleAutostart"),
        }

        // Rows 10, 11: Reset & Exit
        match &rows[10].kind {
            MenuRowKind::Item {
                label,
                command,
                destructive,
                ..
            } => {
                assert_eq!(label, "还原待命就绪状态");
                assert_eq!(*command, Some(MenuCommand::ResetIdle));
                assert!(!*destructive);
            }
            _ => panic!("expected ResetIdle"),
        }
        match &rows[11].kind {
            MenuRowKind::Item {
                label,
                command,
                destructive,
                ..
            } => {
                assert_eq!(label, "退出 WNoobar");
                assert_eq!(*command, Some(MenuCommand::Exit));
                assert!(*destructive);
            }
            _ => panic!("expected Exit"),
        }

        // 另一状态快照测试：展开、0 活跃、Verbose、FollowCursor
        let snap_expanded = TrayMenuSnapshot {
            is_expanded: true,
            active_sessions_count: 0,
            noise_level: NoiseFilterLevel::Verbose,
            monitor_mode: MonitorMode::FollowCursor,
            dnd_fullscreen: false,
            sound_enabled: true,
            auto_start: false,
        };
        let rows2 = build_main_rows(&snap_expanded);
        match &rows2[0].kind {
            MenuRowKind::Item {
                label, right_hint, ..
            } => {
                assert_eq!(label, "收起灵动岛");
                assert_eq!(right_hint.as_deref(), Some("贴边"));
            }
            _ => panic!("expected expanded ToggleIsland"),
        }
        match &rows2[1].kind {
            MenuRowKind::Item { right_hint, .. } => {
                assert_eq!(right_hint.as_deref(), Some("Deck"));
            }
            _ => panic!("expected Deck hint"),
        }
        match &rows2[3].kind {
            MenuRowKind::Item {
                right_hint, icon, ..
            } => {
                assert_eq!(right_hint.as_deref(), Some("详细"));
                assert_eq!(*icon, MenuIcon::NoiseDot(NoiseFilterLevel::Verbose));
            }
            _ => panic!("expected Verbose"),
        }
        match &rows2[4].kind {
            MenuRowKind::Item { right_hint, .. } => {
                assert_eq!(right_hint.as_deref(), Some("跟随鼠标"));
            }
            _ => panic!("expected FollowCursor"),
        }
    }

    #[test]
    fn test_build_submenu_rows() {
        let mut snap = sample_snapshot();
        snap.noise_level = NoiseFilterLevel::Silent;
        let noise_rows = build_submenu_rows(SubmenuKind::Noise, &snap);
        assert_eq!(noise_rows.len(), 3);
        let expected_noise = [
            ("极简模式", "Silent", NoiseFilterLevel::Silent, true),
            ("标准模式", "Normal", NoiseFilterLevel::Normal, false),
            ("详细模式", "Verbose", NoiseFilterLevel::Verbose, false),
        ];
        for (row, (lbl, hint, lvl, exp_checked)) in noise_rows.iter().zip(expected_noise.iter()) {
            match &row.kind {
                MenuRowKind::Item {
                    label,
                    right_hint,
                    icon,
                    command,
                    checked,
                    ..
                } => {
                    assert_eq!(label, lbl);
                    assert_eq!(right_hint.as_deref(), Some(*hint));
                    assert_eq!(*icon, MenuIcon::NoiseDot(*lvl));
                    assert_eq!(*command, Some(MenuCommand::SetNoise(*lvl)));
                    assert_eq!(*checked, *exp_checked);
                }
                _ => panic!("expected submenu item"),
            }
        }

        snap.monitor_mode = MonitorMode::FollowCursor;
        let mon_rows = build_submenu_rows(SubmenuKind::Monitor, &snap);
        assert_eq!(mon_rows.len(), 2);
        match &mon_rows[0].kind {
            MenuRowKind::Item {
                label,
                icon,
                command,
                checked,
                ..
            } => {
                assert_eq!(label, "固定主显示器");
                assert_eq!(*icon, MenuIcon::Monitor);
                assert_eq!(*command, Some(MenuCommand::SetMonitor(MonitorMode::Primary)));
                assert!(!*checked);
            }
            _ => panic!("expected Primary monitor item"),
        }
        match &mon_rows[1].kind {
            MenuRowKind::Item {
                label,
                icon,
                command,
                checked,
                ..
            } => {
                assert_eq!(label, "跟随鼠标所在屏幕");
                assert_eq!(*icon, MenuIcon::MonitorCursor);
                assert_eq!(
                    *command,
                    Some(MenuCommand::SetMonitor(MonitorMode::FollowCursor))
                );
                assert!(*checked);
            }
            _ => panic!("expected FollowCursor monitor item"),
        }
    }

    #[test]
    fn test_measure_panel_size_and_row_top_y() {
        let snap = sample_snapshot();
        let main_rows = build_main_rows(&snap);
        let (w, h) = measure_panel_size(&main_rows, MAIN_CARD_W);
        // 9 * 32.0 + 3 * 9.0 = 315.0; + CARD_PAD_Y*2 (12.0) + SHADOW_PAD*2 (20.0) = 347.0
        assert!((w - (MAIN_CARD_W + SHADOW_PAD * 2.0)).abs() < f32::EPSILON);
        assert!((h - (CARD_PAD_Y * 2.0 + 315.0 + SHADOW_PAD * 2.0)).abs() < f32::EPSILON);

        assert!((row_top_y(&main_rows, 0) - 16.0).abs() < f32::EPSILON);
        assert!((row_top_y(&main_rows, 1) - 48.0).abs() < f32::EPSILON);
        assert!((row_top_y(&main_rows, 2) - 80.0).abs() < f32::EPSILON); // Separator 0
        assert!((row_top_y(&main_rows, 3) - 89.0).abs() < f32::EPSILON); // After SEP_H=9.0
        assert!((row_top_y(&main_rows, main_rows.len()) - (16.0 + 315.0)).abs() < f32::EPSILON);

        let noise_rows = build_submenu_rows(SubmenuKind::Noise, &snap);
        let (sw, sh) = measure_panel_size(&noise_rows, SUBMENU_CARD_W);
        assert!((sw - (SUBMENU_CARD_W + SHADOW_PAD * 2.0)).abs() < f32::EPSILON);
        assert!((sh - (12.0 + 96.0 + 20.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn test_hit_test_row_boundaries_and_separators() {
        let snap = sample_snapshot();
        let rows = build_main_rows(&snap);

        // X 热区边界：[SHADOW_PAD + 4.0, SHADOW_PAD + MAIN_CARD_W - 4.0] = [14.0, 250.0]
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 13.9, 20.0), None);
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 10.0, 20.0), None);
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 250.1, 20.0), None);
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 254.0, 20.0), None);
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 14.0, 16.0), Some(0));
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 250.0, 47.99), Some(0));

        // 顶部阴影与内边距不可命中
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 100.0, 15.99), None);
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 100.0, 0.0), None);

        // Row 0 -> Row 1 精确分界 (48.0)
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 100.0, 47.99), Some(0));
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 100.0, 48.0), Some(1));

        // Separator 2: [80.0, 89.0) 必须跳过并返回 None
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 100.0, 79.99), Some(1));
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 100.0, 80.0), None);
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 100.0, 84.5), None);
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 100.0, 88.99), None);
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 100.0, 89.0), Some(3));

        // Separator 5: [153.0, 162.0) & Separator 9: [258.0, 267.0)
        let sep5_y = row_top_y(&rows, 5);
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 100.0, sep5_y + 2.0), None);
        let sep9_y = row_top_y(&rows, 9);
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 100.0, sep9_y + 4.0), None);

        // 最后一行 Exit (idx 11): [299.0, 331.0)
        let exit_y = row_top_y(&rows, 11);
        assert_eq!(hit_test_row(&rows, MAIN_CARD_W, 100.0, exit_y), Some(11));
        assert_eq!(
            hit_test_row(&rows, MAIN_CARD_W, 100.0, exit_y + ROW_H - 0.01),
            Some(11)
        );
        assert_eq!(
            hit_test_row(&rows, MAIN_CARD_W, 100.0, exit_y + ROW_H),
            None
        );

        // 子菜单命中测试
        let mon_rows = build_submenu_rows(SubmenuKind::Monitor, &snap);
        assert_eq!(hit_test_row(&mon_rows, SUBMENU_CARD_W, 14.0, 16.0), Some(0));
        assert_eq!(
            hit_test_row(&mon_rows, SUBMENU_CARD_W, 204.0, 48.0),
            Some(1)
        );
        assert_eq!(hit_test_row(&mon_rows, SUBMENU_CARD_W, 204.1, 48.0), None);
    }

    #[test]
    fn test_tray_menu_renderer_offscreen_smoke() {
        let _gdip = GdiplusContext::init().expect("GdiplusContext::init should succeed on Windows");
        let renderer = TrayMenuRenderer::new();
        let snap = sample_snapshot();

        // 1. 主菜单离屏渲染（含普通悬停、子菜单激活行、销毁项悬停、透明态）
        let main_rows = build_main_rows(&snap);
        let (cw, ch) = measure_panel_size(&main_rows, MAIN_CARD_W);
        let width = cw.ceil() as i32;
        let height = ch.ceil() as i32;
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

            renderer.render_panel(gfx, &main_rows, MAIN_CARD_W, Some(3), Some(3), 1.0);
            assert!(
                buf.iter().any(|&b| b != 0),
                "render_panel should draw non-empty pixels"
            );

            renderer.render_panel(gfx, &main_rows, MAIN_CARD_W, Some(11), None, 0.75);
            renderer.render_panel(gfx, &main_rows, MAIN_CARD_W, None, None, 0.0);

            // 验证子菜单文本宽度在 SUBMENU_CARD_W 内不发生拥挤碰撞
            let label_w = TrayMenuRenderer::measure_text_w(
                gfx,
                renderer.font_label,
                "跟随鼠标所在屏幕",
                renderer.format_left,
            );
            assert!(
                label_w <= SUBMENU_CARD_W - 40.0 - 14.0 - 18.0,
                "Monitor submenu label width ({label_w}) should fit inside card"
            );

            let noise_label_w = TrayMenuRenderer::measure_text_w(
                gfx,
                renderer.font_label,
                "详细模式",
                renderer.format_left,
            );
            let noise_hint_w = TrayMenuRenderer::measure_text_w(
                gfx,
                renderer.font_hint,
                "Verbose",
                renderer.format_left,
            );
            assert!(
                40.0 + noise_label_w + 8.0 + noise_hint_w + 18.0 + 14.0 <= SUBMENU_CARD_W,
                "Noise submenu label ({noise_label_w}) + hint ({noise_hint_w}) should fit without overlap"
            );

            GdipDeleteGraphics(gfx);
            GdipDisposeImage(bmp);
        }

        // 2. 子菜单离屏渲染（Noise 与 Monitor）
        for kind in [SubmenuKind::Noise, SubmenuKind::Monitor] {
            let sub_rows = build_submenu_rows(kind, &snap);
            let (sw, sh) = measure_panel_size(&sub_rows, SUBMENU_CARD_W);
            let sw_i = sw.ceil() as i32;
            let sh_i = sh.ceil() as i32;
            let mut sbuf = vec![0u8; (sw_i * sh_i * 4) as usize];
            let mut sbmp: *mut c_void = null_mut();
            let mut sgfx: *mut c_void = null_mut();
            unsafe {
                assert_eq!(
                    GdipCreateBitmapFromScan0(
                        sw_i,
                        sh_i,
                        sw_i * 4,
                        PIXEL_FORMAT_32BPP_PARGB,
                        sbuf.as_mut_ptr(),
                        &mut sbmp,
                    ),
                    0
                );
                assert_eq!(GdipGetImageGraphicsContext(sbmp, &mut sgfx), 0);
                renderer.render_panel(sgfx, &sub_rows, SUBMENU_CARD_W, Some(0), None, 1.0);
                assert!(
                    sbuf.iter().any(|&b| b != 0),
                    "submenu render_panel should draw non-empty pixels"
                );
                GdipDeleteGraphics(sgfx);
                GdipDisposeImage(sbmp);
            }
        }
    }

    #[test]
    fn test_render_panel_to_dib_cleartype_alpha_and_shadow() {
        use std::mem::size_of;
        use windows_sys::Win32::Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, DeleteDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
            DIB_RGB_COLORS, RGBQUAD,
        };

        let _gdip = GdiplusContext::init().expect("GdiplusContext::init should succeed on Windows");
        let mut renderer = TrayMenuRenderer::new();
        let snap = sample_snapshot();
        let main_rows = build_main_rows(&snap);
        let (cw, ch) = measure_panel_size(&main_rows, MAIN_CARD_W);

        for &dpi_scale in &[1.0_f32, 1.25_f32] {
            let phys_w = (cw * dpi_scale).round().max(1.0) as i32;
            let phys_h = (ch * dpi_scale).round().max(1.0) as i32;

            unsafe {
                let mem_dc = CreateCompatibleDC(null_mut());
                assert!(!mem_dc.is_null(), "CreateCompatibleDC failed");

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

                let mut bits_raw: *mut c_void = null_mut();
                let dib_bmp = CreateDIBSection(
                    mem_dc,
                    &bmi,
                    DIB_RGB_COLORS,
                    &mut bits_raw,
                    null_mut(),
                    0,
                );
                assert!(!dib_bmp.is_null() && !bits_raw.is_null(), "CreateDIBSection failed");
                let bits_ptr = bits_raw as *mut u8;
                let old_bmp = SelectObject(mem_dc, dib_bmp as _);
                std::ptr::write_bytes(bits_ptr, 0, (phys_w * phys_h * 4) as usize);

                let mut gp_bmp: *mut c_void = null_mut();
                assert_eq!(
                    GdipCreateBitmapFromScan0(
                        phys_w,
                        phys_h,
                        phys_w * 4,
                        PIXEL_FORMAT_32BPP_PARGB,
                        bits_ptr,
                        &mut gp_bmp,
                    ),
                    0
                );
                let mut gp_gfx: *mut c_void = null_mut();
                assert_eq!(GdipGetImageGraphicsContext(gp_bmp, &mut gp_gfx), 0);
                GdipScaleWorldTransform(gp_gfx, dpi_scale, dpi_scale, MATRIX_ORDER_PREPEND);

                renderer.render_panel_to_dib(
                    gp_gfx,
                    mem_dc,
                    bits_ptr,
                    phys_w,
                    phys_h,
                    dpi_scale,
                    &main_rows,
                    MAIN_CARD_W,
                    None,
                    None,
                    1.0,
                );

                // 再次以相同 dpi_scale 渲染一次（hovered_idx = Some(1)），覆盖字体缓存命中路径
                std::ptr::write_bytes(bits_ptr, 0, (phys_w * phys_h * 4) as usize);
                renderer.render_panel_to_dib(
                    gp_gfx,
                    mem_dc,
                    bits_ptr,
                    phys_w,
                    phys_h,
                    dpi_scale,
                    &main_rows,
                    MAIN_CARD_W,
                    Some(1),
                    None,
                    1.0,
                );

                let slice = std::slice::from_raw_parts(bits_ptr, (phys_w * phys_h * 4) as usize);

                // 1) 验证第 0 行未悬停文本区 label_rect 内：
                //    - 存在被 GDI ClearType 修改过的文字前景像素（不等于卡片底色 #1F1F23: B=35, G=31, R=31）
                //    - 所有像素 alpha == 255（确认 Alpha 修复无遗漏）
                let lx0 = ((SHADOW_PAD + 40.0) * dpi_scale).round() as i32;
                let lx1 = ((SHADOW_PAD + MAIN_CARD_W - 60.0) * dpi_scale).round() as i32;
                let ly0 = (row_top_y(&main_rows, 0) * dpi_scale).round() as i32;
                let ly1 = ((row_top_y(&main_rows, 0) + ROW_H) * dpi_scale).round() as i32;

                let mut fg_pixels = 0usize;
                for y in ly0..ly1 {
                    for x in lx0..lx1 {
                        let off = ((y * phys_w + x) * 4) as usize;
                        let (b, g, r, a) = (slice[off], slice[off + 1], slice[off + 2], slice[off + 3]);
                        assert_eq!(
                            a, 255,
                            "dpi={dpi_scale}: pixel ({x},{y}) inside label_rect must have alpha==255, got {a}"
                        );
                        if (b, g, r) != (35, 31, 31) {
                            fg_pixels += 1;
                        }
                    }
                }
                assert!(
                    fg_pixels > 0,
                    "dpi={dpi_scale}: label_rect should contain ClearType foreground glyph pixels"
                );

                // 2) 验证多行（含右侧 right_hint 与悬停行）文本带全部保持 alpha == 255
                for row_idx in [0usize, 1, 3, 11] {
                    let ry0 = (row_top_y(&main_rows, row_idx) * dpi_scale).round() as i32;
                    let ry1 = ((row_top_y(&main_rows, row_idx) + ROW_H) * dpi_scale).round() as i32;
                    let rx0 = ((SHADOW_PAD + 40.0) * dpi_scale).round() as i32;
                    let rx1 = ((SHADOW_PAD + MAIN_CARD_W - 14.0) * dpi_scale).round() as i32;
                    for y in ry0..ry1 {
                        for x in rx0..rx1 {
                            let off = ((y * phys_w + x) * 4) as usize;
                            let a = slice[off + 3];
                            assert_eq!(
                                a, 255,
                                "dpi={dpi_scale}: row {row_idx} pixel ({x},{y}) text region alpha must be 255, got {a}"
                            );
                        }
                    }
                }

                // 3) 验证外围 SHADOW_PAD 阴影区保留 0 < alpha < 255 的柔和半透明投影
                let shadow_max_y = ((SHADOW_PAD * dpi_scale).floor() as i32 - 1).max(2);
                let mut shadow_semitransparent = 0usize;
                for y in 1..shadow_max_y {
                    for x in (phys_w / 4)..(phys_w * 3 / 4) {
                        let off = ((y * phys_w + x) * 4) as usize;
                        let a = slice[off + 3];
                        if a > 0 && a < 255 {
                            shadow_semitransparent += 1;
                        }
                    }
                }
                assert!(
                    shadow_semitransparent > 0,
                    "dpi={dpi_scale}: outer SHADOW_PAD should preserve 0 < alpha < 255 soft shadow"
                );

                // 验证圆角外角像素未被错误强制写成 alpha=255
                let corner_x = (SHADOW_PAD * dpi_scale).floor() as i32;
                let corner_y = (SHADOW_PAD * dpi_scale).floor() as i32;
                let corner_off = ((corner_y * phys_w + corner_x) * 4) as usize;
                assert!(
                    slice[corner_off + 3] < 255,
                    "dpi={dpi_scale}: rounded corner exterior pixel ({corner_x},{corner_y}) should keep alpha < 255"
                );

                GdipDeleteGraphics(gp_gfx);
                GdipDisposeImage(gp_bmp);
                SelectObject(mem_dc, old_bmp);
                DeleteObject(dib_bmp as _);
                DeleteDC(mem_dc);
            }
        }
    }
}
