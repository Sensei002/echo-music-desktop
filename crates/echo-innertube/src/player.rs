//! Stream resolution.
//!
//! The `player` endpoint is walked across the [`STREAM_CHAIN`] clients until one
//! returns playable audio. Formats are filtered to audio-only, scored against
//! the requested quality, then converted into a directly playable URL (handling
//! `signatureCipher` when the chosen client protects its URLs).

use crate::cipher::{resolve_format_url, Cipher};
use crate::clients::{self, YtClient, STREAM_CHAIN};
use crate::http::InnerTube;
use crate::parse;
use anyhow::{anyhow, Context, Result};
use echo_core::{AudioQuality, AudioStream, Song};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

static BASE_JS_PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"/s/player/[A-Za-z0-9_-]+/player_ias\.vflset/[A-Za-z_]+/base\.js")
        .expect("valid regex")
});

/// A resolved, playable stream plus the metadata the player needs.
#[derive(Debug, Clone)]
pub struct ResolvedStream {
    pub song: Song,
    pub stream: AudioStream,
}

/// Resolves video ids into playable audio streams.
pub struct StreamResolver {
    it: Arc<InnerTube>,
    cipher: Mutex<Option<Arc<Cipher>>>,
    player_js: Mutex<Option<Arc<String>>>,
}

impl StreamResolver {
    pub fn new(it: Arc<InnerTube>) -> Self {
        Self {
            it,
            cipher: Mutex::new(None),
            player_js: Mutex::new(None),
        }
    }

    /// The transport shared with the rest of the client.
    pub fn transport(&self) -> &InnerTube {
        &self.it
    }

