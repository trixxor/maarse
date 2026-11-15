//! # maarse-core
//!
//! The core of the Må:rse protocol: addresses, packet format, duplicate filter,
//! route table, airtime budget and the routing engine.
//!
//! The crate is `no_std` and uses no heap, so it can run directly on an
//! nRF52840. All tables have fixed sizes, chosen with const generics.
//!
//! See the specification "Må:rse – protocol specification v0.1".

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod addr;
pub mod appdata;
pub mod battery;
pub mod airtime;
pub mod companion;
pub mod content;
pub mod dedup;
pub mod diag;
pub mod duty;
pub mod packet;
pub mod qos;
pub mod rangetest;
pub mod region;
pub mod router;
pub mod routes;
pub mod stamp;

pub use addr::{Address, PathId};
pub use packet::{Header, Packet, PacketType, Path};
pub use router::{Action, Energy, Role, Router};

/// Largest LoRa packet in bytes.
pub const MAX_PACKET: usize = 255;
/// Largest number of hops a packet may take (protocol limit).
pub const MAX_HOPS: u8 = 64;
/// Largest number of hops in a source path (2 bytes per hop, 24 bytes in total).
pub const MAX_PATH: usize = 12;
/// Default range for flooded channels and announces (hops).
pub const LOCAL_SCOPE: u8 = 8;
