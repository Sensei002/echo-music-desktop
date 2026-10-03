//! BetterLyrics — <https://lyrics-api.boidu.dev>
//!
//! Returns TTML with per-word timings, which is what powers the word-by-word
//! lyric view.

use super::RawLyrics;
use crate::parse::detect_format;
use crate::service::LyricsQuery;
use anyhow::Result;

const BASE: &str = "https://lyrics-api.boidu.dev";

/// Fetches lyrics from BetterLyrics.
pub fn fetch(client: &reqwest::blocking::Client, query: &LyricsQuery) -> Result<Option<RawLyrics>> {
    let mut params: Vec<(&str, String)> = vec![
        ("s", query.title.clone()),
        ("a", query.artist.clone()),
    ];
    if let Some(duration) = query.duration_secs {
        params.push(("d", duration.to_string()));
    }
    if let Some(album) = &query.album {
        if !album.trim().is_empty() {
            params.push(("al", album.clone()));
        }
    }

    let response = client
        .get(format!("{BASE}/getLyrics"))
        .query(&params)
        .send()?;
    if !response.status().is_success() {
        return Ok(None);
    }

    let body = response.text()?;
    let trimmed = body.trim();
    if trimmed.is_empty() || trimmed == "[]" || trimmed == "{}" || trimmed == "null" {
        return Ok(None);
    }

    // The endpoint occasionally answers with JSON wrapping the TTML.
    let payload = if trimmed.starts_with('{') || trimmed.starts_with('[') {
        serde_json::from_str::<serde_json::Value>(trimmed)
            .ok()
            .and_then(|value| super::find_string(&value, &["lyrics", "ttml", "data", "text"]))
            .unwrap_or_else(|| trimmed.to_string())
    } else {
        trimmed.to_string()
    };

    if payload.trim().is_empty() {
        return Ok(None);
    }

    Ok(Some(RawLyrics::new(payload.clone(), detect_format(&payload))))
}
