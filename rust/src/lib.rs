//! mapsize: interactive terminal disk usage analyser (Rust port).

pub mod brand;
pub mod cancel;
pub mod config;
pub mod export;
pub mod inventory;
pub mod platform;
pub mod scan;
pub mod snapshot;
pub mod textutil;

/// Boxed error used across mode boundaries.
pub type Error = Box<dyn std::error::Error + Send + Sync>;
