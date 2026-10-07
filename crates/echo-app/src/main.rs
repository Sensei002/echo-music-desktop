//! Echo Music Desktop — application entry point.
//!
//! The Slint UI runs on the main thread. Every blocking operation (network,
//! stream resolution, downloads, artwork) is delegated to a worker thread that
//! reports back through a channel. A 120 ms timer drains those messages, folds
//! in the audio engine's events and pushes the result onto the window.

mod covers;
mod state;
mod workers;

slint::include_modules!();

use crate::state::{greeting, model, string_model, AppState};
use crate::workers::{Update, Workers};
use echo_core::{
    AppPaths, AudioQuality, Library, MediaItem, PlaybackState, RepeatMode, Settings, Song,
    ThemeMode,
};
use echo_innertube::{MusicClient, SearchFilter};
use echo_lyrics::LyricsService;
use echo_playback::{Command, Event, PlaybackEngine};
use slint::{ComponentHandle, SharedString, Timer, TimerMode};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

/// Shared, UI-thread-owned application state.
type Shared = Rc<RefCell<AppState>>;

/// Accent colours cycled by the "Accent colour" setting.
const ACCENTS: [&str; 5] = ["#ED5564", "#7C4DFF", "#00BCD4", "#4CAF50", "#FF9800"];

/// The label of the home screen's favourites card.
const FAVOURITES_TITLE: &str = "Forgotten favourites";

/// Builds the shared HTTP client used for artwork and downloads.
///
/// Echo does not talk to YouTube through this client (the InnerTube crate owns
/// that); it is only for fetching media and images from CDN URLs.
pub fn http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("echo-music-desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("failed to build the HTTP client")
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let paths = AppPaths::new()?;
    paths.ensure()?;

    let mut settings = Settings::load(&paths.settings_file());
    settings.sanitize();

    let library = Library::open(&paths.library_db())?;
    let client = Arc::new(MusicClient::new(&settings)?);
    let lyrics = Arc::new(LyricsService::new(&settings.lyrics_provider_order));
    let engine = PlaybackEngine::try_new_or_silent();

    let state: Shared = Rc::new(RefCell::new(AppState::new(
        paths, settings, library, client, lyrics, engine,
    )));

    let (tx, rx) = mpsc::channel::<Update>();
    let workers = {
        let s = state.borrow();
        Workers::new(
            s.client.clone(),
            s.lyrics.clone(),
            s.library.clone(),
            &s.paths,
            tx,
        )
    };

    let window = AppWindow::new()?;

    // ---- initial theme and static properties ------------------------------
    {
        let s = state.borrow();
        s.apply_theme(&window, true);
        window.set_app_version(env!("CARGO_PKG_VERSION").into());
        window.set_greeting(greeting().into());
        window.set_favorites_title(FAVOURITES_TITLE.into());
        window.set_lyrics_visible(s.lyrics_visible);
        window.set_selected_chip(s.selected_chip);
        window.set_shuffle(s.shuffle);
        window.set_repeat_mode(s.repeat_mode);
    }
    refresh_settings(&window, &state.borrow());
    refresh_library_models(&window, &state.borrow());

    // Push the persisted equalizer and crossfade into the engine.
    {
        let s = state.borrow();
        s.push_equalizer();
        if let Some(engine) = &s.engine {
            engine.send(Command::SetCrossfade {
                enabled: s.settings.crossfade_enabled,
                seconds: s.settings.crossfade_duration,
            });
        }
    }

    // Start Discord Rich Presence if the user left it enabled.
    state.borrow_mut().sync_discord();

    wire_callbacks(&window, &state, &workers);

    // ---- first loads -------------------------------------------------------
    workers.home();
    request_covers(&state, &workers);

    // ---- the update loop ---------------------------------------------------
    let timer = Timer::default();
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        timer.start(TimerMode::Repeated, Duration::from_millis(120), move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            pump(&window, &state, &workers, &rx);
        });
    }

    window.run()?;
    state.borrow().save_settings();
    Ok(())
}

// ---------------------------------------------------------------------------
// Model refresh helpers
// ---------------------------------------------------------------------------

