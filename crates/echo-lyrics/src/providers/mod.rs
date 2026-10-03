//! Lyrics providers, ported one-for-one from the upstream Android modules.
//!
//! Every provider exposes the same shape — `fetch(client, query)` returning an
//! optional raw document — so the [`crate::service::LyricsService`] can walk
//! them in the user's preferred order and stop at the first hit.

pub mod betterlyrics;
pub mod kugou;
pub mod lrclib;
pub mod paxsenix;
pub mod simpmusic;
pub mod youlyplus;

use crate::parse::LyricsFormat;
use crate::service::LyricsQuery;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A lyrics document exactly as a provider returned it.
///
/// Providers that hand back structured, already-timed lines can fill `lines`
/// directly; the rest leave it `None` and the service parses `text`.
#[derive(Debug, Clone)]
pub struct RawLyrics {
    pub text: String,
    pub format: LyricsFormat,
    pub lines: Option<Vec<crate::model::LyricLine>>,
}

impl RawLyrics {
    pub fn new(text: impl Into<String>, format: LyricsFormat) -> Self {
        Self {
            text: text.into(),
            format,
            lines: None,
        }
    }

    /// Builds a document from structured lines (bypasses text parsing).
    pub fn from_lines(lines: Vec<crate::model::LyricLine>) -> Self {
        Self {
            text: String::new(),
            format: LyricsFormat::Lrc,
            lines: Some(lines),
        }
    }
}

/// The available lyrics sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Lrclib,
    BetterLyrics,
    SimpMusic,
    YouLyPlus,
    Kugou,
    Paxsenix,
}

impl Provider {
    /// The stable identifier used in settings and caches.
    pub fn id(self) -> &'static str {
        match self {
            Provider::Lrclib => "lrclib",
            Provider::BetterLyrics => "betterlyrics",
            Provider::SimpMusic => "simpmusic",
            Provider::YouLyPlus => "youlyplus",
            Provider::Kugou => "kugou",
            Provider::Paxsenix => "paxsenix",
        }
    }

    /// The name shown in the settings screen.
    pub fn display_name(self) -> &'static str {
        match self {
            Provider::Lrclib => "LRCLIB",
            Provider::BetterLyrics => "BetterLyrics",
            Provider::SimpMusic => "SimpMusic",
            Provider::YouLyPlus => "YouLy+",
            Provider::Kugou => "Kugou",
            Provider::Paxsenix => "Paxsenix",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|provider| provider.id().eq_ignore_ascii_case(id))
    }

    /// Every provider, in the upstream default order.
    pub fn all() -> &'static [Provider] {
        &[
            Provider::Lrclib,
            Provider::BetterLyrics,
            Provider::SimpMusic,
            Provider::YouLyPlus,
            Provider::Kugou,
            Provider::Paxsenix,
        ]
    }

    /// Fetches lyrics from this provider.
    pub fn fetch(
        self,
        client: &reqwest::blocking::Client,
        query: &LyricsQuery,
    ) -> Result<Option<RawLyrics>> {
        match self {
            Provider::Lrclib => lrclib::fetch(client, query),
            Provider::BetterLyrics => betterlyrics::fetch(client, query),
            Provider::SimpMusic => simpmusic::fetch(client, query),
            Provider::YouLyPlus => youlyplus::fetch(client, query),
            Provider::Kugou => kugou::fetch(client, query),
            Provider::Paxsenix => paxsenix::fetch(client, query),
        }
    }
}

/// Removes the `(Official Video)`-style noise LRCLIB matching trips over.
pub(crate) fn clean_title(title: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;

    static PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        [
            r"(?i)\s*\(.*?(official|video|audio|lyrics?|visualizer|hd|hq|4k|remaster|remix|live|acoustic|version|edit|extended|radio|clean|explicit).*?\)",
            r"(?i)\s*\[.*?(official|video|audio|lyrics?|visualizer|hd|hq|4k|remaster|remix|live|acoustic|version|edit|extended|radio|clean|explicit).*?\]",
            r"\s*【.*?】",
            r"\s*\|.*$",
            r"(?i)\s*-\s*(official|video|audio|lyrics?|visualizer).*$",
            r"(?i)\s*\((feat|ft)\..*?\)",
            r"(?i)\s*(feat|ft)\..*$",
        ]
        .iter()
        .filter_map(|pattern| Regex::new(pattern).ok())
        .collect()
    });

    let mut cleaned = title.trim().to_string();
    for pattern in PATTERNS.iter() {
        cleaned = pattern.replace_all(&cleaned, "").to_string();
    }
    cleaned.trim().to_string()
}

/// Reduces a multi-artist credit to the primary artist.
pub(crate) fn clean_artist(artist: &str) -> String {
    const SEPARATORS: [&str; 10] = [
        " & ", " and ", ", ", " x ", " X ", " feat. ", " feat ", " ft. ", " ft ", " featuring ",
    ];
    let mut cleaned = artist.trim().to_string();
    for separator in SEPARATORS {
        if let Some(index) = cleaned.to_lowercase().find(&separator.to_lowercase()) {
            cleaned = cleaned[..index].to_string();
            break;
        }
    }
    cleaned.trim().to_string()
}

/// Recursively finds the first string value under any of `keys`.
pub(crate) fn find_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(found) = map.get(*key).and_then(Value::as_str) {
                    if !found.trim().is_empty() {
                        return Some(found.to_string());
                    }
                }
            }
            for child in map.values() {
                if let Some(found) = find_string(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(|item| find_string(item, keys)),
        _ => None,
    }
}

/// Recursively finds the first array value under any of `keys`.
pub(crate) fn find_array<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Vec<Value>> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(found) = map.get(*key).and_then(Value::as_array) {
                    return Some(found);
                }
            }
            for child in map.values() {
                if let Some(found) = find_array(child, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(|item| find_array(item, keys)),
        _ => None,
    }
}
