//! The crate's one test harness (bmo-chat's docs/conventions.md, 「Rust builds」): each file of tests a
//! module of it, sharing the process.

mod siren_server;

mod api;
mod attribute;
mod burst;
mod classify;
mod cli_fmt;
mod guards;
mod plugin;
mod posted;
mod scan;
mod siren;
