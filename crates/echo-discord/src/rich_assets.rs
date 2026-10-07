//! Rich Presence artwork keys.
//!
//! The Discord protocol accepts either a registered asset key belonging to the
//! application, or a raw URL in `large_image` for some client versions. Keys
//! are the portable choice: an application can upload a small set of images in
//! the Discord developer portal and reference them by name on every platform.
//!
//! These names are the contract with that portal. Uploading an image under a
//! different name means Discord silently drops the artwork and shows a blank
//! placeholder, so the lookup is centralised here and covered by a test.

/// Asset key used while a track is playing.
pub const ASSET_PLAYING: &str = "echo_playing";

/// Asset key used while playback is paused.
pub const ASSET_PAUSED: &str = "echo_paused";

/// Asset key used as the small badge identifying the app.
pub const ASSET_LOGO: &str = "echo_logo";

/// The artwork key for the current transport state.
pub fn live_key(playing: bool) -> &'static str {
    if playing {
        ASSET_PLAYING
    } else {
        ASSET_PAUSED
    }
}

/// The default Discord application id used when the user has not supplied one.
///
/// A Rich Presence client is identified by an application id, and the assets
/// above must be uploaded to that same application. Shipping a placeholder here
/// keeps the setting optional for users who run their own application.
pub const DEFAULT_CLIENT_ID: &str = "1179128917280403576";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_distinct_and_non_empty() {
        assert_ne!(ASSET_PLAYING, ASSET_PAUSED);
        assert!(!ASSET_LOGO.is_empty());
        assert_eq!(live_key(true), ASSET_PLAYING);
        assert_eq!(live_key(false), ASSET_PAUSED);
    }
}
