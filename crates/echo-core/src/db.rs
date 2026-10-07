//! Local library persistence backed by SQLite.
//!
//! Mirrors the upstream Room schema: songs, liked tracks, user playlists,
//! play history, downloads and search history. Every write goes through
//! `upsert_song` first so foreign keys always resolve.

use crate::models::{Playlist, Song};
use crate::util::{now_ms, now_secs};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::path::Path;
use std::sync::Arc;

/// Column list shared by every `SELECT` that materialises a [`Song`].
const SONG_COLUMNS: &str = "id, title, artists, artist_ids, album, album_id, duration, \
     thumbnail, set_video_id, is_explicit, is_video, year, radio_playlist_id";

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS songs (
    id                TEXT PRIMARY KEY,
    title             TEXT NOT NULL,
    artists           TEXT NOT NULL DEFAULT '[]',
    artist_ids        TEXT NOT NULL DEFAULT '[]',
    album             TEXT,
    album_id          TEXT,
    duration          INTEGER,
    thumbnail         TEXT,
    set_video_id      TEXT,
    is_explicit       INTEGER NOT NULL DEFAULT 0,
    is_video          INTEGER NOT NULL DEFAULT 0,
    year              INTEGER,
    radio_playlist_id TEXT
);

CREATE TABLE IF NOT EXISTS liked (
    song_id  TEXT PRIMARY KEY REFERENCES songs(id) ON DELETE CASCADE,
    liked_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS playlists (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    thumbnail   TEXT,
    is_local    INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS playlist_songs (
    playlist_id TEXT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    song_id     TEXT NOT NULL REFERENCES songs(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    added_at    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, song_id)
);

CREATE TABLE IF NOT EXISTS history (
    song_id    TEXT PRIMARY KEY REFERENCES songs(id) ON DELETE CASCADE,
    played_at  INTEGER NOT NULL,
    play_count INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS downloads (
    song_id       TEXT PRIMARY KEY REFERENCES songs(id) ON DELETE CASCADE,
    path          TEXT NOT NULL,
    quality       TEXT,
    size_bytes    INTEGER,
    downloaded_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS search_history (
    query       TEXT PRIMARY KEY,
    searched_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_history_played  ON history(played_at DESC);
CREATE INDEX IF NOT EXISTS idx_playlist_order  ON playlist_songs(playlist_id, position);
CREATE INDEX IF NOT EXISTS idx_search_recent   ON search_history(searched_at DESC);
"#;

/// A downloaded track plus its on-disk metadata.
#[derive(Debug, Clone)]
pub struct DownloadRecord {
    pub song: Song,
    pub path: String,
    pub quality: Option<String>,
    pub size_bytes: Option<u64>,
    pub downloaded_at: i64,
}

/// Aggregate counters for the library screen.
#[derive(Debug, Clone, Copy, Default)]
pub struct LibraryStats {
    pub songs: u64,
    pub liked: u64,
    pub playlists: u64,
    pub downloads: u64,
    pub plays: u64,
}

/// Thread-safe handle to the local library database.
#[derive(Clone)]
pub struct Library {
    conn: Arc<Mutex<Connection>>,
}

impl Library {
    /// Opens (or creates) the library at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open library at {}", path.display()))?;
        Self::from_connection(conn)
    }

    /// Opens a throwaway in-memory library (used by tests).
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA)
            .context("failed to apply library schema")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    // ---------------------------------------------------------------- songs

    /// Inserts or refreshes a song row. Returns the song id.
    pub fn upsert_song(&self, song: &Song) -> Result<()> {
        let artists = serde_json::to_string(&song.artists)?;
        let artist_ids = serde_json::to_string(&song.artist_ids)?;
        self.conn.lock().execute(
            "INSERT INTO songs
                (id, title, artists, artist_ids, album, album_id, duration,
                 thumbnail, set_video_id, is_explicit, is_video, year, radio_playlist_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                title = excluded.title,
                artists = excluded.artists,
                artist_ids = excluded.artist_ids,
                album = COALESCE(excluded.album, songs.album),
                album_id = COALESCE(excluded.album_id, songs.album_id),
                duration = COALESCE(excluded.duration, songs.duration),
                thumbnail = COALESCE(excluded.thumbnail, songs.thumbnail),
                set_video_id = COALESCE(excluded.set_video_id, songs.set_video_id),
                is_explicit = excluded.is_explicit,
                is_video = excluded.is_video,
                year = COALESCE(excluded.year, songs.year),
                radio_playlist_id = COALESCE(excluded.radio_playlist_id, songs.radio_playlist_id)",
            params![
                &song.id,
                &song.title,
                artists,
                artist_ids,
                &song.album,
                &song.album_id,
                &song.duration,
                &song.thumbnail,
                &song.set_video_id,
                song.is_explicit as i32,
                song.is_video as i32,
                &song.year,
                &song.radio_playlist_id,
            ],
        )?;
        Ok(())
    }

    /// Fetches a single song by id.
    pub fn song(&self, id: &str) -> Result<Option<Song>> {
        let sql = format!("SELECT {SONG_COLUMNS} FROM songs WHERE id = ?1");
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let song = stmt.query_row(params![id], song_from_row).optional()?;
        Ok(song)
    }

    // ---------------------------------------------------------------- liked

    pub fn like_song(&self, song: &Song) -> Result<()> {
        self.upsert_song(song)?;
        self.conn.lock().execute(
            "INSERT INTO liked (song_id, liked_at) VALUES (?1, ?2)
             ON CONFLICT(song_id) DO NOTHING",
            params![&song.id, now_ms()],
        )?;
        Ok(())
    }

    pub fn unlike_song(&self, id: &str) -> Result<()> {
        self.conn
            .lock()
            .execute("DELETE FROM liked WHERE song_id = ?1", params![id])?;
        Ok(())
    }

    pub fn is_liked(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM liked WHERE song_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn liked_songs(&self) -> Result<Vec<Song>> {
        let sql = format!(
            "SELECT {SONG_COLUMNS} FROM songs
             JOIN liked ON liked.song_id = songs.id
             ORDER BY liked.liked_at DESC"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], song_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn liked_count(&self) -> Result<u64> {
        let conn = self.conn.lock();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM liked", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    // ------------------------------------------------------------ playlists

    /// Creates a local playlist and returns its generated id.
    pub fn create_playlist(&self, name: &str, description: Option<&str>) -> Result<String> {
        let id = format!("local:{}", uuid::Uuid::new_v4());
        self.create_playlist_with_id(&id, name, description, None)?;
        Ok(id)
    }

    /// Creates a playlist with an explicit id (used when importing).
    pub fn create_playlist_with_id(
        &self,
        id: &str,
        name: &str,
        description: Option<&str>,
        thumbnail: Option<&str>,
    ) -> Result<()> {
        let now = now_ms();
        self.conn.lock().execute(
            "INSERT INTO playlists (id, name, description, thumbnail, is_local, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, updated_at = excluded.updated_at",
            params![id, name, description, thumbnail, now],
        )?;
        Ok(())
    }

    pub fn rename_playlist(&self, id: &str, name: &str) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE playlists SET name = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, name, now_ms()],
        )?;
        Ok(())
    }

    pub fn delete_playlist(&self, id: &str) -> Result<()> {
        self.conn
            .lock()
            .execute("DELETE FROM playlists WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT p.id, p.name, p.description, p.thumbnail, p.is_local,
                    (SELECT COUNT(*) FROM playlist_songs ps WHERE ps.playlist_id = p.id)
             FROM playlists p
             ORDER BY p.updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let thumbnail: Option<String> = row.get(3)?;
            Ok(Playlist {
                id: row.get(0)?,
                title: row.get(1)?,
                description: row.get(2)?,
                thumbnail,
                author: Some("You".into()),
                song_count: Some(row.get::<_, i64>(5)?.max(0) as u32),
                is_local: row.get::<_, i32>(4)? != 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn playlist(&self, id: &str) -> Result<Option<Playlist>> {
        Ok(self.playlists()?.into_iter().find(|p| p.id == id))
    }

    /// Appends a song to a playlist (no-op when already present).
    pub fn add_to_playlist(&self, playlist_id: &str, song: &Song) -> Result<()> {
        self.upsert_song(song)?;
        let conn = self.conn.lock();
        let next: i64 = conn.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM playlist_songs WHERE playlist_id = ?1",
            params![playlist_id],
            |row| row.get(0),
        )?;
        conn.execute(
            "INSERT INTO playlist_songs (playlist_id, song_id, position, added_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(playlist_id, song_id) DO NOTHING",
            params![playlist_id, &song.id, next, now_ms()],
        )?;
        conn.execute(
            "UPDATE playlists SET updated_at = ?2 WHERE id = ?1",
            params![playlist_id, now_ms()],
        )?;
        Ok(())
    }

    pub fn remove_from_playlist(&self, playlist_id: &str, song_id: &str) -> Result<()> {
        self.conn.lock().execute(
            "DELETE FROM playlist_songs WHERE playlist_id = ?1 AND song_id = ?2",
            params![playlist_id, song_id],
        )?;
        Ok(())
    }

    pub fn playlist_songs(&self, playlist_id: &str) -> Result<Vec<Song>> {
        let sql = format!(
            "SELECT {SONG_COLUMNS} FROM songs
             JOIN playlist_songs ps ON ps.song_id = songs.id
             WHERE ps.playlist_id = ?1
             ORDER BY ps.position ASC"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![playlist_id], song_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // -------------------------------------------------------------- history

    /// Records a play, incrementing the counter for repeat listens.
    pub fn record_play(&self, song: &Song) -> Result<()> {
        self.upsert_song(song)?;
        self.conn.lock().execute(
            "INSERT INTO history (song_id, played_at, play_count) VALUES (?1, ?2, 1)
             ON CONFLICT(song_id) DO UPDATE SET played_at = ?2, play_count = play_count + 1",
            params![&song.id, now_secs()],
        )?;
        Ok(())
    }

    pub fn history(&self, limit: usize) -> Result<Vec<Song>> {
        let sql = format!(
            "SELECT {SONG_COLUMNS} FROM songs
             JOIN history ON history.song_id = songs.id
             ORDER BY history.played_at DESC LIMIT ?1"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit as i64], song_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Most played tracks ("My top 50" in the library screen).
    pub fn top_songs(&self, limit: usize) -> Result<Vec<Song>> {
        let sql = format!(
            "SELECT {SONG_COLUMNS} FROM songs
             JOIN history ON history.song_id = songs.id
             ORDER BY history.play_count DESC, history.played_at DESC LIMIT ?1"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit as i64], song_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Least played tracks ("My bottom 50").
    pub fn bottom_songs(&self, limit: usize) -> Result<Vec<Song>> {
        let sql = format!(
            "SELECT {SONG_COLUMNS} FROM songs
             JOIN history ON history.song_id = songs.id
             ORDER BY history.play_count ASC, history.played_at DESC LIMIT ?1"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit as i64], song_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn clear_history(&self) -> Result<()> {
        self.conn.lock().execute("DELETE FROM history", [])?;
        Ok(())
    }

    // ------------------------------------------------------------ downloads

    pub fn add_download(
        &self,
        song: &Song,
        path: &str,
        quality: Option<&str>,
        size_bytes: Option<u64>,
    ) -> Result<()> {
        self.upsert_song(song)?;
        self.conn.lock().execute(
            "INSERT INTO downloads (song_id, path, quality, size_bytes, downloaded_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(song_id) DO UPDATE SET
                path = excluded.path, quality = excluded.quality,
                size_bytes = excluded.size_bytes, downloaded_at = excluded.downloaded_at",
            params![
                &song.id,
                path,
                quality,
                size_bytes.map(|v| v as i64),
                now_ms()
            ],
        )?;
        Ok(())
    }

    pub fn remove_download(&self, song_id: &str) -> Result<()> {
        self.conn
            .lock()
            .execute("DELETE FROM downloads WHERE song_id = ?1", params![song_id])?;
        Ok(())
    }

    pub fn downloads(&self) -> Result<Vec<DownloadRecord>> {
        let sql = format!(
            "SELECT {SONG_COLUMNS}, d.path, d.quality, d.size_bytes, d.downloaded_at
             FROM songs
             JOIN downloads d ON d.song_id = songs.id
             ORDER BY d.downloaded_at DESC"
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            Ok(DownloadRecord {
                song: song_from_row(row)?,
                path: row.get("path")?,
                quality: row.get("quality")?,
                size_bytes: row.get::<_, Option<i64>>("size_bytes")?.map(|v| v as u64),
                downloaded_at: row.get("downloaded_at")?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn is_downloaded(&self, song_id: &str) -> Result<bool> {
        let conn = self.conn.lock();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM downloads WHERE song_id = ?1",
            params![song_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn download_path(&self, song_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT path FROM downloads WHERE song_id = ?1",
                params![song_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    // ------------------------------------------------------- search history

    pub fn record_search(&self, query: &str) -> Result<()> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(());
        }
        self.conn.lock().execute(
            "INSERT INTO search_history (query, searched_at) VALUES (?1, ?2)
             ON CONFLICT(query) DO UPDATE SET searched_at = ?2",
            params![query, now_secs()],
        )?;
        Ok(())
    }

    pub fn search_history(&self, limit: usize) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT query FROM search_history ORDER BY searched_at DESC LIMIT ?1")?;
        let rows = stmt.query_map(params![limit as i64], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn clear_search_history(&self) -> Result<()> {
        self.conn.lock().execute("DELETE FROM search_history", [])?;
        Ok(())
    }

    // ----------------------------------------------------------------- misc

    pub fn stats(&self) -> Result<LibraryStats> {
        let conn = self.conn.lock();
        let count = |table: &str| -> rusqlite::Result<u64> {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            conn.query_row(&sql, [], |row| row.get::<_, i64>(0))
                .map(|v| v as u64)
        };
        Ok(LibraryStats {
            songs: count("songs")?,
            liked: count("liked")?,
            playlists: count("playlists")?,
            downloads: count("downloads")?,
            plays: conn
                .query_row(
                    "SELECT COALESCE(SUM(play_count), 0) FROM history",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map(|v| v as u64)?,
        })
    }
}

/// Maps a `SELECT` row using [`SONG_COLUMNS`] into a [`Song`].
fn song_from_row(row: &Row<'_>) -> rusqlite::Result<Song> {
    let artists: String = row.get("artists")?;
    let artist_ids: String = row.get("artist_ids")?;
    Ok(Song {
        id: row.get("id")?,
        title: row.get("title")?,
        artists: serde_json::from_str(&artists).unwrap_or_default(),
        artist_ids: serde_json::from_str(&artist_ids).unwrap_or_default(),
        album: row.get("album")?,
        album_id: row.get("album_id")?,
        duration: row.get::<_, Option<i64>>("duration")?.map(|v| v as u32),
        thumbnail: row.get("thumbnail")?,
        set_video_id: row.get("set_video_id")?,
        is_explicit: row.get::<_, i32>("is_explicit")? != 0,
        is_video: row.get::<_, i32>("is_video")? != 0,
        play_count: None,
        year: row.get::<_, Option<i64>>("year")?.map(|v| v as i32),
        radio_playlist_id: row.get("radio_playlist_id")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str) -> Song {
        Song {
            id: id.into(),
            title: format!("Track {id}"),
            artists: vec!["Artist".into()],
            duration: Some(200),
            ..Default::default()
        }
    }

    #[test]
    fn likes_round_trip() {
        let lib = Library::open_in_memory().unwrap();
        let s = song("a");
        lib.like_song(&s).unwrap();
        assert!(lib.is_liked("a").unwrap());
        assert_eq!(lib.liked_songs().unwrap().len(), 1);
        assert_eq!(
            lib.liked_songs().unwrap()[0].artists,
            vec!["Artist".to_string()]
        );
        lib.unlike_song("a").unwrap();
        assert!(!lib.is_liked("a").unwrap());
    }

    #[test]
    fn playlist_lifecycle() {
        let lib = Library::open_in_memory().unwrap();
        let id = lib.create_playlist("Chill", None).unwrap();
        lib.add_to_playlist(&id, &song("a")).unwrap();
        lib.add_to_playlist(&id, &song("b")).unwrap();
        // duplicate insert is a no-op
        lib.add_to_playlist(&id, &song("b")).unwrap();
        assert_eq!(lib.playlist_songs(&id).unwrap().len(), 2);
        assert_eq!(lib.playlists().unwrap()[0].song_count, Some(2));
        lib.remove_from_playlist(&id, "a").unwrap();
        assert_eq!(lib.playlist_songs(&id).unwrap().len(), 1);
        lib.delete_playlist(&id).unwrap();
        assert!(lib.playlists().unwrap().is_empty());
    }

    #[test]
    fn history_counts_plays() {
        let lib = Library::open_in_memory().unwrap();
        lib.record_play(&song("a")).unwrap();
        lib.record_play(&song("a")).unwrap();
        lib.record_play(&song("b")).unwrap();
        assert_eq!(lib.history(10).unwrap().len(), 2);
        assert_eq!(lib.top_songs(10).unwrap()[0].id, "a");
        assert_eq!(lib.stats().unwrap().plays, 3);
    }

    #[test]
    fn downloads_and_search() {
        let lib = Library::open_in_memory().unwrap();
        lib.add_download(&song("a"), "/tmp/a.m4a", Some("high"), Some(1234))
            .unwrap();
        assert!(lib.is_downloaded("a").unwrap());
        assert_eq!(lib.downloads().unwrap()[0].size_bytes, Some(1234));
        lib.record_search("daft punk").unwrap();
        lib.record_search("daft punk").unwrap();
        assert_eq!(
            lib.search_history(5).unwrap(),
            vec!["daft punk".to_string()]
        );
    }
}
