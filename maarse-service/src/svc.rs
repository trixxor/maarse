//! The Windows service plumbing: registration with the service control manager and the stop signal.

use std::ffi::OsString;
use std::time::Duration;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

use crate::core;
use maarse_service::ipc::SERVICE_NAME;

define_windows_service!(ffi_service_main, service_main);

pub fn run_service() -> Result<(), String> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .map_err(|e| format!("not started by the service manager ({e}); use 'maarse-service run' to test in a console"))
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run() {
        core::log_line(&format!("service failed: {e}"));
    }
}

fn status(state: ServiceState, accept: ServiceControlAccept, exit: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: accept,
        exit_code: ServiceExitCode::Win32(exit),
        checkpoint: 0,
        wait_hint: Duration::from_secs(20),
        process_id: None,
    }
}

fn run() -> Result<(), String> {
    let handler = service_control_handler::register(SERVICE_NAME, |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            core::request_stop();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })
    .map_err(|e| e.to_string())?;
    let none = ServiceControlAccept::empty();
    let _ = handler.set_service_status(status(ServiceState::StartPending, none, 0));
    let started = core::start();
    let accept = ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN;
    let _ = handler.set_service_status(status(ServiceState::Running, accept, 0));
    if let Err(e) = &started {
        core::log_line(&format!("start failed: {e}"));
    }
    core::wait_for_stop();
    let _ = handler.set_service_status(status(ServiceState::StopPending, none, 0));
    core::shutdown();
    let _ = handler.set_service_status(status(ServiceState::Stopped, none, 0));
    Ok(())
}

/// Runs everything in the console until Enter is pressed (testing).
pub fn run_console() -> Result<(), String> {
    core::CONSOLE.store(true, std::sync::atomic::Ordering::SeqCst);
    core::start()?;
    println!("maarse-service running in the console. Press Enter to stop.");
    let _ = std::io::stdin().read_line(&mut String::new());
    core::request_stop();
    core::wait_for_stop();
    core::shutdown();
    Ok(())
}
