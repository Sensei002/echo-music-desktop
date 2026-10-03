//! Application state and the mapping from domain models to Slint models.
//!
//! All mutable state lives here, on the UI thread. Worker threads never touch
//! it directly — they post [`crate::workers::Update`] values that `main` applies
//! in order.

use crate::{
    AppWindow, EqBandData, LyricLineData, QuickAction, SettingRow, ShelfData, SongData, Theme,
    TileData,
};
use echo_core::{Album, AppPaths, Artist, Library, MediaItem, Palette, PlaybackState, Settings, Song};
use echo_innertube::MusicClient;
use echo_lyrics::{Lyrics, LyricsService, Provider};
use echo_playback::{EqConfig, PlaybackEngine, BANDS};
use slint::{ComponentHandle, SharedString};
use std::collections::HashMap;
use std::sync::Arc;

/// Everything the UI needs to render a frame.
pub struct AppState {
    pub paths: AppPaths,
    pub settings: Settings,
    pub library: Library,
    pub client: Arc<MusicClient>,
    pub lyrics: Arc<LyricsService>,
    pub engine: Option<PlaybackEngine>,

    /// URL -> decoded artwork, populated lazily as downloads finish.
    pub covers: HashMap<String, slint::Image>,
    /// Artwork URLs already handed to a worker, so they are not fetched twice.
    pub pending_covers: std::collections::HashSet<String>,
    /// Set when cached data or artwork changed so the models rebuild once per tick.
    pub covers_dirty: bool,

    // ---- cached domain data (kept so models can be rebuilt for new artwork)
    pub home: Option<echo_core::HomePage>,
    pub search: Option<echo_core::SearchResults>,
    pub library_songs: Vec<Song>,
    pub collections: Vec<echo_core::Playlist>,
    pub favorites: Vec<Song>,

    // ---- playback mirrors -------------------------------------------------
    pub queue: Vec<Song>,
    pub current: Option<Song>,
    pub is_playing: bool,
    pub progress: f32,
    pub position_label: String,
    pub duration_label: String,
    pub shuffle: bool,
    pub repeat_mode: i32,
    pub liked: bool,
    pub downloaded: bool,
    /// Id currently being resolved, so the engine's `TrackStarted` echo does not
    /// trigger a second resolution.
    pub pending_resolve: Option<String>,

    // ---- lyrics ------------------------------------------------------------
    pub lyrics_doc: Option<Lyrics>,
    pub lyrics_visible: bool,
    /// The active lyric line index last rendered (avoids rebuilding the model).
    pub last_active_line: Option<usize>,

    pub selected_chip: i32,
    pub status: String,
    pub loading: bool,
}

impl AppState {
    pub fn new(
        paths: AppPaths,
        settings: Settings,
        library: Library,
        client: Arc<MusicClient>,
        lyrics: Arc<LyricsService>,
        engine: Option<PlaybackEngine>,
    ) -> Self {
        Self {
            paths,
            settings,
            library,
            client,
            lyrics,
            engine,
            covers: HashMap::new(),
            pending_covers: std::collections::HashSet::new(),
            covers_dirty: false,
            home: None,
            search: None,
            library_songs: Vec::new(),
            collections: Vec::new(),
            favorites: Vec::new(),
            queue: Vec::new(),
            current: None,
            is_playing: false,
            progress: 0.0,
            position_label: "0:00".into(),
            duration_label: "0:00".into(),
            shuffle: false,
            repeat_mode: 0,
            liked: false,
            downloaded: false,
            pending_resolve: None,
            lyrics_doc: None,
            lyrics_visible: true,
            last_active_line: None,
            selected_chip: -1,
            status: String::new(),
            loading: false,
        }
    }

    /// The palette for the current appearance.
    pub fn palette(&self, system_dark: bool) -> Palette {
        self.settings.palette(system_dark)
    }

    /// The current audio quality preference.
    pub fn quality(&self) -> echo_core::AudioQuality {
        self.settings.audio_quality
    }

