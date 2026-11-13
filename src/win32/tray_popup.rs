#![allow(dead_code)]

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::null_mut;
use std::sync::{Once, OnceLock};
use std::time::Instant;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW,
    MonitorFromPoint, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP, HDC, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST, RGBQUAD,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetSystemMetrics, KillTimer, LoadCursorW,
    PostMessageW, RegisterClassExW, SetForegroundWindow, SetTimer, SetWindowPos, ShowWindow,
    UpdateLayeredWindow, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, HWND_TOPMOST, IDC_ARROW,
    MA_NOACTIVATE, SM_CXSCREEN, SM_CYSCREEN, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOW,
    ULW_ALPHA, WA_INACTIVE, WM_ACTIVATE, WM_CANCELMODE, WM_CLOSE, WM_COMMAND, WM_DESTROY,
    WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE, WM_TIMER, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::core::bus::NoiseFilterLevel;
use crate::core::config::MonitorMode;
use crate::ui::tray_menu::{
    build_main_rows, build_submenu_rows, hit_test_row, measure_panel_size, row_top_y, MenuCommand,
    MenuRow, MenuRowKind, SubmenuKind, TrayMenuRenderer, TrayMenuSnapshot, CARD_PAD_Y, MAIN_CARD_W,
    SHADOW_PAD, SUBMENU_CARD_W,
};
use crate::win32::gdiplus::{
    GdipCreateBitmapFromScan0, GdipDeleteGraphics, GdipDisposeImage, GdipGetImageGraphicsContext,
    GdipScaleWorldTransform, GdipSetSmoothingMode, GdipSetTextRenderingHint, MATRIX_ORDER_PREPEND,
    PIXEL_FORMAT_32BPP_PARGB, SMOOTHING_MODE_ANTIALIAS, TEXT_RENDERING_HINT_ANTIALIAS_GRID_FIT,
};
use crate::win32::system::{encode_wide, resolve_proc};
use crate::win32::window::{
    AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, IDM_MONITOR_FOLLOW, IDM_MONITOR_PRIMARY,
    IDM_NOISE_NORMAL, IDM_NOISE_SILENT, IDM_NOISE_VERBOSE, IDM_TOGGLE_AUTOSTART, IDM_TOGGLE_DND,
    IDM_TOGGLE_SOUND, IDM_TRAY_DECK, IDM_TRAY_EXIT, IDM_TRAY_RESET, IDM_TRAY_TOGGLE, WM_MOUSELEAVE,
};

const TRAY_MAIN_CLASS_NAME: &str = "WNoobarTrayMenuMainClass";
const TRAY_SUB_CLASS_NAME: &str = "WNoobarTrayMenuSubClass";

const TIMER_FADE_IN: usize = 101;
const TIMER_SUBMENU_CLOSE: usize = 102;
const FADE_DURATION_SECS: f32 = 0.075;
const FADE_TICK_MS: u32 = 15;
const FADE_START_ALPHA: u8 = 175;
const SUBMENU_GRACE_MS: u32 = 150;
const SUBMENU_VISUAL_GAP_PX: i32 = 4;
const MDT_EFFECTIVE_DPI: i32 = 0;
const VK_ESCAPE: WPARAM = 0x1B;
const TME_LEAVE: u32 = 0x0000_0002;

fn compute_fade_source_alpha(elapsed_secs: f32) -> (u8, bool) {
    let t = (elapsed_secs / FADE_DURATION_SECS).clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    let alpha = (FADE_START_ALPHA as f32 + (255.0 - FADE_START_ALPHA as f32) * eased)
        .round()
        .clamp(FADE_START_ALPHA as f32, 255.0) as u8;
    if t >= 1.0 {
        (255, true)
    } else {
        (alpha, false)
    }
}

#[allow(non_snake_case)]
#[repr(C)]
struct TRACKMOUSEEVENT {
    cbSize: u32,
    dwFlags: u32,
    hwndTrack: HWND,
    dwHoverTime: u32,
}

unsafe extern "system" {
    fn GetWindowLongPtrW(hWnd: HWND, nIndex: i32) -> isize;
    fn SetWindowLongPtrW(hWnd: HWND, nIndex: i32, dwNewLong: isize) -> isize;
    fn SetFocus(hWnd: HWND) -> HWND;
    fn TrackMouseEvent(lpEventTrack: *mut TRACKMOUSEEVENT) -> i32;
}

static REGISTER_CLASSES_ONCE: Once = Once::new();

/// 将菜单语义指令映射为 `window.rs` 中 `WM_COMMAND` 使用的唯一 `IDM_*` 标识符。
pub const fn menu_command_id(cmd: MenuCommand) -> usize {
    match cmd {
        MenuCommand::ToggleIsland => IDM_TRAY_TOGGLE,
        MenuCommand::OpenDeck => IDM_TRAY_DECK,
        MenuCommand::SetNoise(NoiseFilterLevel::Silent) => IDM_NOISE_SILENT,
        MenuCommand::SetNoise(NoiseFilterLevel::Normal) => IDM_NOISE_NORMAL,
        MenuCommand::SetNoise(NoiseFilterLevel::Verbose) => IDM_NOISE_VERBOSE,
        MenuCommand::SetMonitor(MonitorMode::Primary) => IDM_MONITOR_PRIMARY,
        MenuCommand::SetMonitor(MonitorMode::FollowCursor) => IDM_MONITOR_FOLLOW,
        MenuCommand::ToggleDnd => IDM_TOGGLE_DND,
        MenuCommand::ToggleSound => IDM_TOGGLE_SOUND,
        MenuCommand::ToggleAutostart => IDM_TOGGLE_AUTOSTART,
        MenuCommand::ResetIdle => IDM_TRAY_RESET,
        MenuCommand::Exit => IDM_TRAY_EXIT,
    }
}

/// 纯函数：计算主菜单分层窗口在物理屏幕坐标中的左上角原点 `(x, y)`。
/// 优先贴近光标向上弹出；若上方工作区空间不足（如顶部任务栏），自动切换向下弹出；
/// 最终严格钳位在显示器工作区 `work_rect = (left, top, right, bottom)` 之内。
pub fn compute_main_menu_origin(
    cursor_x: i32,
    cursor_y: i32,
    phys_w: i32,
    phys_h: i32,
    work_rect: (i32, i32, i32, i32),
    shadow_pad_px: i32,
) -> (i32, i32) {
    let wl = work_rect.0 as i64;
    let wt = work_rect.1 as i64;
    let wr = work_rect.2 as i64;
    let wb = work_rect.3 as i64;
    let w = (phys_w.max(1)) as i64;
    let h = (phys_h.max(1)) as i64;
    let pad = (shadow_pad_px.max(0)) as i64;
    let cx = cursor_x as i64;
    let cy = cursor_y as i64;

    let max_x = (wr - w).max(wl);
    let max_y = (wb - h).max(wt);

    let x = (cx - pad).clamp(wl, max_x);

    // 卡片底边贴近光标：up_y + h - pad = cy => up_y = cy - (h - pad)
    let up_y = cy - (h - pad);
    let raw_y = if up_y >= wt {
        up_y
    } else {
        // 上方空间不足：向下弹出，使卡片顶边贴近光标
        cy - pad
    };
    let y = raw_y.clamp(wt, max_y);

    (x as i32, y as i32)
}

