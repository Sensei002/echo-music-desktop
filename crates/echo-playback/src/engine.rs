//! The playback engine.
//!
//! Everything audio-related lives on a single dedicated thread that owns the
//! `rodio` output stream. The UI talks to it through a command channel and
//! receives notifications through an event channel, so the Slint thread never
//! blocks on decoding, networking or device I/O.

use crate::equalizer::{EqConfig, EqHandle, EqualizerSource};
use crate::queue::Queue;
use crate::source::HttpRangeSource;
use anyhow::{anyhow, Result};
use echo_core::{PlaybackState, PlayerSnapshot, RepeatMode, Song};
use parking_lot::Mutex;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How often the engine thread wakes up to tick progress and drain commands.
const TICK: Duration = Duration::from_millis(100);
/// Ignore end-of-stream detection for this long after a track starts, to avoid
/// racing the audio thread's queue population.
const START_GRACE: Duration = Duration::from_millis(400);

/// Instructions sent from the UI to the engine thread.
#[derive(Debug)]
pub enum Command {
    /// Start playing a resolved stream.
    Play { song: Box<Song>, url: String },
    /// Replace the queue and start at `start`.
    SetQueue { songs: Vec<Song>, start: usize },
    /// Append tracks to the queue.
    Enqueue(Vec<Song>),
    /// Insert a track immediately after the current one.
    PlayNext(Box<Song>),
    PlayPause,
    Pause,
    Resume,
    Stop,
    Next,
    Previous,
    /// Seek to a position in milliseconds.
    Seek(u64),
    SetVolume(f32),
    SetMuted(bool),
    SetShuffle(bool),
    SetRepeat(RepeatMode),
    SetEqualizer(Box<EqConfig>),
    SetCrossfade { enabled: bool, seconds: u32 },
    JumpTo(usize),
    Remove(usize),
    /// Arm the sleep timer for `minutes` (0 disables).
    SleepTimer(u32),
    /// Ask for a fresh position/queue notification.
    Refresh,
    Shutdown,
}

/// Notifications pushed from the engine thread to the UI.
#[derive(Debug, Clone)]
pub enum Event {
    State(PlaybackState),
    /// A new track began playing.
    TrackStarted(Box<Song>),
    /// Progress update (~10 Hz while playing).
    Position { position_ms: u64, duration_ms: u64 },
    /// The queue or its cursor changed.
    Queue { songs: Vec<Song>, index: usize },
    /// The last track finished and nothing else is queued.
    Ended,
    /// A recoverable error (decoder, network, device).
    Error(String),
}

/// Handle to the audio engine.
pub struct PlaybackEngine {
    commands: Sender<Command>,
    events: Receiver<Event>,
    shared: Arc<Mutex<PlayerSnapshot>>,
    eq: EqHandle,
    thread: Option<JoinHandle<()>>,
}

impl PlaybackEngine {
    /// Spawns the engine thread and waits until the output device is open.
    pub fn new() -> Result<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(PlayerSnapshot::default()));
        let shared_thread = shared.clone();
        let eq = EqHandle::default();
        let eq_thread = eq.clone();

        let thread = thread::Builder::new()
            .name("echo-audio".into())
            .spawn(move || {
                let engine = Engine::start(command_rx, event_tx, shared_thread, ready_tx, eq_thread);
                if let Some(mut engine) = engine {
                    engine.run();
                }
            })
            .map_err(|err| anyhow!("failed to spawn the audio thread: {err}"))?;

        match ready_rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(())) => Ok(Self {
                commands: command_tx,
                events: event_rx,
                shared,
                eq,
                thread: Some(thread),
            }),
            Ok(Err(message)) => Err(anyhow!(message)),
            Err(_) => Err(anyhow!("the audio engine did not report readiness")),
        }
    }

    /// Sends a command to the engine thread.
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// Drains pending notifications (non-blocking).
    pub fn drain_events(&self) -> Vec<Event> {
        let mut events = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            events.push(event);
        }
        events
    }

    /// The latest player snapshot, readable from any thread.
    pub fn snapshot(&self) -> PlayerSnapshot {
        self.shared.lock().clone()
    }

    /// A cloneable handle to the live equalizer configuration.
    pub fn equalizer(&self) -> EqHandle {
        self.eq.clone()
    }
}

