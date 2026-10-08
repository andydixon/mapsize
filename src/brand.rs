//! Product name isolated so it can be changed in one place.

pub const NAME: &str = "mapsize";
pub const SNAPSHOT_EXT: &str = ".msz";
pub const SNAPSHOT_MAGIC: &[u8; 8] = b"MAPSIZE\x00";

/// Overridden at release time via the MAPSIZE_VERSION environment variable.
pub const VERSION: &str = match option_env!("MAPSIZE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