fn refresh_settings(window: &AppWindow, state: &AppState) {
    let (appearance, playback, lyrics, extras) = state.settings_groups();
    window.set_settings_appearance(model(appearance));
    window.set_settings_playback(model(playback));
    window.set_settings_lyrics(model(lyrics));
    window.set_settings_extras(model(extras));
    window.set_eq_bands(model(state.eq_bands()));
    window.set_eq_enabled(state.settings.equalizer_enabled);
    window.set_provider_order(state.provider_order_label().into());
}

fn refresh_library_models(window: &AppWindow, state: &AppState) {
    window.set_quick_actions(model(state.quick_actions()));
    window.set_library_songs(model(state.song_list(&state.library_songs)));
    let (title, collections) = state.library_collections_for(window.get_library_tab());
    window.set_library_title(title.into());
    window.set_library_collections(model(collections));
}

fn refresh_search(window: &AppWindow, state: &AppState) {
    match &state.search {
        Some(results) => {
            window.set_search_songs(model(state.song_list(&results.songs)));
            window.set_search_videos(model(state.song_list(&results.videos)));
            window.set_search_albums(model(
                results
                    .albums
                    .iter()
                    .map(|album| state.tile_data(&MediaItem::Album(album.clone())))
                    .collect::<Vec<_>>(),
            ));
            window.set_search_artists(model(
                results
                    .artists
                    .iter()
                    .map(|artist| state.tile_data(&MediaItem::Artist(artist.clone())))
                    .collect::<Vec<_>>(),
            ));
            window.set_search_playlists(model(
                results
                    .playlists
                    .iter()
                    .map(|playlist| state.tile_data(&MediaItem::Playlist(playlist.clone())))
                    .collect::<Vec<_>>(),
            ));
        }
        None => {
            window.set_search_songs(model(Vec::<SongData>::new()));
            window.set_search_videos(model(Vec::<SongData>::new()));
            window.set_search_albums(model(Vec::<TileData>::new()));
            window.set_search_artists(model(Vec::<TileData>::new()));
            window.set_search_playlists(model(Vec::<TileData>::new()));
        }
    }
}

/// Rebuilds every list model (used after new artwork arrives).
fn refresh_all_models(window: &AppWindow, state: &AppState) {
    window.set_chips(string_model(&state.chips()));
    window.set_shelves(model(state.shelves()));
    window.set_favorites(model(state.song_list(&state.favorites)));
    refresh_search(window, state);
    refresh_library_models(window, state);
    if let Some(song) = &state.current {
        window.set_now_playing(state.song_data(song));
    }
}

fn load_library_tab(window: &AppWindow, state: &Shared) {
    let songs = state.borrow().library_songs_for(window.get_library_tab());
    state.borrow_mut().library_songs = songs;
    refresh_library_models(window, &state.borrow());
}

