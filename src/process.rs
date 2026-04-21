use serde::Serialize;
use std::cell::Cell;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::Win32::System::ProcessStatus::GetModuleFileNameExW;
use windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_SUSPEND_RESUME,
};
use windows::Win32::System::Threading::{
    OpenThread, THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    SW_MINIMIZE, SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows::core::PCSTR;

pub type NtSuspendProcess = unsafe extern "system" fn(HANDLE) -> u32;

#[derive(Serialize)]
pub struct AppInfo {
    pub pid: u32,
    pub name: String,
    pub path: String,
}

#[derive(Serialize)]
pub struct AppDetail {
    pub pid: u32,
    pub name: String,
    pub state: String,
    pub path: String,
}

/// A running window with its cached suspension state.
pub struct WindowEntry {
    pub info: AppInfo,
    pub suspended: bool,
}

/// Returns the PID of the process that owns the current foreground window,
/// or `None` if there is no foreground window (e.g. the desktop).
pub fn get_foreground_pid() -> Option<u32> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == std::ptr::null_mut() {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 { None } else { Some(pid) }
    }
}

pub fn get_process_path(pid: u32) -> String {
    unsafe {
        let handle = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_QUERY_INFORMATION,
            false,
            pid,
        );
        if let Ok(handle) = handle {
            let mut buf = [0u16; 512];
            let len = GetModuleFileNameExW(handle, None, &mut buf);
            if len > 0 {
                return String::from_utf16_lossy(&buf[..len as usize]);
            }
        }
    }
    String::new()
}

pub fn get_all_windows() -> Vec<AppInfo> {
    let mut apps = Vec::new();
    unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if unsafe { IsWindowVisible(hwnd).as_bool() } {
            let mut title = [0u16; 512];
            let len = unsafe { GetWindowTextW(hwnd, &mut title) };
            if len > 0 {
                let title = String::from_utf16_lossy(&title[..len as usize]);
                let apps = unsafe { &mut *(lparam.0 as *mut Vec<AppInfo>) };
                let path = get_process_path(pid);
                apps.push(AppInfo {
                    pid,
                    name: title,
                    path,
                });
            }
        }
        BOOL(1)
    }
    let apps_ptr = &mut apps as *mut _ as isize;
    unsafe {
        let _ = EnumWindows(Some(enum_windows_proc), LPARAM(apps_ptr));
    }
    apps
}

/// Build the full list of visible windows, checking each process's suspension state.
pub fn get_all_window_entries() -> Vec<WindowEntry> {
    get_all_windows()
        .into_iter()
        .map(|info| {
            let suspended = is_process_suspended(info.pid).unwrap_or(false);
            WindowEntry { info, suspended }
        })
        .collect()
}

pub fn find_window_by_pid(target_pid: u32) -> Option<HWND> {
    thread_local! {
        static FOUND_HWND: Cell<HWND> = Cell::new(HWND::default());
    }
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == lparam.0 as u32 && unsafe { IsWindowVisible(hwnd).as_bool() } {
            FOUND_HWND.with(|cell| cell.set(hwnd));
            return BOOL(0); // stop enumeration
        }
        BOOL(1)
    }
    FOUND_HWND.with(|cell| cell.set(HWND::default()));
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(target_pid as isize));
    }
    let hwnd = FOUND_HWND.with(|cell| cell.get());
    if hwnd != HWND::default() {
        Some(hwnd)
    } else {
        None
    }
}

pub fn is_process_suspended(pid: u32) -> windows::core::Result<bool> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?;
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut all_suspended = true;
        let mut has_thread = false;
        if Thread32First(snapshot, &mut entry).is_ok() {
            loop {
                if entry.th32OwnerProcessID == pid {
                    has_thread = true;
                    let hthread = OpenThread(
                        THREAD_SUSPEND_RESUME | THREAD_QUERY_INFORMATION,
                        false,
                        entry.th32ThreadID,
                    );
                    if let Ok(hthread) = hthread {
                        let prev = windows::Win32::System::Threading::SuspendThread(hthread);
                        if prev == u32::MAX {
                            all_suspended = false;
                        } else {
                            if prev == 0 {
                                all_suspended = false;
                            }
                            windows::Win32::System::Threading::ResumeThread(hthread);
                        }
                    } else {
                        all_suspended = false;
                    }
                }
                if Thread32Next(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        Ok(has_thread && all_suspended)
    }
}

pub fn suspend_process_by_pid(pid: u32) -> windows::core::Result<()> {
    unsafe {
        let handle = OpenProcess(
            PROCESS_SUSPEND_RESUME | PROCESS_QUERY_INFORMATION,
            false,
            pid,
        )?;
        if handle.is_invalid() {
            return Err(windows::core::Error::from_win32());
        }
        let ntdll = GetModuleHandleA(PCSTR(b"ntdll.dll\0".as_ptr()))?;
        let func = GetProcAddress(ntdll, PCSTR(b"NtSuspendProcess\0".as_ptr()));
        let nt_suspend: NtSuspendProcess = match func {
            Some(f) => std::mem::transmute(f),
            None => return Err(windows::core::Error::from_win32()),
        };
        let status = nt_suspend(handle);
        if status != 0 {
            return Err(windows::core::Error::from_win32());
        }
        Ok(())
    }
}

pub fn suspend_process_by_pid_and_minimize(pid: u32) -> windows::core::Result<()> {
    if let Some(hwnd) = find_window_by_pid(pid) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
        }
    }
    suspend_process_by_pid(pid)
}

pub fn resume_process_by_pid(pid: u32) -> windows::core::Result<()> {
    unsafe {
        let handle = OpenProcess(
            PROCESS_SUSPEND_RESUME | PROCESS_QUERY_INFORMATION,
            false,
            pid,
        )?;
        if handle.is_invalid() {
            return Err(windows::core::Error::from_win32());
        }
        let ntdll = GetModuleHandleA(PCSTR(b"ntdll.dll\0".as_ptr()))?;
        let func = GetProcAddress(ntdll, PCSTR(b"NtResumeProcess\0".as_ptr()));
        let nt_resume: NtSuspendProcess = match func {
            Some(f) => std::mem::transmute(f),
            None => return Err(windows::core::Error::from_win32()),
        };
        let status = nt_resume(handle);
        if status != 0 {
            return Err(windows::core::Error::from_win32());
        }
        Ok(())
    }
}

pub fn resume_process_by_pid_and_restore(pid: u32) -> windows::core::Result<()> {
    resume_process_by_pid(pid)?;
    if let Some(hwnd) = find_window_by_pid(pid) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
    }
    Ok(())
}

pub fn focus_window_by_pid(pid: u32) -> windows::core::Result<()> {
    if let Some(hwnd) = find_window_by_pid(pid) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
        Ok(())
    } else {
        Err(windows::core::Error::from_win32())
    }
}
