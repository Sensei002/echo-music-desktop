//! Paxsenix — <https://lyrics.paxsenix.org>
//!
//! Apple Music backed: search for a song id, then fetch that song's lyrics.

use super::RawLyrics;
use crate::parse::detect_format;
use crate::service::LyricsQuery;
use anyhow::Result;
use serde_json::Value;

const BASE: &str = "https://lyrics.paxsenix.org";

/// Fetches lyrics from Paxsenix.
pub fn fetch(client: &reqwest::blocking::Client, query: &LyricsQuery) -> Result<Option<RawLyrics>> {
    let search_term = format!("{} {}", query.title.trim(), query.artist.trim());
    let response = client
        .get(format!("{BASE}/apple-music/search"))
        .query(&[("q", search_term.as_str())])
        .send()?;
    if !response.status().is_success() {
        return Ok(None);
    }
    let Ok(search) = response.json::<Value>() else {
        return Ok(None);
    };

    let Some(song_id) = pick_song_id(&search) else {
        return Ok(None);
    };

    let response = client
        .get(format!("{BASE}/apple-music/lyrics"))
        .query(&[("id", song_id.as_str())])
        .send()?;
    if !response.status().is_success() {
        return Ok(None);
    }
    let Ok(payload) = response.json::<Value>() else {
        return Ok(None);
    };

    let Some(text) = super::find_string(
        &payload,
        &["lyrics", "ttml", "syncedLyrics", "plainLyrics", "lrc"],
    ) else {
        return Ok(None);
    };
    if text.trim().is_empty() {
        return Ok(None);
    }

    let format = detect_format(&text);
    Ok(Some(RawLyrics::new(text, format)))
}

/// Finds the first plausible song identifier in a search response.
fn pick_song_id(value: &Value) -> Option<String> {
    if let Some(items) = super::find_array(value, &["results", "songs", "data", "items"]) {
        for item in items {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                if !id.trim().is_empty() {
                    return Some(id.to_string());
                }
            }
        }
    }
    // Some responses expose a bare id at the top level.
    value.get("id").and_then(Value::as_str).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_a_song_id_in_results() {
        let value = json!({"results": [{"id": "12345", "title": "Song"}]});
        assert_eq!(pick_song_id(&value).as_deref(), Some("12345"));
    }

    #[test]
    fn handles_a_top_level_id() {
        assert_eq!(pick_song_id(&json!({"id": "9"})).as_deref(), Some("9"));
        assert!(pick_song_id(&json!({})).is_none());
    }
}
