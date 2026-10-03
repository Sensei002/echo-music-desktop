//! Platform-correct application directories.
//!
//! Everything Echo Music Desktop writes to disk lives under a single root so it
//! can be removed cleanly and so the CI packaging step knows what to ship.

use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

/// Resolved on-disk locations used by the application.
#[derive(Debug, Clone)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    /// Resolves the standard locations for the current platform.
    ///
    /// * Windows — `%APPDATA%\echomusic\Echo Music Desktop`
    /// * macOS — `~/Library/Application Support/fun.echomusic.Echo Music Desktop`
    /// * Linux — `~/.local/share/echomusic/Echo Music Desktop`
    pub fn new() -> Result<Self> {
        let dirs = ProjectDirs::from("fun", "echomusic", "Echo Music Desktop")
            .context("unable to resolve the platform data directory")?;
        Ok(Self {
            root: dirs.data_dir().to_path_buf(),
        })
    }

    /// Builds a path set rooted at an explicit directory (used by tests).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory holding downloaded/exported audio files.
    pub fn downloads_dir(&self) -> PathBuf {
        self.root.join("downloads")
    }

    /// Directory holding cached album artwork.
    pub fn artwork_cache_dir(&self) -> PathBuf {
        self.root.join("artwork")
    }

    /// Directory holding cached lyrics documents.
    pub fn lyrics_cache_dir(&self) -> PathBuf {
        self.root.join("lyrics")
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    pub fn library_db(&self) -> PathBuf {
        self.root.join("library.db")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Creates every directory the app may write into.
    pub fn ensure(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.downloads_dir(),
            self.artwork_cache_dir(),
            self.lyrics_cache_dir(),
            self.log_dir(),
        ] {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        Ok(())
    }
}

impl Default for AppPaths {
    fn default() -> Self {
        Self::new().unwrap_or_else(|_| Self::with_root(std::env::temp_dir().join("echo-music")))
    }
}
