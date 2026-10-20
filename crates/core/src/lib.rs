//! Shabdly core: platform-neutral logic shared by desktop and (later) mobile.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod dictionary;
pub mod history;
pub mod learning;
pub mod settings;
pub mod store;
pub mod swaps;
