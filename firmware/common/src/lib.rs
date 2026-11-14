//! Fælles firmware-logik for Må:rse. Board-crates (nRF52840, ESP32-S3)
//! leverer radio, USB/seriel, flash og eventuelt display.

#![no_std]

pub mod bus;
pub mod node;
pub mod power;
pub mod radio;
pub mod storage;