    /// Resolves `video_id`, trying every client in the chain.
    pub fn resolve(&self, video_id: &str, quality: AudioQuality) -> Result<ResolvedStream> {
        // ANDROID_VR refuses to answer without a visitor id.
        if let Err(err) = self.it.ensure_visitor_data() {
            log::debug!("visitor data unavailable: {err:#}");
        }

        let mut last_error = None;
        for client in STREAM_CHAIN {
            match self.try_client(client, video_id, quality) {
                Ok(resolved) => {
                    log::debug!(
                        "resolved `{video_id}` via {} ({} bps)",
                        client.friendly_name,
                        resolved.stream.bitrate
                    );
                    return Ok(resolved);
                }
                Err(err) => {
                    log::debug!("stream via {} failed: {err:#}", client.friendly_name);
                    last_error = Some(err);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("no InnerTube client could resolve `{video_id}`")))
    }

    /// Fetches the up-next queue for a track.
    pub fn related(&self, video_id: &str) -> Result<Vec<Song>> {
        let client = &clients::WEB_REMIX;
        let body = self.it.body(
            client,
            json!({
                "videoId": video_id,
                "playlistId": format!("RDAMVM{video_id}"),
                "isAudioOnly": true
            }),
        );
        let response = self.it.post("next", client, &body)?;
        Ok(parse::collect_queue(&response)
            .into_iter()
            .filter(|song| song.id != video_id)
            .collect())
    }

    fn try_client(
        &self,
        client: &YtClient,
        video_id: &str,
        quality: AudioQuality,
    ) -> Result<ResolvedStream> {
        let body = self.it.body(
            client,
            json!({
                "videoId": video_id,
                "contentCheckOk": true,
                "racyCheckOk": true,
                "playbackContext": {
                    "contentPlaybackContext": { "html5Preference": "HTML5_PREF_WANTS" }
                }
            }),
        );
        let response = self.it.post("player", client, &body)?;

        let status = response
            .pointer("/playabilityStatus/status")
            .and_then(Value::as_str)
            .unwrap_or("ERROR");
        if status != "OK" {
            let reason = response
                .pointer("/playabilityStatus/reason")
                .and_then(Value::as_str)
                .unwrap_or("");
            return Err(anyhow!("playability status `{status}` {reason}"));
        }

        let formats = collect_audio_formats(&response);
        if formats.is_empty() {
            return Err(anyhow!("the player response contained no audio formats"));
        }

        let ciphered = formats
            .iter()
            .any(|f| f.get("signatureCipher").is_some() || f.get("cipher").is_some());
        let cipher = if ciphered { self.ensure_cipher() } else { None };
        let player_js = self.player_js();
        let player_js_ref: Option<&str> = player_js.as_ref().map(|js| js.as_str());

        let best = pick_best(&formats, quality)?;
        let url = resolve_format_url(best, cipher.as_deref(), player_js_ref)
            .context("failed to produce a playable URL")?;

        let stream = AudioStream {
            url,
            mime_type: best
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("audio/mp4")
                .to_string(),
            codec: best
                .get("mimeType")
                .and_then(Value::as_str)
                .and_then(|m| m.split("codecs=\"").nth(1))
                .map(|c| c.trim_end_matches('"').to_string())
                .unwrap_or_else(|| "unknown".into()),
            bitrate: best.get("bitrate").and_then(Value::as_u64).unwrap_or(0) as u32,
            sample_rate: best
                .get("audioSampleRate")
                .and_then(Value::as_str)
                .and_then(|v| v.parse().ok()),
            channels: best.get("audioChannels").and_then(Value::as_u64).map(|v| v as u32),
            content_length: best.get("contentLength").and_then(Value::as_str).and_then(|v| v.parse().ok()),
            client: client.friendly_name.to_string(),
        };

        Ok(ResolvedStream {
            song: song_from_player(&response, video_id),
            stream,
        })
    }

    /// Returns the cached player JS without forcing a download.
    fn player_js(&self) -> Option<Arc<String>> {
        self.player_js.lock().ok().and_then(|guard| guard.clone())
    }

    fn ensure_cipher(&self) -> Option<Arc<Cipher>> {
        if let Some(cipher) = self.cipher.lock().ok().and_then(|guard| guard.clone()) {
            return Some(cipher);
        }
        match self.load_cipher() {
            Ok(cipher) => {
                let cipher = Arc::new(cipher);
                if let Ok(mut guard) = self.cipher.lock() {
                    *guard = Some(cipher.clone());
                }
                Some(cipher)
            }
            Err(err) => {
                log::warn!("signature decipher unavailable ({err:#}); trying an unciphered client");
                None
            }
        }
    }

    fn load_cipher(&self) -> Result<Cipher> {
        let js = self.load_player_js()?;
        Cipher::parse(&js)
    }

    fn load_player_js(&self) -> Result<Arc<String>> {
        if let Some(js) = self.player_js() {
            return Ok(js);
        }
        let index = self
            .it
            .get("https://www.youtube.com/iframe_api")
            .context("could not fetch the YouTube iframe API")?;
        let path = BASE_JS_PATH
            .find(&index)
            .map(|m| m.as_str())
            .ok_or_else(|| anyhow!("could not locate base.js inside iframe_api"))?;
        let url = format!("https://www.youtube.com{path}");
        let js = Arc::new(self.it.get(&url).context("could not fetch base.js")?);
        if let Ok(mut guard) = self.player_js.lock() {
            *guard = Some(js.clone());
        }
        Ok(js)
    }
}

/// Collects every audio-only format from `streamingData`.
fn collect_audio_formats(response: &Value) -> Vec<&Value> {
    let mut formats = Vec::new();
    for key in ["adaptiveFormats", "formats"] {
        if let Some(list) = response
            .pointer(&format!("/streamingData/{key}"))
            .and_then(Value::as_array)
        {
            for format in list {
                let is_audio = format
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .map(|mime| mime.starts_with("audio/"))
                    .unwrap_or(false);
                if is_audio {
                    formats.push(format);
                }
            }
        }
    }
    formats
}

/// Scores formats against the requested quality and returns the best match.
fn pick_best<'a>(formats: &[&'a Value], quality: AudioQuality) -> Result<&'a Value> {
    let target = quality.target_bitrate() as i64;
    let mut best: Option<&Value> = None;
    let mut best_score = i64::MIN;
    for format in formats {
        let bitrate = format.get("bitrate").and_then(Value::as_u64).unwrap_or(0) as i64;
        // Prefer the highest bitrate at or below the target; above the target we
        // still accept the closest one but penalise the overshoot.
        let score = if bitrate <= target {
            bitrate
        } else {
            target - (bitrate - target)
        };
        if score > best_score {
            best_score = score;
            best = Some(format);
        }
    }
    best.ok_or_else(|| anyhow!("no audio format matched the requested quality"))
}