    // ------------------------------------------------------------- covers

    /// Looks up decoded artwork for a URL.
    pub fn cover(&self, url: Option<&str>) -> slint::Image {
        url.and_then(|url| self.covers.get(url).cloned())
            .unwrap_or_default()
    }

    /// Every artwork URL referenced by the cached domain data.
    pub fn collect_thumbnails(&self) -> Vec<String> {
        let mut urls: Vec<String> = Vec::new();
        let mut push = |url: Option<&str>| {
            if let Some(url) = url {
                if !url.is_empty() && !urls.iter().any(|existing| existing == url) {
                    urls.push(url.to_string());
                }
            }
        };

        if let Some(home) = &self.home {
            for shelf in &home.shelves {
                for item in &shelf.items {
                    push(item.thumbnail());
                }
            }
        }
        if let Some(search) = &self.search {
            for song in search.songs.iter().chain(search.videos.iter()) {
                push(song.thumbnail.as_deref());
            }
            for album in &search.albums {
                push(album.thumbnail.as_deref());
            }
            for artist in &search.artists {
                push(artist.thumbnail.as_deref());
            }
            for playlist in &search.playlists {
                push(playlist.thumbnail.as_deref());
            }
        }
        for song in self
            .library_songs
            .iter()
            .chain(self.favorites.iter())
            .chain(self.queue.iter())
            .chain(self.current.iter())
        {
            push(song.thumbnail.as_deref());
        }
        urls
    }

    // ------------------------------------------------------------- models

    pub fn song_data(&self, song: &Song) -> SongData {
        let liked = self.library.is_liked(&song.id).unwrap_or(false);
        let downloaded = self.library.is_downloaded(&song.id).unwrap_or(false);
        SongData {
            id: song.id.clone().into(),
            title: song.title.clone().into(),
            subtitle: song.subtitle().into(),
            duration: song.duration_label().into(),
            cover: self.cover(song.thumbnail.as_deref()),
            is_video: song.is_video,
            is_liked: liked,
            is_downloaded: downloaded,
        }
    }

    pub fn song_list(&self, songs: &[Song]) -> Vec<SongData> {
        songs.iter().map(|song| self.song_data(song)).collect()
    }

    pub fn tile_data(&self, item: &MediaItem) -> TileData {
        let kind = match item {
            MediaItem::Song(_) => "song",
            MediaItem::Album(_) => "album",
            MediaItem::Artist(_) => "artist",
            MediaItem::Playlist(_) => "playlist",
        };
        TileData {
            id: item.id().to_string().into(),
            title: item.title().to_string().into(),
            subtitle: item.subtitle().into(),
            cover: self.cover(item.thumbnail()),
            kind: kind.into(),
        }
    }

    pub fn shelves(&self) -> Vec<ShelfData> {
        let Some(home) = &self.home else {
            return Vec::new();
        };
        home.shelves
            .iter()
            .filter(|shelf| !shelf.items.is_empty())
            .map(|shelf| {
                let kind = if shelf
                    .items
                    .iter()
                    .all(|item| matches!(item, MediaItem::Song(_)))
                {
                    "song"
                } else {
                    "collection"
                };
                ShelfData {
                    title: shelf.title.clone().into(),
                    subtitle: shelf.subtitle.clone().unwrap_or_default().into(),
                    kind: kind.into(),
                    items: shelf
                        .items
                        .iter()
                        .map(|item| self.tile_data(item))
                        .collect::<Vec<_>>()
                        .into(),
                }
            })
            .collect()
    }

    pub fn lyric_lines(&self) -> Vec<LyricLineData> {
        let Some(lyrics) = &self.lyrics_doc else {
            return Vec::new();
        };
        let active = self
            .engine
            .as_ref()
            .map(|engine| engine.snapshot().position_ms)
            .and_then(|position| lyrics.active_line_index(position))
            .unwrap_or(0);
        lyrics
            .lines
            .iter()
            .enumerate()
            .map(|(index, line)| LyricLineData {
                text: line.text.clone().into(),
                translation: line.translation.clone().unwrap_or_default().into(),
                start_ms: line.start_ms as i32,
                is_active: index == active,
            })
            .collect()
    }

