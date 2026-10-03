//! Domain models shared by every Echo Music Desktop crate.
//!
//! The shapes intentionally mirror the upstream Android entities so that the
//! port stays a faithful 1:1 mapping of the original data model.

use serde::{Deserialize, Serialize};

/// A playable track (or music video). `id` is the YouTube `videoId`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub artist_ids: Vec<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub album_id: Option<String>,
    /// Duration in seconds.
    #[serde(default)]
    pub duration: Option<u32>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub set_video_id: Option<String>,
    #[serde(default)]
    pub is_explicit: bool,
    #[serde(default)]
    pub is_video: bool,
    #[serde(default)]
    pub play_count: Option<u64>,
    #[serde(default)]
    pub year: Option<i32>,
    /// Radio / automix endpoint used to keep playing "related" tracks.
    #[serde(default)]
    pub radio_playlist_id: Option<String>,
}

impl Song {
    pub fn artist_line(&self) -> String {
        if self.artists.is_empty() {
            "Unknown artist".to_string()
        } else {
            self.artists.join(", ")
        }
    }

    /// `"Artists • Album"` subtitle used across list rows.
    pub fn subtitle(&self) -> String {
        match &self.album {
            Some(album) if !album.is_empty() => format!("{} • {}", self.artist_line(), album),
            _ => self.artist_line(),
        }
    }

    pub fn duration_label(&self) -> String {
        self.duration
            .map(crate::util::format_duration)
            .unwrap_or_else(|| "--:--".to_string())
    }
}

/// An album shelf entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub year: Option<i32>,
    #[serde(default)]
    pub song_count: Option<u32>,
    #[serde(default)]
    pub duration: Option<u32>,
}

/// An artist shelf entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub subscribers: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// A playlist shelf entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub song_count: Option<u32>,
    /// True for playlists that only exist in the local database.
    #[serde(default)]
    pub is_local: bool,
}

/// Any entry that can appear inside a horizontal/vertical shelf.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MediaItem {
    Song(Song),
    Album(Album),
    Artist(Artist),
    Playlist(Playlist),
}

impl MediaItem {
    pub fn title(&self) -> &str {
        match self {
            MediaItem::Song(s) => &s.title,
            MediaItem::Album(a) => &a.title,
            MediaItem::Artist(a) => &a.name,
            MediaItem::Playlist(p) => &p.title,
        }
    }

    pub fn subtitle(&self) -> String {
        match self {
            MediaItem::Song(s) => s.artist_line(),
            MediaItem::Album(a) => a.artists.join(", "),
            MediaItem::Artist(a) => a.subscribers.clone().unwrap_or_default(),
            MediaItem::Playlist(p) => p.author.clone().unwrap_or_default(),
        }
    }

    pub fn thumbnail(&self) -> Option<&str> {
        match self {
            MediaItem::Song(s) => s.thumbnail.as_deref(),
            MediaItem::Album(a) => a.thumbnail.as_deref(),
            MediaItem::Artist(a) => a.thumbnail.as_deref(),
            MediaItem::Playlist(p) => p.thumbnail.as_deref(),
        }
    }

    pub fn id(&self) -> &str {
        match self {
            MediaItem::Song(s) => &s.id,
            MediaItem::Album(a) => &a.id,
            MediaItem::Artist(a) => &a.id,
            MediaItem::Playlist(p) => &p.id,
        }
    }
}

/// A titled shelf of items (e.g. "Forgotten Favorites", "Trending").
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Shelf {
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub browse_id: Option<String>,
    #[serde(default)]
    pub items: Vec<MediaItem>,
}

/// The home / explore page: a list of shelves plus the mood chips.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HomePage {
    #[serde(default)]
    pub chips: Vec<String>,
    #[serde(default)]
    pub shelves: Vec<Shelf>,
}

/// A full album page.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AlbumPage {
    pub album: Album,
    #[serde(default)]
    pub songs: Vec<Song>,
    #[serde(default)]
    pub other_versions: Vec<Album>,
}

/// A full artist page.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArtistPage {
    pub artist: Artist,
    #[serde(default)]
    pub sections: Vec<Shelf>,
}

/// A full playlist page.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistPage {
    pub playlist: Playlist,
    #[serde(default)]
    pub songs: Vec<Song>,
}

/// Grouped search results.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchResults {
    #[serde(default)]
    pub songs: Vec<Song>,
    #[serde(default)]
    pub videos: Vec<Song>,
    #[serde(default)]
    pub albums: Vec<Album>,
    #[serde(default)]
    pub artists: Vec<Artist>,
    #[serde(default)]
    pub playlists: Vec<Playlist>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
            && self.videos.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
    }
}

/// A single resolved audio stream.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStream {
    pub url: String,
    pub mime_type: String,
    pub codec: String,
    /// Bits per second.
    pub bitrate: u32,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub content_length: Option<u64>,
    /// The InnerTube client name that produced this stream.
    pub client: String,
}

/// Result of resolving a track for playback.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackInfo {
    pub song: Song,
    pub stream: AudioStream,
    /// Up-next queue as returned by the `next` endpoint.
    #[serde(default)]
    pub related: Vec<Song>,
    #[serde(default)]
    pub lyrics_id: Option<String>,
}

/// Repeat behaviour of the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

/// High level transport state exposed to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum PlaybackState {
    #[default]
    Idle,
    Loading,
    Buffering,
    Playing,
    Paused,
    Stopped,
}

/// A fully materialised player snapshot for the UI.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerSnapshot {
    pub state: PlaybackState,
    pub current: Option<Song>,
    pub queue: Vec<Song>,
    pub index: usize,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
}
