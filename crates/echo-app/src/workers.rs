//! Background workers.
//!
//! Every network or disk operation runs on a short-lived thread and reports back
//! with an [`Update`] message. The UI thread drains those messages on a timer,
//! so the Slint event loop is never blocked.

use echo_core::{AlbumPage, ArtistPage, HomePage, Library, PlaylistPage, SearchResults, Song};
use echo_innertube::{MusicClient, SearchFilter};
use echo_lyrics::{Lyrics, LyricsQuery, LyricsService};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// A message from a worker to the UI thread.
#[derive(Debug)]
pub enum Update {
    Home(Result<HomePage, String>),
    Search(Result<SearchResults, String>),
    Suggestions(Vec<String>),
    Album(Result<AlbumPage, String>),
    Artist(Result<ArtistPage, String>),
    Playlist(Result<PlaylistPage, String>),
    /// A stream URL was resolved and playback can begin.
    StreamReady {
        song: Song,
        url: String,
    },
    StreamFailed(String),
    Lyrics(Result<Option<Lyrics>, String>),
    /// Artwork finished downloading; the UI should decode and rebuild models.
    Covers(Vec<String>),
    Status(String),
}

/// Spawns a named worker thread, logging rather than panicking on failure.
fn spawn<F: FnOnce() + Send + 'static>(name: &str, job: F) {
    if let Err(err) = std::thread::Builder::new().name(name.into()).spawn(job) {
        log::warn!("failed to spawn the `{name}` worker: {err}");
    }
}

fn fail<T>(result: anyhow::Result<T>) -> Result<T, String> {
    result.map_err(|err| format!("{err:#}"))
}

/// A handle to everything the background jobs need.
///
/// Each job is a free function taking its own dependencies; this type bundles
/// the shared ones (HTTP client, lyrics service, library, cache directory and
/// the update channel) so the UI code can call `workers.home()` instead of
/// threading five arguments through every call site. It is cheap to clone —
/// every field is an `Arc` or a `Sender` — so closures can capture it directly.
#[derive(Clone)]
pub struct Workers {
    client: Arc<MusicClient>,
    lyrics: Arc<LyricsService>,
    library: Library,
    http: Arc<reqwest::blocking::Client>,
    artwork_dir: PathBuf,
    downloads_dir: PathBuf,
    tx: Sender<Update>,
}

impl Workers {
    pub fn new(
        client: Arc<MusicClient>,
        lyrics: Arc<LyricsService>,
        library: Library,
        paths: &echo_core::AppPaths,
        tx: Sender<Update>,
    ) -> Self {
        Self {
            client,
            lyrics,
            library,
            http: Arc::new(crate::http_client()),
            artwork_dir: paths.artwork_cache_dir(),
            downloads_dir: paths.downloads_dir(),
            tx,
        }
    }

    /// Loads the personalised home feed.
    pub fn home(&self) {
        home(self.client.clone(), self.tx.clone());
    }

    /// Runs a search.
    pub fn search(&self, query: String, filter: SearchFilter) {
        search(self.client.clone(), self.tx.clone(), query, filter);
    }

    /// Fetches type-ahead suggestions.
    pub fn suggestions(&self, query: String) {
        suggestions(self.client.clone(), self.tx.clone(), query);
    }

    /// Opens an arbitrary collection (album, artist, playlist or saved set).
    pub fn collection(&self, id: String, kind: String) {
        match kind.as_str() {
            "album" => album(self.client.clone(), self.tx.clone(), id),
            "artist" => artist(self.client.clone(), self.tx.clone(), id),
            // Playlists and anything unrecognised go through the playlist
            // endpoint, which also resolves saved library ids.
            _ => playlist(self.client.clone(), self.tx.clone(), id),
        }
    }

    /// Resolves a playable stream URL for `song`.
    pub fn resolve(&self, song: Song, quality: echo_core::AudioQuality) {
        resolve_stream(self.client.clone(), self.tx.clone(), song, quality);
    }

    /// Fetches lyrics for the track described by `query`.
    pub fn lyrics(&self, query: LyricsQuery) {
        lyrics(self.lyrics.clone(), self.tx.clone(), query);
    }

    /// Downloads artwork for `urls` that are not cached yet.
    pub fn covers(&self, urls: Vec<String>) {
        covers(
            self.http.clone(),
            self.artwork_dir.clone(),
            self.tx.clone(),
            urls,
        );
    }

    /// Saves a track offline at the requested quality.
    pub fn download(&self, song: Song, quality: echo_core::AudioQuality) {
        download(
            self.client.clone(),
            self.library.clone(),
            self.downloads_dir.clone(),
            self.tx.clone(),
            song,
            quality,
        );
    }

    /// Empties the artwork and lyrics caches.
    pub fn clear_cache(&self) {
        clear_cache(self.artwork_dir.clone(), self.tx.clone());
    }
}

/// Loads the personalised home feed.
pub fn home(client: Arc<MusicClient>, tx: Sender<Update>) {
    spawn("echo-home", move || {
        let _ = tx.send(Update::Home(fail(client.home())));
    });
}

