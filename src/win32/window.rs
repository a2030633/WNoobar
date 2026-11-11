#![allow(dead_code)]
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
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyIcon, DestroyMenu, DestroyWindow, GetCursorPos,
    PostMessageW, RegisterWindowMessageW, SetForegroundWindow, TrackPopupMenuEx, HICON,
    MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON, WM_COMMAND, WM_LBUTTONDBLCLK,
    WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_USER,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::core::bus::AgentEvent;
use crate::core::fsm::{EdgeDockMode, IslandState};
use crate::ui::capsule::CapsuleRenderer;
use crate::ui::spring::SpringValue;
use crate::win32::gdiplus::{
    create_bitmap_from_bytes, GdipCreateBitmapFromScan0, GdipCreateHICONFromBitmap,
    GdipDisposeImage, GdipGetImageGraphicsContext, GdipScaleWorldTransform,
    GdipSetSmoothingMode, GdipSetTextRenderingHint, MATRIX_ORDER_PREPEND,
    PIXEL_FORMAT_32BPP_PARGB, SMOOTHING_MODE_ANTIALIAS,
    TEXT_RENDERING_HINT_ANTIALIAS_GRID_FIT,
};

const WNOOBAR_PNG: &[u8] = include_bytes!("../../assets/icons/wnoobar.png");

const WM_TRAYICON: u32 = WM_USER + 101;
const IDM_TRAY_TOGGLE: usize = 2001;
const IDM_TRAY_DECK: usize = 2002;
const IDM_TRAY_RESET: usize = 2003;
const IDM_TRAY_EXIT: usize = 2004;

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

struct WindowState {
    width_spring: SpringValue,
    height_spring: SpringValue,
    current_state: IslandState,
    dock_mode: EdgeDockMode,
    renderer: CapsuleRenderer,
    event_rx: Receiver<AgentEvent>,
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
    is_mouse_over: bool,
    mouse_leave_instant: Option<Instant>,
    recent_sessions: Vec<(String, String, String, String, String)>, // id, project, role, desc, agent
    tray_icon: HICON,
    taskbar_created_msg: u32,
}

impl Drop for WindowState {
    fn drop(&mut self) {
        unsafe {
            if !self.gp_bitmap.is_null() {
                GdipDisposeImage(self.gp_bitmap);
            }
            if !self.dib_bitmap.is_null() {
                DeleteObject(self.dib_bitmap as _);
            }
            if !self.mem_dc.is_null() {
                DeleteDC(self.mem_dc);
            }
            if !self.tray_icon.is_null() {
                DestroyIcon(self.tray_icon);
                self.tray_icon = 0 as _;
            }
        }
    }
}

impl IslandWindow {
    pub fn new(event_rx: Receiver<AgentEvent>) -> Result<Self, String> {
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

            let dpi_scale = Self::get_system_dpi_scale();

            let logical_w = 480.0;
            let logical_h = 210.0;
            let phys_w = (logical_w * dpi_scale) as i32;
            let phys_h = (logical_h * dpi_scale) as i32;

            let screen_w = GetSystemMetrics(SM_CXSCREEN);
            let pos_x = (screen_w - phys_w) / 2;
            let pos_y = 0; // 贴附屏幕最顶边缘

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
                phys_w,
                phys_h,
                0 as _,
                0 as _,
                instance,
                null_mut(),
            );

            if hwnd.is_null() {
                return Err("Failed to create Win32 layered window".into());
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
                bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
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

            // 逻辑像素与 Retina 物理像素映射
            GdipScaleWorldTransform(gp_graphics, dpi_scale, dpi_scale, MATRIX_ORDER_PREPEND);

            let renderer = CapsuleRenderer::new();
            // 初始默认为贴边隐藏微光条
            let dock_mode = EdgeDockMode::Peeking;
            let (target_w, target_h) = IslandState::Idle.target_dimensions(dock_mode);

            let taskbar_created_msg = RegisterWindowMessageW(encode_wide("TaskbarCreated").as_ptr());

            // 注册到 Windows 任务栏系统托盘（状态栏通知区）
            add_tray_icon(hwnd, tray_icon, "WNoobar - 智能灵动岛 (待命就绪)");

            let state = Box::new(WindowState {
                width_spring: SpringValue::new(target_w),
                height_spring: SpringValue::new(target_h),
                current_state: IslandState::Idle,
                dock_mode,
                renderer,
                event_rx,
                logical_w,
                logical_h,
                phys_w,
                phys_h,
                dpi_scale,
                mem_dc,
                dib_bitmap,
                bits_ptr: bits_ptr as *mut u8,
                gp_bitmap,
                gp_graphics,
                last_frame: Instant::now(),
                is_mouse_over: false,
                mouse_leave_instant: None,
                recent_sessions: Vec::new(),
                tray_icon,
                taskbar_created_msg,
            });

            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);