/// Asks the workers for any artwork that is not cached yet.
fn request_covers(state: &Shared, workers: &Workers) {
    let missing: Vec<String> = {
        let mut s = state.borrow_mut();
        let candidates = s.collect_thumbnails();
        let mut missing = Vec::new();
        for url in candidates {
            if !s.covers.contains_key(&url) && !s.pending_covers.contains(&url) {
                s.pending_covers.insert(url.clone());
                missing.push(url);
            }
        }
        missing
    };
    workers.covers(missing);
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

/// Replaces the queue, starts playing `songs[start]` and resolves its stream.
fn play_songs(
    window: &AppWindow,
    state: &Shared,
    workers: &Workers,
    songs: Vec<Song>,
    start: usize,
) {
    if songs.is_empty() {
        return;
    }
    let start = start.min(songs.len() - 1);
    let song = songs[start].clone();

    {
        let mut s = state.borrow_mut();
        s.queue = songs.clone();
        s.current = Some(song.clone());
        s.pending_resolve = Some(song.id.clone());
        s.progress = 0.0;
        s.position_label = "0:00".into();
        s.duration_label = song.duration_label();
        s.status = "Loading\u{2026}".into();
        s.loading = true;
        s.lyrics_doc = None;
        s.last_active_line = None;
    }

    if let Some(engine) = &state.borrow().engine {
        engine.send(Command::SetQueue {
            songs: songs.clone(),
            start,
        });
    }

    window.set_now_playing(state.borrow().song_data(&song));
    window.set_has_track(true);
    window.set_player_open(true);
    window.set_is_playing(true);
    window.set_loading(true);
    window.set_status("Loading\u{2026}".into());
    window.set_progress(0.0);
    window.set_position_label("0:00".into());
    window.set_duration_label(song.duration_label().into());
    window.set_lyrics(model(Vec::<LyricLineData>::new()));
    window.set_lyrics_source("".into());

    let quality = state.borrow().quality();
    workers.resolve(song, quality);
}

/// Plays a track by id, using its list context as the queue.
fn activate_song(window: &AppWindow, state: &Shared, workers: &Workers, id: &str) {
    match state.borrow().lookup(id) {
        Some((song, context)) => {
            let start = context
                .iter()
                .position(|candidate| candidate.id == song.id)
                .unwrap_or(0);
            play_songs(window, state, workers, context, start);
        }
        None => window.set_status("Track not found".into()),
    }
}

// ---------------------------------------------------------------------------
// Callbacks
// ---------------------------------------------------------------------------

fn wire_callbacks(window: &AppWindow, state: &Shared, workers: &Workers) {
    // ---- navigation --------------------------------------------------------
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_tab_selected(move |index| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            window.set_current_tab(index);
            if index == 2 {
                load_library_tab(&window, &state);
            }
        });
    }
    {
        let window_weak = window.as_weak();
        window.on_close_player(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            window.set_player_open(false)
        });
    }
    {
        let window_weak = window.as_weak();
        window.on_open_player(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            window.set_player_open(true)
        });
    }
    {
        let window_weak = window.as_weak();
        window.on_close_settings(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            window.set_settings_open(false)
        });
    }
    {
        let window_weak = window.as_weak();
        window.on_open_settings(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            window.set_settings_open(true)
        });
    }

    // ---- home --------------------------------------------------------------
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        window.on_chip_selected(move |index| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            window.set_selected_chip(index);
            let chip = state.borrow().chips().get(index.max(0) as usize).cloned();
            if let Some(chip) = chip {
                let _ = state.borrow().library.record_search(&chip);
                window.set_current_tab(1);
                window.set_search_query(chip.as_str().into());
                window.set_searching(true);
                workers.search(chip, SearchFilter::All);
            }
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        window.on_shelf_item_clicked(move |id, kind| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            let kind = kind.to_string();
            match kind.as_str() {
                "song" => activate_song(&window, &state, &workers, &id),
                "playlist" if id.starts_with("local:") => {
                    let songs = state
                        .borrow()
                        .library
                        .playlist_songs(&id)
                        .unwrap_or_default();
                    if songs.is_empty() {
                        window.set_status("This playlist is empty".into());
                    } else {
                        play_songs(&window, &state, &workers, songs, 0);
                    }
                }
                _ => workers.collection(id, kind),
            }
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        window.on_home_play_all(move |title, _kind| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let title = title.to_string();
            let songs = {
                let s = state.borrow();
                if title == FAVOURITES_TITLE {
                    s.favorites.clone()
                } else {
                    s.shelf_songs(&title)
                }
            };
            if songs.is_empty() {
                window.set_status("Nothing to play here".into());
            } else {
                play_songs(&window, &state, &workers, songs, 0);
            }
        });
    }

    // ---- search ------------------------------------------------------------
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        window.on_search_edited(move |text| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            window.set_search_query(text.clone());
            if text.trim().is_empty() {
                state.borrow_mut().search = None;
                window.set_suggestions(model(Vec::<SharedString>::new()));
                refresh_search(&window, &state.borrow());
            } else {
                workers.suggestions(text.to_string());
            }
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        window.on_search_submit(move |text| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            window.set_search_query(text.clone());
            window.set_suggestions(model(Vec::<SharedString>::new()));
            if text.trim().is_empty() {
                state.borrow_mut().search = None;
                window.set_searching(false);
                refresh_search(&window, &state.borrow());
                return;
            }
            let _ = state.borrow().library.record_search(&text);
            window.set_searching(true);
            workers.search(text.to_string(), SearchFilter::All);
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        window.on_suggestion_chosen(move |text| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let text = text.to_string();
            window.set_search_query(text.as_str().into());
            window.set_suggestions(model(Vec::<SharedString>::new()));
            let _ = state.borrow().library.record_search(&text);
            window.set_searching(true);
            workers.search(text, SearchFilter::All);
        });
    }

    // ---- library -----------------------------------------------------------
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_library_tab_selected(move |index| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            window.set_library_tab(index);
            load_library_tab(&window, &state);
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        window.on_quick_action(move |id| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            if id == "local" {
                let folders = state.borrow().settings.local_folders.len();
                let message = if folders == 0 {
                    "Add folders in settings to scan local files"
                } else {
                    "Local scanning is available on the desktop build"
                };
                window.set_status(message.into());
                return;
            }
            let songs = {
                let s = state.borrow();
                match id.as_str() {
                    "liked" => s.library.liked_songs().unwrap_or_default(),
                    "downloaded" => s
                        .library
                        .downloads()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|record| record.song)
                        .collect(),
                    "history" => s.library.history(300).unwrap_or_default(),
                    "top" => s.library.top_songs(50).unwrap_or_default(),
                    "bottom" => s.library.bottom_songs(50).unwrap_or_default(),
                    _ => Vec::new(),
                }
            };
            {
                let mut s = state.borrow_mut();
                s.library_songs = songs;
                s.covers_dirty = true;
            }
            window.set_library_tab(1);
            refresh_library_models(&window, &state.borrow());
            request_covers(&state, &workers);
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        window.on_create_playlist(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let name = format!(
                "New playlist {}",
                chrono::Local::now().format("%b %d, %H:%M")
            );
            match state.borrow().library.create_playlist(&name, None) {
                Ok(_) => window.set_status(format!("Created \u{201c}{name}\u{201d}").into()),
                Err(err) => window.set_status(format!("Could not create it: {err}").into()),
            }
            window.set_library_tab(0);
            load_library_tab(&window, &state);
            request_covers(&state, &workers);
        });
    }

    // ---- playback ----------------------------------------------------------
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        window.on_song_activated(move |id| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            activate_song(&window, &state, &workers, id.as_str());
        });
    }
    {
        let state = state.clone();
        window.on_toggle_play(move || {
            if let Some(engine) = &state.borrow().engine {
                engine.send(Command::PlayPause);
            }
        });
    }
    {
        let state = state.clone();
        window.on_play_next(move || {
            if let Some(engine) = &state.borrow().engine {
                engine.send(Command::Next);
            }
        });
    }
    {
        let state = state.clone();
        window.on_play_previous(move || {
            if let Some(engine) = &state.borrow().engine {
                engine.send(Command::Previous);
            }
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_seek(move |value| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let value = value.clamp(0.0, 1.0);
            let duration = state
                .borrow()
                .engine
                .as_ref()
                .map(|engine| engine.snapshot().duration_ms)
                .unwrap_or(0);
            if let Some(engine) = &state.borrow().engine {
                engine.send(Command::Seek((value as f64 * duration as f64) as u64));
            }
            window.set_progress(value);
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_toggle_shuffle(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let shuffle = {
                let mut s = state.borrow_mut();
                s.shuffle = !s.shuffle;
                s.shuffle
            };
            window.set_shuffle(shuffle);
            if let Some(engine) = &state.borrow().engine {
                engine.send(Command::SetShuffle(shuffle));
            }
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_cycle_repeat(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let mode = {
                let mut s = state.borrow_mut();
                s.repeat_mode = (s.repeat_mode + 1) % 3;
                s.repeat_mode
            };
            window.set_repeat_mode(mode);
            let repeat = match mode {
                1 => RepeatMode::All,
                2 => RepeatMode::One,
                _ => RepeatMode::Off,
            };
            if let Some(engine) = &state.borrow().engine {
                engine.send(Command::SetRepeat(repeat));
            }
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        window.on_toggle_like(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let Some(song) = state.borrow().current.clone() else {
                return;
            };
            let liked = state.borrow().library.is_liked(&song.id).unwrap_or(false);
            let result = if liked {
                state.borrow().library.unlike_song(&song.id)
            } else {
                state.borrow().library.like_song(&song)
            };
            if let Err(err) = result {
                window.set_status(format!("Could not update the library: {err}").into());
            }
            window.set_now_playing(state.borrow().song_data(&song));
            state.borrow_mut().covers_dirty = true;
            request_covers(&state, &workers);
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        window.on_toggle_download(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let Some(song) = state.borrow().current.clone() else {
                return;
            };
            let downloaded = state
                .borrow()
                .library
                .is_downloaded(&song.id)
                .unwrap_or(false);
            if downloaded {
                let _ = state.borrow().library.remove_download(&song.id);
                window.set_now_playing(state.borrow().song_data(&song));
                window.set_status("Removed from downloads".into());
                state.borrow_mut().covers_dirty = true;
            } else {
                window.set_status("Downloading\u{2026}".into());
                let quality = state.borrow().quality();
                workers.download(song, quality);
            }
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_toggle_lyrics(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let visible = {
                let mut s = state.borrow_mut();
                s.lyrics_visible = !s.lyrics_visible;
                s.lyrics_visible
            };
            window.set_lyrics_visible(visible);
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_open_lyrics_source(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let label = state.borrow().lyrics_source_label();
            let message = if label.is_empty() {
                "No lyrics source found".to_string()
            } else {
                format!("Lyrics from {label}")
            };
            window.set_status(message.into());
        });
    }

    // ---- settings ----------------------------------------------------------
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_setting_toggled(move |id, value| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            let mut theme_changed = false;
            let mut crossfade = false;
            let mut discord_changed = false;
            {
                let mut s = state.borrow_mut();
                match id.as_str() {
                    "pure_black" => {
                        s.settings.pure_black = value;
                        theme_changed = true;
                    }
                    "crossfade" => {
                        s.settings.crossfade_enabled = value;
                        crossfade = true;
                    }
                    "gapless" => s.settings.gapless = value,
                    "data_saver" => s.settings.data_saver = value,
                    "hide_videos" => s.settings.hide_video_songs = value,
                    "word_by_word" => s.settings.word_by_word_lyrics = value,
                    "translate_lyrics" => s.settings.translate_lyrics = value,
                    "canvas" => s.settings.canvas_enabled = value,
                    "echo_brain" => s.settings.echo_brain_enabled = value,
                    "pause_on_mute" => s.settings.pause_on_mute = value,
                    "resume_on_bluetooth" => s.settings.resume_on_bluetooth = value,
                    "discord_rpc" => {
                        s.settings.discord_rpc = value;
                        discord_changed = true;
                    }
                    _ => {}
                }
                s.save_settings();
            }
            if theme_changed {
                state.borrow().apply_theme(&window, true);
            }
            if crossfade {
                let (enabled, seconds) = {
                    let s = state.borrow();
                    (s.settings.crossfade_enabled, s.settings.crossfade_duration)
                };
                if let Some(engine) = &state.borrow().engine {
                    engine.send(Command::SetCrossfade { enabled, seconds });
                }
            }
            if discord_changed {
                state.borrow_mut().sync_discord();
            }
            refresh_settings(&window, &state.borrow());
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        let workers = workers.clone();
        window.on_setting_activated(move |id| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            let id = id.to_string();
            match id.as_str() {
                "clear_cache" => {
                    state.borrow_mut().covers.clear();
                    state.borrow_mut().pending_covers.clear();
                    workers.clear_cache();
                    return;
                }
                "about" => {
                    window.set_status(
                        format!("Echo Music Desktop {}", env!("CARGO_PKG_VERSION")).into(),
                    );
                    return;
                }
                "proxy" => {
                    let mut s = state.borrow_mut();
                    s.settings.proxy = None;
                    s.save_settings();
                    window.set_status("Proxy cleared".into());
                    refresh_settings(&window, &s);
                    return;
                }
                _ => {}
            }

            let mut theme_changed = false;
            let mut crossfade_changed = false;
            let mut lyrics_order_changed = false;
            {
                let mut s = state.borrow_mut();
                match id.as_str() {
                    "theme_mode" => {
                        s.settings.theme_mode = match s.settings.theme_mode {
                            ThemeMode::System => ThemeMode::Light,
                            ThemeMode::Light => ThemeMode::Dark,
                            ThemeMode::Dark => ThemeMode::System,
                        };
                        theme_changed = true;
                    }
                    "theme_color" => {
                        let current = s.settings.theme_color.clone();
                        let index = ACCENTS
                            .iter()
                            .position(|accent| accent.eq_ignore_ascii_case(&current))
                            .unwrap_or(0);
                        s.settings.theme_color = ACCENTS[(index + 1) % ACCENTS.len()].to_string();
                        theme_changed = true;
                    }
                    "ui_scale" => {
                        s.settings.ui_scale = if s.settings.ui_scale >= 1.3 {
                            0.85
                        } else {
                            s.settings.ui_scale + 0.15
                        };
                        s.settings.sanitize();
                        theme_changed = true;
                    }
                    "crossfade_duration" => {
                        s.settings.crossfade_duration = (s.settings.crossfade_duration + 2) % 14;
                        crossfade_changed = true;
                    }
                    "audio_quality" => {
                        s.settings.audio_quality = match s.settings.audio_quality {
                            AudioQuality::Low => AudioQuality::Medium,
                            AudioQuality::Medium => AudioQuality::High,
                            AudioQuality::High => AudioQuality::Highest,
                            AudioQuality::Highest => AudioQuality::Low,
                        };
                    }
                    "lyrics_providers" => {
                        if !s.settings.lyrics_provider_order.is_empty() {
                            let first = s.settings.lyrics_provider_order.remove(0);
                            s.settings.lyrics_provider_order.push(first);
                        }
                        lyrics_order_changed = true;
                    }
                    _ => {}
                }
                s.save_settings();
            }

            if theme_changed {
                state.borrow().apply_theme(&window, true);
            }
            if crossfade_changed {
                let (enabled, seconds) = {
                    let s = state.borrow();
                    (s.settings.crossfade_enabled, s.settings.crossfade_duration)
                };
                if let Some(engine) = &state.borrow().engine {
                    engine.send(Command::SetCrossfade { enabled, seconds });
                }
            }
            if lyrics_order_changed {
                let order = state.borrow().settings.lyrics_provider_order.clone();
                state.borrow().lyrics.set_order(&order);
            }
            refresh_settings(&window, &state.borrow());
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_eq_toggled(move |value| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            {
                let mut s = state.borrow_mut();
                s.settings.equalizer_enabled = value;
                s.save_settings();
                s.push_equalizer();
            }
            window.set_eq_enabled(value);
            window.set_eq_bands(model(state.borrow().eq_bands()));
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_eq_band_changed(move |index, value| {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            {
                let mut s = state.borrow_mut();
                let index = index.max(0) as usize;
                if index < s.settings.equalizer_bands.len() {
                    s.settings.equalizer_bands[index] = value;
                }
                s.push_equalizer();
            }
            window.set_eq_bands(model(state.borrow().eq_bands()));
        });
    }
    {
        let window_weak = window.as_weak();
        let state = state.clone();
        window.on_reset_eq(move || {
            let Some(window) = window_weak.upgrade() else {
                return;
            };
            {
                let mut s = state.borrow_mut();
                s.settings.equalizer_bands = vec![0.0; 10];
                s.settings.equalizer_preset = "flat".into();
                s.save_settings();
                s.push_equalizer();
            }
            window.set_eq_bands(model(state.borrow().eq_bands()));
        });
    }
}

// ---------------------------------------------------------------------------
// The update loop
// ---------------------------------------------------------------------------

fn pump(window: &AppWindow, state: &Shared, workers: &Workers, rx: &Receiver<Update>) {
    let events = state
        .borrow()
        .engine
        .as_ref()
        .map(|engine| engine.drain_events())
        .unwrap_or_default();
    for event in events {
        handle_event(window, state, workers, event);
    }

    while let Ok(update) = rx.try_recv() {
        handle_update(window, state, workers, update);
    }

    update_transport(window, state);

    if state.borrow().covers_dirty {
        state.borrow_mut().covers_dirty = false;
        refresh_all_models(window, &state.borrow());
    }
}

fn handle_event(window: &AppWindow, state: &Shared, workers: &Workers, event: Event) {
    match event {
        Event::State(playback) => {
            let playing = playback == PlaybackState::Playing;
            state.borrow_mut().is_playing = playing;
            window.set_is_playing(playing);
        }
        Event::TrackStarted(song) => {
            let song = *song;
            let expected = state.borrow().pending_resolve.as_deref() == Some(song.id.as_str());
            if expected {
                // This is the echo of a stream we already resolved.
                state.borrow_mut().pending_resolve = None;
                return;
            }
            state.borrow_mut().pending_resolve = Some(song.id.clone());
            {
                let mut s = state.borrow_mut();
                s.current = Some(song.clone());
                s.lyrics_doc = None;
                s.last_active_line = None;
                s.loading = true;
                s.status = "Loading\u{2026}".into();
            }
            window.set_now_playing(state.borrow().song_data(&song));
            window.set_has_track(true);
            window.set_loading(true);
            window.set_status("Loading\u{2026}".into());
            window.set_lyrics(model(Vec::<LyricLineData>::new()));
            window.set_lyrics_source("".into());
            let quality = state.borrow().quality();
            workers.resolve(song, quality);
        }
        Event::Queue { songs, .. } => {
            state.borrow_mut().queue = songs;
        }
        Event::Ended => {
            state.borrow_mut().is_playing = false;
            window.set_is_playing(false);
            window.set_status("Queue finished".into());
        }
        Event::Error(message) => {
            state.borrow_mut().loading = false;
            state.borrow_mut().pending_resolve = None;
            window.set_loading(false);
            window.set_status(message.into());
        }
        Event::Position { .. } => {
            // The shared snapshot already carries the position; see
            // `update_transport`.
        }
    }
}

fn handle_update(window: &AppWindow, state: &Shared, workers: &Workers, update: Update) {
    match update {
        Update::Home(result) => match result {
            Ok(home) => {
                {
                    let mut s = state.borrow_mut();
                    s.home = Some(home);
                    s.loading = false;
                    s.covers_dirty = true;
                }
                window.set_loading(false);
                window.set_chips(string_model(&state.borrow().chips()));
                window.set_shelves(model(state.borrow().shelves()));
                request_covers(state, workers);
            }
            Err(err) => {
                window.set_loading(false);
                window.set_status(format!("Could not load the feed: {err}").into());
            }
        },
        Update::Search(result) => {
            // The worker does not echo the query back, so there is no stale
            // result guard here; results simply replace the previous ones.
            window.set_searching(false);
            match result {
                Ok(results) => {
                    {
                        let mut s = state.borrow_mut();
                        s.search = Some(results);
                        s.covers_dirty = true;
                    }
                    refresh_search(window, &state.borrow());
                    request_covers(state, workers);
                }
                Err(err) => window.set_status(format!("Search failed: {err}").into()),
            }
        }
        Update::Suggestions(suggestions) => {
            window.set_suggestions(string_model(&suggestions));
        }
        // Album / artist / playlist pages all carry a flat song list.
        Update::Album(result) => match result {
            Ok(page) => play_songs(window, state, workers, page.songs, 0),
            Err(err) => window.set_status(format!("Could not open the album: {err}").into()),
        },
        Update::Artist(result) => match result {
            Ok(page) => {
                // An artist page is a set of shelves; play the first one.
                let songs: Vec<Song> = page
                    .sections
                    .into_iter()
                    .flat_map(|shelf| shelf.items)
                    .filter_map(|item| match item {
                        MediaItem::Song(song) => Some(song),
                        _ => None,
                    })
                    .collect();
                if songs.is_empty() {
                    window.set_status("This artist has no playable tracks".into());
                } else {
                    play_songs(window, state, workers, songs, 0);
                }
            }
            Err(err) => window.set_status(format!("Could not open the artist: {err}").into()),
        },
        Update::Playlist(result) => match result {
            Ok(page) => play_songs(window, state, workers, page.songs, 0),
            Err(err) => window.set_status(format!("Could not open the playlist: {err}").into()),
        },
        Update::StreamReady { song, url } => {
            if state.borrow().pending_resolve.as_deref() != Some(song.id.as_str()) {
                log::debug!("discarding a stale stream for {}", song.id);
                return;
            }
            {
                let mut s = state.borrow_mut();
                s.current = Some(song.clone());
                s.loading = false;
                s.status.clear();
                s.is_playing = true;
            }

            let engine_present = state.borrow().engine.is_some();
            if let Some(engine) = &state.borrow().engine {
                engine.send(Command::Play {
                    song: Box::new(song.clone()),
                    url,
                });
            }
            if !engine_present {
                // Without an audio device the engine never echoes
                // `TrackStarted`, so clear the guard ourselves.
                state.borrow_mut().pending_resolve = None;
            }

            if let Err(err) = state.borrow().library.record_play(&song) {
                log::debug!("could not record the play: {err:#}");
            }

            window.set_now_playing(state.borrow().song_data(&song));
            window.set_has_track(true);
            window.set_loading(false);
            window.set_status("".into());
            window.set_is_playing(true);

            state.borrow_mut().covers_dirty = true;
            // Clear the previous track's lyrics before the new ones arrive, so
            // the panel does not show stale text.
            {
                let mut s = state.borrow_mut();
                s.lyrics_doc = None;
                s.last_active_line = None;
            }
            window.set_lyrics(model(Vec::<LyricLineData>::new()));
            window.set_lyrics_source("".into());
            workers.lyrics(AppState::lyrics_query_for(&song));
        }
        Update::StreamFailed(message) => {
            state.borrow_mut().loading = false;
            state.borrow_mut().pending_resolve = None;
            window.set_loading(false);
            window.set_status(message.into());
        }
        Update::Lyrics(result) => {
            // The worker does not echo the video id, so the guard is implicit:
            // whatever arrives is applied to the current track.
            match result {
                Ok(doc) => {
                    {
                        let mut s = state.borrow_mut();
                        s.lyrics_doc = doc;
                        s.last_active_line = None;
                    }
                    window.set_lyrics(model(state.borrow().lyric_lines()));
                    window.set_lyrics_source(state.borrow().lyrics_source_label().into());
                }
                Err(err) => log::debug!("lyrics lookup failed: {err}"),
            }
        }
        Update::Covers(urls) => {
            // Workers download to the cache; decoding must happen on this
            // (UI) thread, so the images are built here.
            let dir = state.borrow().paths.artwork_cache_dir();
            let mut s = state.borrow_mut();
            for url in urls {
                let path = crate::covers::cache_path(&dir, &url);
                if let Ok(image) = slint::Image::load_from_path(&path) {
                    s.pending_covers.remove(&url);
                    s.covers.insert(url, image);
                }
            }
            s.covers_dirty = true;
        }
        Update::Status(message) => window.set_status(message.into()),
    }
}

/// Pushes the engine's snapshot onto the window and advances the lyric line.
fn update_transport(window: &AppWindow, state: &Shared) {
    let (snapshot, active_line) = {
        let s = state.borrow();
        (
            s.engine.as_ref().map(|engine| engine.snapshot()),
            s.active_line(),
        )
    };
    let Some(snapshot) = snapshot else {
        return;
    };

    let progress = if snapshot.duration_ms > 0 {
        (snapshot.position_ms as f32 / snapshot.duration_ms as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let playing = snapshot.state == PlaybackState::Playing;

    window.set_progress(progress);
    window.set_position_label(echo_core::util::format_position_ms(snapshot.position_ms).into());
    window.set_duration_label(echo_core::util::format_position_ms(snapshot.duration_ms).into());
    window.set_is_playing(playing);

    {
        let mut s = state.borrow_mut();
        s.progress = progress;
        s.is_playing = playing;
    }

    if state.borrow().last_active_line != active_line {
        state.borrow_mut().last_active_line = active_line;
        let lines = state.borrow().lyric_lines();
        window.set_lyrics(model(lines));
    }

    // Discord only needs to hear about a track change or a pause/resume; the
    // protocol extrapolates the progress bar itself, so this is filtered
    // internally and is cheap to call every tick.
    state.borrow_mut().update_discord(false);
}
