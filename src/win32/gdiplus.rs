#![allow(dead_code)]
use std::ffi::c_void;
use std::ptr::null_mut;

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

#[repr(C)]
pub struct GdiplusStartupInput {
    pub gdiplus_version: u32,
    pub debug_event_callback: usize,
    pub suppress_background_thread: i32,
    pub suppress_external_codecs: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PointF {
    pub x: f32,
    pub y: f32,
}

pub const PIXEL_FORMAT_32BPP_PARGB: i32 = 0xE200B; // 32bpp Premultiplied ARGB
pub const SMOOTHING_MODE_ANTIALIAS: i32 = 2;
pub const TEXT_RENDERING_HINT_SYSTEM_DEFAULT: i32 = 0;
pub const TEXT_RENDERING_HINT_SINGLE_BIT_PER_PIXEL_GRID_FIT: i32 = 1;
pub const TEXT_RENDERING_HINT_SINGLE_BIT_PER_PIXEL: i32 = 2;
pub const TEXT_RENDERING_HINT_ANTIALIAS_GRID_FIT: i32 = 3; // 必须是 3 (包含像素网格拟合 Hinting，彻底消除字体模糊)
pub const TEXT_RENDERING_HINT_ANTIALIAS: i32 = 4;
pub const TEXT_RENDERING_HINT_CLEAR_TYPE_GRID_FIT: i32 = 5;
pub const INTERPOLATION_MODE_HIGH_QUALITY_BICUBIC: i32 = 7;
pub const UNIT_PIXEL: i32 = 2;
pub const FONT_STYLE_REGULAR: i32 = 0;
pub const FONT_STYLE_BOLD: i32 = 1;
pub const MATRIX_ORDER_PREPEND: i32 = 0;

pub const STRING_FORMAT_FLAGS_NO_WRAP: i32 = 0x00001000;
pub const STRING_TRIMMING_NONE: i32 = 0;
pub const STRING_TRIMMING_ELLIPSIS_CHARACTER: i32 = 3;

#[link(name = "gdiplus")]
unsafe extern "system" {
    pub fn GdiplusStartup(
        token: *mut usize,
        input: *const GdiplusStartupInput,
        output: *mut c_void,
    ) -> i32;
    pub fn GdiplusShutdown(token: usize);

    pub fn GdipCreateBitmapFromScan0(
        width: i32,
        height: i32,
        stride: i32,
        format: i32,
        scan0: *mut u8,
        bitmap: *mut *mut c_void,
    ) -> i32;
    pub fn GdipCreateBitmapFromStream(
        stream: *mut c_void,
        bitmap: *mut *mut c_void,
    ) -> i32;
    pub fn GdipDisposeImage(image: *mut c_void) -> i32;
    pub fn GdipCreateHICONFromBitmap(bitmap: *mut c_void, hicon: *mut windows_sys::Win32::UI::WindowsAndMessaging::HICON) -> i32;

    pub fn GdipGetImageGraphicsContext(image: *mut c_void, graphics: *mut *mut c_void) -> i32;
    pub fn GdipDeleteGraphics(graphics: *mut c_void) -> i32;

    pub fn GdipSetSmoothingMode(graphics: *mut c_void, mode: i32) -> i32;
    pub fn GdipSetTextRenderingHint(graphics: *mut c_void, mode: i32) -> i32;
    pub fn GdipSetTextContrast(graphics: *mut c_void, contrast: u32) -> i32;
    pub fn GdipSetInterpolationMode(graphics: *mut c_void, mode: i32) -> i32;

    pub fn GdipScaleWorldTransform(graphics: *mut c_void, sx: f32, sy: f32, order: i32) -> i32;
    pub fn GdipTranslateWorldTransform(graphics: *mut c_void, dx: f32, dy: f32, order: i32) -> i32;
    pub fn GdipResetWorldTransform(graphics: *mut c_void) -> i32;
    pub fn GdipSaveGraphics(graphics: *mut c_void, state: *mut u32) -> i32;
    pub fn GdipRestoreGraphics(graphics: *mut c_void, state: u32) -> i32;

    pub fn GdipCreateSolidFill(color: u32, brush: *mut *mut c_void) -> i32;
    pub fn GdipDeleteBrush(brush: *mut c_void) -> i32;

    pub fn GdipCreatePen1(color: u32, width: f32, unit: i32, pen: *mut *mut c_void) -> i32;
    pub fn GdipDeletePen(pen: *mut c_void) -> i32;

    pub fn GdipCreatePath(fill_mode: i32, path: *mut *mut c_void) -> i32;
    pub fn GdipDeletePath(path: *mut c_void) -> i32;
    pub fn GdipResetPath(path: *mut c_void) -> i32;
    pub fn GdipAddPathArc(
        path: *mut c_void,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        startAngle: f32,
        sweepAngle: f32,
    ) -> i32;
    pub fn GdipAddPathLine(
        path: *mut c_void,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    ) -> i32;
    pub fn GdipClosePathFigure(path: *mut c_void) -> i32;

    pub fn GdipFillPath(graphics: *mut c_void, brush: *mut c_void, path: *mut c_void) -> i32;
    pub fn GdipDrawPath(graphics: *mut c_void, pen: *mut c_void, path: *mut c_void) -> i32;

    pub fn GdipSetClipPath(graphics: *mut c_void, path: *mut c_void, combineMode: i32) -> i32;
    pub fn GdipResetClip(graphics: *mut c_void) -> i32;

    pub fn GdipFillRectangle(
        graphics: *mut c_void,
        brush: *mut c_void,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> i32;
    pub fn GdipDrawLine(
        graphics: *mut c_void,
        pen: *mut c_void,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    ) -> i32;

    pub fn GdipDrawImageRect(
        graphics: *mut c_void,
        image: *mut c_void,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> i32;

    pub fn GdipGetImageWidth(image: *mut c_void, width: *mut u32) -> i32;
    pub fn GdipGetImageHeight(image: *mut c_void, height: *mut u32) -> i32;

    pub fn GdipFillEllipse(
        graphics: *mut c_void,
        brush: *mut c_void,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> i32;
    pub fn GdipDrawEllipse(
        graphics: *mut c_void,
        pen: *mut c_void,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> i32;

    pub fn GdipCreateFontFamilyFromName(
        name: *const u16,
        fontCollection: *mut c_void,
        fontFamily: *mut *mut c_void,
    ) -> i32;
    pub fn GdipDeleteFontFamily(fontFamily: *mut c_void) -> i32;

    pub fn GdipCreateFont(
        fontFamily: *mut c_void,
        emSize: f32,
        style: i32,
        unit: i32,
        font: *mut *mut c_void,
    ) -> i32;
    pub fn GdipDeleteFont(font: *mut c_void) -> i32;

    pub fn GdipDrawString(
        graphics: *mut c_void,
        string: *const u16,
        length: i32,
        font: *mut c_void,
        layoutRect: *const RectF,
        stringFormat: *mut c_void,
        brush: *mut c_void,
    ) -> i32;

    pub fn GdipCreateLineBrush(
        point1: *const PointF,
        point2: *const PointF,
        color1: u32,
        color2: u32,
        wrapMode: i32,
        lineGradient: *mut *mut c_void,
    ) -> i32;

    pub fn GdipCreateStringFormat(
        formatAttributes: i32,
        language: u16,
        format: *mut *mut c_void,
    ) -> i32;
    pub fn GdipSetStringFormatLineAlign(format: *mut c_void, align: i32) -> i32;
    pub fn GdipSetStringFormatAlign(format: *mut c_void, align: i32) -> i32;
    pub fn GdipSetStringFormatFlags(format: *mut c_void, flags: i32) -> i32;
    pub fn GdipSetStringFormatTrimming(format: *mut c_void, trimming: i32) -> i32;
    pub fn GdipMeasureString(
        graphics: *mut c_void,
        string: *const u16,
        length: i32,
        font: *mut c_void,
        layoutRect: *const RectF,
        stringFormat: *mut c_void,
        boundingBox: *mut RectF,
        codepointsFitted: *mut i32,
        linesFilled: *mut i32,
    ) -> i32;
    pub fn GdipDeleteStringFormat(format: *mut c_void) -> i32;
}

pub struct GdiplusContext {
    token: usize,
}

impl GdiplusContext {
    pub fn init() -> Result<Self, i32> {
        let mut token: usize = 0;
        let input = GdiplusStartupInput {
            gdiplus_version: 1,
            debug_event_callback: 0,
            suppress_background_thread: 0,
            suppress_external_codecs: 0,
        };
        let status = unsafe { GdiplusStartup(&mut token, &input, null_mut()) };
        if status == 0 {
            Ok(Self { token })
        } else {
            Err(status)
        }
    }
}

impl Drop for GdiplusContext {
    fn drop(&mut self) {
        if self.token != 0 {
            unsafe { GdiplusShutdown(self.token) };
        }
    }
}

pub fn make_argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// 从内联内存字节直接创建 GDI+ 高清位图（基于 shlwapi SHCreateMemStream）
pub unsafe fn create_bitmap_from_bytes(bytes: &[u8]) -> *mut c_void {
    unsafe {
        let shlwapi_name: Vec<u16> = "shlwapi.dll\0".encode_utf16().collect();
        let shlwapi = GetModuleHandleW(shlwapi_name.as_ptr());
        let shlwapi = if shlwapi.is_null() {
            // 尝试加载
            let load_lib: unsafe extern "system" fn(*const u16) -> *mut c_void =
                std::mem::transmute(GetProcAddress(GetModuleHandleW("kernel32.dll\0".encode_utf16().collect::<Vec<u16>>().as_ptr()), b"LoadLibraryW\0".as_ptr()).unwrap());
            load_lib(shlwapi_name.as_ptr()) as _
        } else {
            shlwapi
        };

        if shlwapi.is_null() {
            return null_mut();
        }

        let func = GetProcAddress(shlwapi, b"SHCreateMemStream\0".as_ptr());
        if let Some(f) = func {
            let sh_create_mem_stream: unsafe extern "system" fn(*const u8, u32) -> *mut c_void =
                std::mem::transmute(f);
            let stream = sh_create_mem_stream(bytes.as_ptr(), bytes.len() as u32);
            if !stream.is_null() {
                let mut bitmap: *mut c_void = null_mut();
                let status = GdipCreateBitmapFromStream(stream, &mut bitmap);

                // 释放 COM stream (IUnknown::Release 位于虚表第三个指针)
                let vtable = *(stream as *mut *mut usize);
                let release_fn: unsafe extern "system" fn(*mut c_void) -> u32 =
                    std::mem::transmute(*vtable.add(2));
                release_fn(stream);

                if status == 0 {
                    return bitmap;
                }
            }
        }
        null_mut()
    }
}

/// 辅助函数：向路径添加圆角矩形
pub unsafe fn add_round_rect_to_path(
    path: *mut c_void,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
) {
    unsafe {
        let radius = radius.min(w / 2.0).min(h / 2.0);
        let dia = radius * 2.0;
        if dia <= 0.1 {
            GdipAddPathLine(path, x, y, x + w, y);
            GdipAddPathLine(path, x + w, y, x + w, y + h);
            GdipAddPathLine(path, x + w, y + h, x, y + h);
            GdipAddPathLine(path, x + y, y + h, x, y);
            GdipClosePathFigure(path);
            return;
        }

        GdipAddPathArc(path, x, y, dia, dia, 180.0, 90.0);
        GdipAddPathLine(path, x + radius, y, x + w - radius, y);
        GdipAddPathArc(path, x + w - dia, y, dia, dia, 270.0, 90.0);
        GdipAddPathLine(path, x + w, y + radius, x + w, y + h - radius);
        GdipAddPathArc(path, x + w - dia, y + h - dia, dia, dia, 0.0, 90.0);
        GdipAddPathLine(path, x + w - radius, y + h, x + radius, y + h);
        GdipAddPathArc(path, x, y + h - dia, dia, dia, 90.0, 90.0);
        GdipAddPathLine(path, x, y + h - radius, x, y + radius);
        GdipClosePathFigure(path);
    }
}
