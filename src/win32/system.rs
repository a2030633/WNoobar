#![allow(dead_code)]

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, FARPROC, HANDLE, HWND, POINT, RECT,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetClassNameW, GetCursorPos, GetForegroundWindow, GetSystemMetrics,
    GetWindowRect, IsWindowVisible, PostMessageW, MB_ICONASTERISK, MB_ICONEXCLAMATION, SM_CXSCREEN,
};

const SINGLE_INSTANCE_MUTEX_NAME: &str = "Local\\WNoobarSingleInstanceMutex";
const ISLAND_WINDOW_CLASS_NAME: &str = "WNoobarIslandWindowClass";
const AUTOSTART_RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const AUTOSTART_VALUE_NAME: &str = "WNoobar";

// 64 位 Windows 上 HKEY_CURRENT_USER 为 sign-extended LONG(0x80000001)
const HKEY_CURRENT_USER: isize = 0x8000_0001_u32 as i32 as isize;
const KEY_READ: u32 = 0x0002_0019;
const KEY_SET_VALUE: u32 = 0x0000_0002;
const REG_OPTION_NON_VOLATILE: u32 = 0;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;
const ERROR_SUCCESS: i32 = 0;
const ERROR_FILE_NOT_FOUND: i32 = 2;

const SND_ASYNC: u32 = 0x0000_0001;
const SND_NODEFAULT: u32 = 0x0000_0002;
const SND_ALIAS: u32 = 0x0001_0000;

const QUNS_BUSY: i32 = 2;
const QUNS_RUNNING_D3D_FULL_SCREEN: i32 = 3;
const QUNS_PRESENTATION_MODE: i32 = 4;

const MDT_EFFECTIVE_DPI: i32 = 0;
const FULLSCREEN_TOLERANCE_PX: i32 = 2;

/// 单实例互斥锁守卫，析构时自动释放内核互斥体句柄。
pub struct SingleInstanceGuard(*mut c_void);

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseHandle(self.0 as HANDLE);
            }
            self.0 = null_mut();
        }
    }
}

/// 轻柔提示音类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueKind {
    ActionNeeded,
    Done,
}

/// 目标显示器的物理坐标与 DPI 缩放信息。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorPlacement {
    pub origin_x: i32,
    pub origin_y: i32,
    pub monitor_w: i32,
    pub dpi_scale: f32,
}

impl MonitorPlacement {
    /// 计算给定物理宽度窗口在当前显示器水平居中的 X 坐标。
    pub fn centered_window_x(&self, win_phys_w: i32) -> i32 {
        self.origin_x + (self.monitor_w - win_phys_w) / 2
    }
}

/// 将 UTF-8 字符串编码为以 NUL 结尾的 UTF-16 缓冲区。
pub(crate) fn encode_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 动态解析 Win32 DLL 导出函数地址（缺失依赖时安全返回 None，确保 GNU 工具链零额外链接库）。
pub(crate) unsafe fn resolve_proc(dll_name: &str, proc_name: &[u8]) -> FARPROC {
    unsafe {
        let wide_dll = encode_wide(dll_name);
        let mut module = GetModuleHandleW(wide_dll.as_ptr());
        if module.is_null() {
            module = LoadLibraryW(wide_dll.as_ptr());
        }
        if module.is_null() {
            return None;
        }
        GetProcAddress(module, proc_name.as_ptr())
    }
}

/// 读取窗口类名并判断是否为桌面/任务栏等系统壳层窗口。
pub(crate) fn is_shell_window(hwnd: HWND) -> bool {
    if hwnd.is_null() {
        return false;
    }
    let mut buf = [0u16; 128];
    let len = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if len <= 0 {
        return false;
    }
    let class_name = String::from_utf16_lossy(&buf[..len as usize]);
    matches!(
        class_name.as_str(),
        "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Progman" | "WorkerW"
    )
}

type CreateMutexWFn = unsafe extern "system" fn(*const c_void, i32, *const u16) -> HANDLE;

/// 尝试获取指定名称的互斥锁（供内部及测试复用）。
fn acquire_named_mutex_lock(mutex_name: &str) -> Option<SingleInstanceGuard> {
    static CREATE_MUTEX: OnceLock<Option<CreateMutexWFn>> = OnceLock::new();
    let create_mutex = (*CREATE_MUTEX.get_or_init(|| unsafe {
        resolve_proc("kernel32.dll", b"CreateMutexW\0").map(|f| std::mem::transmute(f))
    }))?;

    unsafe {
        let name = encode_wide(mutex_name);
        let handle = create_mutex(null(), 0, name.as_ptr());
        if handle.is_null() {
            return None;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(handle);
            return None;
        }
        Some(SingleInstanceGuard(handle as *mut c_void))
    }
}