    /// The index of the currently highlighted lyric line.
    pub fn active_line(&self) -> Option<usize> {
        let lyrics = self.lyrics_doc.as_ref()?;
        let position = self
            .engine
            .as_ref()
            .map(|engine| engine.snapshot().position_ms)
            .unwrap_or(0);
        lyrics.active_line_index(position)
    }

    /// The quick-access tiles of the library screen.
    pub fn quick_actions(&self) -> Vec<QuickAction> {
        let stats = self.library.stats().unwrap_or_default();
        let local = self.settings.local_folders.len().to_string();
        [
            ("liked", "♥", "Liked", stats.liked.to_string()),
            ("downloaded", "⤓", "Downloaded", stats.downloads.to_string()),
            ("history", "⏱", "Recently played", stats.plays.to_string()),
            ("top", "↗", "My top 50", String::new()),
            ("bottom", "↘", "My bottom 50", String::new()),
            ("local", "▣", "Local files", local),
        ]
        .iter()
        .map(|(id, glyph, label, count)| QuickAction {
            id: (*id).into(),
            glyph: (*glyph).into(),
            label: (*label).into(),
            count: count.clone().into(),
        })
        .collect()
    }

    /// The equalizer bands for the settings screen.
    pub fn eq_bands(&self) -> Vec<EqBandData> {
        BANDS
            .iter()
            .enumerate()
            .map(|(index, frequency)| {
                let label = if *frequency >= 1000.0 {
                    format!("{:.0}k", frequency / 1000.0)
                } else {
                    format!("{frequency:.0}")
                };
                EqBandData {
                    label: label.into(),
                    gain: self
                        .settings
                        .equalizer_bands
                        .get(index)
                        .copied()
                        .unwrap_or(0.0),
                }
            })
            .collect()
    }

    /// The equalizer configuration implied by the current settings.
    pub fn eq_config(&self) -> EqConfig {
        EqConfig {
            enabled: self.settings.equalizer_enabled,
            gains: self.settings.equalizer_bands.clone(),
            stereo_width: self.settings.stereo_widening,
            preamp: 1.0,
        }
    }

    /// The provider order shown under the lyrics settings.
    pub fn provider_order_label(&self) -> String {
        self.settings
            .lyrics_provider_order
            .iter()
            .filter_map(|id| Provider::from_id(id))
            .map(|provider| provider.display_name())
            .collect::<Vec<_>>()
            .join(" › ")
    }

    /// The label shown next to the lyrics header.
    pub fn lyrics_source_label(&self) -> String {
        self.lyrics_doc
            .as_ref()
            .and_then(|lyrics| Provider::from_id(&lyrics.provider))
            .map(|provider| provider.display_name().to_string())
            .unwrap_or_default()
    }