            // 启动 16ms 渲染循环
            SetTimer(hwnd, 1, 16, None);

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
                    let screen_x = (lparam & 0xFFFF) as i16 as i32;
                    let screen_y = ((lparam >> 16) & 0xFFFF) as i16 as i32;

                    let mut pt = POINT { x: screen_x, y: screen_y };
                    ScreenToClient(hwnd, &mut pt);

                    let log_x = pt.x as f32 / state.dpi_scale;
                    let log_y = pt.y as f32 / state.dpi_scale;

                    let cap_w = state.width_spring.current;
                    let cap_h = state.height_spring.current;
                    let cap_x = (state.logical_w - cap_w) / 2.0;
                    let morph = ((cap_h - 4.0) / 26.0).clamp(0.0, 1.0);
                    let cap_y = 4.5 * morph;

                    // 连续几何命中测试（包含底垫缓冲）
                    let in_hit = if state.dock_mode == EdgeDockMode::DeckExpanded {
                        log_x >= cap_x - 14.0 && log_x <= cap_x + cap_w + 14.0 && log_y >= 0.0 && log_y <= cap_y + cap_h + 14.0
                    } else if state.dock_mode == EdgeDockMode::Peeking {
                        // 贴顶极细微光条核心感应带
                        log_x >= (state.logical_w - 160.0) / 2.0 && log_x <= (state.logical_w + 160.0) / 2.0 && log_y >= 0.0 && log_y <= 12.0
                    } else {
                        log_x >= cap_x - 14.0 && log_x <= cap_x + cap_w + 14.0 && log_y >= 0.0 && log_y <= cap_y + cap_h + 14.0
                    };

