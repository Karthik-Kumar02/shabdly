//! Shabdly core: platform-neutral logic shared by desktop and (later) mobile.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod asr;
pub mod audio;
pub mod dictionary;
pub mod history;
pub mod learning;
pub mod model_store;
pub mod models;
pub mod pipeline;
pub mod prompt;
pub mod providers;
pub mod settings;
pub mod setup;
pub mod store;
pub mod swaps;