/// 尝试获取单实例全局互斥锁；若已有实例在运行（ERROR_ALREADY_EXISTS = 183）则返回 None。
pub fn acquire_single_instance_lock() -> Option<SingleInstanceGuard> {
    acquire_named_mutex_lock(SINGLE_INSTANCE_MUTEX_NAME)
}

/// 查找已存在的 WNoobar 窗口并投递唤醒消息。
pub fn wake_existing_instance(wake_msg: u32) -> bool {
    unsafe {
        let class_name = encode_wide(ISLAND_WINDOW_CLASS_NAME);
        let hwnd = FindWindowW(class_name.as_ptr(), null());
        if hwnd.is_null() {
            return false;
        }
        PostMessageW(hwnd, wake_msg, 0, 0) != 0
    }
}

type RegOpenKeyExWFn = unsafe extern "system" fn(isize, *const u16, u32, u32, *mut isize) -> i32;
type RegCreateKeyExWFn = unsafe extern "system" fn(
    isize,
    *const u16,
    u32,
    *mut u16,
    u32,
    u32,
    *const c_void,
    *mut isize,
    *mut u32,
) -> i32;
type RegQueryValueExWFn =
    unsafe extern "system" fn(isize, *const u16, *mut u32, *mut u32, *mut u8, *mut u32) -> i32;
type RegSetValueExWFn =
    unsafe extern "system" fn(isize, *const u16, u32, u32, *const u8, u32) -> i32;
type RegDeleteValueWFn = unsafe extern "system" fn(isize, *const u16) -> i32;
type RegCloseKeyFn = unsafe extern "system" fn(isize) -> i32;

struct AdvapiRegFns {
    open_key: RegOpenKeyExWFn,
    create_key: RegCreateKeyExWFn,
    query_value: RegQueryValueExWFn,
    set_value: RegSetValueExWFn,
    delete_value: RegDeleteValueWFn,
    close_key: RegCloseKeyFn,
}

fn load_advapi_reg() -> Option<&'static AdvapiRegFns> {
    static FNS: OnceLock<Option<AdvapiRegFns>> = OnceLock::new();
    FNS.get_or_init(|| unsafe {
        let open_key: RegOpenKeyExWFn =
            std::mem::transmute(resolve_proc("advapi32.dll", b"RegOpenKeyExW\0")?);
        let create_key: RegCreateKeyExWFn =
            std::mem::transmute(resolve_proc("advapi32.dll", b"RegCreateKeyExW\0")?);
        let query_value: RegQueryValueExWFn =
            std::mem::transmute(resolve_proc("advapi32.dll", b"RegQueryValueExW\0")?);
        let set_value: RegSetValueExWFn =
            std::mem::transmute(resolve_proc("advapi32.dll", b"RegSetValueExW\0")?);
        let delete_value: RegDeleteValueWFn =
            std::mem::transmute(resolve_proc("advapi32.dll", b"RegDeleteValueW\0")?);
        let close_key: RegCloseKeyFn =
            std::mem::transmute(resolve_proc("advapi32.dll", b"RegCloseKey\0")?);
        Some(AdvapiRegFns {
            open_key,
            create_key,
            query_value,
            set_value,
            delete_value,
            close_key,
        })
    })
    .as_ref()
}

fn format_autostart_command(exe_path: &std::path::Path) -> String {
    let raw = exe_path.to_string_lossy();
    let trimmed = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    format!("\"{trimmed}\"")
}

