//! Persisted user settings.
//!
//! Mirrors the upstream preference surface (DataStore keys) but stores a single
//! JSON document, which keeps the desktop port trivially inspectable and
//! forward-compatible: unknown keys are ignored, missing keys fall back to
//! defaults.

use crate::theme::{Palette, Rgb, ECHO_SEED};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The Discord application id used for Rich Presence when the user has not
/// supplied their own.
///
/// Kept here rather than in `echo-discord` so settings can be loaded and saved
/// without pulling in the presence client. Rich Presence artwork assets are
/// registered against this application, so changing it changes which images
/// Discord is able to render.
pub const DEFAULT_DISCORD_CLIENT_ID: &str = "1179128917280403576";

/// The bundled Discord application id.
fn echo_discord_id() -> String {
    DEFAULT_DISCORD_CLIENT_ID.to_string()
}

/// Which colour scheme to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    /// Follow the operating system.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
            ThemeMode::System => system_dark,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ThemeMode::System => "system",
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        }
    }
}

/// Audio quality preference for stream selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AudioQuality {
    Low,
    Medium,
    #[default]
    High,
    Highest,
}

impl AudioQuality {
    /// Preferred target bitrate in bits per second.
    pub fn target_bitrate(self) -> u32 {
        match self {
            AudioQuality::Low => 64_000,
            AudioQuality::Medium => 128_000,
            AudioQuality::High => 256_000,
            AudioQuality::Highest => 320_000,
        }
    }
}

/// The complete settings document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    // ---- appearance --------------------------------------------------------
    pub theme_mode: ThemeMode,
    /// Accent colour as `#RRGGBB`. Defaults to the Echo seed.
    pub theme_color: String,
    /// Use the OS accent colour instead of `theme_color`.
    pub dynamic_color: bool,
    /// Force pure-black surfaces in dark mode (OLED).
    pub pure_black: bool,
    /// Font family name; `"system"` uses the platform default.
    pub font: String,
    /// Global UI scale multiplier (0.8 – 1.4).
    pub ui_scale: f32,

    // ---- playback ----------------------------------------------------------
    pub crossfade_enabled: bool,
    /// Crossfade length in seconds.
    pub crossfade_duration: u32,
    pub gapless: bool,
    pub audio_quality: AudioQuality,
    /// Reduce bandwidth by preferring lower bitrates on metered links.
    pub data_saver: bool,
    pub normalize_volume: bool,
    pub volume: f32,
    /// Restore playback position when reopening the app.
    pub resume_on_launch: bool,

    // ---- library / browsing ------------------------------------------------
    pub hide_video_songs: bool,
    pub hide_shorts: bool,
    pub library_layout: LibraryLayout,
    /// Directories scanned for local media.
    pub local_folders: Vec<String>,

    // ---- lyrics ------------------------------------------------------------
    /// Provider ids in priority order, e.g. `["lrclib", "betterlyrics", ...]`.
    pub lyrics_provider_order: Vec<String>,
    pub word_by_word_lyrics: bool,
    pub translate_lyrics: bool,
    /// Target language code for lyric translation (e.g. `"en"`).
    pub translation_language: String,

    // ---- extras ------------------------------------------------------------
    pub canvas_enabled: bool,
    pub echo_brain_enabled: bool,
    pub pause_on_mute: bool,
    pub resume_on_bluetooth: bool,
    pub discord_rpc: bool,
    /// Discord application id used for Rich Presence.
    ///
    /// Users who register their own Discord application (and upload the
    /// artwork assets to it) can override this; otherwise the bundled id is
    /// used. Stored as a string because it is a snowflake, not a number.
    pub discord_client_id: String,
    /// Show the elapsed/remaining progress bar in the Discord presence.
    pub discord_show_timestamps: bool,
    pub listen_together_name: String,

    // ---- equalizer ---------------------------------------------------------
    pub equalizer_enabled: bool,
    pub equalizer_preset: String,
    /// Gain in dB for each of the 10 ISO bands.
    pub equalizer_bands: Vec<f32>,
    pub stereo_widening: f32,

    // ---- network -----------------------------------------------------------
    /// Optional `http://host:port` proxy.
    pub proxy: Option<String>,
    /// Region (`gl`) and language (`hl`) sent to InnerTube.
    pub locale_gl: String,
    pub locale_hl: String,
    /// Persisted `visitorData` for InnerTube.
    pub visitor_data: Option<String>,

    // ---- misc --------------------------------------------------------------
    pub sleep_timer_minutes: u32,
    pub confirm_before_exit: bool,
    pub check_for_updates: bool,
}

