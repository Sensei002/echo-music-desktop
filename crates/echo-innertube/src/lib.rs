//! # echo-innertube
//!
//! A native, dependency-light YouTube Music client written in Rust.
//!
//! This is a direct port of the upstream Android `:innertube` module: the same
//! client identities, the same endpoints, the same response shapes — only the
//! implementation language differs. Everything is synchronous and driven from
//! worker threads, which keeps the Slint UI thread free of blocking work.
//!
//! ```no_run
//! use echo_core::{AudioQuality, Settings};
//! use echo_innertube::MusicClient;
//!
//! let client = MusicClient::new(&Settings::default()).unwrap();
//! let results = client.search("daft punk", echo_innertube::SearchFilter::Songs).unwrap();
//! println!("{} songs", results.songs.len());
//!
//! let stream = client.resolve(&results.songs[0].id, AudioQuality::High).unwrap();
//! println!("streaming from {}", stream.stream.client);
//! ```

pub mod api;
pub mod cipher;
pub mod clients;
pub mod http;
pub mod parse;
pub mod player;
pub mod util;

use anyhow::Result;
use echo_core::{
    AlbumPage, ArtistPage, AudioQuality, HomePage, PlaylistPage, SearchResults, Settings, Shelf,
    Song,
};
use std::sync::Arc;

pub use api::SearchFilter;
pub use clients::YtClient;
pub use http::InnerTube;
pub use player::{ResolvedStream, StreamResolver};

/// The single entry point used by the application layer.
///
/// Owns one shared HTTP transport so connections, cookies and the visitor id
/// are reused across every request.
pub struct MusicClient {
    transport: Arc<InnerTube>,
    resolver: StreamResolver,
}

impl MusicClient {
    /// Builds a client from the persisted settings.
    pub fn new(settings: &Settings) -> Result<Self> {
        let transport = Arc::new(InnerTube::new(
            settings.locale_gl.clone(),
            settings.locale_hl.clone(),
            settings.proxy.clone(),
        )?);
        if let Some(visitor_data) = &settings.visitor_data {
            transport.set_visitor_data(Some(visitor_data.clone()));
        }
        Ok(Self {
            resolver: StreamResolver::new(transport.clone()),
            transport,
        })
    }

    /// The shared transport, for advanced callers.
    pub fn transport(&self) -> &InnerTube {
        &self.transport
    }

    /// The visitor id currently in use, if any.
    pub fn visitor_data(&self) -> Option<String> {
        self.transport.visitor_data()
    }

    // ------------------------------------------------------------- browsing

    pub fn home(&self) -> Result<HomePage> {
        api::home(&self.transport)
    }

    pub fn explore(&self) -> Result<HomePage> {
        api::explore(&self.transport)
    }

    pub fn charts(&self) -> Result<HomePage> {
        api::charts(&self.transport)
    }

    pub fn moods_and_genres(&self) -> Result<Vec<Shelf>> {
        api::moods_and_genres(&self.transport)
    }

    pub fn browse(&self, browse_id: &str) -> Result<serde_json::Value> {
        api::browse(&self.transport, browse_id)
    }

    // --------------------------------------------------------------- search

    pub fn search(&self, query: &str, filter: SearchFilter) -> Result<SearchResults> {
        api::search(&self.transport, query, filter)
    }

    pub fn search_suggestions(&self, query: &str) -> Result<Vec<String>> {
        api::search_suggestions(&self.transport, query)
    }

    // ------------------------------------------------------------- entities

    pub fn album(&self, browse_id: &str) -> Result<AlbumPage> {
        api::album(&self.transport, browse_id)
    }

    pub fn artist(&self, browse_id: &str) -> Result<ArtistPage> {
        api::artist(&self.transport, browse_id)
    }

    pub fn playlist(&self, playlist_id: &str) -> Result<PlaylistPage> {
        api::playlist(&self.transport, playlist_id)
    }

    // --------------------------------------------------------------- lyrics

    pub fn lyrics(&self, video_id: &str) -> Result<Option<String>> {
        api::lyrics(&self.transport, video_id)
    }

    // ------------------------------------------------------------ playback

    /// Resolves a video id into a directly playable audio stream.
    pub fn resolve(&self, video_id: &str, quality: AudioQuality) -> Result<ResolvedStream> {
        self.resolver.resolve(video_id, quality)
    }

    /// The up-next queue for a track (used to keep playing related music).
    pub fn related(&self, video_id: &str) -> Result<Vec<Song>> {
        self.resolver.related(video_id)
    }
}
