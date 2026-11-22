//! The named pipe server. Reachable only by SYSTEM, administrators and the interactive (signed-in
//! local) user, never over the network.

use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};

use windows_sys::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

use crate::core;
use maarse_service::ipc::PIPE;

/// SYSTEM and administrators: full control. Interactive users: read and write.
const SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn spawn_server() {
    std::thread::spawn(|| {
        loop {
            if let Err(e) = serve_one() {
                core::log_line(&format!("pipe: {e}"));
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        }
    });
}

/// Waits for one client, answers it and disconnects. Each client is handled in its own thread, so a slow
/// command (a check takes a few seconds) does not block the next one.
fn serve_one() -> Result<(), String> {
    let handle = unsafe {
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(wide(SDDL).as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) == 0 {
            return Err("could not build the pipe security descriptor".into());
        }
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
        let h = CreateNamedPipeW(
            wide(PIPE).as_ptr(),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            &sa,
        );
        LocalFree(sd);
        h
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!("CreateNamedPipe failed ({})", unsafe { GetLastError() }));
    }
    let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED };
    if !connected {
        drop(unsafe { File::from_raw_handle(handle as RawHandle) });
        return Err("ConnectNamedPipe failed".into());
    }
    let raw = handle as usize;
    std::thread::spawn(move || {
        let handle = raw as RawHandle;
        let mut file = unsafe { File::from_raw_handle(handle) };
        // One command line, at most 64 bytes.
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while line.len() < 64 && file.read(&mut byte).map(|n| n == 1).unwrap_or(false) && byte[0] != b'\n' {
            line.push(byte[0]);
        }
        let answer = core::handle(&String::from_utf8_lossy(&line));
        let text = serde_json::to_string(&answer).unwrap_or_default();
        let _ = file.write_all(text.as_bytes());
        let _ = file.sync_all();
        unsafe { DisconnectNamedPipe(handle as _) };
    });
    Ok(())
}
