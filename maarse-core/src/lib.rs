//! # maarse-core
//!
//! Kernen i Må:rse-protokollen: adresser, pakkeformat, dublet-filter,
//! rutetabel, sendetidsbudget og routing-motoren.
//!
//! Crate'en er `no_std` og bruger ingen heap, så den kan køre direkte på en
//! nRF52840. Alle tabeller har faste størrelser, som vælges med const generics.
//!
//! Se specifikationen "Må:rse – protokolspecifikation v0.1".

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod addr;
pub mod airtime;
pub mod companion;
pub mod content;
pub mod dedup;
pub mod duty;
pub mod packet;
pub mod region;
pub mod router;
pub mod routes;

pub use addr::{Address, PathId};
pub use packet::{Header, Packet, PacketType, Path};
pub use router::{Action, Energy, Role, Router};

/// Største LoRa-pakke i bytes.
pub const MAX_PACKET: usize = 255;
/// Største antal hop en pakke må tage (protokolgrænse).
pub const MAX_HOPS: u8 = 64;
/// Største antal hop i en kildesti (2 bytes pr. hop, 24 bytes i alt).
pub const MAX_PATH: usize = 12;
/// Standard-rækkevidde for floodede kanaler og announces (hop).
pub const LOCAL_SCOPE: u8 = 8;
