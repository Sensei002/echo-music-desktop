//! # echo-lyrics
//!
//! Lyrics for Echo Music Desktop, ported from the upstream Android modules.
//!
//! Six providers are supported — LRCLIB, BetterLyrics, SimpMusic, YouLyPlus,
//! Kugou and Paxsenix — tried in a user-configurable order until one returns a
//! document. Three formats are understood: classic LRC, enhanced (word-by-word)
//! LRC, and TTML as used by BetterLyrics / YouLyPlus.

pub mod model;
pub mod parse;
pub mod providers;
pub mod service;

pub use model::{LyricLine, LyricWord, Lyrics};
pub use parse::{parse_lyrics, LyricsFormat};
pub use providers::Provider;
pub use service::{LyricsQuery, LyricsService};
