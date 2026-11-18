//! The host's pure rules, shared by `maarse-host` and the simulator.
//!
//! Everything here is free of I/O: no sockets, no files, no threads and no
//! clock. Time is passed in as milliseconds (`u64`), so the same rules run
//! in the host (with a real clock) and in the wasm simulator (with a
//! simulated one).
//!
//! - [`filebox`]: route kind, file size limits, airtime estimate, file mailbox time.
//! - [`calls`]: when a call is refused.
//! - [`tunnel`]: the IP tunnel's addresses, header compression, pieces and NACKs.
//! - [`guard`]: backbone port limits, token buckets and ban decisions.
//! - [`pages`]: page paths, picture copies, dithering and the burst window.
//! - [`pagelock`]: `@allow`/`@locked` lock lines, boards and picture previews (text only).
//! - [`rules`]: post office and backbone timings as an editable [`rules::Rules`] profile.

pub mod calls;
pub mod filebox;
pub mod guard;
pub mod pagelock;
pub mod pages;
pub mod rules;
pub mod tunnel;
