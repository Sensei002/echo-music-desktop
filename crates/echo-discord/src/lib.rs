//! Discord Rich Presence for Echo Music Desktop.
//!
//! Publishes what the user is listening to — track, artist, album and a live
//! progress counter — so it appears on their Discord profile.
//!
//! The public surface is deliberately tiny: build an [`Activity`] from the
//! current track and hand it to [`DiscordPresence`]. Everything else (finding
//! the socket, reconnecting after Discord restarts, freezing the timer while
//! paused) happens on a dedicated worker thread, so the UI thread never blocks
//! on IPC and never has to care whether Discord is even running.

mod ipc;
mod presence;
mod rich_assets;

pub use presence::{Activity, DiscordPresence};
pub use rich_assets::{ASSET_LOGO, ASSET_PAUSED, ASSET_PLAYING, DEFAULT_CLIENT_ID};

/// Convenience: starts presence with the bundled application id.
pub fn start_default() -> DiscordPresence {
    DiscordPresence::start(DEFAULT_CLIENT_ID)
}
