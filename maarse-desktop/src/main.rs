//! Må:rse tray app for Windows (`maarse.exe`).
//!
//! The Windows service (`maarse-service.exe`, runs as SYSTEM from boot) runs the host and installs
//! updates. This app runs as the signed-in user, needs no rights, and
//! * shows a splash screen and asks the service to check GitHub for a newer release; if there is one it
//!   asks "Update now?" (No simply goes on);
//! * keeps an icon in the notification area (the taskbar) that shows whether the service and host run,
//!   with a blue dot when an update is ready; the menu shows what changed (the release notes on GitHub)
//!   and has the update button;
//! * opens the PC client in the browser.
//!
//! Option: `--no-browser` (do not open the web client at start, e.g. when started with Windows).

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

#[cfg(not(windows))]
fn main() {
    eprintln!("maarse (desktop app) is Windows only.");
}

#[cfg(windows)]
fn main() {
    if let Err(e) = win::run() {
        win::error_box(&e);
        std::process::exit(1);
    }
}
