//! Rich Presence state and the background worker that keeps Discord in sync.
//!
//! Discord reflects *activity*, not commands: the client is told what the user
//! is currently doing and the client decides how to render it. The important
//! consequence is that reconnecting is cheap and idempotent — when Discord is
//! restarted the worker simply replays the current activity.
//!
//! Two channels are used. `SET_ACTIVITY` replaces the full activity, while
//! `SET_ACTIVITY` with only timestamps is not a thing — so when a track is
//! paused the worker re-sends the activity with the elapsed time frozen, which
//! is what makes the counter stop advancing in Discord.

use crate::ipc::{IpcStream, OP_FRAME, OP_HANDSHAKE};
use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// How long the worker sleeps between connection attempts while Discord is down.
const RECONNECT_DELAY: Duration = Duration::from_secs(15);

/// What the user is currently listening to.
///
/// This is deliberately a plain data snapshot rather than a reference to the
/// player: the presence worker lives on its own thread and must not touch UI
/// state.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Original track length in seconds, used to size the progress bar.
    pub duration_secs: Option<u32>,
    /// Playback position in seconds at the moment this snapshot was taken.
    pub position_secs: u64,
    /// Whether the track is currently advancing.
    pub playing: bool,
    /// Artwork URL for the large image slot.
    pub artwork_url: Option<String>,
}

impl Activity {
    /// Clears every field so the worker can tell "nothing playing" apart from a
    /// real track.
    pub fn is_empty(&self) -> bool {
        self.title.is_empty()
    }

    /// The `details` line Discord shows in bold.
    fn details(&self) -> String {
        truncate(&self.title, 128)
    }

    /// The `state` line shown underneath.
    fn state(&self) -> String {
        let artist = if self.artist.trim().is_empty() {
            "Unknown artist".to_string()
        } else {
            self.artist.clone()
        };
        match self.album.as_deref().filter(|album| !album.is_empty()) {
            Some(album) => truncate(&format!("{artist} - {album}"), 128),
            None => truncate(&artist, 128),
        }
    }

    /// Discord rejects any `large_text`/`small_text` above 128 bytes.
    fn large_text(&self) -> Option<String> {
        self.album
            .as_deref()
            .filter(|album| !album.is_empty())
            .map(|album| truncate(album, 128))
    }
}

/// Truncates to `max` characters on a char boundary.
///
/// Discord's limits are expressed in bytes, but cutting mid-codepoint produces
/// invalid UTF-8 in the JSON frame, so this trims by characters and stays
/// comfortably underneath.
fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_string();
    }
    input
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>()
        + "…"
}

/// Commands sent from the UI thread to the presence worker.
#[derive(Debug)]
enum Command {
    /// Publish a new activity, or clear it when the payload is `None`.
    Set(Option<Activity>),
    Shutdown,
}

/// Handle to the Discord presence worker.
pub struct DiscordPresence {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
    /// Number of successful connections, exposed for diagnostics/tests.
    connections: Arc<AtomicU64>,
}

impl DiscordPresence {
    /// Spawns the worker. Never fails: when Discord is not running this simply
    /// keeps retrying in the background.
    pub fn start(client_id: &str) -> Self {
        let (tx, rx) = mpsc::channel();
        let connections = Arc::new(AtomicU64::new(0));
        let counter = connections.clone();
        let client_id = client_id.to_string();

        let thread = thread::Builder::new()
            .name("echo-discord".into())
            .spawn(move || {
                let mut worker = Worker::new(client_id, counter);
                worker.run(rx);
            })
            .ok();

        Self {
            commands: tx,
            thread,
            connections,
        }
    }

    /// Publishes the given track, replacing any previous activity.
    pub fn set_activity(&self, activity: Activity) {
        let _ = self.commands.send(Command::Set(Some(activity)));
    }

    /// Clears the presence ("not listening to anything").
    pub fn clear(&self) {
        let _ = self.commands.send(Command::Set(None));
    }

    /// How many times the worker has connected to Discord this session.
    pub fn connections(&self) -> u64 {
        self.connections.load(Ordering::Relaxed)
    }
}

impl Drop for DiscordPresence {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The worker thread's owned state.
struct Worker {
    client_id: String,
    connections: Arc<AtomicU64>,
    socket: Option<IpcStream>,
    /// The activity currently believed to be live in Discord.
    current: Option<Activity>,
    /// Active so Discord does not clear the presence while we are idle.
    started_at_ms: i64,
    /// Serialises access to the socket so `set_activity` is thread-safe even
    /// when called from several UI callbacks in quick succession.
    send_lock: Mutex<()>,
}

impl Worker {
    fn new(client_id: String, connections: Arc<AtomicU64>) -> Self {
        Self {
            client_id,
            connections,
            socket: None,
            current: None,
            started_at_ms: 0,
            send_lock: Mutex::new(()),
        }
    }

