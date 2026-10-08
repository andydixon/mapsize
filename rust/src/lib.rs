//! mapsize: interactive terminal disk usage analyser (Rust port).

pub mod brand;
pub mod cancel;
pub mod config;
pub mod duplicate;
pub mod export;
pub mod filter;
pub mod inventory;
pub mod platform;
pub mod scan;
pub mod snapshot;
pub mod textutil;
pub mod treemap;

/// Boxed error used across mode boundaries.
pub type Error = Box<dyn std::error::Error + Send + Sync>;
