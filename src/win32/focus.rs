#![allow(dead_code)]

use std::ptr::null_mut;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HANDLE, HWND, LPARAM};
use windows_sys::Win32::System::Threading::{GetCurrentThreadId, OpenProcess};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, SW_RESTORE,
    SW_SHOW,
};

use super::system::{is_shell_window, resolve_proc};

const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

const TERMINAL_HOSTS: &[&str] = &[
    "windowsterminal.exe",
    "wt.exe",
    "powershell.exe",
    "pwsh.exe",
    "cmd.exe",
    "wezterm-gui.exe",
    "alacritty.exe",
];

const CLI_AGENT_HOSTS: &[&str] = &[
    "windowsterminal.exe",
    "wt.exe",
    "powershell.exe",
    "pwsh.exe",
    "cmd.exe",
    "wezterm-gui.exe",
    "alacritty.exe",
    "code.exe",
    "cursor.exe",
    "antigravity.exe",
];

const CURSOR_HOSTS: &[&str] = &["cursor.exe"];
const ANTIGRAVITY_HOSTS: &[&str] = &["antigravity.exe"];
const VSCODE_HOSTS: &[&str] = &["code.exe", "code - insiders.exe"];

fn host_processes_for_agent(agent_name: &str) -> (&'static [&'static str], bool) {
    let lower = agent_name.trim().to_ascii_lowercase();
    if lower.contains("cursor") {
        (CURSOR_HOSTS, false)
    } else if lower.contains("antigravity") {
        (ANTIGRAVITY_HOSTS, false)
    } else if lower.contains("copilot") || lower.contains("vscode") || lower.contains("vs code") {
        (VSCODE_HOSTS, false)
    } else if lower.contains("claude")
        || lower.contains("codex")
        || lower.contains("opencode")
        || lower.contains("deepseek")
        || lower.contains("grok")
    {
        (CLI_AGENT_HOSTS, true)
    } else {
        (&[], false)
    }
}

/// 纯函数：根据窗口标题、进程可执行文件名与会话上下文计算聚焦匹配得分。
/// 项目名与宿主进程双命中给高分；仅宿主命中给兜底分；无关窗口返回 `<= 0`。
pub fn score_window_candidate(
    title: &str,
    exe_name: &str,
    project_name: &str,
    agent_name: &str,
    role_type: &str,
) -> i32 {
    let title_trimmed = title.trim();
    if title_trimmed.is_empty() {
        return 0;
    }

    let exe_base = exe_name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(exe_name)
        .trim()
        .to_ascii_lowercase();

    if exe_base == "wnoobar.exe" {
        return 0;
    }

    let (hosts, is_cli_agent) = host_processes_for_agent(agent_name);
    let host_matched = !hosts.is_empty() && hosts.iter().any(|&h| h == exe_base);

    // 若该 Agent 存在明确的宿主进程映射，但当前窗口不属于任何候选宿主进程，则视为无关窗口。
    if !hosts.is_empty() && !host_matched {
        return 0;
    }

    let title_lower = title_trimmed.to_lowercase();
    let proj_trimmed = project_name.trim();
    let proj_lower = proj_trimmed.to_lowercase();
    let is_fallback_wnoobar = proj_lower == "wnoobar";
    let proj_matched = !proj_lower.is_empty() && title_lower.contains(&proj_lower);

    // 未知 Agent 时仅允许非空且非默认兜底名 "WNoobar" 的项目标题命中。
    if hosts.is_empty() {
        return if proj_matched && !is_fallback_wnoobar {
            40
        } else {
            0
        };
    }

    let mut score: i32 = 25;

    if is_cli_agent && TERMINAL_HOSTS.iter().any(|&t| t == exe_base) {
        score += 5;
    }

    if proj_matched {
        // 当 project_name 为默认兜底值 "WNoobar" 时适度降权，避免压过或误抢其他关键信号。
        score += if is_fallback_wnoobar { 15 } else { 100 };
    }

    let agent_lower = agent_name.trim().to_lowercase();
    let role_lower = role_type.trim().to_lowercase();

    if !agent_lower.is_empty() && title_lower.contains(&agent_lower) {
        score += 20;
    } else if agent_lower.contains("copilot") {
        if title_lower.contains("copilot") {
            score += 20;
        } else if title_lower.contains("visual studio code") {
            score += 10;
        }
    } else if agent_lower.contains("claude") && title_lower.contains("claude") {
        score += 20;
    }

    if !role_lower.is_empty() && role_lower != agent_lower && title_lower.contains(&role_lower) {
        score += 15;
    }

    score
}

type QueryFullProcessImageNameWFn =
    unsafe extern "system" fn(HANDLE, u32, *mut u16, *mut u32) -> i32;

fn get_process_exe_name(pid: u32) -> Option<String> {
    static QUERY_PROC_NAME: OnceLock<Option<QueryFullProcessImageNameWFn>> = OnceLock::new();
    let query_fn = (*QUERY_PROC_NAME.get_or_init(|| unsafe {
        resolve_proc("kernel32.dll", b"QueryFullProcessImageNameW\0")
            .map(|f| std::mem::transmute(f))
    }))?;

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }

        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = query_fn(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(handle);

        if ok == 0 || size == 0 {
            return None;
        }

        let full_path = String::from_utf16_lossy(&buf[..size as usize]);
        let file_name = full_path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(&full_path)
            .to_string();
        Some(file_name)
    }
}

