//! JNI bridge between the Android app and maarse-host.
//!
//! The app starts the host in a service that runs in its own process
//! (`:host`), so the host's `process::exit` on a fatal error ends only that
//! process, and stopping the node is ending the process. The UI talks to the
//! host like the web client does: the companion protocol over WebSocket on
//! 127.0.0.1, with the access key (`--require-key`).

use jni::JNIEnv;
use jni::objects::{JClass, JObjectArray, JString};
use jni::sys::jboolean;

/// `HostService.runHost(String[] args, boolean server)`: runs the host with
/// these arguments. Never returns.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dk_maarse_app_HostService_runHost(
    mut env: JNIEnv,
    _class: JClass,
    args: JObjectArray,
    server: jboolean,
) {
    let mut argv = Vec::new();
    let n = env.get_array_length(&args).unwrap_or(0);
    for i in 0..n {
        let Ok(obj) = env.get_object_array_element(&args, i) else { continue };
        let s: JString = obj.into();
        if let Ok(v) = env.get_string(&s) {
            argv.push(String::from(v));
        }
    }
    log::to_logcat();
    maarse_host::set_server_mode(server != 0);
    maarse_host::run(argv);
}

#[cfg(target_os = "android")]
mod log {
    use std::ffi::CString;
    use std::io::{BufRead, BufReader};
    use std::os::fd::FromRawFd;

    unsafe extern "C" {
        fn __android_log_write(prio: i32, tag: *const libc::c_char, text: *const libc::c_char) -> i32;
    }

    /// Android throws stdout and stderr away; send them to logcat instead
    /// (tag "maarse-host"), so `adb logcat` shows what the host prints.
    pub fn to_logcat() {
        for (fd, prio) in [(1, 4 /* INFO */), (2, 6 /* ERROR */)] {
            let mut p = [0; 2];
            // SAFETY: plain POSIX calls on fds we own; the read end is handed
            // to a File that owns it from then on.
            let file = unsafe {
                if libc::pipe(p.as_mut_ptr()) != 0 || libc::dup2(p[1], fd) < 0 {
                    continue;
                }
                libc::close(p[1]);
                std::fs::File::from_raw_fd(p[0])
            };
            std::thread::spawn(move || {
                let tag = CString::new("maarse-host").unwrap();
                for line in BufReader::new(file).lines().map_while(Result::ok) {
                    if let Ok(t) = CString::new(line) {
                        // SAFETY: both pointers are valid C strings for the call.
                        unsafe { __android_log_write(prio, tag.as_ptr(), t.as_ptr()) };
                    }
                }
            });
        }
    }
}

#[cfg(not(target_os = "android"))]
mod log {
    pub fn to_logcat() {}
}