impl Drop for PlaybackEngine {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A monotonic clock tracking playback position.
#[derive(Debug)]
struct TrackClock {
    offset: Duration,
    anchor: Instant,
    running: bool,
}

impl TrackClock {
    fn new() -> Self {
        Self {
            offset: Duration::ZERO,
            anchor: Instant::now(),
            running: false,
        }
    }

    fn position(&self) -> Duration {
        if self.running {
            self.offset + self.anchor.elapsed()
        } else {
            self.offset
        }
    }

    fn resume(&mut self) {
        if !self.running {
            self.anchor = Instant::now();
            self.running = true;
        }
    }

    fn pause(&mut self) {
        if self.running {
            self.offset = self.position();
            self.running = false;
        }
    }

    fn seek(&mut self, to: Duration) {
        self.offset = to;
        self.anchor = Instant::now();
    }

    fn reset(&mut self) {
        self.offset = Duration::ZERO;
        self.anchor = Instant::now();
        self.running = true;
    }
}

/// State of an in-flight crossfade.
struct Crossfade {
    started: Instant,
    duration: Duration,
}

/// The engine thread's owned state.
struct Engine {
    handle: OutputStreamHandle,
    _stream: OutputStream,
    commands: Receiver<Command>,
    events: Sender<Event>,
    shared: Arc<Mutex<PlayerSnapshot>>,

    eq: EqHandle,
    queue: Queue,
    current: Option<Sink>,
    outgoing: Option<Sink>,
    crossfade: Option<Crossfade>,
    crossfade_enabled: bool,
    crossfade_seconds: u32,

    clock: TrackClock,
    duration: Duration,
    started_at: Option<Instant>,
    state: PlaybackState,
    volume: f32,
    muted: bool,
    http: reqwest::blocking::Client,
    sleep_deadline: Option<Instant>,
    last_position_emit: Instant,
}

impl Engine {
    fn start(
        commands: Receiver<Command>,
        events: Sender<Event>,
        shared: Arc<Mutex<PlayerSnapshot>>,
        ready: Sender<Result<(), String>>,
        eq: EqHandle,
    ) -> Option<Self> {
        let (stream, handle) = match OutputStream::try_default() {
            Ok(pair) => pair,
            Err(err) => {
                let _ = ready.send(Err(format!("no audio output device available: {err}")));
                return None;
            }
        };
        let _ = ready.send(Ok(()));

        // Streaming requests must not time out mid-track.
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(6)
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());

        Some(Self {
            handle,
            _stream: stream,
            commands,
            events,
            shared,
            eq,
            queue: Queue::new(),
            current: None,
            outgoing: None,
            crossfade: None,
            crossfade_enabled: false,
            crossfade_seconds: 6,
            clock: TrackClock::new(),
            duration: Duration::ZERO,
            started_at: None,
            state: PlaybackState::Idle,
            volume: 1.0,
            muted: false,
            http,
            sleep_deadline: None,
            last_position_emit: Instant::now(),
        })
    }