    /// Builds the four settings groups from the current settings.
    #[allow(clippy::type_complexity)]
    pub fn settings_groups(
        &self,
    ) -> (
        Vec<SettingRow>,
        Vec<SettingRow>,
        Vec<SettingRow>,
        Vec<SettingRow>,
    ) {
        let s = &self.settings;
        let toggle = |id: &str, label: &str, description: &str, value: bool| SettingRow {
            id: id.into(),
            label: label.into(),
            description: description.into(),
            kind: "toggle".into(),
            value: "".into(),
            toggle: value,
        };
        let nav = |id: &str, label: &str, description: &str, value: &str| SettingRow {
            id: id.into(),
            label: label.into(),
            description: description.into(),
            kind: "nav".into(),
            value: value.into(),
            toggle: false,
        };

        let appearance = vec![
            nav(
                "theme_mode",
                "Theme",
                "Follow the system, or force light/dark",
                match s.theme_mode {
                    echo_core::ThemeMode::System => "System",
                    echo_core::ThemeMode::Light => "Light",
                    echo_core::ThemeMode::Dark => "Dark",
                },
            ),
            nav("theme_color", "Accent colour", "Echo red by default", &s.theme_color),
            toggle("pure_black", "Pure black", "Use OLED-friendly black surfaces", s.pure_black),
            nav(
                "ui_scale",
                "UI scale",
                "Adjust the interface density",
                &format!("{:.2}×", s.ui_scale),
            ),
        ];

        let playback = vec![
            toggle("crossfade", "Crossfade", "Blend the end of one track into the next", s.crossfade_enabled),
            nav(
                "crossfade_duration",
                "Crossfade length",
                "Seconds of overlap between tracks",
                &format!("{}s", s.crossfade_duration),
            ),
            toggle("gapless", "Gapless playback", "Remove silence between tracks", s.gapless),
            nav(
                "audio_quality",
                "Audio quality",
                "Higher quality uses more data",
                match s.audio_quality {
                    echo_core::AudioQuality::Low => "Low",
                    echo_core::AudioQuality::Medium => "Medium",
                    echo_core::AudioQuality::High => "High",
                    echo_core::AudioQuality::Highest => "Highest",
                },
            ),
            toggle("data_saver", "Data saver", "Reduce bandwidth on metered connections", s.data_saver),
            toggle("hide_videos", "Hide video songs", "Filter music videos from the feed", s.hide_video_songs),
        ];

        let lyrics = vec![
            toggle("word_by_word", "Word-by-word", "Highlight each word as it is sung", s.word_by_word_lyrics),
            toggle("translate_lyrics", "Translate lyrics", "Machine-translate into your language", s.translate_lyrics),
            nav("lyrics_providers", "Provider order", "Tap to rotate the source order", ""),
        ];

        let extras = vec![
            toggle("canvas", "Canvas animations", "Looping artwork behind the player", s.canvas_enabled),
            toggle("echo_brain", "Echo Brain", "Auto-inject aligned tracks into the queue", s.echo_brain_enabled),
            toggle("pause_on_mute", "Pause on mute", "Pause when the output is muted", s.pause_on_mute),
            toggle("resume_on_bluetooth", "Resume on connect", "Resume when headphones reconnect", s.resume_on_bluetooth),
            toggle("discord_rpc", "Discord Rich Presence", "Show what you are listening to", s.discord_rpc),
            nav("proxy", "Proxy", "Route requests through a proxy", s.proxy.as_deref().unwrap_or("None")),
            nav("clear_cache", "Clear cache", "Remove downloaded artwork and lyrics", ""),
            nav("about", "About", "Echo Music Desktop", env!("CARGO_PKG_VERSION")),
        ];

        (appearance, playback, lyrics, extras)
    }

