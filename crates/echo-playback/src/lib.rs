//! # echo-playback
//!
//! The audio engine for Echo Music Desktop.
//!
//! * [`source::HttpRangeSource`] streams a remote file lazily so playback starts
//!   before the track is fully downloaded.
//! * [`equalizer`] provides the 10-band parametric EQ and stereo widener.
//! * [`queue`] holds the play order, shuffle state and repeat mode.
//! * [`engine`] runs the whole thing on a dedicated thread behind a command /
//!   event channel pair.

pub mod engine;
pub mod equalizer;
pub mod queue;
pub mod source;

pub use engine::{Command, Event, PlaybackEngine};
pub use equalizer::{EqConfig, EqHandle, EqualizerSource, BANDS};
pub use queue::Queue;
pub use source::HttpRangeSource;

/// Truncates a URL so signing parameters never reach the log file.
pub(crate) fn truncate_url(url: &str) -> String {
    let head: String = url.chars().take(48).collect();
    if url.chars().count() > 48 {
        format!("{head}…")
    } else {
        head
    }
}
