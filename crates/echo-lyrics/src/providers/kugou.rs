//! Kugou — <https://lyrics.kugou.com>
//!
//! Two-step protocol: search for a candidate, then download its LRC payload
//! (base64 encoded in the response).

use super::RawLyrics;
use crate::parse::LyricsFormat;
use crate::service::LyricsQuery;
use anyhow::Result;
use base64::Engine;
use serde::Deserialize;

const SEARCH: &str = "https://lyrics.kugou.com/search";
const DOWNLOAD: &str = "https://lyrics.kugou.com/download";

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    candidates: Vec<Candidate>,
}

#[derive(Debug, Clone, Deserialize)]
struct Candidate {
    #[serde(default)]
    id: String,
    #[serde(default)]
    accesskey: String,
    #[serde(default)]
    duration: i64,
    #[serde(default)]
    song: String,
}

#[derive(Debug, Deserialize)]
struct DownloadResponse {
    #[serde(default)]
    content: String,
}

/// Fetches lyrics from Kugou.
pub fn fetch(client: &reqwest::blocking::Client, query: &LyricsQuery) -> Result<Option<RawLyrics>> {
    let keyword = format!("{} - {}", query.title.trim(), query.artist.trim());
    let mut params: Vec<(&str, String)> = vec![
        ("ver", "1".into()),
        ("man", "yes".into()),
        ("client", "pc".into()),
        ("keyword", keyword),
    ];
    if let Some(duration) = query.duration_secs {
        params.push(("duration", (duration * 1000).to_string()));
    }

    let response = client.get(SEARCH).query(&params).send()?;
    if !response.status().is_success() {
        return Ok(None);
    }
    let search: SearchResponse = match response.json() {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };

    let Some(candidate) = pick_candidate(&search.candidates, query.duration_secs) else {
        return Ok(None);
    };

    let download = client
        .get(DOWNLOAD)
        .query(&[
            ("fmt", "lrc".to_string()),
            ("charset", "utf8".to_string()),
            ("client", "pc".to_string()),
            ("ver", "1".to_string()),
            ("id", candidate.id.clone()),
            ("accesskey", candidate.accesskey.clone()),
        ])
        .send()?;
    if !download.status().is_success() {
        return Ok(None);
    }
    let payload: DownloadResponse = match download.json() {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };

    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload.content.trim())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let Some(text) = decoded else {
        return Ok(None);
    };
    if text.trim().is_empty() {
        return Ok(None);
    }

    Ok(Some(RawLyrics::new(text, LyricsFormat::Lrc)))
}

/// Chooses the candidate whose duration is closest to the track we are playing.
fn pick_candidate(candidates: &[Candidate], duration: Option<u32>) -> Option<&Candidate> {
    if candidates.is_empty() {
        return None;
    }
    let Some(duration) = duration else {
        return candidates.first();
    };
    let target = duration as i64;
    candidates.iter().min_by_key(|candidate| (candidate.duration - target).abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, duration: i64) -> Candidate {
        Candidate {
            id: id.into(),
            accesskey: "key".into(),
            duration,
            song: "Song".into(),
        }
    }

    #[test]
    fn picks_the_closest_candidate() {
        let list = vec![candidate("a", 100), candidate("b", 197_000), candidate("c", 400_000)];
        assert_eq!(pick_candidate(&list, Some(196)).unwrap().id, "b");
    }

    #[test]
    fn falls_back_to_the_first_candidate() {
        let list = vec![candidate("a", 1), candidate("b", 2)];
        assert_eq!(pick_candidate(&list, None).unwrap().id, "a");
        assert!(pick_candidate(&[], Some(1)).is_none());
    }
}