/// 检查 HKCU Run 下是否已启用 WNoobar 开机自启。
pub fn is_autostart_enabled() -> bool {
    let Some(reg) = load_advapi_reg() else {
        return false;
    };
    unsafe {
        let subkey = encode_wide(AUTOSTART_RUN_SUBKEY);
        let value_name = encode_wide(AUTOSTART_VALUE_NAME);
        let mut hkey: isize = 0;

        let status = (reg.open_key)(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey);
        if status != ERROR_SUCCESS {
            return false;
        }

        let mut val_type: u32 = 0;
        let mut data_len: u32 = 0;
        let query_status = (reg.query_value)(
            hkey,
            value_name.as_ptr(),
            null_mut(),
            &mut val_type,
            null_mut(),
            &mut data_len,
        );
        if query_status != ERROR_SUCCESS || (val_type != REG_SZ && val_type != REG_EXPAND_SZ) || data_len <= 2 {
            (reg.close_key)(hkey);
            return false;
        }

        let mut buf = vec![0u16; (data_len as usize).div_ceil(2)];
        let mut actual_len = (buf.len() * 2) as u32;
        let read_status = (reg.query_value)(
            hkey,
            value_name.as_ptr(),
            null_mut(),
            &mut val_type,
            buf.as_mut_ptr() as *mut u8,
            &mut actual_len,
        );
        (reg.close_key)(hkey);

        if read_status != ERROR_SUCCESS {
            return false;
        }
        let u16_len = (actual_len as usize / 2).min(buf.len());
        let text = String::from_utf16_lossy(&buf[..u16_len]);
        !text.trim_matches('\0').trim().is_empty()
    }
}

/// 写入或移除 HKCU Run 注册表项以控制开机自启动。
pub fn set_autostart_enabled(enabled: bool) -> Result<(), String> {
    let reg = load_advapi_reg().ok_or_else(|| "Failed to load advapi32.dll registry APIs".to_string())?;

    let subkey = encode_wide(AUTOSTART_RUN_SUBKEY);
    let value_name = encode_wide(AUTOSTART_VALUE_NAME);

    unsafe {
        if enabled {
            let exe_path = std::env::current_exe()
                .map_err(|e| format!("Failed to resolve current executable path: {e}"))?;
            let command = format_autostart_command(&exe_path);
            let wide_val = encode_wide(&command);

            let mut hkey: isize = 0;
            let status = (reg.create_key)(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                null_mut(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                null(),
                &mut hkey,
                null_mut(),
            );
            if status != ERROR_SUCCESS {
                return Err(format!("RegCreateKeyExW failed with code {status}"));
            }

            let byte_len = (wide_val.len() * size_of::<u16>()) as u32;
            let set_status = (reg.set_value)(
                hkey,
                value_name.as_ptr(),
                0,
                REG_SZ,
                wide_val.as_ptr() as *const u8,
                byte_len,
            );
            (reg.close_key)(hkey);

            if set_status != ERROR_SUCCESS {
                return Err(format!("RegSetValueExW failed with code {set_status}"));
            }
            Ok(())
        } else {
            let mut hkey: isize = 0;
            let status = (reg.open_key)(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            );
            if status == ERROR_FILE_NOT_FOUND {
                return Ok(());
            }
            if status != ERROR_SUCCESS {
                return Err(format!("RegOpenKeyExW failed with code {status}"));
            }

            let del_status = (reg.delete_value)(hkey, value_name.as_ptr());
            (reg.close_key)(hkey);

            if del_status == ERROR_SUCCESS || del_status == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!("RegDeleteValueW failed with code {del_status}"))
            }
        }
    }
}

type PlaySoundWFn = unsafe extern "system" fn(*const u16, *mut c_void, u32) -> i32;
type MessageBeepFn = unsafe extern "system" fn(u32) -> i32;

fn load_play_sound_w() -> Option<PlaySoundWFn> {
    static PLAY_SOUND: OnceLock<Option<PlaySoundWFn>> = OnceLock::new();
    *PLAY_SOUND.get_or_init(|| unsafe {
        resolve_proc("winmm.dll", b"PlaySoundW\0").map(|f| std::mem::transmute(f))
    })
}

fn load_message_beep() -> Option<MessageBeepFn> {
    static MESSAGE_BEEP: OnceLock<Option<MessageBeepFn>> = OnceLock::new();
    *MESSAGE_BEEP.get_or_init(|| unsafe {
        resolve_proc("user32.dll", b"MessageBeep\0").map(|f| std::mem::transmute(f))
    })
}