    /// Applies the current palette to the Slint `Theme` global.
    pub fn apply_theme(&self, window: &AppWindow, system_dark: bool) {
        let palette = self.palette(system_dark);
        let color = |rgb: echo_core::Rgb| slint::Color::from_argb_u8(255, rgb.0, rgb.1, rgb.2);
        let theme = window.global::<Theme>();

        theme.set_primary(color(palette.primary));
        theme.set_on_primary(color(palette.on_primary));
        theme.set_primary_container(color(palette.primary_container));
        theme.set_on_primary_container(color(palette.on_primary_container));
        theme.set_secondary(color(palette.secondary));
        theme.set_on_secondary(color(palette.on_secondary));
        theme.set_secondary_container(color(palette.secondary_container));
        theme.set_on_secondary_container(color(palette.on_secondary_container));
        theme.set_tertiary(color(palette.tertiary));
        theme.set_on_tertiary(color(palette.on_tertiary));
        theme.set_tertiary_container(color(palette.tertiary_container));
        theme.set_on_tertiary_container(color(palette.on_tertiary_container));
        theme.set_error(color(palette.error));
        theme.set_on_error(color(palette.on_error));
        theme.set_background(color(palette.background));
        theme.set_on_background(color(palette.on_background));
        theme.set_surface(color(palette.surface));
        theme.set_on_surface(color(palette.on_surface));
        theme.set_surface_variant(color(palette.surface_variant));
        theme.set_on_surface_variant(color(palette.on_surface_variant));
        theme.set_surface_container_lowest(color(palette.surface_container_lowest));
        theme.set_surface_container_low(color(palette.surface_container_low));
        theme.set_surface_container(color(palette.surface_container));
        theme.set_surface_container_high(color(palette.surface_container_high));
        theme.set_surface_container_highest(color(palette.surface_container_highest));
        theme.set_outline(color(palette.outline));
        theme.set_outline_variant(color(palette.outline_variant));
        theme.set_inverse_surface(color(palette.inverse_surface));
        theme.set_inverse_on_surface(color(palette.inverse_on_surface));

        // Translucent surfaces used by cards and chips.
        let card = slint::Color::from_argb_u8(
            0x4d,
            palette.surface_variant.0,
            palette.surface_variant.1,
            palette.surface_variant.2,
        );
        let chip = slint::Color::from_argb_u8(
            0x59,
            palette.surface_variant.0,
            palette.surface_variant.1,
            palette.surface_variant.2,
        );
        let glass = slint::Color::from_argb_u8(
            0xd9,
            palette.surface_container.0,
            palette.surface_container.1,
            palette.surface_container.2,
        );
        theme.set_card(card);
        theme.set_chip(chip);
        theme.set_chip_selected(color(palette.primary));
        theme.set_glass(glass);
        theme.set_glass_strong(glass_strong);
    }

    // ------------------------------------------------------------ lookups

