//! maarse-server: a Må:rse backbone node for a plain Windows or Linux
//! machine (a small VPS, a Raspberry Pi) with a reachable port.
//!
//! The same program as maarse-host, without radio and web client by
//! default: it keeps the open backbone together (a DHT node others find
//! each other through), relays for hosts behind NAT that connect out to it,
//! and keeps copies of mail for offline nodes.
//!
//! Start:  maarse-server --public-addr <host:port>
//! Help:   maarse-server --help
#![forbid(unsafe_code)]

include!("app.rs");