/// Builds the song metadata from `videoDetails` / `microformat`.
fn song_from_player(response: &Value, video_id: &str) -> Song {
    let details = response.get("videoDetails");
    let microformat = response
        .get("microformat")
        .and_then(|m| m.get("playerMicroformatRenderer"));

    let title = details
        .and_then(|d| d.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("Unknown title")
        .to_string();
    let author = details
        .and_then(|d| d.get("author"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let author_id = details
        .and_then(|d| d.get("channelId"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let duration = details
        .and_then(|d| d.get("lengthSeconds"))
        .and_then(Value::as_str)
        .and_then(|v| v.parse::<u32>().ok());
    let thumbnail = details
        .and_then(|d| d.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(Value::as_array)
        .and_then(|list| list.last())
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let album = microformat
        .and_then(|m| m.get("category"))
        .and_then(Value::as_str)
        .filter(|c| !c.eq_ignore_ascii_case("music"))
        .map(str::to_string);
    let year = microformat
        .and_then(|m| m.get("publishDate"))
        .and_then(Value::as_str)
        .and_then(|date| date.get(0..4))
        .and_then(|y| y.parse::<i32>().ok());

    Song {
        id: video_id.to_string(),
        title,
        artists: author.into_iter().collect(),
        artist_ids: author_id.into_iter().collect(),
        album,
        album_id: None,
        duration,
        thumbnail,
        set_video_id: None,
        is_explicit: false,
        is_video: false,
        play_count: None,
        year,
        radio_playlist_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_closest_bitrate() {
        let formats = vec![
            json!({"mimeType": "audio/mp4", "bitrate": 64000}),
            json!({"mimeType": "audio/mp4", "bitrate": 128000}),
            json!({"mimeType": "audio/mp4", "bitrate": 256000}),
        ];
        let refs: Vec<&Value> = formats.iter().collect();
        let best = pick_best(&refs, AudioQuality::High).unwrap();
        assert_eq!(best.get("bitrate").unwrap().as_u64(), Some(256000));

        let best = pick_best(&refs, AudioQuality::Low).unwrap();
        assert_eq!(best.get("bitrate").unwrap().as_u64(), Some(64000));
    }

    #[test]
    fn ignores_video_formats() {
        let response = json!({
            "streamingData": {
                "adaptiveFormats": [
                    {"mimeType": "video/mp4", "bitrate": 900000},
                    {"mimeType": "audio/webm; codecs=\"opus\"", "bitrate": 160000}
                ]
            }
        });
        let formats = collect_audio_formats(&response);
        assert_eq!(formats.len(), 1);
        assert!(formats[0]
            .get("mimeType")
            .unwrap()
            .as_str()
            .unwrap()
            .starts_with("audio/"));
    }

    #[test]
    fn reads_song_metadata() {
        let response = json!({
            "videoDetails": {
                "title": "Khalasi",
                "author": "Aditya Gadhvi",
                "channelId": "UC123",
                "lengthSeconds": "196",
                "thumbnail": {"thumbnails": [{"url": "https://img/1.jpg"}, {"url": "https://img/2.jpg"}]}
            },
            "microformat": {"playerMicroformatRenderer": {"publishDate": "2024-01-05T00:00:00Z"}}
        });
        let song = song_from_player(&response, "abc");
        assert_eq!(song.title, "Khalasi");
        assert_eq!(song.artists, vec!["Aditya Gadhvi".to_string()]);
        assert_eq!(song.duration, Some(196));
        assert_eq!(song.year, Some(2024));
        assert_eq!(song.thumbnail.as_deref(), Some("https://img/2.jpg"));
    }
}