    fn run(&mut self, rx: Receiver<Command>) {
        loop {
            // While disconnected, keep the loop responsive to commands but do
            // not busy-spin on connect attempts.
            match rx.recv_timeout(RECONNECT_DELAY) {
                Ok(Command::Shutdown) => break,
                Ok(Command::Set(activity)) => {
                    self.current = activity;
                    self.publish();
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        self.clear_presence();
    }

    /// Pushes the current activity, connecting first if necessary.
    fn publish(&mut self) {
        if self.socket.is_none() && self.connect().is_err() {
            return;
        }
        if let Err(err) = self.write_current() {
            log::debug!("discord presence write failed ({err:#}); will reconnect");
            self.socket = None;
        }
    }

    fn connect(&mut self) -> Result<()> {
        let mut socket = IpcStream::connect()?;
        let handshake = json!({ "v": 1, "client_id": self.client_id }).to_string();
        socket.send(OP_HANDSHAKE, &handshake)?;
        self.connections.fetch_add(1, Ordering::Relaxed);
        self.socket = Some(socket);
        self.started_at_ms = now_ms();
        log::info!("connected to Discord Rich Presence");
        Ok(())
    }

    /// Composes and sends the `SET_ACTIVITY` frame for the current activity.
    fn write_current(&mut self) -> Result<()> {
        // Holding this across the write keeps frame interleaving impossible.
        let _guard = self.send_lock.lock();
        let Some(socket) = self.socket.as_mut() else {
            return Err(anyhow!("not connected to Discord"));
        };

        let activity = match &self.current {
            Some(activity) if !activity.is_empty() => build_activity(activity),
            // Clearing uses a null activity, which is how the protocol spells
            // "stop showing anything".
            _ => Value::Null,
        };

        let payload = json!({
            "cmd": "SET_ACTIVITY",
            "args": {
                "pid": std::process::id(),
                "activity": activity,
            },
            "nonce": format!("{}", now_ms()),
        })
        .to_string();

        socket.send(OP_FRAME, &payload)?;

        // The reply is informational. Reading it keeps the pipe from backing up
        // on Windows, where an unread response eventually blocks the writer.
        if let Err(err) = socket.recv() {
            log::trace!("no discord acknowledgement: {err:#}");
        }
        Ok(())
    }

    fn clear_presence(&mut self) {
        self.current = None;
        let _ = self.write_current();
    }
}

/// Converts a snapshot into the `activity` object Discord expects.
///
/// Timestamps are the whole point of the feature: `start` makes Discord render
/// a live progress counter, and it is recomputed from the current position on
/// every update so a seek or a pause is reflected immediately.
///
/// Artwork comes from the Discord application's registered assets rather than
/// from `artwork_url`. The protocol's `large_image` accepts either a registered
/// asset key or, on some client versions, an external URL — but external URLs
/// are only honoured for whitelisted proxies, so the asset key is the one that
/// actually renders. `artwork_url` is retained on the snapshot for callers that
/// want to build a richer payload later.
fn build_activity(activity: &Activity) -> Value {
    let artwork_key = super::rich_assets::live_key(activity.playing);
    let mut obj = json!({
        "type": 2, // Listening
        "details": activity.details(),
        "state": activity.state(),
        "assets": {
            "large_image": artwork_key,
            "large_text": activity
                .large_text()
                .unwrap_or_else(|| "Echo Music".to_string()),
            "small_image": super::rich_assets::ASSET_LOGO,
            "small_text": if activity.playing { "Playing" } else { "Paused" },
        },
    });

    // `start` alone is enough for Discord to count up from that instant. When
    // paused the elapsed time is frozen by re-sending the same `start`, which is
    // why the worker replays the activity instead of issuing a pause command.
    let elapsed = activity.position_secs.min(u64::from(u32::MAX)) as i64;
    let start = now_ms() - elapsed * 1000;
    obj["timestamps"] = json!({ "start": start });

    if let Some(duration) = activity.duration_secs.filter(|d| *d > 0) {
        obj["timestamps"]["end"] = json!(start + i64::from(duration) * 1000);
    }
    obj
}

/// Current unix time in milliseconds.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Activity {
        Activity {
            title: "Khalasi".into(),
            artist: "Aditya Gadhvi".into(),
            album: Some("Coke Studio".into()),
            duration_secs: Some(196),
            position_secs: 42,
            playing: true,
            artwork_url: None,
        }
    }

    #[test]
    fn empty_activity_is_detected() {
        assert!(Activity::default().is_empty());
        assert!(!sample().is_empty());
    }

    #[test]
    fn timestamps_are_derived_from_the_position() {
        let value = build_activity(&sample());
        let start = value["timestamps"]["start"].as_i64().unwrap();
        let end = value["timestamps"]["end"].as_i64().unwrap();
        // The elapsed window must match the track duration exactly, otherwise
        // Discord draws a progress bar of the wrong length.
        assert_eq!(end - start, 196_000);
        assert!(start <= now_ms());
    }

    #[test]
    fn missing_duration_omits_the_end_timestamp() {
        let mut activity = sample();
        activity.duration_secs = None;
        let value = build_activity(&activity);
        assert!(value["timestamps"].get("end").is_none());
        assert!(value["timestamps"]["start"].as_i64().is_some());
    }

    #[test]
    fn long_fields_are_truncated() {
        let mut activity = sample();
        activity.title = "x".repeat(400);
        let value = build_activity(&activity);
        let details = value["details"].as_str().unwrap();
        assert!(details.chars().count() <= 128);
    }
}