/// 纯函数：计算子菜单分层窗口在物理屏幕坐标中的左上角原点 `(x, y)`。
/// 默认紧贴主菜单卡片右侧展开（重叠阴影边距后保留 ~4px 视觉间隙）；
/// 若右侧超出 `work_rect.2`，自动向左翻转展开；垂直方向首行与父行对齐并钳位在工作区内。
pub fn compute_submenu_origin(
    main_x: i32,
    main_y: i32,
    main_phys_w: i32,
    row_top_px: i32,
    card_pad_y_px: i32,
    sub_phys_w: i32,
    sub_phys_h: i32,
    work_rect: (i32, i32, i32, i32),
    shadow_pad_px: i32,
) -> (i32, i32) {
    let wl = work_rect.0 as i64;
    let wt = work_rect.1 as i64;
    let wr = work_rect.2 as i64;
    let wb = work_rect.3 as i64;
    let mx = main_x as i64;
    let my = main_y as i64;
    let mw = (main_phys_w.max(1)) as i64;
    let sw = (sub_phys_w.max(1)) as i64;
    let sh = (sub_phys_h.max(1)) as i64;
    let pad = (shadow_pad_px.max(0)) as i64;
    let gap = SUBMENU_VISUAL_GAP_PX as i64;

    // 主菜单可见卡片右边界：mx + mw - pad；子菜单可见卡片左边界：sub_x + pad
    let right_x = mx + mw - 2 * pad + gap;
    // 主菜单可见卡片左边界：mx + pad；子菜单可见卡片右边界：sub_x + sw - pad
    let left_x = mx + 2 * pad - sw - gap;

    let max_x = (wr - sw).max(wl);
    let max_y = (wb - sh).max(wt);

    let raw_x = if right_x + sw <= wr {
        right_x
    } else if left_x >= wl {
        left_x
    } else {
        let space_right = wr - (mx + mw - pad);
        let space_left = (mx + pad) - wl;
        if space_right >= space_left {
            right_x
        } else {
            left_x
        }
    };
    let x = raw_x.clamp(wl, max_x);

    // 子菜单首行顶边 (sub_y + pad + card_pad_y_px) 对齐主菜单父行顶边 (my + row_top_px)
    let aligned_y = my + (row_top_px as i64) - pad - (card_pad_y_px as i64);
    let y = aligned_y.clamp(wt, max_y);

    (x as i32, y as i32)
}

type GetDpiForMonitorFn =
    unsafe extern "system" fn(HMONITOR, i32, *mut u32, *mut u32) -> i32;
type GetDpiForSystemFn = unsafe extern "system" fn() -> u32;

