//! Shared firmware logic for Må:rse. Board crates (nRF52840, ESP32-S3)
//! provide radio, USB/serial, flash and possibly a display.

#![no_std]

pub mod bme280;
pub mod i2c_sensor;
pub mod bus;
pub mod gps;
pub mod node;
pub mod power;
pub mod ota;
pub mod radio;
pub mod storage;
