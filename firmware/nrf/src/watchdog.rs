//! The nRF52840's hardware watchdog (see `maarse_fw_common::health`).
//!
//! Once started it cannot be stopped, and it keeps running through a soft reset: into a
//! newly installed app, and into the UF2 bootloader, which feeds it itself while it waits
//! for a file. It runs while the CPU sleeps (a hung task leaves the CPU asleep) and stops
//! in System OFF (the tag powered off).

use maarse_fw_common::health;

const WDT_TASKS_START: *mut u32 = 0x4001_0000 as *mut u32;
const WDT_CRV: *mut u32 = 0x4001_0504 as *mut u32;
const WDT_CONFIG: *mut u32 = 0x4001_050C as *mut u32;
pub const WDT_RR0: *mut u32 = 0x4001_0600 as *mut u32;
/// What must be written to a reload register to feed the watchdog.
pub const RELOAD: u32 = 0x6E52_4635;

pub fn feed() {
    // SAFETY: a write of the reload value to the watchdog's first reload register.
    unsafe { core::ptr::write_volatile(WDT_RR0, RELOAD) };
}

/// Starts the watchdog. If it already runs (after a soft reset) the hardware ignores the
/// settings, and this only feeds it.
pub fn start() {
    // SAFETY: plain register writes to the watchdog, which only this module uses (and the
    // update routine, which feeds it while interrupts are off).
    unsafe {
        // 32.768 kHz ticks. SLEEP = run while the CPU sleeps, HALT = pause under a debugger.
        core::ptr::write_volatile(WDT_CRV, health::TIMEOUT_S * 32_768 - 1);
        core::ptr::write_volatile(WDT_CONFIG, 1);
        core::ptr::write_volatile(WDT_TASKS_START, 1);
    }
    feed();
}

#[embassy_executor::task]
pub async fn task() {
    health::supervise(feed).await
}