fn query_monitor_dpi_scale(hmon: HMONITOR) -> f32 {
    static GET_DPI_FOR_MONITOR: OnceLock<Option<GetDpiForMonitorFn>> = OnceLock::new();
    static GET_DPI_FOR_SYSTEM: OnceLock<Option<GetDpiForSystemFn>> = OnceLock::new();

    if !hmon.is_null() {
        let func = *GET_DPI_FOR_MONITOR.get_or_init(|| unsafe {
            resolve_proc("shcore.dll", b"GetDpiForMonitor\0").map(|f| std::mem::transmute(f))
        });
        if let Some(get_dpi_for_monitor) = func {
            let mut dpi_x: u32 = 0;
            let mut dpi_y: u32 = 0;
            let hr =
                unsafe { get_dpi_for_monitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
            if hr == 0 && dpi_x > 0 {
                return dpi_x as f32 / 96.0;
            }
        }
    }

    let sys_fn = *GET_DPI_FOR_SYSTEM.get_or_init(|| unsafe {
        resolve_proc("user32.dll", b"GetDpiForSystem\0").map(|f| std::mem::transmute(f))
    });
    if let Some(get_dpi_for_system) = sys_fn {
        let dpi = unsafe { get_dpi_for_system() };
        if dpi > 0 {
            return dpi as f32 / 96.0;
        }
    }

    1.0
}

unsafe fn query_monitor_work_and_dpi(pt_x: i32, pt_y: i32) -> ((i32, i32, i32, i32), f32) {
    unsafe {
        let hmon = MonitorFromPoint(POINT { x: pt_x, y: pt_y }, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if !hmon.is_null() && GetMonitorInfoW(hmon, &mut mi) != 0 {
            let work_rect = (
                mi.rcWork.left,
                mi.rcWork.top,
                mi.rcWork.right,
                mi.rcWork.bottom,
            );
            let dpi_scale = query_monitor_dpi_scale(hmon).max(0.5);
            return (work_rect, dpi_scale);
        }

        let screen_w = GetSystemMetrics(SM_CXSCREEN).max(1);
        let screen_h = GetSystemMetrics(SM_CYSCREEN).max(1);
        let dpi_scale = query_monitor_dpi_scale(null_mut()).max(0.5);
        ((0, 0, screen_w, screen_h), dpi_scale)
    }
}

struct PopupPanelSurface {
    phys_w: i32,
    phys_h: i32,
    dpi_scale: f32,
    mem_dc: HDC,
    dib_bitmap: HBITMAP,
    bits_ptr: *mut u8,
    gp_bitmap: *mut c_void,
    gp_graphics: *mut c_void,
}

impl PopupPanelSurface {
    fn empty() -> Self {
        Self {
            phys_w: 0,
            phys_h: 0,
            dpi_scale: 1.0,
            mem_dc: 0 as _,
            dib_bitmap: 0 as _,
            bits_ptr: null_mut(),
            gp_bitmap: null_mut(),
            gp_graphics: null_mut(),
        }
    }

    fn is_valid(&self) -> bool {
        !self.mem_dc.is_null()
            && !self.dib_bitmap.is_null()
            && !self.bits_ptr.is_null()
            && !self.gp_bitmap.is_null()
            && !self.gp_graphics.is_null()
    }

    unsafe fn ensure_size(&mut self, hwnd: HWND, phys_w: i32, phys_h: i32, dpi_scale: f32) -> bool {
        let safe_w = phys_w.max(1);
        let safe_h = phys_h.max(1);
        let safe_scale = if dpi_scale > 0.1 { dpi_scale } else { 1.0 };

        if self.is_valid()
            && self.phys_w == safe_w
            && self.phys_h == safe_h
            && (self.dpi_scale - safe_scale).abs() <= 0.01
        {
            return true;
        }

        unsafe {
            self.release();

            let screen_dc = GetDC(hwnd);
            let mem_dc = CreateCompatibleDC(screen_dc);
            if !screen_dc.is_null() {
                ReleaseDC(hwnd, screen_dc);
            }
            if mem_dc.is_null() {
                return false;
            }

            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: safe_w,
                    biHeight: -safe_h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    biSizeImage: (safe_w * safe_h * 4) as u32,
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
            if dib_bitmap.is_null() || bits_ptr.is_null() {
                DeleteDC(mem_dc);
                if !dib_bitmap.is_null() {
                    DeleteObject(dib_bitmap as _);
                }
                return false;
            }

            SelectObject(mem_dc, dib_bitmap as _);

            let mut gp_bitmap: *mut c_void = null_mut();
            let bmp_status = GdipCreateBitmapFromScan0(
                safe_w,
                safe_h,
                safe_w * 4,
                PIXEL_FORMAT_32BPP_PARGB,
                bits_ptr as *mut u8,
                &mut gp_bitmap,
            );
            if bmp_status != 0 || gp_bitmap.is_null() {
                DeleteDC(mem_dc);
                DeleteObject(dib_bitmap as _);
                return false;
            }

            let mut gp_graphics: *mut c_void = null_mut();
            let gfx_status = GdipGetImageGraphicsContext(gp_bitmap, &mut gp_graphics);
            if gfx_status != 0 || gp_graphics.is_null() {
                GdipDisposeImage(gp_bitmap);
                DeleteDC(mem_dc);
                DeleteObject(dib_bitmap as _);
                return false;
            }

            GdipSetSmoothingMode(gp_graphics, SMOOTHING_MODE_ANTIALIAS);
            GdipSetTextRenderingHint(gp_graphics, TEXT_RENDERING_HINT_ANTIALIAS_GRID_FIT);
            GdipScaleWorldTransform(gp_graphics, safe_scale, safe_scale, MATRIX_ORDER_PREPEND);

            self.phys_w = safe_w;
            self.phys_h = safe_h;
            self.dpi_scale = safe_scale;
            self.mem_dc = mem_dc;
            self.dib_bitmap = dib_bitmap;
            self.bits_ptr = bits_ptr as *mut u8;
            self.gp_bitmap = gp_bitmap;
            self.gp_graphics = gp_graphics;
            true
        }
    }

    unsafe fn release(&mut self) {
        unsafe {
            if !self.gp_graphics.is_null() {
                GdipDeleteGraphics(self.gp_graphics);
                self.gp_graphics = null_mut();
            }
            if !self.gp_bitmap.is_null() {
                GdipDisposeImage(self.gp_bitmap);
                self.gp_bitmap = null_mut();
            }
            if !self.mem_dc.is_null() {
                DeleteDC(self.mem_dc);
                self.mem_dc = 0 as _;
            }
            if !self.dib_bitmap.is_null() {
                DeleteObject(self.dib_bitmap as _);
                self.dib_bitmap = 0 as _;
            }
            self.bits_ptr = null_mut();
            self.phys_w = 0;
            self.phys_h = 0;
        }
    }
}

impl Drop for PopupPanelSurface {
    fn drop(&mut self) {
        unsafe {
            self.release();
        }
    }
}

struct TrayPopupState {
    island_hwnd: HWND,
    main_hwnd: HWND,
    sub_hwnd: HWND,
    renderer: TrayMenuRenderer,
    main_surface: PopupPanelSurface,
    sub_surface: PopupPanelSurface,
    is_open: bool,
    is_opening: bool,
    destroyed: bool,
    snapshot: Option<TrayMenuSnapshot>,
    main_rows: Vec<MenuRow>,
    sub_rows: Vec<MenuRow>,
    main_hovered: Option<usize>,
    sub_hovered: Option<usize>,
    open_submenu: Option<(usize, SubmenuKind)>,
    main_tracking_leave: bool,
    sub_tracking_leave: bool,
    submenu_close_pending: bool,
    main_origin: (i32, i32),
    main_phys_size: (i32, i32),
    work_rect: (i32, i32, i32, i32),
    dpi_scale: f32,
    fade_start: Instant,
    source_alpha: u8,
}

impl TrayPopupState {
    unsafe fn draw_surface_to_dib(
        renderer: &mut TrayMenuRenderer,
        surface: &PopupPanelSurface,
        dpi_scale: f32,
        rows: &[MenuRow],
        card_w: f32,
        hovered: Option<usize>,
        open_sub_row: Option<usize>,
    ) -> (HDC, i32, i32) {
        unsafe {
            let total_bytes = (surface.phys_w * surface.phys_h * 4) as usize;
            std::ptr::write_bytes(surface.bits_ptr, 0, total_bytes);
            renderer.render_panel_to_dib(
                surface.gp_graphics,
                surface.mem_dc,
                surface.bits_ptr,
                surface.phys_w,
                surface.phys_h,
                dpi_scale,
                rows,
                card_w,
                hovered,
                open_sub_row,
                1.0,
            );
            (surface.mem_dc, surface.phys_w, surface.phys_h)
        }
    }

    unsafe fn render_main_raw(state_ptr: *mut TrayPopupState, pt_dst: Option<(i32, i32)>) {
        if state_ptr.is_null() {
            return;
        }
        unsafe {
            let hwnd = (*state_ptr).main_hwnd;
            if hwnd.is_null() || !(*state_ptr).main_surface.is_valid() {
                return;
            }
            let (mem_dc, phys_w, phys_h, source_alpha) = {
                let s = &mut *state_ptr;
                let open_sub_row = s.open_submenu.map(|(idx, _)| idx);
                let (dc, w, h) = Self::draw_surface_to_dib(
                    &mut s.renderer,
                    &s.main_surface,
                    s.dpi_scale,
                    &s.main_rows,
                    MAIN_CARD_W,
                    s.main_hovered,
                    open_sub_row,
                );
                (dc, w, h, s.source_alpha)
            };
            commit_layered_window(hwnd, mem_dc, phys_w, phys_h, pt_dst, source_alpha);
        }
    }

    unsafe fn render_sub_raw(state_ptr: *mut TrayPopupState, pt_dst: Option<(i32, i32)>) {
        if state_ptr.is_null() {
            return;
        }
        unsafe {
            let hwnd = (*state_ptr).sub_hwnd;
            if hwnd.is_null()
                || !(*state_ptr).sub_surface.is_valid()
                || (*state_ptr).sub_rows.is_empty()
            {
                return;
            }
            let (mem_dc, phys_w, phys_h, source_alpha) = {
                let s = &mut *state_ptr;
                let (dc, w, h) = Self::draw_surface_to_dib(
                    &mut s.renderer,
                    &s.sub_surface,
                    s.dpi_scale,
                    &s.sub_rows,
                    SUBMENU_CARD_W,
                    s.sub_hovered,
                    None,
                );
                (dc, w, h, s.source_alpha)
            };
            commit_layered_window(hwnd, mem_dc, phys_w, phys_h, pt_dst, source_alpha);
        }
    }

    unsafe fn cancel_submenu_close_timer_raw(state_ptr: *mut TrayPopupState) {
        if state_ptr.is_null() {
            return;
        }
        unsafe {
            if (*state_ptr).submenu_close_pending {
                (*state_ptr).submenu_close_pending = false;
                let main_hwnd = (*state_ptr).main_hwnd;
                if !main_hwnd.is_null() {
                    KillTimer(main_hwnd, TIMER_SUBMENU_CLOSE);
                }
            }
        }
    }

    unsafe fn schedule_submenu_close_timer_raw(state_ptr: *mut TrayPopupState) {
        if state_ptr.is_null() {
            return;
        }
        unsafe {
            if (*state_ptr).open_submenu.is_some() && !(*state_ptr).submenu_close_pending {
                let main_hwnd = (*state_ptr).main_hwnd;
                if !main_hwnd.is_null() {
                    (*state_ptr).submenu_close_pending = true;
                    SetTimer(main_hwnd, TIMER_SUBMENU_CLOSE, SUBMENU_GRACE_MS, None);
                }
            }
        }
    }

    unsafe fn open_submenu_for_raw(
        state_ptr: *mut TrayPopupState,
        row_idx: usize,
        kind: SubmenuKind,
    ) {
        if state_ptr.is_null() {
            return;
        }
        unsafe {
            Self::cancel_submenu_close_timer_raw(state_ptr);
            if (*state_ptr).open_submenu == Some((row_idx, kind)) {
                return;
            }

            let sub_hwnd = (*state_ptr).sub_hwnd;
            if sub_hwnd.is_null() {
                return;
            }

            let (sub_rows, sub_phys_w, sub_phys_h, dpi_scale, sub_x, sub_y) = {
                let s = &*state_ptr;
                let Some(snap) = s.snapshot.as_ref() else {
                    return;
                };
                let rows = build_submenu_rows(kind, snap);
                let (sub_canvas_w, sub_canvas_h) = measure_panel_size(&rows, SUBMENU_CARD_W);
                let phys_w = (sub_canvas_w * s.dpi_scale).round().max(1.0) as i32;
                let phys_h = (sub_canvas_h * s.dpi_scale).round().max(1.0) as i32;
                let row_top_px = (row_top_y(&s.main_rows, row_idx) * s.dpi_scale).round() as i32;
                let card_pad_y_px = (CARD_PAD_Y * s.dpi_scale).round() as i32;
                let shadow_pad_px = (SHADOW_PAD * s.dpi_scale).round() as i32;
                let (sx, sy) = compute_submenu_origin(
                    s.main_origin.0,
                    s.main_origin.1,
                    s.main_phys_size.0,
                    row_top_px,
                    card_pad_y_px,
                    phys_w,
                    phys_h,
                    s.work_rect,
                    shadow_pad_px,
                );
                (rows, phys_w, phys_h, s.dpi_scale, sx, sy)
            };

            if !(*state_ptr)
                .sub_surface
                .ensure_size(sub_hwnd, sub_phys_w, sub_phys_h, dpi_scale)
            {
                return;
            }

            (*state_ptr).sub_rows = sub_rows;
            (*state_ptr).sub_hovered = None;
            (*state_ptr).sub_tracking_leave = false;
            (*state_ptr).open_submenu = Some((row_idx, kind));

            Self::render_sub_raw(state_ptr, Some((sub_x, sub_y)));
            SetWindowPos(
                sub_hwnd,
                HWND_TOPMOST,
                sub_x,
                sub_y,
                sub_phys_w,
                sub_phys_h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            if (*state_ptr).is_open {
                Self::render_main_raw(state_ptr, None);
            }
        }
    }

    unsafe fn hide_submenu_only_raw(state_ptr: *mut TrayPopupState) {
        if state_ptr.is_null() {
            return;
        }
        unsafe {
            Self::cancel_submenu_close_timer_raw(state_ptr);
            if (*state_ptr).open_submenu.is_some() {
                (*state_ptr).open_submenu = None;
                (*state_ptr).sub_hovered = None;
                (*state_ptr).sub_tracking_leave = false;
                (*state_ptr).sub_rows.clear();

                let sub_hwnd = (*state_ptr).sub_hwnd;
                if !sub_hwnd.is_null() {
                    ShowWindow(sub_hwnd, SW_HIDE);
                }
                if (*state_ptr).is_open {
                    Self::render_main_raw(state_ptr, None);
                }
            }
        }
    }

    unsafe fn close_all_raw(state_ptr: *mut TrayPopupState) {
        if state_ptr.is_null() {
            return;
        }
        unsafe {
            if (*state_ptr).destroyed {
                return;
            }
            (*state_ptr).is_open = false;
            (*state_ptr).is_opening = false;
            (*state_ptr).main_hovered = None;
            (*state_ptr).sub_hovered = None;
            (*state_ptr).open_submenu = None;
            (*state_ptr).main_tracking_leave = false;
            (*state_ptr).sub_tracking_leave = false;
            (*state_ptr).submenu_close_pending = false;
            (*state_ptr).sub_rows.clear();

            let main_hwnd = (*state_ptr).main_hwnd;
            let sub_hwnd = (*state_ptr).sub_hwnd;

            if !main_hwnd.is_null() {
                KillTimer(main_hwnd, TIMER_FADE_IN);
                KillTimer(main_hwnd, TIMER_SUBMENU_CLOSE);
            }
            if !sub_hwnd.is_null() {
                ShowWindow(sub_hwnd, SW_HIDE);
            }
            if !main_hwnd.is_null() {
                ShowWindow(main_hwnd, SW_HIDE);
            }
        }
    }

    unsafe fn open_at_raw(
        state_ptr: *mut TrayPopupState,
        pt_x: i32,
        pt_y: i32,
        snap: TrayMenuSnapshot,
    ) {
        if state_ptr.is_null() {
            return;
        }
        unsafe {
            if (*state_ptr).destroyed
                || (*state_ptr).main_hwnd.is_null()
                || (*state_ptr).sub_hwnd.is_null()
            {
                return;
            }

            Self::close_all_raw(state_ptr);

            let (work_rect, dpi_scale) = query_monitor_work_and_dpi(pt_x, pt_y);
            let main_rows = build_main_rows(&snap);
            let (canvas_w, canvas_h) = measure_panel_size(&main_rows, MAIN_CARD_W);
            let main_phys_w = (canvas_w * dpi_scale).round().max(1.0) as i32;
            let main_phys_h = (canvas_h * dpi_scale).round().max(1.0) as i32;
            let shadow_pad_px = (SHADOW_PAD * dpi_scale).round() as i32;

            let main_hwnd = (*state_ptr).main_hwnd;
            if !(*state_ptr)
                .main_surface
                .ensure_size(main_hwnd, main_phys_w, main_phys_h, dpi_scale)
            {
                return;
            }

            let (main_x, main_y) = compute_main_menu_origin(
                pt_x,
                pt_y,
                main_phys_w,
                main_phys_h,
                work_rect,
                shadow_pad_px,
            );

            (*state_ptr).work_rect = work_rect;
            (*state_ptr).dpi_scale = dpi_scale;
            (*state_ptr).main_origin = (main_x, main_y);
            (*state_ptr).main_phys_size = (main_phys_w, main_phys_h);
            (*state_ptr).snapshot = Some(snap);
            (*state_ptr).main_rows = main_rows;
            (*state_ptr).fade_start = Instant::now();
            (*state_ptr).source_alpha = FADE_START_ALPHA;

            Self::render_main_raw(state_ptr, Some((main_x, main_y)));

            (*state_ptr).is_opening = true;
            (*state_ptr).is_open = true;

            SetWindowPos(
                main_hwnd,
                HWND_TOPMOST,
                main_x,
                main_y,
                main_phys_w,
                main_phys_h,
                SWP_SHOWWINDOW,
            );
            ShowWindow(main_hwnd, SW_SHOW);
            SetForegroundWindow(main_hwnd);
            SetFocus(main_hwnd);

            (*state_ptr).is_opening = false;

            if (*state_ptr).is_open && !(*state_ptr).main_hwnd.is_null() {
                SetTimer(main_hwnd, TIMER_FADE_IN, FADE_TICK_MS, None);
            }
        }
    }

    unsafe fn destroy_raw(state_ptr: *mut TrayPopupState) {
        if state_ptr.is_null() {
            return;
        }
        unsafe {
            if (*state_ptr).destroyed {
                return;
            }
            Self::close_all_raw(state_ptr);
            (*state_ptr).destroyed = true;

            let sub_hwnd = (*state_ptr).sub_hwnd;
            let main_hwnd = (*state_ptr).main_hwnd;
            (*state_ptr).sub_hwnd = 0 as _;
            (*state_ptr).main_hwnd = 0 as _;

            if !sub_hwnd.is_null() {
                SetWindowLongPtrW(sub_hwnd, GWLP_USERDATA, 0);
                DestroyWindow(sub_hwnd);
            }
            if !main_hwnd.is_null() {
                SetWindowLongPtrW(main_hwnd, GWLP_USERDATA, 0);
                DestroyWindow(main_hwnd);
            }

            (*state_ptr).sub_surface.release();
            (*state_ptr).main_surface.release();
        }
    }
}

/// 托盘右键自绘双分层弹窗宿主（主菜单顶层窗 + 无焦点右侧子菜单浮层）。
pub struct TrayPopupHost {
    inner: *mut TrayPopupState,
}

impl TrayPopupHost {
    pub fn new(island_hwnd: HWND) -> Result<Self, String> {
        unsafe {
            register_popup_classes();

            let instance = GetModuleHandleW(null_mut());
            let main_class = encode_wide(TRAY_MAIN_CLASS_NAME);
            let sub_class = encode_wide(TRAY_SUB_CLASS_NAME);
            let empty_title = encode_wide("");

            let main_hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                main_class.as_ptr(),
                empty_title.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                0 as _,
                0 as _,
                instance,
                null_mut(),
            );
            if main_hwnd.is_null() {
                let err = windows_sys::Win32::Foundation::GetLastError();
                return Err(format!("Failed to create tray main menu window, err={err}"));
            }

            let sub_hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
                sub_class.as_ptr(),
                empty_title.as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                0 as _,
                0 as _,
                instance,
                null_mut(),
            );
            if sub_hwnd.is_null() {
                let err = windows_sys::Win32::Foundation::GetLastError();
                DestroyWindow(main_hwnd);
                return Err(format!("Failed to create tray submenu window, err={err}"));
            }

            let state_ptr = Box::into_raw(Box::new(TrayPopupState {
                island_hwnd,
                main_hwnd,
                sub_hwnd,
                renderer: TrayMenuRenderer::new(),
                main_surface: PopupPanelSurface::empty(),
                sub_surface: PopupPanelSurface::empty(),
                is_open: false,
                is_opening: false,
                destroyed: false,
                snapshot: None,
                main_rows: Vec::new(),
                sub_rows: Vec::new(),
                main_hovered: None,
                sub_hovered: None,
                open_submenu: None,
                main_tracking_leave: false,
                sub_tracking_leave: false,
                submenu_close_pending: false,
                main_origin: (0, 0),
                main_phys_size: (0, 0),
                work_rect: (0, 0, 1920, 1080),
                dpi_scale: 1.0,
                fade_start: Instant::now(),
                source_alpha: 255,
            }));

            SetWindowLongPtrW(main_hwnd, GWLP_USERDATA, state_ptr as isize);
            SetWindowLongPtrW(sub_hwnd, GWLP_USERDATA, state_ptr as isize);

            Ok(Self { inner: state_ptr })
        }
    }

    pub fn open_at(&mut self, pt_x: i32, pt_y: i32, snap: TrayMenuSnapshot) {
        if !self.inner.is_null() {
            unsafe {
                TrayPopupState::open_at_raw(self.inner, pt_x, pt_y, snap);
            }
        }
    }

    pub fn close_all(&mut self) {
        if !self.inner.is_null() {
            unsafe {
                TrayPopupState::close_all_raw(self.inner);
            }
        }
    }

    pub fn is_visible(&self) -> bool {
        if self.inner.is_null() {
            return false;
        }
        unsafe { (*self.inner).is_open && !(*self.inner).destroyed }
    }

    pub fn destroy(&mut self) {
        if !self.inner.is_null() {
            unsafe {
                TrayPopupState::destroy_raw(self.inner);
            }
        }
    }
}

impl Drop for TrayPopupHost {
    fn drop(&mut self) {
        self.destroy();
        if !self.inner.is_null() {
            unsafe {
                let _ = Box::from_raw(self.inner);
            }
            self.inner = null_mut();
        }
    }
}

unsafe fn register_popup_classes() {
    REGISTER_CLASSES_ONCE.call_once(|| unsafe {
        let instance = GetModuleHandleW(null_mut());
        let cursor = LoadCursorW(0 as _, IDC_ARROW);

        let main_class = encode_wide(TRAY_MAIN_CLASS_NAME);
        let wc_main = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(main_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: 0 as _,
            hCursor: cursor,
            hbrBackground: 0 as _,
            lpszMenuName: null_mut(),
            lpszClassName: main_class.as_ptr(),
            hIconSm: 0 as _,
        };
        RegisterClassExW(&wc_main);

        let sub_class = encode_wide(TRAY_SUB_CLASS_NAME);
        let wc_sub = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(sub_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: 0 as _,
            hCursor: cursor,
            hbrBackground: 0 as _,
            lpszMenuName: null_mut(),
            lpszClassName: sub_class.as_ptr(),
            hIconSm: 0 as _,
        };
        RegisterClassExW(&wc_sub);
    });
}

unsafe fn track_mouse_leave(hwnd: HWND) {
    unsafe {
        let mut tme = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        TrackMouseEvent(&mut tme);
    }
}

unsafe fn commit_layered_window(
    hwnd: HWND,
    mem_dc: HDC,
    phys_w: i32,
    phys_h: i32,
    pt_dst: Option<(i32, i32)>,
    source_alpha: u8,
) {
    unsafe {
        let screen_dc = GetDC(0 as _);
        let mut pt_src = POINT { x: 0, y: 0 };
        let mut size_wnd = SIZE {
            cx: phys_w,
            cy: phys_h,
        };
        let mut blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER,
            BlendFlags: 0,
            SourceConstantAlpha: source_alpha,
            AlphaFormat: AC_SRC_ALPHA,
        };
        let mut dst_pt = POINT {
            x: pt_dst.map_or(0, |p| p.0),
            y: pt_dst.map_or(0, |p| p.1),
        };
        let dst_ptr = if pt_dst.is_some() {
            &mut dst_pt as *mut POINT
        } else {
            null_mut()
        };

        UpdateLayeredWindow(
            hwnd,
            screen_dc,
            dst_ptr,
            &mut size_wnd,
            mem_dc,
            &mut pt_src,
            0,
            &mut blend as *mut _ as _,
            ULW_ALPHA,
        );

        if !screen_dc.is_null() {
            ReleaseDC(0 as _, screen_dc);
        }
    }
}

