#![allow(dead_code)]
use std::ffi::c_void;
use std::ptr::null_mut;

use crate::win32::gdiplus::*;

const WNOOBAR_PNG: &[u8] = include_bytes!("../../assets/icons/wnoobar.png");
const ANTIGRAVITY_PNG: &[u8] = include_bytes!("../../assets/icons/antigravity.png");
const CLAUDE_PNG: &[u8] = include_bytes!("../../assets/icons/claude.png");
const CODEX_PNG: &[u8] = include_bytes!("../../assets/icons/codex.png");
const DEEPSEEK_PNG: &[u8] = include_bytes!("../../assets/icons/deepseek.png");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentBrand {
    WNoobar,
    Antigravity,
    Claude,
    Codex,
    DeepSeek,
}

impl AgentBrand {
    pub fn from_name(name: &str) -> Self {
        let lower = name.to_lowercase();
        if lower.contains("wnoobar") {
            AgentBrand::WNoobar
        } else if lower.contains("claude") {
            AgentBrand::Claude
        } else if lower.contains("codex") || lower.contains("copilot") {
            AgentBrand::Codex
        } else if lower.contains("deepseek") {
            AgentBrand::DeepSeek
        } else {
            AgentBrand::Antigravity
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            AgentBrand::WNoobar => "WNoobar",
            AgentBrand::Antigravity => "Antigravity",
            AgentBrand::Claude => "Claude Code",
            AgentBrand::Codex => "Codex",
            AgentBrand::DeepSeek => "DeepSeek",
        }
    }
}

pub struct GlyphManager {
    bmp_wnoobar: *mut c_void,
    bmp_antigravity: *mut c_void,
    bmp_claude: *mut c_void,
    bmp_codex: *mut c_void,
    bmp_deepseek: *mut c_void,
}

impl GlyphManager {
    pub fn new() -> Self {
        unsafe {
            let bmp_wnoobar = create_bitmap_from_bytes(WNOOBAR_PNG);
            let bmp_antigravity = create_bitmap_from_bytes(ANTIGRAVITY_PNG);
            let bmp_claude = create_bitmap_from_bytes(CLAUDE_PNG);
            let bmp_codex = create_bitmap_from_bytes(CODEX_PNG);
            let bmp_deepseek = create_bitmap_from_bytes(DEEPSEEK_PNG);

            Self {
                bmp_wnoobar,
                bmp_antigravity,
                bmp_claude,
                bmp_codex,
                bmp_deepseek,
            }
        }
    }

    pub unsafe fn draw(
        &self,
        graphics: *mut c_void,
        brand: AgentBrand,
        cx: f32,
        cy: f32,
        size: f32,
        pulse_phase: f32,
    ) {
        unsafe {
            if graphics.is_null() {
                return;
            }

            // 1. 开启高质量双三次插值抗锯齿缩放
            GdipSetInterpolationMode(graphics, INTERPOLATION_MODE_HIGH_QUALITY_BICUBIC);

            // 2. 外围精致微光环
            let halo_color = match brand {
                AgentBrand::WNoobar => {
                    let a = (38.0 + 16.0 * pulse_phase.sin()) as u8;
                    make_argb(a, 10, 185, 255)
                }
                AgentBrand::Antigravity => {
                    let a = (35.0 + 15.0 * pulse_phase.sin()) as u8;
                    make_argb(a, 100, 150, 255)
                }
                AgentBrand::Claude => make_argb(35, 217, 119, 87),
                AgentBrand::Codex => make_argb(35, 16, 163, 127),
                AgentBrand::DeepSeek => make_argb(35, 30, 111, 255),
            };

            let r = size / 2.0;
            let mut halo_brush: *mut c_void = null_mut();
            GdipCreateSolidFill(halo_color, &mut halo_brush);
            GdipFillEllipse(graphics, halo_brush, cx - r - 2.0, cy - r - 2.0, (r + 2.0) * 2.0, (r + 2.0) * 2.0);
            GdipDeleteBrush(halo_brush);

            // 3. 绘制 Dashboard Icons 官方高精内嵌位图
            let target_bmp = match brand {
                AgentBrand::WNoobar => self.bmp_wnoobar,
                AgentBrand::Antigravity => self.bmp_antigravity,
                AgentBrand::Claude => self.bmp_claude,
                AgentBrand::Codex => self.bmp_codex,
                AgentBrand::DeepSeek => self.bmp_deepseek,
            };

            if !target_bmp.is_null() {
                let img_x = (cx - size / 2.0).round();
                let img_y = (cy - size / 2.0).round();
                GdipDrawImageRect(graphics, target_bmp, img_x, img_y, size, size);
            } else {
                // 备用矢量后备
                self.draw_fallback_vector(graphics, brand, cx, cy, size);
            }
        }
    }

    unsafe fn draw_fallback_vector(&self, graphics: *mut c_void, brand: AgentBrand, cx: f32, cy: f32, size: f32) {
        unsafe {
            let r = size * 0.45;
            let color = match brand {
                AgentBrand::WNoobar => make_argb(255, 10, 185, 255),
                AgentBrand::Antigravity => make_argb(255, 90, 180, 255),
                AgentBrand::Claude => make_argb(255, 217, 119, 87),
                AgentBrand::Codex => make_argb(255, 0, 245, 212),
                AgentBrand::DeepSeek => make_argb(255, 30, 111, 255),
            };
            let mut brush: *mut c_void = null_mut();
            GdipCreateSolidFill(color, &mut brush);
            GdipFillEllipse(graphics, brush, cx - r, cy - r, r * 2.0, r * 2.0);
            GdipDeleteBrush(brush);
        }
    }
}

impl Drop for GlyphManager {
    fn drop(&mut self) {
        unsafe {
            if !self.bmp_wnoobar.is_null() { GdipDisposeImage(self.bmp_wnoobar); }
            if !self.bmp_antigravity.is_null() { GdipDisposeImage(self.bmp_antigravity); }
            if !self.bmp_claude.is_null() { GdipDisposeImage(self.bmp_claude); }
            if !self.bmp_codex.is_null() { GdipDisposeImage(self.bmp_codex); }
            if !self.bmp_deepseek.is_null() { GdipDisposeImage(self.bmp_deepseek); }
        }
    }
}