    fn run(&mut self) {
        loop {
            match self.commands.recv_timeout(TICK) {
                Ok(Command::Shutdown) => break,
                Ok(command) => self.handle_command(command),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.tick();
        }
        if let Some(sink) = self.current.take() {
            sink.stop();
        }
        if let Some(sink) = self.outgoing.take() {
            sink.stop();
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::Play { song, url } => {
                let crossfade = self.crossfade_enabled && self.state == PlaybackState::Playing;
                if let Err(err) = self.start_track(&song, &url, crossfade) {
                    self.emit(Event::Error(err.to_string()));
                }
            }
            Command::SetQueue { songs, start } => {
                self.queue.set_songs(songs, start);
                self.broadcast_queue();
            }
            Command::Enqueue(songs) => {
                self.queue.enqueue(songs);
                self.broadcast_queue();
            }
            Command::PlayNext(song) => {
                self.queue.play_next(*song);
                self.broadcast_queue();
            }
            Command::PlayPause => match self.state {
                PlaybackState::Playing => self.pause(),
                PlaybackState::Paused => self.resume(),
                _ => {}
            },
            Command::Pause => self.pause(),
            Command::Resume => self.resume(),
            Command::Stop => self.stop(),
            Command::Next => self.next_track(false),
            Command::Previous => self.previous_track(),
            Command::Seek(ms) => self.seek(ms),
            Command::SetVolume(volume) => {
                self.volume = volume.clamp(0.0, 1.0);
                self.apply_volume();
            }
            Command::SetMuted(muted) => {
                self.muted = muted;
                self.apply_volume();
            }
            Command::SetShuffle(shuffle) => {
                self.queue.set_shuffle(shuffle);
                self.broadcast_queue();
            }
            Command::SetRepeat(repeat) => {
                self.queue.set_repeat(repeat);
                self.broadcast_queue();
            }
            Command::SetEqualizer(config) => self.eq.update(*config),
            Command::SetCrossfade { enabled, seconds } => {
                self.crossfade_enabled = enabled;
                self.crossfade_seconds = seconds.min(12);
            }
            Command::JumpTo(index) => {
                if let Some(song) = self.queue.jump(index).cloned() {
                    self.emit(Event::TrackStarted(Box::new(song)));
                    self.broadcast_queue();
                    // The application resolves and sends `Play` for the new track.
                }
            }
            Command::Remove(index) => {
                self.queue.remove(index);
                self.broadcast_queue();
            }
            Command::SleepTimer(minutes) => {
                self.sleep_deadline = if minutes == 0 {
                    None
                } else {
                    Some(Instant::now() + Duration::from_secs(minutes as u64 * 60))
                };
            }
            Command::Refresh => {
                self.broadcast_queue();
                self.emit_position(true);
            }
            Command::Shutdown => {}
        }
        self.update_shared();
    }

    fn tick(&mut self) {
        // Sleep timer.
        if let Some(deadline) = self.sleep_deadline {
            if Instant::now() >= deadline {
                self.sleep_deadline = None;
                self.pause();
            }
        }

        // Crossfade ramp.
        if let Some(crossfade) = &self.crossfade {
            let elapsed = crossfade.started.elapsed().as_secs_f32();
            let progress = (elapsed / crossfade.duration.as_secs_f32().max(0.01)).clamp(0.0, 1.0);
            let target = self.effective_volume();
            if let Some(sink) = &self.current {
                sink.set_volume(target * progress);
            }
            if let Some(sink) = &self.outgoing {
                sink.set_volume(target * (1.0 - progress));
            }
            if progress >= 1.0 {
                self.crossfade = None;
                self.outgoing = None;
                self.apply_volume();
            }
        }

        // End-of-track detection.
        if self.state == PlaybackState::Playing {
            let finished = self
                .current
                .as_ref()
                .map(|sink| sink.empty())
                .unwrap_or(false);
            let settled = self
                .started_at
                .map(|start| start.elapsed() > START_GRACE)
                .unwrap_or(false);
            if finished && settled {
                self.on_track_finished();
            }
        }

        self.emit_position(false);
        self.update_shared();
    }

    // ------------------------------------------------------------- transport

    fn start_track(&mut self, song: &Song, url: &str, crossfade: bool) -> Result<()> {
        let source = HttpRangeSource::open(self.http.clone(), url)?;
        let content_length = source.length();
        let decoder = Decoder::new(source).map_err(|err| anyhow!("failed to decode the stream: {err}"))?;
        let equalized = EqualizerSource::new(decoder, self.eq.clone());

        let duration = equalized
            .total_duration()
            .or_else(|| content_length.map(|len| Duration::from_secs_f32(len as f32 / 16_000.0)))
            .unwrap_or(Duration::ZERO);

        let sink = Sink::try_new(&self.handle).map_err(|err| anyhow!("failed to open a sink: {err}"))?;
        sink.set_volume(self.effective_volume());

        if crossfade && self.crossfade_enabled {
            sink.set_volume(0.0);
            sink.append(equalized);
            sink.play();
            self.outgoing = self.current.take();
            self.crossfade = Some(Crossfade {
                started: Instant::now(),
                duration: Duration::from_secs(self.crossfade_seconds.max(1) as u64),
            });
        } else {
            if let Some(previous) = self.current.take() {
                previous.stop();
            }
            self.outgoing = None;
            self.crossfade = None;
            sink.append(equalized);
            sink.play();
        }

        self.current = Some(sink);
        self.duration = duration;
        self.clock.reset();
        self.started_at = Some(Instant::now());
        self.state = PlaybackState::Playing;
        self.emit(Event::TrackStarted(Box::new(song.clone())));
        self.emit(Event::State(PlaybackState::Playing));
        self.emit_position(true);
        Ok(())
    }

    fn pause(&mut self) {
        if let Some(sink) = &self.current {
            sink.pause();
        }
        self.clock.pause();
        self.state = PlaybackState::Paused;
        self.emit(Event::State(PlaybackState::Paused));
    }

    fn resume(&mut self) {
        if let Some(sink) = &self.current {
            sink.play();
        }
        self.clock.resume();
        self.state = PlaybackState::Playing;
        self.emit(Event::State(PlaybackState::Playing));
    }

    fn stop(&mut self) {
        if let Some(sink) = self.current.take() {
            sink.stop();
        }
        if let Some(sink) = self.outgoing.take() {
            sink.stop();
        }
        self.crossfade = None;
        self.clock = TrackClock::new();
        self.duration = Duration::ZERO;
        self.started_at = None;
        self.state = PlaybackState::Stopped;
        self.emit(Event::State(PlaybackState::Stopped));
    }

    fn seek(&mut self, position_ms: u64) {
        let target = Duration::from_millis(position_ms);
        if let Some(sink) = &self.current {
            if let Err(err) = sink.try_seek(target) {
                log::warn!("seek failed: {err}");
                return;
            }
        }
        self.clock.seek(target);
        self.emit_position(true);
    }

    /// Advances to the next track, resolving is the application's job.
    fn next_track(&mut self, auto: bool) {
        if self.queue.repeat() == RepeatMode::One && auto {
            // Replay the same track by asking the application to reload it.
            if let Some(song) = self.queue.current().cloned() {
                self.emit(Event::TrackStarted(Box::new(song)));
            }
            return;
        }
        match self.queue.advance() {
            Some(song) => {
                let song = song.clone();
                self.emit(Event::TrackStarted(Box::new(song)));
                self.broadcast_queue();
            }
            None => {
                self.state = PlaybackState::Stopped;
                self.emit(Event::State(PlaybackState::Stopped));
                self.emit(Event::Ended);
            }
        }
    }

    fn previous_track(&mut self) {
        // Restart the current track when we are past the first few seconds.
        if self.clock.position() > Duration::from_secs(3) {
            self.seek(0);
            return;
        }
        if let Some(song) = self.queue.rewind().cloned() {
            self.emit(Event::TrackStarted(Box::new(song)));
            self.broadcast_queue();
        }
    }

    fn on_track_finished(&mut self) {
        self.next_track(true);
    }

    // --------------------------------------------------------------- helpers

    fn effective_volume(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.volume
        }
    }

