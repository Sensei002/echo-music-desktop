//! Playback queue with shuffle and repeat, mirroring upstream's `Queue` types.

use echo_core::{RepeatMode, Song};
use rand::seq::SliceRandom;

/// An ordered list of tracks with a movable cursor.
#[derive(Debug, Clone, Default)]
pub struct Queue {
    songs: Vec<Song>,
    /// Indices into `songs`, in playback order (differs when shuffled).
    order: Vec<usize>,
    cursor: usize,
    shuffle: bool,
    repeat: RepeatMode,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the queue contents and positions the cursor on `start`.
    pub fn set_songs(&mut self, songs: Vec<Song>, start: usize) {
        self.songs = songs;
        self.rebuild_order(start);
    }

    /// Appends tracks to the end of the queue.
    pub fn enqueue(&mut self, songs: Vec<Song>) {
        let base = self.songs.len();
        let additions = songs.len();
        self.songs.extend(songs);
        let extra: Vec<usize> = (base..base + additions).collect();
        if self.shuffle {
            // Insert the new tracks right after the cursor so they play soon.
            let at = (self.cursor + 1).min(self.order.len());
            for (offset, index) in extra.into_iter().enumerate() {
                self.order.insert(at + offset, index);
            }
        } else {
            self.order.extend(extra);
        }
    }

    /// Inserts a track immediately after the current one.
    pub fn play_next(&mut self, song: Song) {
        self.songs.push(song);
        let index = self.songs.len() - 1;
        let at = (self.cursor + 1).min(self.order.len());
        self.order.insert(at, index);
    }

    pub fn clear(&mut self) {
        self.songs.clear();
        self.order.clear();
        self.cursor = 0;
    }

    pub fn songs(&self) -> &[Song] {
        &self.songs
    }

    pub fn len(&self) -> usize {
        self.songs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
    }

    /// The index of the current track inside [`Queue::songs`].
    pub fn current_index(&self) -> usize {
        self.order.get(self.cursor).copied().unwrap_or(0)
    }

    pub fn current(&self) -> Option<&Song> {
        self.order
            .get(self.cursor)
            .and_then(|index| self.songs.get(*index))
    }

    pub fn is_shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    /// Enables/disables shuffle while keeping the current track playing.
    pub fn set_shuffle(&mut self, shuffle: bool) {
        if self.shuffle == shuffle {
            return;
        }
        let current = self.current_index();
        self.shuffle = shuffle;
        self.rebuild_order(Some(current));
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
    }

    /// Moves to the next track, honouring the repeat mode.
    pub fn advance(&mut self) -> Option<&Song> {
        if self.order.is_empty() {
            return None;
        }
        if self.cursor + 1 < self.order.len() {
            self.cursor += 1;
            return self.current();
        }
        match self.repeat {
            RepeatMode::All => {
                if self.shuffle {
                    let current = self.current_index();
                    self.rebuild_order(Some(current));
                    // Skip the track we just finished.
                    if self.order.len() > 1 {
                        self.cursor = 1;
                    }
                } else {
                    self.cursor = 0;
                }
                self.current()
            }
            // `One` is handled by the engine (it re-plays the same track) and
            // `Off` simply stops at the end of the queue.
            RepeatMode::One | RepeatMode::Off => None,
        }
    }

    /// Moves to the previous track, wrapping when repeating.
    pub fn rewind(&mut self) -> Option<&Song> {
        if self.order.is_empty() {
            return None;
        }
        if self.cursor > 0 {
            self.cursor -= 1;
            return self.current();
        }
        if self.repeat == RepeatMode::All {
            self.cursor = self.order.len() - 1;
            return self.current();
        }
        self.current()
    }

    /// Jumps to a specific index inside [`Queue::songs`].
    pub fn jump(&mut self, song_index: usize) -> Option<&Song> {
        let position = self.order.iter().position(|index| *index == song_index)?;
        self.cursor = position;
        self.current()
    }

