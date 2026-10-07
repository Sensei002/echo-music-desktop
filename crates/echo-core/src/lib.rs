//! Echo Music Desktop — shared core.
//!
//! This crate holds everything that is independent of the network, the audio
//! device and the UI: the domain models used across the app, the persisted user
//! settings, the local SQLite library and the Echo brand palette.

pub mod config;
pub mod db;
pub mod models;
pub mod paths;
pub mod theme;
pub mod util;

pub use config::{AudioQuality, LibraryLayout, Settings, ThemeMode, DEFAULT_DISCORD_CLIENT_ID};
pub use db::Library;
pub use models::*;
pub use paths::AppPaths;
pub use theme::{Palette, Rgb, ECHO_SEED};