/// Runs a search.
pub fn search(client: Arc<MusicClient>, tx: Sender<Update>, query: String, filter: SearchFilter) {
    spawn("echo-search", move || {
        let _ = tx.send(Update::Search(fail(client.search(&query, filter))));
    });
}

/// Fetches type-ahead suggestions.
pub fn suggestions(client: Arc<MusicClient>, tx: Sender<Update>, query: String) {
    spawn("echo-suggest", move || {
        let list = client.search_suggestions(&query).unwrap_or_default();
        let _ = tx.send(Update::Suggestions(list));
    });
}

/// Loads an album page.
pub fn album(client: Arc<MusicClient>, tx: Sender<Update>, browse_id: String) {
    spawn("echo-album", move || {
        let _ = tx.send(Update::Album(fail(client.album(&browse_id))));
    });
}

/// Loads an artist page.
pub fn artist(client: Arc<MusicClient>, tx: Sender<Update>, browse_id: String) {
    spawn("echo-artist", move || {
        let _ = tx.send(Update::Artist(fail(client.artist(&browse_id))));
    });
}

/// Loads a playlist page.
pub fn playlist(client: Arc<MusicClient>, tx: Sender<Update>, playlist_id: String) {
    spawn("echo-playlist", move || {
        let _ = tx.send(Update::Playlist(fail(client.playlist(&playlist_id))));
    });
}

/// Resolves a stream URL for `song`.
pub fn resolve_stream(
    client: Arc<MusicClient>,
    tx: Sender<Update>,
    song: Song,
    quality: echo_core::AudioQuality,
) {
    spawn("echo-stream", move || {
        match client.resolve(&song.id, quality) {
            Ok(resolved) => {
                let _ = tx.send(Update::StreamReady {
                    song: resolved.song,
                    url: resolved.stream.url,
                });
            }
            Err(err) => {
                let _ = tx.send(Update::StreamFailed(format!(
                    "Could not play `{}`: {err:#}",
                    song.title
                )));
            }
        }
    });
}

/// Looks up lyrics for a track.
pub fn lyrics(service: Arc<LyricsService>, tx: Sender<Update>, query: LyricsQuery) {
    spawn("echo-lyrics", move || {
        let _ = tx.send(Update::Lyrics(fail(service.fetch(&query))));
    });
}

/// Downloads artwork for a batch of URLs, then tells the UI to decode them.
pub fn covers(
    http: Arc<reqwest::blocking::Client>,
    dir: PathBuf,
    tx: Sender<Update>,
    urls: Vec<String>,
) {
    if urls.is_empty() {
        return;
    }
    spawn("echo-covers", move || {
        let mut ready = Vec::new();
        for url in urls {
            if crate::covers::ensure_cached(&http, &dir, &url).is_ok() {
                ready.push(url);
            }
        }
        if !ready.is_empty() {
            let _ = tx.send(Update::Covers(ready));
        }
    });
}

/// Saves a track offline and records it in the library.
pub fn download(
    client: Arc<MusicClient>,
    library: Library,
    dir: PathBuf,
    tx: Sender<Update>,
    song: Song,
    quality: echo_core::AudioQuality,
) {
    spawn("echo-download", move || {
        let result = (|| -> anyhow::Result<(PathBuf, u64)> {
            let resolved = client.resolve(&song.id, quality)?;
            let target = dir.join(format!("{}.m4a", sanitize(&song.id)));
            std::fs::create_dir_all(&dir)
                .map_err(|err| anyhow::anyhow!("failed to create {}: {err}", dir.display()))?;

            let mut response = reqwest::blocking::get(&resolved.stream.url)
                .map_err(|err| anyhow::anyhow!("download request failed: {err}"))?;
            if !response.status().is_success() {
                anyhow::bail!("download returned HTTP {}", response.status());
            }

            let mut file = std::fs::File::create(&target)
                .map_err(|err| anyhow::anyhow!("failed to create {}: {err}", target.display()))?;
            let size = std::io::copy(&mut response, &mut file)
                .map_err(|err| anyhow::anyhow!("failed to write the track: {err}"))?;
            Ok((target, size))
        })();

        let message = match result {
            Ok((path, size)) => {
                if let Err(err) = library.add_download(
                    &song,
                    &path.to_string_lossy(),
                    Some(&format!("{quality:?}")),
                    Some(size),
                ) {
                    log::warn!("could not record the download: {err:#}");
                }
                format!("Saved `{}` offline", song.title)
            }
            Err(err) => format!("Download failed: {err:#}"),
        };
        let _ = tx.send(Update::Status(message));
    });
}

/// Deletes the cached artwork and lyrics files.
pub fn clear_cache(artwork_dir: PathBuf, tx: Sender<Update>) {
    spawn("echo-cache", move || {
        let removed = crate::covers::clear(&artwork_dir).unwrap_or(0);
        let _ = tx.send(Update::Status(format!("Cleared {removed} cached files")));
    });
}

/// Replaces characters that are unsafe in a file name.
fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
