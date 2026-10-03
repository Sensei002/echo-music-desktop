//! LRCLIB — <https://lrclib.net>
//!
//! A community lyrics database with both plain and synced (LRC) documents. The
//! search strategy here mirrors upstream exactly: cleaned title+artist first,
//! then progressively looser queries, then the closest duration match.

use super::{clean_artist, clean_title, RawLyrics};
use crate::parse::LyricsFormat;
use crate::service::LyricsQuery;
use anyhow::Result;
use serde::Deserialize;

const BASE: &str = "https://lrclib.net";

#[derive(Debug, Clone, Deserialize)]
struct Track {
    #[serde(rename = "trackName", default)]
    track_name: String,
    #[serde(rename = "artistName", default)]
    artist_name: String,
    #[serde(default)]
    duration: f64,
    #[serde(rename = "syncedLyrics", default)]
    synced_lyrics: Option<String>,
    #[serde(rename = "plainLyrics", default)]
    plain_lyrics: Option<String>,
}

impl Track {
    fn usable(&self) -> bool {
        self.synced_lyrics
            .as_deref()
            .map(|text| !text.trim().is_empty())
            .unwrap_or(false)
            || self
                .plain_lyrics
                .as_deref()
                .map(|text| !text.trim().is_empty())
                .unwrap_or(false)
    }
}

/// Fetches lyrics from LRCLIB.
pub fn fetch(client: &reqwest::blocking::Client, query: &LyricsQuery) -> Result<Option<RawLyrics>> {
    let title = clean_title(&query.title);
    let artist = clean_artist(&query.artist);
    let album = query.album.clone().unwrap_or_default();

    // Strategy 1 — cleaned title + artist (+ album when known).
    let mut candidates = search(
        client,
        &[
            ("track_name", title.as_str()),
            ("artist_name", artist.as_str()),
            ("album_name", album.as_str()),
        ],
    )?;

    // Strategy 2 — title only (the credited artist often differs).
    if candidates.is_empty() {
        candidates = search(client, &[("track_name", title.as_str())])?;
    }

    // Strategy 3 — free-text query with both fields.
    if candidates.is_empty() {
        let combined = format!("{artist} {title}");
        candidates = search(client, &[("q", combined.as_str())])?;
    }

    // Strategy 4 — free-text query with the title only.
    if candidates.is_empty() {
        candidates = search(client, &[("q", title.as_str())])?;
    }

    // Strategy 5 — the untouched metadata.
    if candidates.is_empty() && title != query.title.trim() {
        candidates = search(
            client,
            &[
                ("track_name", query.title.trim()),
                ("artist_name", query.artist.trim()),
            ],
        )?;
    }

    let usable: Vec<Track> = candidates.into_iter().filter(Track::usable).collect();
    let Some(best) = pick_best(&usable, query.duration_secs) else {
        return Ok(None);
    };

    if let Some(synced) = best.synced_lyrics.filter(|text| !text.trim().is_empty()) {
        return Ok(Some(RawLyrics::new(synced, LyricsFormat::Lrc)));
    }
    Ok(best
        .plain_lyrics
        .filter(|text| !text.trim().is_empty())
        .map(|text| RawLyrics::new(text, LyricsFormat::Plain)))
}

fn search(client: &reqwest::blocking::Client, params: &[(&str, &str)]) -> Result<Vec<Track>> {
    let params: Vec<(&str, &str)> = params
        .iter()
        .filter(|(_, value)| !value.trim().is_empty())
        .copied()
        .collect();
    if params.is_empty() {
        return Ok(Vec::new());
    }

    let response = client
        .get(format!("{BASE}/api/search"))
        .query(&params)
        .send()?;
    if !response.status().is_success() {
        return Ok(Vec::new());
    }
    let tracks: Vec<Track> = response.json().unwrap_or_default();
    Ok(tracks)
}

/// Chooses the track whose duration is closest to the one we are playing.
fn pick_best(tracks: &[Track], duration: Option<u32>) -> Option<&Track> {
    let duration = duration?;
    let target = duration as f64;
    tracks.iter().min_by(|a, b| {
        let da = (a.duration - target).abs();
        let db = (b.duration - target).abs();
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(name: &str, duration: f64, synced: bool) -> Track {
        Track {
            track_name: name.into(),
            artist_name: "Artist".into(),
            duration,
            synced_lyrics: synced.then(|| "[00:01.00]hi".to_string()),
            plain_lyrics: Some("hi".into()),
        }
    }

    #[test]
    fn picks_the_closest_duration() {
        let tracks = vec![track("a", 100.0, true), track("b", 197.0, true), track("c", 400.0, true)];
        assert_eq!(pick_best(&tracks, Some(196)).unwrap().track_name, "b");
    }

    #[test]
    fn usable_requires_content() {
        let empty = Track {
            track_name: "x".into(),
            artist_name: "y".into(),
            duration: 10.0,
            synced_lyrics: Some("   ".into()),
            plain_lyrics: None,
        };
        assert!(!empty.usable());
        assert!(track("a", 1.0, true).usable());
    }

    #[test]
    fn cleans_title_and_artist() {
        assert_eq!(clean_title("Song (Official Video)"), "Song");
        assert_eq!(clean_title("Song [HD]"), "Song");
        assert_eq!(clean_artist("A & B"), "A");
        assert_eq!(clean_artist("A feat. C"), "A");
    }
}