    /// Removes a track by its index inside [`Queue::songs`].
    pub fn remove(&mut self, song_index: usize) {
        if song_index >= self.songs.len() {
            return;
        }
        let was_current = self.current_index() == song_index;
        self.songs.remove(song_index);

        // Re-map the order, dropping the removed entry and shifting the rest.
        let mut order: Vec<usize> = self
            .order
            .iter()
            .filter(|index| **index != song_index)
            .map(|index| if *index > song_index { *index - 1 } else { *index })
            .collect();
        if order.is_empty() {
            self.order = order;
            self.cursor = 0;
            return;
        }
        let cursor_shift = self.order.iter().take(self.cursor).filter(|i| **i == song_index).count();
        self.cursor = self.cursor.saturating_sub(cursor_shift).min(order.len() - 1);
        if was_current {
            self.cursor = self.cursor.min(order.len() - 1);
        }
        order.shrink_to_fit();
        self.order = order;
    }

    /// Replaces the queue order for an explicit drag-and-drop reorder.
    pub fn reorder(&mut self, new_order: Vec<usize>) {
        if new_order.len() != self.songs.len() {
            return;
        }
        let current = self.current_index();
        self.order = new_order;
        if let Some(position) = self.order.iter().position(|index| *index == current) {
            self.cursor = position;
        }
    }

    fn rebuild_order(&mut self, start: Option<usize>) {
        let len = self.songs.len();
        if len == 0 {
            self.order.clear();
            self.cursor = 0;
            return;
        }
        let start_index = start.unwrap_or(0).min(len - 1);
        if self.shuffle {
            let mut rest: Vec<usize> = (0..len).filter(|index| *index != start_index).collect();
            rest.shuffle(&mut rand::thread_rng());
            self.order = std::iter::once(start_index).chain(rest).collect();
            self.cursor = 0;
        } else {
            self.order = (0..len).collect();
            self.cursor = start_index;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn songs(n: usize) -> Vec<Song> {
        (0..n)
            .map(|i| Song {
                id: format!("s{i}"),
                title: format!("Song {i}"),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn advances_and_stops_at_the_end() {
        let mut queue = Queue::new();
        queue.set_songs(songs(3), 0);
        assert_eq!(queue.current().unwrap().id, "s0");
        assert_eq!(queue.advance().unwrap().id, "s1");
        assert_eq!(queue.advance().unwrap().id, "s2");
        assert!(queue.advance().is_none());
    }

    #[test]
    fn repeat_all_wraps_around() {
        let mut queue = Queue::new();
        queue.set_songs(songs(3), 0);
        queue.set_repeat(RepeatMode::All);
        queue.advance();
        queue.advance();
        assert_eq!(queue.advance().unwrap().id, "s0");
    }

    #[test]
    fn rewind_wraps_only_when_repeating() {
        let mut queue = Queue::new();
        queue.set_songs(songs(3), 0);
        assert_eq!(queue.rewind().unwrap().id, "s0");
        queue.set_repeat(RepeatMode::All);
        assert_eq!(queue.rewind().unwrap().id, "s2");
    }

    #[test]
    fn jump_selects_a_track() {
        let mut queue = Queue::new();
        queue.set_songs(songs(5), 0);
        assert_eq!(queue.jump(3).unwrap().id, "s3");
        assert_eq!(queue.current_index(), 3);
    }

    #[test]
    fn shuffle_keeps_the_starting_track_first() {
        let mut queue = Queue::new();
        queue.set_songs(songs(20), 7);
        queue.set_shuffle(true);
        assert_eq!(queue.current().unwrap().id, "s7");
        assert_eq!(queue.len(), 20);
    }

    #[test]
    fn enqueue_appends() {
        let mut queue = Queue::new();
        queue.set_songs(songs(2), 0);
        queue.enqueue(songs(3).into_iter().map(|mut s| { s.id = format!("x{}", s.id); s }).collect());
        assert_eq!(queue.len(), 5);
    }

    #[test]
    fn remove_keeps_the_cursor_valid() {
        let mut queue = Queue::new();
        queue.set_songs(songs(4), 2);
        queue.remove(0);
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.current().unwrap().id, "s2");
    }

    #[test]
    fn play_next_inserts_after_current() {
        let mut queue = Queue::new();
        queue.set_songs(songs(3), 0);
        queue.play_next(Song {
            id: "n".into(),
            title: "Next".into(),
            ..Default::default()
        });
        assert_eq!(queue.advance().unwrap().id, "n");
    }
}
