//! YouLy+ (LyricsPlus) — community-hosted mirrors of the open-source
//! [lyricsplus](https://github.com/ibratabian17/lyricsplus) backend.
//!
//! Several mirrors exist; the last one that answered is promoted to the front of
//! the list so repeated lookups hit a warm server first, exactly like the
//! upstream implementation.

use super::RawLyrics;
use crate::parse::detect_format;
use crate::service::LyricsQuery;
use anyhow::Result;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Mirrors of the LyricsPlus backend.
const SERVERS: [&str; 6] = [
    "https://lyricsplus.prjktla.my.id",
    "https://lyricsplus.atomix.one",
    "https://lyricsplus.binimum.org",
    "https://lyricsplus.prjktla.workers.dev",
    "https://lyricsplus-seven.vercel.app",
    "https://lyrics-plus-backend.vercel.app",
];

/// Index of the mirror that most recently produced a result.
static LAST_WORKING: AtomicUsize = AtomicUsize::new(0);
/// Serialises updates to [`LAST_WORKING`] so concurrent fetches stay coherent.
static PROMOTE: Mutex<()> = Mutex::new(());

/// Fetches lyrics from YouLy+.
pub fn fetch(client: &reqwest::blocking::Client, query: &LyricsQuery) -> Result<Option<RawLyrics>> {
    let start = LAST_WORKING.load(Ordering::Relaxed) % SERVERS.len();

    for offset in 0..SERVERS.len() {
        let index = (start + offset) % SERVERS.len();
        let base = SERVERS[index].trim_end_matches('/');

        let mut params: Vec<(&str, String)> = vec![
            ("title", query.title.clone()),
            ("artist", query.artist.clone()),
        ];
        if let Some(duration) = query.duration_secs {
            params.push(("duration", duration.to_string()));
        }
        if let Some(album) = &query.album {
            if !album.trim().is_empty() {
                params.push(("album", album.clone()));
            }
        }
        if !query.video_id.trim().is_empty() {
            params.push(("id", query.video_id.clone()));
        }

        let response = match client
            .get(format!("{base}/v2/lyrics/get"))
            .query(&params)
            .send()
        {
            Ok(response) if response.status().is_success() => response,
            _ => continue,
        };

        let body = match response.text() {
            Ok(body) => body,
            Err(_) => continue,
        };
        let trimmed = body.trim();
        if trimmed.is_empty() || trimmed == "{}" || trimmed == "[]" || trimmed == "null" {
            continue;
        }

        // The backend answers with TTML directly, or with a JSON envelope.
        let payload = if trimmed.starts_with('{') || trimmed.starts_with('[') {
            serde_json::from_str::<serde_json::Value>(trimmed)
                .ok()
                .and_then(|value| {
                    super::find_string(&value, &["lyrics", "ttml", "lrc", "data", "text"])
                })
                .unwrap_or_default()
        } else {
            trimmed.to_string()
        };
        if payload.trim().is_empty() {
            continue;
        }

        // Remember this mirror for next time.
        let _guard = PROMOTE.lock();
        LAST_WORKING.store(index, Ordering::Relaxed);

        let format = detect_format(&payload);
        return Ok(Some(RawLyrics::new(payload, format)));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_list_is_not_empty() {
        assert!(!SERVERS.is_empty());
        assert!(SERVERS.iter().all(|server| server.starts_with("https://")));
    }

    #[test]
    fn rotation_starts_at_the_last_working_server() {
        LAST_WORKING.store(3, Ordering::Relaxed);
        let start = LAST_WORKING.load(Ordering::Relaxed) % SERVERS.len();
        assert_eq!(start, 3);
        LAST_WORKING.store(0, Ordering::Relaxed);
    }
}
