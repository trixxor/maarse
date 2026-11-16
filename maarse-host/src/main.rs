//! maarse-host: gateway and test node for Må:rse. Runs on Windows and Linux.
//!
//! * Runs a full Må:rse node with its own identity and encryption.
//! * Connects to other maarse-host instances over TCP (backbone).
//! * Can use a LoRa node over serial in modem mode (requires firmware).
//! * Serves the web client at http://127.0.0.1:8080, which talks to the node
//!   over WebSocket.
//!
//! Start:  maarse-host
//! Help:   maarse-host --help
#![forbid(unsafe_code)]

include!("app.rs");
