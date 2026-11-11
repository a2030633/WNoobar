#![allow(dead_code)]
use std::ffi::c_void;
use std::ptr::null_mut;

use crate::win32::gdiplus::*;

const WNOOBAR_PNG: &[u8] = include_bytes!("../../assets/icons/wnoobar.png");
const ANTIGRAVITY_PNG: &[u8] = include_bytes!("../../assets/icons/antigravity.png");
const CLAUDE_PNG: &[u8] = include_bytes!("../../assets/icons/claude.png");
const CODEX_PNG: &[u8] = include_bytes!("../../assets/icons/codex.png");
const CURSOR_PNG: &[u8] = include_bytes!("../../assets/icons/cursor.png");
const DEEPSEEK_PNG: &[u8] = include_bytes!("../../assets/icons/deepseek.png");
const OPENCODE_PNG: &[u8] = include_bytes!("../../assets/icons/opencode.png");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentBrand {
    WNoobar,
    Antigravity,
    Claude,
    Codex,
    Cursor,
    DeepSeek,
    OpenCode,
}

impl AgentBrand {
    pub fn from_name(name: &str) -> Self {
        let lower = name.to_lowercase();
        if lower.contains("wnoobar") {
            AgentBrand::WNoobar
        } else if lower.contains("claude") {
            AgentBrand::Claude
        } else if lower.contains("cursor") {
            AgentBrand::Cursor
        } else if lower.contains("opencode") {
            AgentBrand::OpenCode
        } else if lower.contains("deepseek") {
            AgentBrand::DeepSeek
        } else if lower.contains("codex") || lower.contains("copilot") {
            AgentBrand::Codex
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
            AgentBrand::Cursor => "Cursor",
            AgentBrand::DeepSeek => "DeepSeek",
            AgentBrand::OpenCode => "OpenCode",
        }
    }
}

pub struct GlyphManager {
    bmp_wnoobar: *mut c_void,
    bmp_antigravity: *mut c_void,
    bmp_claude: *mut c_void,
    bmp_codex: *mut c_void,
    bmp_cursor: *mut c_void,
    bmp_deepseek: *mut c_void,
    bmp_opencode: *mut c_void,
}

impl GlyphManager {
    pub fn new() -> Self {
        unsafe {
            let bmp_wnoobar = create_bitmap_from_bytes(WNOOBAR_PNG);
            let bmp_antigravity = create_bitmap_from_bytes(ANTIGRAVITY_PNG);
            let bmp_claude = create_bitmap_from_bytes(CLAUDE_PNG);
            let bmp_codex = create_bitmap_from_bytes(CODEX_PNG);
            let bmp_cursor = create_bitmap_from_bytes(CURSOR_PNG);
            let bmp_deepseek = create_bitmap_from_bytes(DEEPSEEK_PNG);
            let bmp_opencode = create_bitmap_from_bytes(OPENCODE_PNG);

            Self {
                bmp_wnoobar,
                bmp_antigravity,
                bmp_claude,
                bmp_codex,
                bmp_cursor,
                bmp_deepseek,
                bmp_opencode,
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

            // 2. 外围精致微光环专属配色
            let halo_color = match brand {
                AgentBrand::WNoobar => {
                    let a = (38.0 + 16.0 * pulse_phase.sin()) as u8;
                    make_argb(a, 10, 185, 255)
                }
                AgentBrand::Antigravity => {
                    let a = (35.0 + 15.0 * pulse_phase.sin()) as u8;
                    make_argb(a, 100, 150, 255)
                }
                AgentBrand::Claude => make_argb(35, 217, 119, 87),    // 赤陶橙红
                AgentBrand::Codex => make_argb(35, 16, 163, 127),
                AgentBrand::Cursor => make_argb(35, 99, 102, 241),    // 赛博幻紫
                AgentBrand::DeepSeek => make_argb(35, 30, 111, 255),  // 深海深蓝
                AgentBrand::OpenCode => make_argb(35, 79, 70, 229),   // 极客靛蓝
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
                AgentBrand::Cursor => self.bmp_cursor,
                AgentBrand::DeepSeek => self.bmp_deepseek,
                AgentBrand::OpenCode => self.bmp_opencode,
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
                AgentBrand::Cursor => make_argb(255, 99, 102, 241),
                AgentBrand::DeepSeek => make_argb(255, 30, 111, 255),
                AgentBrand::OpenCode => make_argb(255, 79, 70, 229),
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
            if !self.bmp_cursor.is_null() { GdipDisposeImage(self.bmp_cursor); }
            if !self.bmp_deepseek.is_null() { GdipDisposeImage(self.bmp_deepseek); }
            if !self.bmp_opencode.is_null() { GdipDisposeImage(self.bmp_opencode); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_brand_mapping() {
        assert_eq!(AgentBrand::from_name("Cursor"), AgentBrand::Cursor);
        assert_eq!(AgentBrand::from_name("opencode-cli"), AgentBrand::OpenCode);
        assert_eq!(AgentBrand::from_name("claude-code"), AgentBrand::Claude);
        assert_eq!(AgentBrand::from_name("DeepSeek-Coder"), AgentBrand::DeepSeek);
        assert_eq!(AgentBrand::from_name("Codex"), AgentBrand::Codex);
        assert_eq!(AgentBrand::from_name("WNoobar"), AgentBrand::WNoobar);
        assert_eq!(AgentBrand::from_name("Antigravity"), AgentBrand::Antigravity);

        assert_eq!(AgentBrand::Cursor.display_name(), "Cursor");
        assert_eq!(AgentBrand::OpenCode.display_name(), "OpenCode");
        assert_eq!(AgentBrand::Claude.display_name(), "Claude Code");
        assert_eq!(AgentBrand::DeepSeek.display_name(), "DeepSeek");
    }
}
