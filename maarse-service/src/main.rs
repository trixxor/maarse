//! Må:rse Windows service (`maarse-service.exe`).
//!
//! Runs as LocalSystem from boot, so no user has to be signed in and no rights problems arise. It
//! * runs `maarse-host.exe` next to it and starts it again when it stops,
//! * looks for a newer release on GitHub and installs it, but only if the package is signed with the
//!   Maarse release key (see `update.rs`),
//! * answers the tray app (`maarse.exe`) over a named pipe (see `ipc.rs`).
//!
//! Commands (administrator):
//!   maarse-service install     register the service (starts at boot) and start it
//!   maarse-service uninstall   stop and remove the service (the data folder is kept)
//!   maarse-service run         run in this console instead of as a service (for testing)

#[cfg(not(windows))]
fn main() {
    eprintln!("maarse-service is Windows only.");
}

#[cfg(windows)]
mod core;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
mod svc;
#[cfg(windows)]
mod tray_guard;

#[cfg(windows)]
fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let result = match arg.as_str() {
        "install" => install::install(),
        "uninstall" => install::uninstall(),
        "run" => svc::run_console(),
        "" => svc::run_service(),
        other => Err(format!("unknown command: {other}\nUsage: maarse-service install | uninstall | run")),
    };
    if let Err(e) = result {
        eprintln!("maarse-service: {e}");
        core::log_line(&format!("fatal: {e}"));
        std::process::exit(1);
    }
}