struct EnumSearchCtx<'a> {
    self_hwnd: HWND,
    project_name: &'a str,
    agent_name: &'a str,
    role_type: &'a str,
    best_hwnd: HWND,
    best_score: i32,
}

unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    unsafe {
        if lparam == 0 {
            return 0;
        }
        let ctx = &mut *(lparam as *mut EnumSearchCtx<'_>);

        if hwnd.is_null() || hwnd == ctx.self_hwnd {
            return 1;
        }
        if IsWindowVisible(hwnd) == 0 || is_shell_window(hwnd) {
            return 1;
        }

        let title_len = GetWindowTextLengthW(hwnd);
        if title_len <= 0 {
            return 1;
        }

        let mut title_buf = vec![0u16; (title_len as usize) + 1];
        let copied = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32);
        if copied <= 0 {
            return 1;
        }
        let title = String::from_utf16_lossy(&title_buf[..copied as usize]);
        if title.trim().is_empty() {
            return 1;
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return 1;
        }
        let exe_name = get_process_exe_name(pid).unwrap_or_default();

        let score = score_window_candidate(
            &title,
            &exe_name,
            ctx.project_name,
            ctx.agent_name,
            ctx.role_type,
        );
        // EnumWindows 按 Z-order 由前到后遍历，严格大于时替换，天然优先最近活跃窗口
        if score > ctx.best_score {
            ctx.best_score = score;
            ctx.best_hwnd = hwnd;
        }
        1
    }
}

type AttachThreadInputFn = unsafe extern "system" fn(u32, u32, i32) -> i32;

fn attach_thread_input(id_attach: u32, id_attach_to: u32, attach: bool) -> bool {
    static ATTACH_FN: OnceLock<Option<AttachThreadInputFn>> = OnceLock::new();
    let Some(func) = *ATTACH_FN.get_or_init(|| unsafe {
        resolve_proc("user32.dll", b"AttachThreadInput\0").map(|f| std::mem::transmute(f))
    }) else {
        return false;
    };
    unsafe { func(id_attach, id_attach_to, if attach { 1 } else { 0 }) != 0 }
}

