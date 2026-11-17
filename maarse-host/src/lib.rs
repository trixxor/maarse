//! maarse-host as a library, for programs that embed the whole host: the
//! Android app runs [`run`] in a process of its own, so the phone is a
//! Må:rse node with its own identity. Same code as the two programs.
#![forbid(unsafe_code)]

include!("app.rs");
