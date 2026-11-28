//! What a PC test binary lacks and the radio driver's logging expects from a board. Nothing is logged.
#![allow(non_upper_case_globals)]

// The channels between the tasks. The PC tests are single-threaded and never
// run them; the linker only wants the names.
#[unsafe(no_mangle)]
fn _critical_section_1_0_acquire() {}
#[unsafe(no_mangle)]
fn _critical_section_1_0_release(_: ()) {}
#[unsafe(no_mangle)]
fn _defmt_acquire() {}
#[unsafe(no_mangle)]
fn _defmt_release() {}
#[unsafe(no_mangle)]
fn _defmt_flush() {}
#[unsafe(no_mangle)]
fn _defmt_write(_: &[u8]) {}
#[unsafe(no_mangle)]
fn _defmt_timestamp(_: u8) {}
#[unsafe(no_mangle)]
fn _defmt_panic() -> ! {
    panic!("the radio driver panicked")
}
macro_rules! markers {
    ($($name:ident)*) => { $(#[unsafe(no_mangle)] static $name: u8 = 0;)* };
}
markers!(__DEFMT_MARKER_TRACE_START __DEFMT_MARKER_TRACE_END __DEFMT_MARKER_DEBUG_START __DEFMT_MARKER_DEBUG_END
    __DEFMT_MARKER_INFO_START __DEFMT_MARKER_INFO_END __DEFMT_MARKER_WARN_START __DEFMT_MARKER_WARN_END
    __DEFMT_MARKER_ERROR_START __DEFMT_MARKER_ERROR_END);