#[inline]
fn logical_coords_from_lparam(lparam: LPARAM, dpi_scale: f32) -> (f32, f32) {
    let phys_x = (lparam & 0xFFFF) as i16 as f32;
    let phys_y = ((lparam >> 16) & 0xFFFF) as i16 as f32;
    let safe_scale = if dpi_scale > 0.1 { dpi_scale } else { 1.0 };
    (phys_x / safe_scale, phys_y / safe_scale)
}

unsafe fn dispatch_menu_command(island_hwnd: HWND, cmd: MenuCommand) {
    if !island_hwnd.is_null() {
        unsafe {
            PostMessageW(island_hwnd, WM_COMMAND, menu_command_id(cmd) as WPARAM, 0);
        }
    }
}

unsafe extern "system" fn main_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayPopupState;
        if state_ptr.is_null() || (*state_ptr).destroyed {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }

        match msg {
            WM_ACTIVATE => {
                let activate_state = (wparam & 0xFFFF) as u32;
                let (is_open, is_opening, sub_hwnd, main_hwnd) = (
                    (*state_ptr).is_open,
                    (*state_ptr).is_opening,
                    (*state_ptr).sub_hwnd,
                    (*state_ptr).main_hwnd,
                );
                if activate_state == WA_INACTIVE && is_open && !is_opening {
                    let other_hwnd = lparam as HWND;
                    if other_hwnd != sub_hwnd && other_hwnd != main_hwnd {
                        TrayPopupState::close_all_raw(state_ptr);
                    }
                }
                0
            }
            WM_CANCELMODE => {
                if (*state_ptr).is_open && !(*state_ptr).is_opening {
                    TrayPopupState::close_all_raw(state_ptr);
                }
                0
            }
            WM_CLOSE => {
                if (*state_ptr).is_open {
                    TrayPopupState::close_all_raw(state_ptr);
                }
                0
            }
            WM_DESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                KillTimer(hwnd, TIMER_FADE_IN);
                KillTimer(hwnd, TIMER_SUBMENU_CLOSE);
                if (*state_ptr).main_hwnd == hwnd {
                    (*state_ptr).main_hwnd = 0 as _;
                }
                (*state_ptr).is_open = false;
                (*state_ptr).is_opening = false;
                (*state_ptr).open_submenu = None;
                (*state_ptr).submenu_close_pending = false;
                let sub_hwnd = (*state_ptr).sub_hwnd;
                if !sub_hwnd.is_null() {
                    ShowWindow(sub_hwnd, SW_HIDE);
                }
                0
            }
            WM_KEYDOWN => {
                if wparam == VK_ESCAPE && (*state_ptr).is_open {
                    TrayPopupState::close_all_raw(state_ptr);
                    return 0;
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_MOUSEMOVE => {
                if !(*state_ptr).is_open {
                    return 0;
                }
                if !(*state_ptr).main_tracking_leave {
                    (*state_ptr).main_tracking_leave = true;
                    track_mouse_leave(hwnd);
                }

                let dpi_scale = (*state_ptr).dpi_scale;
                let (log_x, log_y) = logical_coords_from_lparam(lparam, dpi_scale);
                let (next_hover, hovered_submenu) = {
                    let rows = &(*state_ptr).main_rows;
                    let next = hit_test_row(rows, MAIN_CARD_W, log_x, log_y);
                    let sub = next.and_then(|idx| {
                        rows.get(idx).and_then(|row| match row.kind {
                            MenuRowKind::Item {
                                submenu: Some(kind),
                                ..
                            } => Some((idx, kind)),
                            _ => None,
                        })
                    });
                    (next, sub)
                };

                let hover_changed = next_hover != (*state_ptr).main_hovered;
                (*state_ptr).main_hovered = next_hover;

                if let Some((row_idx, kind)) = hovered_submenu {
                    TrayPopupState::cancel_submenu_close_timer_raw(state_ptr);
                    if (*state_ptr).open_submenu != Some((row_idx, kind)) {
                        TrayPopupState::open_submenu_for_raw(state_ptr, row_idx, kind);
                        return 0;
                    }
                } else if (*state_ptr).open_submenu.is_some() {
                    TrayPopupState::schedule_submenu_close_timer_raw(state_ptr);
                }

                if hover_changed {
                    TrayPopupState::render_main_raw(state_ptr, None);
                }
                0
            }
            WM_MOUSELEAVE => {
                (*state_ptr).main_tracking_leave = false;
                if (*state_ptr).main_hovered.is_some() {
                    (*state_ptr).main_hovered = None;
                    if (*state_ptr).is_open {
                        TrayPopupState::render_main_raw(state_ptr, None);
                    }
                }
                if (*state_ptr).is_open && (*state_ptr).open_submenu.is_some() {
                    TrayPopupState::schedule_submenu_close_timer_raw(state_ptr);
                }
                0
            }
            WM_LBUTTONUP => {
                if !(*state_ptr).is_open {
                    return 0;
                }
                let dpi_scale = (*state_ptr).dpi_scale;
                let (log_x, log_y) = logical_coords_from_lparam(lparam, dpi_scale);
                let hit_action = {
                    let rows = &(*state_ptr).main_rows;
                    hit_test_row(rows, MAIN_CARD_W, log_x, log_y).and_then(|idx| {
                        rows.get(idx).and_then(|row| match row.kind {
                            MenuRowKind::Item {
                                command: Some(cmd), ..
                            } => Some((idx, Some(cmd), None)),
                            MenuRowKind::Item {
                                submenu: Some(kind),
                                ..
                            } => Some((idx, None, Some(kind))),
                            _ => None,
                        })
                    })
                };
                if let Some((idx, cmd_opt, sub_opt)) = hit_action {
                    if let Some(cmd) = cmd_opt {
                        let target_hwnd = (*state_ptr).island_hwnd;
                        TrayPopupState::close_all_raw(state_ptr);
                        dispatch_menu_command(target_hwnd, cmd);
                    } else if let Some(kind) = sub_opt {
                        TrayPopupState::open_submenu_for_raw(state_ptr, idx, kind);
                    }
                }
                0
            }
            WM_TIMER => {
                match wparam as usize {
                    TIMER_FADE_IN => {
                        if !(*state_ptr).is_open {
                            KillTimer(hwnd, TIMER_FADE_IN);
                            return 0;
                        }
                        let elapsed = (*state_ptr).fade_start.elapsed().as_secs_f32();
                        let (alpha, done) = compute_fade_source_alpha(elapsed);
                        (*state_ptr).source_alpha = alpha;
                        if done {
                            KillTimer(hwnd, TIMER_FADE_IN);
                        }
                        let (main_args, sub_args) = {
                            let s = &*state_ptr;
                            let m = if !s.main_hwnd.is_null() && s.main_surface.is_valid() {
                                Some((
                                    s.main_hwnd,
                                    s.main_surface.mem_dc,
                                    s.main_surface.phys_w,
                                    s.main_surface.phys_h,
                                    s.source_alpha,
                                ))
                            } else {
                                None
                            };
                            let sub = if s.open_submenu.is_some()
                                && !s.sub_hwnd.is_null()
                                && s.sub_surface.is_valid()
                                && !s.sub_rows.is_empty()
                            {
                                Some((
                                    s.sub_hwnd,
                                    s.sub_surface.mem_dc,
                                    s.sub_surface.phys_w,
                                    s.sub_surface.phys_h,
                                    s.source_alpha,
                                ))
                            } else {
                                None
                            };
                            (m, sub)
                        };
                        if let Some((h, dc, w, hh, a)) = main_args {
                            commit_layered_window(h, dc, w, hh, None, a);
                        }
                        if let Some((h, dc, w, hh, a)) = sub_args {
                            commit_layered_window(h, dc, w, hh, None, a);
                        }
                    }
                    TIMER_SUBMENU_CLOSE => {
                        TrayPopupState::hide_submenu_only_raw(state_ptr);
                    }
                    _ => {}
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe extern "system" fn sub_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayPopupState;
        if state_ptr.is_null() || (*state_ptr).destroyed {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }

        match msg {
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_CLOSE => {
                if (*state_ptr).is_open {
                    TrayPopupState::close_all_raw(state_ptr);
                }
                0
            }
            WM_DESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if (*state_ptr).sub_hwnd == hwnd {
                    (*state_ptr).sub_hwnd = 0 as _;
                }
                (*state_ptr).open_submenu = None;
                (*state_ptr).sub_hovered = None;
                (*state_ptr).sub_rows.clear();
                0
            }
            WM_KEYDOWN => {
                if wparam == VK_ESCAPE && (*state_ptr).is_open {
                    TrayPopupState::close_all_raw(state_ptr);
                    return 0;
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_MOUSEMOVE => {
                if !(*state_ptr).is_open || (*state_ptr).open_submenu.is_none() {
                    return 0;
                }
                if !(*state_ptr).sub_tracking_leave {
                    (*state_ptr).sub_tracking_leave = true;
                    track_mouse_leave(hwnd);
                }
                TrayPopupState::cancel_submenu_close_timer_raw(state_ptr);

                let dpi_scale = (*state_ptr).dpi_scale;
                let (log_x, log_y) = logical_coords_from_lparam(lparam, dpi_scale);
                let next_hover = hit_test_row(&(*state_ptr).sub_rows, SUBMENU_CARD_W, log_x, log_y);
                if next_hover != (*state_ptr).sub_hovered {
                    (*state_ptr).sub_hovered = next_hover;
                    TrayPopupState::render_sub_raw(state_ptr, None);
                }
                0
            }
            WM_MOUSELEAVE => {
                (*state_ptr).sub_tracking_leave = false;
                if (*state_ptr).sub_hovered.is_some() {
                    (*state_ptr).sub_hovered = None;
                    if (*state_ptr).is_open && (*state_ptr).open_submenu.is_some() {
                        TrayPopupState::render_sub_raw(state_ptr, None);
                    }
                }
                if (*state_ptr).is_open && (*state_ptr).open_submenu.is_some() {
                    TrayPopupState::schedule_submenu_close_timer_raw(state_ptr);
                }
                0
            }
            WM_LBUTTONUP => {
                if !(*state_ptr).is_open || (*state_ptr).open_submenu.is_none() {
                    return 0;
                }
                let dpi_scale = (*state_ptr).dpi_scale;
                let (log_x, log_y) = logical_coords_from_lparam(lparam, dpi_scale);
                let clicked_cmd = {
                    let rows = &(*state_ptr).sub_rows;
                    hit_test_row(rows, SUBMENU_CARD_W, log_x, log_y).and_then(|idx| {
                        rows.get(idx).and_then(|row| match row.kind {
                            MenuRowKind::Item {
                                command: Some(cmd), ..
                            } => Some(cmd),
                            _ => None,
                        })
                    })
                };
                if let Some(cmd) = clicked_cmd {
                    let target_hwnd = (*state_ptr).island_hwnd;
                    TrayPopupState::close_all_raw(state_ptr);
                    dispatch_menu_command(target_hwnd, cmd);
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win32::GdiplusContext;
    use std::collections::HashSet;
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindow, SendMessageW};

    #[test]
    fn test_compute_main_menu_origin_bottom_taskbar_and_top_taskbar() {
        let work_bottom_tb = (0, 0, 1920, 1032);
        let phys_w = 264;
        let phys_h = 347;
        let pad = 10;

        // 1. 底部托盘右键：向上弹出并紧贴工作区底边及右边钳位
        let (x, y) = compute_main_menu_origin(1820, 1055, phys_w, phys_h, work_bottom_tb, pad);
        assert_eq!(x, 1920 - phys_w);
        assert_eq!(y, 1032 - phys_h);
        assert!(x >= work_bottom_tb.0 && x + phys_w <= work_bottom_tb.2);
        assert!(y >= work_bottom_tb.1 && y + phys_h <= work_bottom_tb.3);

        // 2. 屏幕中部光标：卡片左下角贴近光标向上弹出
        let (mx, my) = compute_main_menu_origin(600, 700, phys_w, phys_h, work_bottom_tb, pad);
        assert_eq!(mx, 600 - pad);
        assert_eq!(my, 700 - (phys_h - pad));
        assert!(mx >= work_bottom_tb.0 && mx + phys_w <= work_bottom_tb.2);
        assert!(my >= work_bottom_tb.1 && my + phys_h <= work_bottom_tb.3);

        // 3. 顶部任务栏（work_rect.1 = 48）：上方空间不足，自动翻转向下弹出并钳位到 work_rect.1
        let work_top_tb = (0, 48, 1920, 1080);
        let (tx, ty) = compute_main_menu_origin(1780, 24, phys_w, phys_h, work_top_tb, pad);
        assert_eq!(tx, 1920 - phys_w);
        assert_eq!(ty, 48);
        assert!(tx >= work_top_tb.0 && tx + phys_w <= work_top_tb.2);
        assert!(ty >= work_top_tb.1 && ty + phys_h <= work_top_tb.3);

        // 4. 光标靠上但仍在工作区内（y = 120 < 48 + 337）：上方空间不足，向下弹出且卡片顶边贴近光标
        let (tx2, ty2) = compute_main_menu_origin(400, 120, phys_w, phys_h, work_top_tb, pad);
        assert_eq!(tx2, 400 - pad);
        assert_eq!(ty2, 120 - pad);
        assert!(ty2 >= work_top_tb.1 && ty2 + phys_h <= work_top_tb.3);

        // 5. 负坐标副屏左上角钳位
        let work_left_mon = (-1920, 0, 0, 1032);
        let (lx, ly) = compute_main_menu_origin(-1918, 900, phys_w, phys_h, work_left_mon, pad);
        assert_eq!(lx, -1920);
        assert_eq!(ly, 900 - (phys_h - pad));
        assert!(lx >= work_left_mon.0 && lx + phys_w <= work_left_mon.2);
        assert!(ly >= work_left_mon.1 && ly + phys_h <= work_left_mon.3);
    }

    #[test]
    fn test_compute_main_menu_origin_tiny_work_rect_safe() {
        let tiny = (100, 200, 300, 400);
        let (x, y) = compute_main_menu_origin(150, 250, 264, 347, tiny, 10);
        assert_eq!((x, y), (100, 200));
    }

    #[test]
    fn test_compute_submenu_origin_right_left_flip_and_vertical_clamp() {
        let work_rect = (0, 0, 1920, 1032);
        let main_phys_w = 264;
        let sub_phys_w = 218;
        let sub_phys_h = 128;
        let pad = 10;
        let card_pad_y = 6;
        let row_top_px = 89;

        // 1. 右侧空间充足：主菜单卡片右边 (main_x + 254) 与子菜单卡片左边 (sub_x + 10) 恰好间隔 4px
        let (sx, sy) = compute_submenu_origin(
            400,
            500,
            main_phys_w,
            row_top_px,
            card_pad_y,
            sub_phys_w,
            sub_phys_h,
            work_rect,
            pad,
        );
        let main_card_right = 400 + main_phys_w - pad;
        let sub_card_left = sx + pad;
        assert_eq!(sub_card_left - main_card_right, 4);
        // 首行垂直精确对齐：sub_y + pad + card_pad_y == main_y + row_top_px
        assert_eq!(sy + pad + card_pad_y, 500 + row_top_px);

        // 2. 右侧超出 work_rect.2：自动向左翻转，子菜单卡片右边与主菜单卡片左边恰好间隔 4px
        let main_near_right = 1920 - main_phys_w;
        let (fx, fy) = compute_submenu_origin(
            main_near_right,
            600,
            main_phys_w,
            row_top_px,
            card_pad_y,
            sub_phys_w,
            sub_phys_h,
            work_rect,
            pad,
        );
        let main_card_left = main_near_right + pad;
        let sub_card_right = fx + sub_phys_w - pad;
        assert_eq!(main_card_left - sub_card_right, 4);
        assert!(fx >= work_rect.0 && fx + sub_phys_w <= work_rect.2);
        assert_eq!(fy + pad + card_pad_y, 600 + row_top_px);

        // 3. 底部工作区钳位
        let (_, cy_bottom) = compute_submenu_origin(
            400,
            900,
            main_phys_w,
            row_top_px,
            card_pad_y,
            sub_phys_w,
            sub_phys_h,
            work_rect,
            pad,
        );
        assert_eq!(cy_bottom, work_rect.3 - sub_phys_h);

        // 4. 顶部工作区钳位（高分屏 150% DPI）
        let top_tb = (0, 60, 2560, 1440);
        let (_, cy_top) =
            compute_submenu_origin(500, 50, 396, 15, 9, 327, 192, top_tb, 15);
        assert_eq!(cy_top, top_tb.1);
    }

    #[test]
    fn test_menu_command_id_bijection() {
        let all = [
            MenuCommand::ToggleIsland,
            MenuCommand::OpenDeck,
            MenuCommand::SetNoise(NoiseFilterLevel::Silent),
            MenuCommand::SetNoise(NoiseFilterLevel::Normal),
            MenuCommand::SetNoise(NoiseFilterLevel::Verbose),
            MenuCommand::SetMonitor(MonitorMode::Primary),
            MenuCommand::SetMonitor(MonitorMode::FollowCursor),
            MenuCommand::ToggleDnd,
            MenuCommand::ToggleSound,
            MenuCommand::ToggleAutostart,
            MenuCommand::ResetIdle,
            MenuCommand::Exit,
        ];
        let ids: HashSet<usize> = all.iter().map(|&c| menu_command_id(c)).collect();
        assert_eq!(ids.len(), all.len());
    }

    #[test]
    fn test_tray_popup_host_lifecycle_wm_close_and_surface_smoke() {
        let _gdip = GdiplusContext::init().expect("GdiplusContext::init");
        let host = TrayPopupHost::new(0 as _).expect("TrayPopupHost::new");
        assert!(!host.is_visible());

        unsafe {
            let main_hwnd = (*host.inner).main_hwnd;
            let sub_hwnd = (*host.inner).sub_hwnd;
            assert!(IsWindow(main_hwnd) != 0);
            assert!(IsWindow(sub_hwnd) != 0);

            // 验证移动宿主（模拟放入 WindowState/Box）后 GWLP_USERDATA 指针依然有效且可拦截 WM_CLOSE
            let mut moved_host = host;
            SendMessageW(main_hwnd, WM_CLOSE, 0, 0);
            SendMessageW(sub_hwnd, WM_CLOSE, 0, 0);
            assert!(IsWindow(main_hwnd) != 0);
            assert!(IsWindow(sub_hwnd) != 0);
            assert!(!moved_host.is_visible());

            assert!((*moved_host.inner)
                .main_surface
                .ensure_size(main_hwnd, 264, 347, 1.0));
            assert!((*moved_host.inner).main_surface.is_valid());
            assert!((*moved_host.inner)
                .main_surface
                .ensure_size(main_hwnd, 396, 521, 1.5));
            assert!((*moved_host.inner).main_surface.is_valid());

            let snap = TrayMenuSnapshot {
                is_expanded: false,
                active_sessions_count: 1,
                noise_level: NoiseFilterLevel::Normal,
                monitor_mode: MonitorMode::Primary,
                dnd_fullscreen: true,
                sound_enabled: false,
                auto_start: false,
            };
            let rows = build_main_rows(&snap);
            {
                let s = &mut *moved_host.inner;
                let (dc, w, h) = TrayPopupState::draw_surface_to_dib(
                    &mut s.renderer,
                    &s.main_surface,
                    1.5,
                    &rows,
                    MAIN_CARD_W,
                    Some(0),
                    None,
                );
                assert!(!dc.is_null() && w == 396 && h == 521);
            }

            (*moved_host.inner).main_surface.release();
            assert!(!(*moved_host.inner).main_surface.is_valid());

            moved_host.close_all();
            assert!(!moved_host.is_visible());
            moved_host.destroy();
            assert!((*moved_host.inner).destroyed);
            assert!((*moved_host.inner).main_hwnd.is_null());
            assert!((*moved_host.inner).sub_hwnd.is_null());
            assert!(IsWindow(main_hwnd) == 0);
            assert!(IsWindow(sub_hwnd) == 0);

            // Idempotent second destroy & drop
            moved_host.destroy();
        }
    }

    #[test]
    fn test_compute_fade_source_alpha_hardware_ramp() {
        let (a0, d0) = compute_fade_source_alpha(0.0);
        assert_eq!(a0, FADE_START_ALPHA);
        assert!(!d0);

        let (am, dm) = compute_fade_source_alpha(FADE_DURATION_SECS * 0.5);
        assert!(am > FADE_START_ALPHA && am < 255);
        assert!(!dm);

        let (a1, d1) = compute_fade_source_alpha(FADE_DURATION_SECS);
        assert_eq!(a1, 255);
        assert!(d1);

        let (a2, d2) = compute_fade_source_alpha(FADE_DURATION_SECS * 2.0);
        assert_eq!(a2, 255);
        assert!(d2);
    }
}