/// 异步非阻塞播放克制的系统提示音，失败时安全回退至 MessageBeep。
pub fn play_cue_sound(kind: CueKind) {
    let aliases: &[&str] = match kind {
        CueKind::ActionNeeded => &["Notification.Default", "SystemNotification", "SystemExclamation"],
        CueKind::Done => &["SystemAsterisk", "SystemNotification"],
    };

    if let Some(play_sound) = load_play_sound_w() {
        let flags = SND_ALIAS | SND_ASYNC | SND_NODEFAULT;
        for alias in aliases {
            let wide_alias = encode_wide(alias);
            let ok = unsafe { play_sound(wide_alias.as_ptr(), null_mut(), flags) };
            if ok != 0 {
                return;
            }
        }
    }

    if let Some(message_beep) = load_message_beep() {
        let beep_kind = match kind {
            CueKind::ActionNeeded => MB_ICONEXCLAMATION,
            CueKind::Done => MB_ICONASTERISK,
        };
        unsafe {
            let _ = message_beep(beep_kind);
        }
    }
}

/// 纯几何判断：窗口矩形 `(left, top, right, bottom)` 是否完全覆盖或近似覆盖整个显示器矩形。
pub fn rect_covers_monitor(win: (i32, i32, i32, i32), mon: (i32, i32, i32, i32)) -> bool {
    if mon.2 <= mon.0 || mon.3 <= mon.1 {
        return false;
    }
    if win.2 <= win.0 || win.3 <= win.1 {
        return false;
    }
    win.0 <= mon.0 + FULLSCREEN_TOLERANCE_PX
        && win.1 <= mon.1 + FULLSCREEN_TOLERANCE_PX
        && win.2 >= mon.2 - FULLSCREEN_TOLERANCE_PX
        && win.3 >= mon.3 - FULLSCREEN_TOLERANCE_PX
}

type SHQueryUserNotificationStateFn = unsafe extern "system" fn(*mut i32) -> i32;

fn query_user_notification_state() -> Option<i32> {
    static QUERY_STATE: OnceLock<Option<SHQueryUserNotificationStateFn>> = OnceLock::new();
    let func = *QUERY_STATE.get_or_init(|| unsafe {
        resolve_proc("shell32.dll", b"SHQueryUserNotificationState\0").map(|f| std::mem::transmute(f))
    });
    let func = func?;
    let mut state: i32 = 0;
    let hr = unsafe { func(&mut state) };
    if hr == 0 {
        Some(state)
    } else {
        None
    }
}

/// 检测系统当前是否处于全屏独占/演示/勿扰模式，或前台窗口全屏覆盖显示器。
pub fn is_fullscreen_or_dnd_active(self_hwnd: HWND) -> bool {
    if let Some(state) = query_user_notification_state() {
        if matches!(
            state,
            QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE
        ) {
            return true;
        }
    }

    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() || fg == self_hwnd {
            return false;
        }
        if IsWindowVisible(fg) == 0 || is_shell_window(fg) {
            return false;
        }

        let mut win_rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(fg, &mut win_rect) == 0 {
            return false;
        }

        let hmon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        if hmon.is_null() {
            return false;
        }

        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) == 0 {
            return false;
        }

        rect_covers_monitor(
            (win_rect.left, win_rect.top, win_rect.right, win_rect.bottom),
            (
                mi.rcMonitor.left,
                mi.rcMonitor.top,
                mi.rcMonitor.right,
                mi.rcMonitor.bottom,
            ),
        )
    }
}

type GetDpiForMonitorFn =
    unsafe extern "system" fn(HMONITOR, i32, *mut u32, *mut u32) -> i32;
type GetDpiForSystemFn = unsafe extern "system" fn() -> u32;