    fn apply_volume(&mut self) {
        if self.crossfade.is_some() {
            return;
        }
        let volume = self.effective_volume();
        if let Some(sink) = &self.current {
            sink.set_volume(volume);
        }
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn broadcast_queue(&self) {
        self.emit(Event::Queue {
            songs: self.queue.songs().to_vec(),
            index: self.queue.current_index(),
        });
    }

    fn emit_position(&mut self, force: bool) {
        if !force && self.last_position_emit.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.last_position_emit = Instant::now();
        let position_ms = self.clock.position().as_millis() as u64;
        let duration_ms = self.duration.as_millis() as u64;
        self.emit(Event::Position {
            position_ms,
            duration_ms,
        });
    }

    fn update_shared(&self) {
        let mut snapshot = self.shared.lock();
        snapshot.state = self.state;
        snapshot.current = self.queue.current().cloned();
        snapshot.queue = self.queue.songs().to_vec();
        snapshot.index = self.queue.current_index();
        snapshot.position_ms = self.clock.position().as_millis() as u64;
        snapshot.duration_ms = self.duration.as_millis() as u64;
        snapshot.volume = self.volume;
        snapshot.muted = self.muted;
        snapshot.shuffle = self.queue.is_shuffle();
        snapshot.repeat = self.queue.repeat();
    }
}

impl PlaybackEngine {
    /// Convenience constructor used by the app when audio is optional.
    pub fn try_new_or_silent() -> Option<Self> {
        match Self::new() {
            Ok(engine) => Some(engine),
            Err(err) => {
                log::error!("audio disabled: {err}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_tracks_position_while_running() {
        let mut clock = TrackClock::new();
        clock.reset();
        std::thread::sleep(Duration::from_millis(30));
        assert!(clock.position() >= Duration::from_millis(20));

        clock.pause();
        let paused = clock.position();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(clock.position(), paused);

        clock.seek(Duration::from_secs(60));
        assert!(clock.position() >= Duration::from_secs(60));
    }

    #[test]
    fn placeholder_equalizer_is_flat() {
        let handle = EqHandle::default();
        assert!(!handle.snapshot().enabled);
    }
}