    /// Finds a song by id together with the list it came from, so activating it
    /// builds the right play queue.
    pub fn lookup(&self, id: &str) -> Option<(Song, Vec<Song>)> {
        let find = |songs: &[Song]| songs.iter().find(|song| song.id == id).cloned();

        if let Some(song) = find(&self.favorites) {
            return Some((song, self.favorites.clone()));
        }
        if let Some(song) = find(&self.library_songs) {
            return Some((song, self.library_songs.clone()));
        }
        if let Some(song) = find(&self.queue) {
            return Some((song, self.queue.clone()));
        }
        if let Some(search) = &self.search {
            let all: Vec<Song> = search
                .songs
                .iter()
                .chain(search.videos.iter())
                .cloned()
                .collect();
            if let Some(song) = find(&all) {
                return Some((song, all));
            }
        }
        if let Some(home) = &self.home {
            for shelf in &home.shelves {
                let songs: Vec<Song> = shelf
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        MediaItem::Song(song) => Some(song.clone()),
                        _ => None,
                    })
                    .collect();
                if let Some(song) = find(&songs) {
                    return Some((song, songs));
                }
            }
        }
        None
    }

    /// The songs of a home shelf, by title.
    pub fn shelf_songs(&self, title: &str) -> Vec<Song> {
        self.home
            .as_ref()
            .and_then(|home| home.shelves.iter().find(|shelf| shelf.title == title))
            .map(|shelf| {
                shelf
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        MediaItem::Song(song) => Some(song.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The chips of the home feed.
    pub fn chips(&self) -> Vec<String> {
        self.home
            .as_ref()
            .map(|home| home.chips.clone())
            .unwrap_or_default()
    }

    /// The library songs for the selected tab (0 playlists, 1 songs, 2 albums,
    /// 3 artists, 4 local).
    pub fn library_songs_for(&self, tab: i32) -> Vec<Song> {
        match tab {
            1 => self.library.history(300).unwrap_or_default(),
            2 | 3 => self.library.liked_songs().unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// The library collections for the selected tab.
    pub fn library_collections_for(&self, tab: i32) -> (String, Vec<TileData>) {
        match tab {
            0 => {
                let collections = self
                    .library
                    .playlists()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|playlist| self.tile_data(&MediaItem::Playlist(playlist)))
                    .collect();
                ("Playlists".to_string(), collections)
            }
            2 => {
                let songs = self.library.liked_songs().unwrap_or_default();
                let collections = albums_from_songs(&songs)
                    .into_iter()
                    .map(|album| self.tile_data(&MediaItem::Album(album)))
                    .collect();
                ("Albums".to_string(), collections)
            }
            3 => {
                let songs = self.library.liked_songs().unwrap_or_default();
                let collections = artists_from_songs(&songs)
                    .into_iter()
                    .map(|artist| self.tile_data(&MediaItem::Artist(artist)))
                    .collect();
                ("Artists".to_string(), collections)
            }
            _ => ("Local".to_string(), Vec::new()),
        }
    }

    /// Persists the current settings, logging (rather than failing) on error.
    pub fn save_settings(&self) {
        if let Err(err) = self.settings.save(&self.paths.settings_file()) {
            log::warn!("failed to save settings: {err:#}");
        }
    }

    /// Pushes the current equalizer configuration to the audio engine.
    pub fn push_equalizer(&self) {
        let config = self.eq_config();
        if let Some(engine) = &self.engine {
            engine.equalizer().update(config.clone());
            engine.send(echo_playback::Command::SetEqualizer(Box::new(config)));
        }
    }
}

/// Groups songs into albums (by album id, falling back to the album name).
pub fn albums_from_songs(songs: &[Song]) -> Vec<Album> {
    let mut albums: Vec<Album> = Vec::new();
    for song in songs {
        let Some(name) = song.album.clone() else { continue };
        let id = song
            .album_id
            .clone()
            .unwrap_or_else(|| format!("album:{}", name.to_lowercase()));
        if let Some(existing) = albums.iter_mut().find(|album| album.id == id) {
            existing.song_count = Some(existing.song_count.unwrap_or(0) + 1);
        } else {
            albums.push(Album {
                id,
                title: name,
                artists: song.artists.clone(),
                thumbnail: song.thumbnail.clone(),
                year: song.year,
                song_count: Some(1),
                duration: None,
            });
        }
    }
    albums
}

/// Groups songs into artists (by name).
pub fn artists_from_songs(songs: &[Song]) -> Vec<Artist> {
    let mut artists: Vec<Artist> = Vec::new();
    for song in songs {
        for name in &song.artists {
            let id = format!("artist:{}", name.to_lowercase());
            if !artists.iter().any(|artist| artist.id == id) {
                artists.push(Artist {
                    id,
                    name: name.clone(),
                    thumbnail: song.thumbnail.clone(),
                    subscribers: None,
                    description: None,
                });
            }
        }
    }
    artists
}

/// Human-readable label for a transport state.
pub fn state_label(state: PlaybackState) -> &'static str {
    match state {
        PlaybackState::Idle => "Idle",
        PlaybackState::Loading => "Loading",
        PlaybackState::Buffering => "Buffering",
        PlaybackState::Playing => "Playing",
        PlaybackState::Paused => "Paused",
        PlaybackState::Stopped => "Stopped",
    }
}

/// The greeting shown at the top of the home feed, derived from the clock.
pub fn greeting() -> String {
    let hour = chrono::Timelike::hour(&chrono::Local::now());
    match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        18..=22 => "Good evening",
        _ => "Good night",
    }
    .to_string()
}

/// The window's current tab index.
pub fn tab_index(window: &AppWindow) -> i32 {
    window.get_current_tab()
}

/// Converts a Rust string slice into a Slint model of strings.
pub fn string_model(values: &[String]) -> slint::ModelRc<SharedString> {
    values
        .iter()
        .map(|value| SharedString::from(value.as_str()))
        .collect::<Vec<_>>()
        .into()
}
