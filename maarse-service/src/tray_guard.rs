//! Keeps the tray icon up while the service runs.
//!
//! The service runs in session 0 and cannot show an icon itself. After a self-update the old tray app
//! is replaced on disk but is not started again, and before the first sign-in there is none at all. So
//! the service starts `maarse.exe` in the signed-in user's session once per session, using that user's
//! own token (the tray app never runs as SYSTEM). A tray app the person closed on purpose is left
//! closed until the next sign-in or service start.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows_sys::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId, WTSQueryUserToken};
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
};

const NO_SESSION: u32 = 0xFFFF_FFFF;
const EVERY: Duration = Duration::from_secs(10);

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Is a `maarse.exe` (the tray app) running in this session?
fn tray_running_in(session: u32) -> bool {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut entry);
        while ok != 0 {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if name.eq_ignore_ascii_case("maarse.exe") {
                let mut s = 0u32;
                if ProcessIdToSessionId(entry.th32ProcessID, &mut s) != 0 && s == session {
                    found = true;
                    break;
                }
            }
            ok = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
        found
    }
}

/// Starts the tray app as the user signed in on `session`.
fn launch_for(session: u32, exe: &std::path::Path) -> Result<(), String> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if WTSQueryUserToken(session, &mut token) == 0 {
            return Err("no user is signed in yet".into());
        }
        let mut env: *mut c_void = std::ptr::null_mut();
        if CreateEnvironmentBlock(&mut env, token, 0) == 0 {
            env = std::ptr::null_mut();
        }
        let app = wide(exe.as_os_str());
        let mut cmd = wide(std::ffi::OsStr::new(&format!("\"{}\" --no-browser", exe.display())));
        let mut desktop = wide(std::ffi::OsStr::new(r"winsta0\default"));
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.lpDesktop = desktop.as_mut_ptr();
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let dir = wide(exe.parent().unwrap_or(std::path::Path::new("")).as_os_str());
        let ok = CreateProcessAsUserW(
            token,
            app.as_ptr(),
            cmd.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            env,
            dir.as_ptr(),
            &si,
            &mut pi,
        );
        let err = std::io::Error::last_os_error();
        if !env.is_null() {
            DestroyEnvironmentBlock(env);
        }
        CloseHandle(token);
        if ok == 0 {
            return Err(err.to_string());
        }
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
        Ok(())
    }
}

/// Watches the console session until the service stops.
pub fn spawn(install_dir: std::path::PathBuf, log: fn(&str), stopping: fn() -> bool) {
    std::thread::spawn(move || {
        let exe = install_dir.join("maarse.exe");
        let mut handled: Option<u32> = None;
        while !stopping() {
            let session = unsafe { WTSGetActiveConsoleSessionId() };
            if session != NO_SESSION && session != 0 && handled != Some(session) && exe.is_file() {
                if tray_running_in(session) {
                    handled = Some(session);
                } else {
                    match launch_for(session, &exe) {
                        Ok(()) => {
                            log(&format!("started the tray app in session {session}"));
                            handled = Some(session);
                        }
                        // Still on the sign-in screen: try again on the next round.
                        Err(e) => log(&format!("tray app not started in session {session}: {e}")),
                    }
                }
            }
            std::thread::sleep(EVERY);
        }
    });
}