fn monitor_dpi_scale(hmon: HMONITOR) -> f32 {
    static GET_DPI_FOR_MONITOR: OnceLock<Option<GetDpiForMonitorFn>> = OnceLock::new();
    static GET_DPI_FOR_SYSTEM: OnceLock<Option<GetDpiForSystemFn>> = OnceLock::new();

    if !hmon.is_null() {
        let func = *GET_DPI_FOR_MONITOR.get_or_init(|| unsafe {
            resolve_proc("shcore.dll", b"GetDpiForMonitor\0").map(|f| std::mem::transmute(f))
        });
        if let Some(get_dpi_for_monitor) = func {
            let mut dpi_x: u32 = 0;
            let mut dpi_y: u32 = 0;
            let hr = unsafe { get_dpi_for_monitor(hmon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
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

/// 获取目标显示器（主屏或跟随光标所在屏）的坐标原点、宽度及 DPI 缩放比。
pub fn get_target_monitor_placement(follow_cursor: bool) -> MonitorPlacement {
    unsafe {
        let hmon = if follow_cursor {
            let mut pt = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut pt) != 0 {
                MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
            } else {
                MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)
            }
        } else {
            MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)
        };

        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;

        if !hmon.is_null() && GetMonitorInfoW(hmon, &mut mi) != 0 {
            let monitor_w = (mi.rcMonitor.right - mi.rcMonitor.left).max(1);
            let dpi_scale = monitor_dpi_scale(hmon);
            MonitorPlacement {
                origin_x: mi.rcMonitor.left,
                origin_y: mi.rcMonitor.top,
                monitor_w,
                dpi_scale,
            }
        } else {
            let screen_w = GetSystemMetrics(SM_CXSCREEN).max(1);
            let dpi_scale = monitor_dpi_scale(null_mut());
            MonitorPlacement {
                origin_x: 0,
                origin_y: 0,
                monitor_w: screen_w,
                dpi_scale,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_rect_covers_monitor_exact_and_borderless_overhang() {
        let mon = (0, 0, 1920, 1080);
        assert!(rect_covers_monitor((0, 0, 1920, 1080), mon));
        assert!(rect_covers_monitor((-8, -8, 1928, 1088), mon));
        // 1-2px DPI rounding inset still counts as fullscreen
        assert!(rect_covers_monitor((2, 2, 1918, 1078), mon));
    }

    #[test]
    fn test_rect_covers_monitor_excludes_maximized_with_taskbar_and_partial() {
        let mon = (0, 0, 1920, 1080);
        // Normal maximized window stops above 48px taskbar
        assert!(!rect_covers_monitor((-8, -8, 1928, 1040), mon));
        // Partial window
        assert!(!rect_covers_monitor((100, 100, 1800, 900), mon));
        // Different monitor
        assert!(!rect_covers_monitor((1920, 0, 3840, 1080), mon));
        // Degenerate rects
        assert!(!rect_covers_monitor((0, 0, 0, 0), mon));
        assert!(!rect_covers_monitor(mon, (0, 0, 0, 0)));
    }

    #[test]
    fn test_rect_covers_monitor_secondary_negative_origin() {
        let left_mon = (-2560, 0, 0, 1440);
        assert!(rect_covers_monitor((-2560, 0, 0, 1440), left_mon));
        assert!(!rect_covers_monitor((-2560, 0, 0, 1392), left_mon));
    }

    #[test]
    fn test_monitor_placement_centered_window_x() {
        let primary = MonitorPlacement {
            origin_x: 0,
            origin_y: 0,
            monitor_w: 1920,
            dpi_scale: 1.0,
        };
        assert_eq!(primary.centered_window_x(480), 720);

        // Secondary monitor placed to the right with 150% DPI
        let right = MonitorPlacement {
            origin_x: 1920,
            origin_y: 0,
            monitor_w: 2560,
            dpi_scale: 1.5,
        };
        assert_eq!(right.centered_window_x(720), 1920 + 920);

        // Secondary monitor placed to the left (negative origin_x)
        let left = MonitorPlacement {
            origin_x: -1920,
            origin_y: 0,
            monitor_w: 1920,
            dpi_scale: 1.0,
        };
        assert_eq!(left.centered_window_x(480), -1200);
    }

    #[test]
    fn test_format_autostart_command_quotes_and_strips_verbatim_prefix() {
        let p = Path::new(r"\\?\C:\Program Files\WNoobar\wnoobar.exe");
        assert_eq!(
            format_autostart_command(p),
            r#""C:\Program Files\WNoobar\wnoobar.exe""#
        );
    }

    #[test]
    fn test_named_mutex_lock_excludes_second_acquire_and_releases_on_drop() {
        let name = format!("Local\\WNoobarUnitTestMutex_{}", std::process::id());
        let first = acquire_named_mutex_lock(&name);
        assert!(first.is_some());
        let second = acquire_named_mutex_lock(&name);
        assert!(second.is_none());
        drop(first);
        let third = acquire_named_mutex_lock(&name);
        assert!(third.is_some());
    }

    #[test]
    fn test_win32_dynamic_apis_and_monitor_placement_smoke() {
        assert!(load_advapi_reg().is_some());
        assert!(load_play_sound_w().is_some());
        let placement = get_target_monitor_placement(false);
        assert!(placement.monitor_w > 0);
        assert!(placement.dpi_scale > 0.0);
    }
}
