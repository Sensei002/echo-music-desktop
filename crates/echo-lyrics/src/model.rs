//! Lyrics data model.

use serde::{Deserialize, Serialize};

/// A single word inside a word-by-word lyric line.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LyricWord {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
}

/// One line of lyrics, optionally word-timed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LyricLine {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    #[serde(default)]
    pub words: Vec<LyricWord>,
    /// Machine translation of this line, when enabled.
    #[serde(default)]
    pub translation: Option<String>,
}

impl LyricLine {
    pub fn has_word_timing(&self) -> bool {
        !self.words.is_empty()
    }
}

/// A complete lyrics document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
    /// Provider id that produced this document (e.g. `"lrclib"`).
    pub provider: String,
    /// True when at least one line carries a timestamp.
    pub synced: bool,
    /// True when at least one line carries per-word timing.
    pub word_by_word: bool,
}

impl Lyrics {
    /// Wraps a flat list of lines, deriving the flags from their contents.
    pub fn new(lines: Vec<LyricLine>, provider: impl Into<String>) -> Self {
        let synced = lines.iter().any(|line| line.start_ms > 0);
        let word_by_word = lines.iter().any(LyricLine::has_word_timing);
        Self {
            lines,
            provider: provider.into(),
            synced,
            word_by_word,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|line| line.text.trim().is_empty())
    }

    /// Index of the line that should be highlighted at `position_ms`.
    pub fn active_line_index(&self, position_ms: u64) -> Option<usize> {
        if self.lines.is_empty() {
            return None;
        }
        let mut candidate = None;
        for (index, line) in self.lines.iter().enumerate() {
            if line.start_ms <= position_ms {
                candidate = Some(index);
            } else {
                break;
            }
        }
        candidate.or(Some(0))
    }

    /// The line highlighted at `position_ms`.
    pub fn active_line(&self, position_ms: u64) -> Option<&LyricLine> {
        self.active_line_index(position_ms)
            .and_then(|index| self.lines.get(index))
    }

    /// Index of the word highlighted at `position_ms` within the active line.
    pub fn active_word_index(&self, position_ms: u64) -> Option<usize> {
        let line = self.active_line(position_ms)?;
        if line.words.is_empty() {
            return None;
        }
        let mut candidate = None;
        for (index, word) in line.words.iter().enumerate() {
            if word.start_ms <= position_ms {
                candidate = Some(index);
            } else {
                break;
            }
        }
        candidate
    }

    /// Flattens to plain text (used for the "plain lyrics" view and exports).
    pub fn to_plain_text(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Applies translations, matched by line index.
    pub fn apply_translations(&mut self, translations: Vec<String>) {
        for (line, translation) in self.lines.iter_mut().zip(translations) {
            if !translation.trim().is_empty() {
                line.translation = Some(translation);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, start_ms: u64) -> LyricLine {
        LyricLine {
            text: text.into(),
            start_ms,
            ..Default::default()
        }
    }

    #[test]
    fn finds_the_active_line() {
        let lyrics = Lyrics::new(
            vec![
                line("first", 0),
                line("second", 5_000),
                line("third", 10_000),
            ],
            "test",
        );
        assert_eq!(lyrics.active_line(0).unwrap().text, "first");
        assert_eq!(lyrics.active_line(7_000).unwrap().text, "second");
        assert_eq!(lyrics.active_line(99_000).unwrap().text, "third");
    }

    #[test]
    fn finds_the_active_word() {
        let mut lyrics = Lyrics::new(vec![line("hello world", 0)], "test");
        lyrics.lines[0].words = vec![
            LyricWord {
                text: "hello".into(),
                start_ms: 0,
                end_ms: Some(500),
            },
            LyricWord {
                text: "world".into(),
                start_ms: 500,
                end_ms: Some(1_000),
            },
        ];
        lyrics.word_by_word = true;
        assert_eq!(lyrics.active_word_index(100), Some(0));
        assert_eq!(lyrics.active_word_index(600), Some(1));
    }

    #[test]
    fn plain_text_round_trips() {
        let lyrics = Lyrics::new(vec![line("a", 0), line("b", 1)], "test");
        assert_eq!(lyrics.to_plain_text(), "a\nb");
        assert!(lyrics.synced);
    }
}