                    if in_hit {
                        return HTCLIENT as LRESULT;
                    }
                }
                HTTRANSPARENT as LRESULT
            }
            WM_MOUSEMOVE => 0,
            WM_MOUSELEAVE => 0,
            WM_TIMER => {
                if !state_ptr.is_null() {
                    let state = &mut *state_ptr;

                    // 1. 全局硬件光标检测与双阈值施密特触发器 (Schmitt Trigger Hysteresis)
                    let mut cur_pt = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut cur_pt);
                    ScreenToClient(hwnd, &mut cur_pt);
                    let log_x = cur_pt.x as f32 / state.dpi_scale;
                    let log_y = cur_pt.y as f32 / state.dpi_scale;

                    let center_x = state.logical_w / 2.0;
                    let cap_w = state.width_spring.current;
                    let cap_h = state.height_spring.current;
                    let cap_x = (state.logical_w - cap_w) / 2.0;
                    let morph = ((cap_h - 4.0) / 26.0).clamp(0.0, 1.0);
                    let cap_y = 4.5 * morph;

                    // 阈值 A：唤醒盒 (Enter Box - 紧凑精准)：处于贴边时，只要鼠标进入顶部核心条立即唤醒
                    let in_enter_box = log_x >= center_x - 80.0
                        && log_x <= center_x + 80.0
                        && log_y >= 0.0
                        && log_y <= 10.0;

                    // 阈值 B：保持盒 (Keep-Alive Box - 宽裕包裹)：处于展开态时，外扩 18px 几何防抖保护
                    let in_keep_box = log_x >= cap_x - 18.0
                        && log_x <= cap_x + cap_w + 18.0
                        && log_y >= -5.0
                        && log_y <= cap_y + cap_h + 18.0;

                    if state.dock_mode == EdgeDockMode::Peeking {
                        if in_enter_box {
                            state.is_mouse_over = true;
                            state.mouse_leave_instant = None;
                            state.dock_mode = EdgeDockMode::Normal;
                            let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                            state.width_spring.set_target(tw);
                            state.height_spring.set_target(th);
                        }
                    } else {
                        // 当前处于展开态（Normal 或 DeckExpanded）
                        if in_keep_box {
                            state.is_mouse_over = true;
                            state.mouse_leave_instant = None;
                        } else {
                            // 鼠标移出宽裕的保持盒
                            if state.is_mouse_over {
                                state.is_mouse_over = false;
                                state.mouse_leave_instant = Some(Instant::now());
                            }

                            if let Some(leave_time) = state.mouse_leave_instant {
                                if leave_time.elapsed() >= Duration::from_millis(120) {
                                    state.mouse_leave_instant = None;
                                    // 彻底离开超过 120ms：ActionNeeded 状态保持展示供确认，其余状态坚决平滑收缩贴边！
                                    if !matches!(state.current_state, IslandState::ActionNeeded { .. }) {
                                        state.dock_mode = EdgeDockMode::Peeking;
                                        let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                                        state.width_spring.set_target(tw);
                                        state.height_spring.set_target(th);
                                    }
                                }
                            }
                        }
                    }

                    // 2. 检查 Started 状态超时（1.8秒致意后平滑贴边隐藏）
                    if let IslandState::Started { started_at, auto_hide_secs, .. } = &state.current_state {
                        if started_at.elapsed().as_secs_f32() > *auto_hide_secs {
                            if !state.is_mouse_over && state.dock_mode != EdgeDockMode::Peeking {
                                state.dock_mode = EdgeDockMode::Peeking;
                                let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                                state.width_spring.set_target(tw);
                                state.height_spring.set_target(th);
                            }
                        }
                    }

                    // 3. 检查 Done 状态超时（3.5秒展示后平滑贴边隐藏）
                    if let IslandState::Done { finished_at, auto_collapse_secs, .. } = &state.current_state {
                        if finished_at.elapsed().as_secs_f32() > *auto_collapse_secs {
                            state.current_state = IslandState::Idle;
                            if !state.is_mouse_over && state.dock_mode != EdgeDockMode::Peeking {
                                state.dock_mode = EdgeDockMode::Peeking; // 彻底重置为贴边隐藏！
                            }
                            let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                            state.width_spring.set_target(tw);
                            state.height_spring.set_target(th);
                            update_tray_tip(hwnd, "WNoobar - 智能灵动岛 (待命就绪)");
                        }
                    }

                    // 4. 处理核心事件
                    while let Ok(event) = state.event_rx.try_recv() {
                        match event {
                            // 【节拍 1：启动轻弹】：滑出展开 1.8s 后自动贴边隐退
                            AgentEvent::TaskStarted { session_id, project_name, role_type, agent_name, task_summary } => {
                                update_tray_tip(hwnd, &format!("WNoobar - 启动: {}", project_name));
                                state.dock_mode = EdgeDockMode::Normal;
                                state.current_state = IslandState::Started {
                                    session_id: session_id.clone(),
                                    project_name: project_name.clone(),
                                    role_type: role_type.clone(),
                                    agent_name: agent_name.clone(),
                                    task_summary: task_summary.clone(),
                                    started_at: Instant::now(),
                                    auto_hide_secs: 1.8,
                                };

                                let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                                state.width_spring.set_target(tw);
                                state.height_spring.set_target(th);

                                if let Some(item) = state.recent_sessions.iter_mut().find(|(id, _, _, _, _)| id == &session_id) {
                                    item.1 = project_name;
                                    item.2 = role_type;
                                    item.3 = task_summary;
                                    item.4 = agent_name;
                                } else {
                                    state.recent_sessions.insert(0, (session_id, project_name, role_type, task_summary, agent_name));
                                    if state.recent_sessions.len() > 5 {
                                        state.recent_sessions.pop();
                                    }
                                }
                            }
                            // 【节拍 2：执行静默期】：绝不主动弹窗！如果处于 Peeking，保持 Peeking！
                            AgentEvent::Working { session_id, project_name, role_type, agent_name, task_summary, step_index } => {
                                update_tray_tip(hwnd, &format!("WNoobar - 处理中: {}", project_name));
                                let new_desc = task_summary.clone();

                                // 完成卡片视觉保护期（2.5秒）：若当前处于 Done 状态且未展示满 2.5 秒，严禁被并发 Working 冲刷打断！
                                let in_done_protection = match &state.current_state {
                                    IslandState::Done { finished_at, .. } => finished_at.elapsed() < Duration::from_millis(2500),
                                    _ => false,
                                };

                                if !in_done_protection {
                                    state.current_state = IslandState::Working {
                                        session_id: session_id.clone(),
                                        project_name: project_name.clone(),
                                        role_type: role_type.clone(),
                                        agent_name: agent_name.clone(),
                                        task_summary,
                                        step_index,
                                        start_time: Instant::now(),
                                    };

                                    // 弹簧目标尺寸按当前 dock_mode 更新（若为 Peeking 保持不变）
                                    let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                                    state.width_spring.set_target(tw);
                                    state.height_spring.set_target(th);
                                }

                                if let Some(item) = state.recent_sessions.iter_mut().find(|(id, _, _, _, _)| id == &session_id) {
                                    item.1 = project_name;
                                    item.2 = role_type;
                                    item.3 = new_desc;
                                    item.4 = agent_name;
                                } else {
                                    state.recent_sessions.insert(0, (session_id, project_name, role_type, new_desc, agent_name));
                                    if state.recent_sessions.len() > 5 {
                                        state.recent_sessions.pop();
                                    }
                                }
                            }
                            // 【节拍 3：完成交付】：滑出展开大卡片，3.5s 后回缩贴边
                            AgentEvent::Done { session_id, project_name, role_type, agent_name, summary, duration_secs, artifacts_count } => {
                                update_tray_tip(hwnd, &format!("WNoobar - 任务完成: {}", project_name));
                                state.dock_mode = EdgeDockMode::Normal;
                                state.current_state = IslandState::Done {
                                    session_id: session_id.clone(),
                                    project_name: project_name.clone(),
                                    role_type: role_type.clone(),
                                    agent_name: agent_name.clone(),
                                    summary: summary.clone(),
                                    duration_secs,
                                    artifacts_count,
                                    finished_at: Instant::now(),
                                    auto_collapse_secs: 3.5,
                                };

                                let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                                state.width_spring.set_target(tw);
                                state.height_spring.set_target(th);

                                if let Some(item) = state.recent_sessions.iter_mut().find(|(id, _, _, _, _)| id == &session_id) {
                                    item.1 = project_name;
                                    item.2 = role_type;
                                    item.3 = summary;
                                    item.4 = agent_name;
                                }
                            }
                            AgentEvent::ActionNeeded { session_id, project_name, role_type, agent_name, prompt } => {
                                update_tray_tip(hwnd, &format!("WNoobar - 需确认: {}", project_name));
                                state.dock_mode = EdgeDockMode::Normal;
                                state.current_state = IslandState::ActionNeeded {
                                    session_id: session_id.clone(),
                                    project_name: project_name.clone(),
                                    role_type: role_type.clone(),
                                    agent_name: agent_name.clone(),
                                    prompt: prompt.clone(),
                                };

                                let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                                state.width_spring.set_target(tw);
                                state.height_spring.set_target(th);

                                if let Some(item) = state.recent_sessions.iter_mut().find(|(id, _, _, _, _)| id == &session_id) {
                                    item.1 = project_name;
                                    item.2 = role_type;
                                    item.3 = prompt;
                                    item.4 = agent_name;
                                } else {
                                    state.recent_sessions.insert(0, (session_id, project_name, role_type, prompt, agent_name));
                                    if state.recent_sessions.len() > 5 {
                                        state.recent_sessions.pop();
                                    }
                                }
                            }
                            AgentEvent::SwitchSession(_) => {}
                            AgentEvent::ResetIdle => {
                                update_tray_tip(hwnd, "WNoobar - 智能灵动岛 (待命就绪)");
                                state.current_state = IslandState::Idle;
                                state.dock_mode = EdgeDockMode::Peeking;
                                let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                                state.width_spring.set_target(tw);
                                state.height_spring.set_target(th);
                            }
                        }
                    }

                    // 5. 动画更新与重绘
                    let now = Instant::now();
                    let dt = now.duration_since(state.last_frame).as_secs_f32();
                    state.last_frame = now;

                    let w_anim = state.width_spring.update(dt);
                    let h_anim = state.height_spring.update(dt);
                    state.renderer.update(dt);

                    // 自适应帧率：若处于贴边隐藏且弹簧已完全静止且未悬停，可跳过重绘降低系统消耗
                    let is_animating = w_anim || h_anim;
                    let should_render = is_animating
                        || state.dock_mode != EdgeDockMode::Peeking
                        || state.is_mouse_over;

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

                    let cap_w = state.width_spring.current;
                    let cap_x = (state.logical_w - cap_w) / 2.0;

                    // 点击左侧徽标区域：展开/收起抽屉
                    if x_pos >= cap_x && x_pos <= cap_x + 38.0 && y_pos >= 0.0 && y_pos <= 42.0 {
                        state.dock_mode = match state.dock_mode {
                            EdgeDockMode::DeckExpanded => EdgeDockMode::Normal,
                            _ => EdgeDockMode::DeckExpanded,
                        };
                        let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                        state.width_spring.set_target(tw);
                        state.height_spring.set_target(th);
                    } else if state.dock_mode == EdgeDockMode::DeckExpanded {
                        state.dock_mode = EdgeDockMode::Normal;
                        let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                        state.width_spring.set_target(tw);
                        state.height_spring.set_target(th);
                    } else if !state.current_state.is_idle() {
                        // 用户点击正在展示的卡片：仅做避让收缩回贴边微缩，严禁修改 state.current_state！
                        state.dock_mode = EdgeDockMode::Peeking;
                        let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                        state.width_spring.set_target(tw);
                        state.height_spring.set_target(th);
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
                            // 左键单击或双击托盘图标：切换灵动岛展开 / 贴边隐藏
                            state.dock_mode = match state.dock_mode {
                                EdgeDockMode::Peeking => EdgeDockMode::Normal,
                                _ => EdgeDockMode::Peeking,
                            };
                            let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                            state.width_spring.set_target(tw);
                            state.height_spring.set_target(th);
                        }
                        WM_RBUTTONUP => {
                            // 右键单击托盘图标：弹出快捷控制菜单
                            SetForegroundWindow(hwnd);
                            let mut pt = POINT { x: 0, y: 0 };
                            GetCursorPos(&mut pt);

                            let hmenu = CreatePopupMenu();
                            let toggle_text = if state.dock_mode == EdgeDockMode::Peeking {
                                "📌 展开灵动岛 (居顶显示)"
                            } else {
                                "📌 收起灵动岛 (贴边微缩)"
                            };
                            AppendMenuW(hmenu, MF_STRING, IDM_TRAY_TOGGLE, encode_wide(toggle_text).as_ptr());
                            AppendMenuW(hmenu, MF_STRING, IDM_TRAY_DECK, encode_wide("📋 活跃会话抽屉 (Session Deck)").as_ptr());
                            AppendMenuW(hmenu, MF_STRING, IDM_TRAY_RESET, encode_wide("🔄 还原待命就绪状态").as_ptr());
                            AppendMenuW(hmenu, MF_SEPARATOR, 0, null_mut());
                            AppendMenuW(hmenu, MF_STRING, IDM_TRAY_EXIT, encode_wide("❌ 退出 WNoobar").as_ptr());

                            TrackPopupMenuEx(hmenu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, hwnd, null_mut());
                            PostMessageW(hwnd, WM_NULL, 0, 0);
                            DestroyMenu(hmenu);
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
                            state.dock_mode = match state.dock_mode {
                                EdgeDockMode::Peeking => EdgeDockMode::Normal,
                                _ => EdgeDockMode::Peeking,
                            };
                            let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                            state.width_spring.set_target(tw);
                            state.height_spring.set_target(th);
                        }
                        IDM_TRAY_DECK => {
                            state.dock_mode = match state.dock_mode {
                                EdgeDockMode::DeckExpanded => EdgeDockMode::Normal,
                                _ => EdgeDockMode::DeckExpanded,
                            };
                            let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                            state.width_spring.set_target(tw);
                            state.height_spring.set_target(th);
                        }
                        IDM_TRAY_RESET => {
                            state.current_state = IslandState::Idle;
                            state.dock_mode = EdgeDockMode::Peeking;
                            let (tw, th) = state.current_state.target_dimensions(state.dock_mode);
                            state.width_spring.set_target(tw);
                            state.height_spring.set_target(th);
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
                    let _ = Box::from_raw(state_ptr);
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
            state.width_spring.current,
            state.height_spring.current,
            &state.current_state,
            state.dock_mode,
            &state.recent_sessions,
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
