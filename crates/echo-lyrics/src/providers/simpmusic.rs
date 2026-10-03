//! SimpMusic — <https://api-lyrics.simpmusic.org/v1/>
//!
//! Keyed by YouTube video id, so it is the most precise provider when a track
//! comes straight from YouTube Music.

use super::RawLyrics;
use crate::model::LyricLine;
use crate::parse::detect_format;
use crate::service::LyricsQuery;
use anyhow::Result;
use serde_json::Value;

const PRIMARY: &str = "https://api-lyrics.simpmusic.org/v1/";
const FALLBACK: &str = "https://vivi-yt-music-server.onrender.com/v1/";

/// Fetches lyrics from SimpMusic.
pub fn fetch(client: &reqwest::blocking::Client, query: &LyricsQuery) -> Result<Option<RawLyrics>> {
    let video_id = query.video_id.trim();
    if video_id.is_empty() {
        return Ok(None);
    }

    for base in [PRIMARY, FALLBACK] {
        let response = match client.get(format!("{base}{video_id}")).send() {
            Ok(response) if response.status().is_success() => response,
            _ => continue,
        };
        let Ok(value) = response.json::<Value>() else {
            continue;
        };
        if let Some(raw) = extract(&value) {
            return Ok(Some(raw));
        }
    }
    Ok(None)
}

/// Pulls lyrics out of either the structured or the textual response shape.
fn extract(value: &Value) -> Option<RawLyrics> {
    // Shape 1: an array of timed lines.
    if let Some(items) = super::find_array(value, &["lyrics", "lines"]) {
        let mut lines = Vec::with_capacity(items.len());
        for item in items {
            let text = item
                .get("text")
                .or_else(|| item.get("lyric"))
                .or_else(|| item.get("line"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if text.trim().is_empty() {
                continue;
            }
            let start = item
                .get("startTime")
                .or_else(|| item.get("start_time"))
                .or_else(|| item.get("time"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let end = item
                .get("endTime")
                .or_else(|| item.get("end_time"))
                .and_then(Value::as_u64);
            lines.push(LyricLine {
                text: text.to_string(),
                start_ms: start,
                end_ms: end,
                words: Vec::new(),
                translation: None,
            });
        }
        if !lines.is_empty() {
            lines.sort_by_key(|line| line.start_ms);
            return Some(RawLyrics::from_lines(lines));
        }
    }

    // Shape 2: a textual document (LRC or TTML) under a known key.
    if let Some(text) = super::find_string(
        value,
        &["syncedLyrics", "lyrics", "plainLyrics", "lrc", "ttml"],
    ) {
        let format = detect_format(&text);
        return Some(RawLyrics::new(text, format));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_structured_lines() {
        let value = json!({
            "success": true,
            "lyrics": [
                {"text": "first", "startTime": 1000, "endTime": 2000},
                {"text": "second", "startTime": 2000}
            ]
        });
        let raw = extract(&value).expect("should extract");
        let lines = raw.lines.unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].start_ms, 2_000);
    }

    #[test]
    fn extracts_textual_lyrics() {
        let value = json!({"data": {"lyrics": "[00:01.00]hi"}});
        let raw = extract(&value).expect("should extract");
        assert!(raw.lines.is_none());
        assert_eq!(raw.text, "[00:01.00]hi");
    }
}