/// 枚举可见顶层窗口并安全拉起与目标会话最匹配的 Agent/IDE/终端窗口。
pub fn focus_session_window(
    self_hwnd: HWND,
    project_name: &str,
    agent_name: &str,
    role_type: &str,
) -> bool {
    unsafe {
        let mut ctx = EnumSearchCtx {
            self_hwnd,
            project_name,
            agent_name,
            role_type,
            best_hwnd: null_mut(),
            best_score: 0,
        };

        EnumWindows(
            Some(enum_windows_proc),
            &mut ctx as *mut EnumSearchCtx<'_> as LPARAM,
        );

        let target = ctx.best_hwnd;
        if target.is_null() || ctx.best_score <= 0 {
            return false;
        }

        let fg_hwnd = GetForegroundWindow();
        let current_tid = GetCurrentThreadId();
        let fg_tid = if !fg_hwnd.is_null() {
            GetWindowThreadProcessId(fg_hwnd, null_mut())
        } else {
            0
        };
        let target_tid = GetWindowThreadProcessId(target, null_mut());

        let attached_fg =
            fg_tid != 0 && fg_tid != current_tid && attach_thread_input(current_tid, fg_tid, true);
        let attached_target = target_tid != 0
            && target_tid != current_tid
            && target_tid != fg_tid
            && attach_thread_input(current_tid, target_tid, true);

        if IsIconic(target) != 0 {
            ShowWindow(target, SW_RESTORE);
        }
        let _ = BringWindowToTop(target);
        let mut focused = SetForegroundWindow(target) != 0;
        if !focused {
            ShowWindow(target, SW_SHOW);
            focused = SetForegroundWindow(target) != 0;
        }

        if attached_target {
            attach_thread_input(current_tid, target_tid, false);
        }
        if attached_fg {
            attach_thread_input(current_tid, fg_tid, false);
        }

        focused || GetForegroundWindow() == target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_cursor_matching_and_fallback() {
        let exact = score_window_candidate(
            "main.rs - my-saas - Cursor",
            "Cursor.exe",
            "my-saas",
            "Cursor",
            "Cursor Composer",
        );
        let host_only = score_window_candidate(
            "other-repo - Cursor",
            "cursor.exe",
            "my-saas",
            "Cursor",
            "Cursor Composer",
        );
        let wrong_ide = score_window_candidate(
            "my-saas - Visual Studio Code",
            "Code.exe",
            "my-saas",
            "Cursor",
            "Cursor Composer",
        );

        assert!(exact > host_only);
        assert!(host_only > 0);
        assert!(wrong_ide <= 0);
    }

    #[test]
    fn test_score_copilot_vscode_and_insiders() {
        let stable = score_window_candidate(
            "app.ts - billing-api - Visual Studio Code",
            "Code.exe",
            "billing-api",
            "GitHub Copilot",
            "Copilot Chat",
        );
        let insiders = score_window_candidate(
            "Copilot Chat - billing-api - Visual Studio Code - Insiders",
            "Code - Insiders.exe",
            "billing-api",
            "GitHub Copilot",
            "Copilot Chat",
        );
        let host_only = score_window_candidate(
            "notes - Visual Studio Code",
            "code.exe",
            "billing-api",
            "GitHub Copilot",
            "Copilot Chat",
        );

        assert!(insiders > stable);
        assert!(stable > host_only);
        assert!(host_only > 0);
    }

    #[test]
    fn test_score_antigravity_and_wnoobar_caution() {
        let normal_proj = score_window_candidate(
            "QQ-BOT - Antigravity",
            "Antigravity.exe",
            "QQ-BOT",
            "Antigravity",
            "AI 结对助手",
        );
        let wnoobar_in_ag = score_window_candidate(
            "WNoobar - Antigravity",
            "Antigravity.exe",
            "WNoobar",
            "Antigravity",
            "AI 结对助手",
        );
        let wnoobar_other_ag = score_window_candidate(
            "OtherProj - Antigravity",
            "antigravity.exe",
            "WNoobar",
            "Antigravity",
            "AI 结对助手",
        );
        let wnoobar_in_chrome = score_window_candidate(
            "WNoobar - GitHub - Google Chrome",
            "chrome.exe",
            "WNoobar",
            "Antigravity",
            "AI 结对助手",
        );
        let self_window = score_window_candidate(
            "WNoobar",
            "wnoobar.exe",
            "WNoobar",
            "Antigravity",
            "AI 结对助手",
        );

        assert!(normal_proj > wnoobar_in_ag);
        assert!(wnoobar_in_ag > wnoobar_other_ag);
        assert!(wnoobar_other_ag > 0);
        assert!(wnoobar_in_chrome <= 0);
        assert!(self_window <= 0);
    }

    #[test]
    fn test_score_cli_agents_claude_codex_opencode_deepseek_grok() {
        let claude_wt = score_window_candidate(
            "Claude Code - QQ-BOT",
            "WindowsTerminal.exe",
            "QQ-BOT",
            "Claude",
            "Claude Code",
        );
        let claude_vscode = score_window_candidate(
            "QQ-BOT - Visual Studio Code",
            "code.exe",
            "QQ-BOT",
            "Claude",
            "Claude Code",
        );
        let codex_pwsh = score_window_candidate(
            "Codex - backend-svc",
            "pwsh.exe",
            "backend-svc",
            "Codex",
            "Codex 编程助手",
        );
        let codex_fallback = score_window_candidate(
            "Windows PowerShell",
            "powershell.exe",
            "backend-svc",
            "Codex",
            "Codex 编程助手",
        );
        let opencode_wez = score_window_candidate(
            "OpenCode - rust-core",
            "wezterm-gui.exe",
            "rust-core",
            "OpenCode",
            "OpenCode",
        );
        let deepseek_alacritty = score_window_candidate(
            "DeepSeek - algo-lab",
            "alacritty.exe",
            "algo-lab",
            "DeepSeek",
            "DeepSeek 思考模型",
        );
        let grok_cmd = score_window_candidate(
            "Grok - crawler",
            "cmd.exe",
            "crawler",
            "Grok",
            "Grok 4.6",
        );

        assert!(claude_wt > claude_vscode);
        assert!(claude_vscode > codex_fallback);
        assert!(codex_pwsh > codex_fallback);
        assert!(codex_fallback > 0);
        assert!(opencode_wez > 50);
        assert!(deepseek_alacritty > 50);
        assert!(grok_cmd > 50);
    }

    #[test]
    fn test_score_unrelated_and_empty_inputs() {
        assert!(score_window_candidate("", "cursor.exe", "proj", "Cursor", "") <= 0);
        assert!(score_window_candidate("   ", "wt.exe", "proj", "Claude", "") <= 0);
        assert!(
            score_window_candidate(
                "YouTube - Google Chrome",
                "chrome.exe",
                "proj",
                "Antigravity",
                "架构师"
            ) <= 0
        );
        assert!(
            score_window_candidate(
                "proj - File Explorer",
                "explorer.exe",
                "proj",
                "Codex",
                "Codex"
            ) <= 0
        );
        // Empty project name still gives host fallback, never unrelated window
        assert!(
            score_window_candidate("Antigravity", "antigravity.exe", "", "Antigravity", "") > 0
        );
        assert!(score_window_candidate("Some App", "notepad.exe", "", "Antigravity", "") <= 0);
    }

    #[test]
    fn test_get_process_exe_name_for_current_process() {
        let exe = get_process_exe_name(std::process::id());
        assert!(exe.is_some());
        let name = exe.unwrap().to_ascii_lowercase();
        assert!(name.ends_with(".exe"));
        assert!(name.contains("wnoobar"));
    }
}
