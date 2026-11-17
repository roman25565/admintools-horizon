//! AdminTools rules with no game types: tested on their own by `core-tests/`.
//!
//! These modules are the shared "core" of our server mods: config files, regions, the
//! profiler's sums and verdicts, datapack scanning and chat formatting. Other mods reach them
//! through the `admintools` trait mod dependency (see `lib.rs`, `pub mod shared`).
pub mod config;
pub mod datapacks;
pub mod fmt;
pub mod profiler;
pub mod regions;
