//! `maarse-service install` and `uninstall`: register the service with the service control manager.

use std::ffi::OsString;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::time::Duration;

use windows_service::service::{
    ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use maarse_service::ipc::SERVICE_NAME;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn install() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE)
        .map_err(|e| format!("Run this from an administrator prompt ({e})"))?;
    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from("Maarse"),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    };
    let access = ServiceAccess::START | ServiceAccess::CHANGE_CONFIG | ServiceAccess::QUERY_STATUS;
    let service = match manager.create_service(&info, access) {
        Ok(s) => s,
        // Already there (a reinstall): open it and make sure it points at this exe.
        Err(_) => {
            let s = manager.open_service(SERVICE_NAME, access).map_err(|e| e.to_string())?;
            s.change_config(&info).map_err(|e| e.to_string())?;
            s
        }
    };
    let _ = service.set_description("Runs the Maarse PC client in the background and keeps it up to date.");
    // Start again 5 s after a crash.
    let _ = Command::new("sc.exe")
        .args(["failure", SERVICE_NAME, "reset=", "86400", "actions=", "restart/5000/restart/5000/restart/30000"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match service.start::<&str>(&[]) {
        Ok(()) => println!("Maarse service installed and started."),
        Err(e) => println!("Maarse service installed, but it did not start: {e}"),
    }
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| format!("Run this from an administrator prompt ({e})"))?;
    let access = ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS;
    let Ok(service) = manager.open_service(SERVICE_NAME, access) else {
        println!("The Maarse service is not installed.");
        return Ok(());
    };
    if service.query_status().map(|s| s.current_state != ServiceState::Stopped).unwrap_or(false) {
        let _ = service.stop();
        for _ in 0..40 {
            if service.query_status().map(|s| s.current_state == ServiceState::Stopped).unwrap_or(true) {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    service.delete().map_err(|e| e.to_string())?;
    println!("Maarse service removed. Your data in %ProgramData%\\Maarse was kept.");
    Ok(())
}