/// How the library grid is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LibraryLayout {
    #[default]
    Grid,
    List,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme_mode: ThemeMode::System,
            theme_color: ECHO_SEED.to_hex(),
            dynamic_color: false,
            pure_black: false,
            font: "system".into(),
            ui_scale: 1.0,

            crossfade_enabled: false,
            crossfade_duration: 6,
            gapless: true,
            audio_quality: AudioQuality::High,
            data_saver: false,
            normalize_volume: false,
            volume: 1.0,
            resume_on_launch: true,

            hide_video_songs: false,
            hide_shorts: true,
            library_layout: LibraryLayout::Grid,
            local_folders: Vec::new(),

            lyrics_provider_order: vec![
                "lrclib".into(),
                "betterlyrics".into(),
                "simpmusic".into(),
                "youlyplus".into(),
                "kugou".into(),
                "paxsenix".into(),
            ],
            word_by_word_lyrics: true,
            translate_lyrics: false,
            translation_language: "en".into(),

            canvas_enabled: true,
            echo_brain_enabled: true,
            pause_on_mute: false,
            resume_on_bluetooth: true,
            discord_rpc: false,
            discord_client_id: String::new(),
            discord_show_timestamps: true,
            listen_together_name: "Echo Listener".into(),

            equalizer_enabled: false,
            equalizer_preset: "flat".into(),
            equalizer_bands: vec![0.0; 10],
            stereo_widening: 0.0,

            proxy: None,
            locale_gl: "US".into(),
            locale_hl: "en".into(),
            visitor_data: None,

            sleep_timer_minutes: 0,
            confirm_before_exit: false,
            check_for_updates: true,
        }
    }
}

impl Settings {
    /// Loads settings from disk, falling back to defaults when the file is
    /// missing or corrupt (matching upstream's "safe default instead of crash").
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|err| {
                log::warn!("settings file was invalid ({err}); using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    /// Persists settings as pretty JSON, creating parent directories.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).context("failed to encode settings")?;
        std::fs::write(path, json).with_context(|| format!("failed to write {}", path.display()))
    }

    /// The configured accent, or the Echo seed when unset/invalid.
    pub fn accent(&self) -> Rgb {
        Rgb::parse(&self.theme_color).unwrap_or(ECHO_SEED)
    }

    /// The resolved palette for the given system appearance.
    pub fn palette(&self, system_dark: bool) -> Palette {
        let palette = Palette::from_seed(self.accent(), self.theme_mode.is_dark(system_dark));
        if self.pure_black {
            palette.pure_black()
        } else {
            palette
        }
    }

    /// Clamps values that come from untrusted/legacy files.
    pub fn sanitize(&mut self) {
        self.ui_scale = self.ui_scale.clamp(0.8, 1.4);
        self.volume = self.volume.clamp(0.0, 1.0);
        self.crossfade_duration = self.crossfade_duration.min(12);
        self.stereo_widening = self.stereo_widening.clamp(0.0, 1.0);
        if self.equalizer_bands.len() != 10 {
            self.equalizer_bands = vec![0.0; 10];
        }
        if self.theme_color.is_empty() {
            self.theme_color = ECHO_SEED.to_hex();
        }
        self.discord_client_id = self.discord_client_id.trim().to_string();
    }

    /// The Discord application id to use, falling back to the bundled one.
    ///
    /// An empty or whitespace-only value means "use the default", which keeps
    /// the setting genuinely optional rather than forcing every user to paste
    /// an id that most of them do not have.
    pub fn discord_application_id(&self) -> String {
        if self.discord_client_id.trim().is_empty() {
            echo_discord_id()
        } else {
            self.discord_client_id.trim().to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let s = Settings::default();
        assert_eq!(s.accent(), ECHO_SEED);
        assert_eq!(s.equalizer_bands.len(), 10);
        assert!(s.locale_hl == "en");
    }

    #[test]
    fn round_trips_through_json() {
        let s = Settings {
            theme_mode: ThemeMode::Dark,
            volume: 0.42,
            ..Settings::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.theme_mode, ThemeMode::Dark);
        assert!((back.volume - 0.42).abs() < f32::EPSILON);
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let json = r#"{"themeMode":"dark","someFutureKey":true}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.theme_mode, ThemeMode::Dark);
    }

    #[test]
    fn sanitize_clamps() {
        let mut s = Settings {
            volume: 9.0,
            ui_scale: 12.0,
            equalizer_bands: vec![1.0],
            ..Settings::default()
        };
        s.sanitize();
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.ui_scale, 1.4);
        assert_eq!(s.equalizer_bands.len(), 10);
    }
}
