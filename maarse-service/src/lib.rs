//! Shared by the Windows service (`maarse-service.exe`) and the tray app (`maarse.exe`):
//! the update logic with signature check, and the protocol on the named pipe between them.

pub mod ipc;
pub mod update;
