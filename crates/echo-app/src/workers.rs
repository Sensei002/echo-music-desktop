//! Background workers.
//!
//! Every network or disk operation runs on a short-lived thread and reports back
//! with an [`Update`] message. The UI thread drains those messages on a timer,
//! so the Slint event loop is never blocked.

use echo_core::{AlbumPage, ArtistPage, HomePage, Library, Playlist, PlaylistPage, SearchResults, Song};
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
    StreamReady { song: Song, url: String },
    StreamFailed(String),
    Lyrics(Result<Option<Lyrics>, String>),
    Songs(Vec<Song>),
    Playlists(Vec<Playlist>),
    /// Artwork finished downloading; the UI should decode and rebuild models.
    Covers(Vec<String>),
    Status(String),
    Error(String),
}

/// Spawns a named worker thread, logging rather than panicking on failure.
fn spawn<F: FnOnce() + Send + 'static>(name: &str, job: F) {
    if let Err(err) = std::thread::Builder::new()
        .name(name.into())
        .spawn(job)
    {
        log::warn!("failed to spawn the `{name}` worker: {err}");
    }
}

fn fail<T>(result: anyhow::Result<T>) -> Result<T, String> {
    result.map_err(|err| format!("{err:#}"))
}

/// Loads the personalised home feed.
pub fn home(client: Arc<MusicClient>, tx: Sender<Update>) {
    spawn("echo-home", move || {
        let _ = tx.send(Update::Home(fail(client.home())));
    });
}

/// Loads the explore page (used when the home feed is empty).
pub fn explore(client: Arc<MusicClient>, tx: Sender<Update>) {
    spawn("echo-explore", move || {
        let _ = tx.send(Update::Home(fail(client.explore())));
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

/// Loads the liked songs list.
pub fn liked_songs(library: Library, tx: Sender<Update>) {
    spawn("echo-liked", move || {
        let songs = library.liked_songs().unwrap_or_default();
        let _ = tx.send(Update::Songs(songs));
    });
}

/// Loads the play history.
pub fn history(library: Library, tx: Sender<Update>) {
    spawn("echo-history", move || {
        let songs = library.history(200).unwrap_or_default();
        let _ = tx.send(Update::Songs(songs));
    });
}

/// Loads the downloaded tracks.
pub fn downloads(library: Library, tx: Sender<Update>) {
    spawn("echo-downloads", move || {
        let songs = library
            .downloads()
            .unwrap_or_default()
            .into_iter()
            .map(|record| record.song)
            .collect();
        let _ = tx.send(Update::Songs(songs));
    });
}

/// Loads the most played tracks.
pub fn top_songs(library: Library, tx: Sender<Update>) {
    spawn("echo-top", move || {
        let _ = tx.send(Update::Songs(library.top_songs(50).unwrap_or_default()));
    });
}

/// Loads the least played tracks.
pub fn bottom_songs(library: Library, tx: Sender<Update>) {
    spawn("echo-bottom", move || {
        let _ = tx.send(Update::Songs(library.bottom_songs(50).unwrap_or_default()));
    });
}

/// Loads the user's playlists.
pub fn playlists(library: Library, tx: Sender<Update>) {
    spawn("echo-playlists", move || {
        let _ = tx.send(Update::Playlists(library.playlists().unwrap_or_default()));
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

/// Collects every artwork URL referenced by a set of items.
pub fn cover_urls_for_items(items: &[echo_core::MediaItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| item.thumbnail().map(str::to_string))
        .collect()
}
