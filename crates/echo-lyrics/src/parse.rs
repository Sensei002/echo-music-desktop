//! LRC, enhanced-LRC and TTML parsers.
//!
//! Upstream parses lyrics with dedicated parsers per provider; here a single
//! module handles every format, because the providers only ever emit one of
//! three shapes.

use crate::model::{LyricLine, LyricWord, Lyrics};
use once_cell::sync::Lazy;
use regex::Regex;

/// The wire format a provider returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricsFormat {
    /// `[mm:ss.xx]line`
    Lrc,
    /// `<mm:ss.xx>word` inside an LRC line.
    EnhancedLrc,
    /// W3C TTML as used by BetterLyrics and YouLyPlus.
    Ttml,
    /// Untimed text.
    Plain,
}

static LRC_TIME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[(\d{1,3}):(\d{1,2})(?:[.:](\d{1,3}))?\]").expect("valid regex"));
static WORD_TIME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<(\d{1,3}):(\d{1,2})(?:[.:](\d{1,3}))?>").expect("valid regex"));
static TTML_PARA: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<p\b([^>]*)>(.*?)</p>").expect("valid regex"));
static TTML_SPAN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<span\b([^>]*)>(.*?)</span>").expect("valid regex"));
static TTML_ATTR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)(begin|end)\s*=\s*"([^"]*)""#).expect("valid regex"));
static ANY_TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]*>").expect("valid regex"));

/// Guesses the format of a payload.
pub fn detect_format(text: &str) -> LyricsFormat {
    let trimmed = text.trim_start();
    if trimmed.starts_with("<?xml") || trimmed.starts_with("<tt") || trimmed.contains("<tt ") {
        return LyricsFormat::Ttml;
    }
    if WORD_TIME.is_match(text) && LRC_TIME.is_match(text) {
        return LyricsFormat::EnhancedLrc;
    }
    if LRC_TIME.is_match(text) {
        return LyricsFormat::Lrc;
    }
    LyricsFormat::Plain
}

/// Parses a payload of the given format into a [`Lyrics`] document.
pub fn parse_lyrics(text: &str, format: LyricsFormat, provider: &str) -> Lyrics {
    let lines = match format {
        LyricsFormat::Lrc | LyricsFormat::EnhancedLrc => parse_lrc(text),
        LyricsFormat::Ttml => parse_ttml(text),
        LyricsFormat::Plain => parse_plain(text),
    };
    Lyrics::new(lines, provider)
}

/// Parses a payload, detecting its format automatically.
pub fn parse_auto(text: &str, provider: &str) -> Lyrics {
    parse_lyrics(text, detect_format(text), provider)
}

/// Parses classic and enhanced LRC.
pub fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();

    for raw in text.lines() {
        let raw = raw.trim_end_matches(['\r', '\n']);
        if raw.trim().is_empty() {
            continue;
        }

        let stamps: Vec<u64> = LRC_TIME
            .captures_iter(raw)
            .filter_map(|caps| tag_to_ms(&caps))
            .collect();
        if stamps.is_empty() {
            continue;
        }

        // Everything after the final timestamp is the lyric body.
        let content_start = LRC_TIME.find_iter(raw).last().map(|m| m.end()).unwrap_or(0);
        let content = &raw[content_start..];

        let words = parse_words(content);
        let line_text = if words.is_empty() {
            content.trim().to_string()
        } else {
            words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<String>()
                .trim()
                .to_string()
        };

        // Drop the trailing marker used by some providers.
        let line_text = line_text.trim_start_matches('♪').trim().to_string();
        if line_text.is_empty() && words.is_empty() {
            continue;
        }

        // A line may carry several timestamps (repeated choruses).
        for start in stamps {
            lines.push(LyricLine {
                text: line_text.clone(),
                start_ms: start,
                end_ms: None,
                words: words.clone(),
                translation: None,
            });
        }
    }

    lines.sort_by_key(|line| line.start_ms);
    fill_end_times(&mut lines);
    lines
}

/// Extracts per-word timings from an enhanced-LRC body.
fn parse_words(content: &str) -> Vec<LyricWord> {
    let tags: Vec<(usize, usize, u64)> = WORD_TIME
        .captures_iter(content)
        .filter_map(|caps| {
            let whole = caps.get(0)?;
            Some((whole.start(), whole.end(), tag_to_ms(&caps)?))
        })
        .collect();

    let mut words = Vec::with_capacity(tags.len());
    for (index, (_, tag_end, start)) in tags.iter().enumerate() {
        let text_end = tags
            .get(index + 1)
            .map(|next| next.0)
            .unwrap_or(content.len());
        let text = content[*tag_end..text_end].to_string();
        words.push(LyricWord {
            text,
            start_ms: *start,
            end_ms: None,
        });
    }
    for index in 0..words.len().saturating_sub(1) {
        words[index].end_ms = Some(words[index + 1].start_ms);
    }
    words
}

/// Converts an LRC timestamp capture into milliseconds.
fn tag_to_ms(caps: &regex::Captures<'_>) -> Option<u64> {
    let minutes: u64 = caps.get(1)?.as_str().parse().ok()?;
    let seconds: u64 = caps.get(2)?.as_str().parse().ok()?;
    let fraction = caps.get(3).map(|m| m.as_str()).unwrap_or("");
    let millis = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u64>().ok()? * 100,
        2 => fraction.parse::<u64>().ok()? * 10,
        _ => fraction[..3].parse::<u64>().ok()?,
    };
    Some(minutes * 60_000 + seconds * 1_000 + millis)
}

/// Parses a TTML document.
pub fn parse_ttml(text: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();

    for caps in TTML_PARA.captures_iter(text) {
        let attrs = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let body = caps.get(2).map(|m| m.as_str()).unwrap_or("");

        let begin = attribute_time(attrs, "begin").unwrap_or(0);
        let end = attribute_time(attrs, "end");

        let mut words = Vec::new();
        for span in TTML_SPAN.captures_iter(body) {
            let span_attrs = span.get(1).map(|m| m.as_str()).unwrap_or("");
            let span_body = span.get(2).map(|m| m.as_str()).unwrap_or("");
            let word_text = strip_tags(span_body);
            if word_text.trim().is_empty() {
                continue;
            }
            words.push(LyricWord {
                text: word_text,
                start_ms: attribute_time(span_attrs, "begin").unwrap_or(begin),
                end_ms: attribute_time(span_attrs, "end"),
            });
        }

        let line_text = if words.is_empty() {
            strip_tags(body).trim().to_string()
        } else {
            words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };

        if line_text.is_empty() && words.is_empty() {
            continue;
        }

        lines.push(LyricLine {
            text: line_text,
            start_ms: begin,
            end_ms: end,
            words,
            translation: None,
        });
    }

    lines.sort_by_key(|line| line.start_ms);
    fill_end_times(&mut lines);
    lines
}

/// Splits untimed text into lines.
pub fn parse_plain(text: &str) -> Vec<LyricLine> {
    text.lines()
        .map(|line| LyricLine {
            text: line.trim_end().to_string(),
            start_ms: 0,
            end_ms: None,
            words: Vec::new(),
            translation: None,
        })
        .collect()
}

/// Reads a `begin`/`end` attribute value as milliseconds.
fn attribute_time(attrs: &str, key: &str) -> Option<u64> {
    for caps in TTML_ATTR.captures_iter(attrs) {
        if caps.get(1)?.as_str().eq_ignore_ascii_case(key) {
            return parse_time_value(caps.get(2)?.as_str());
        }
    }
    None
}

/// Parses a TTML clock value: `12.34`, `12.34s`, `12000ms`, `0:00:12.340`, `120000t`.
pub fn parse_time_value(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Some(raw) = value.strip_suffix("ms") {
        return raw.trim().parse::<f64>().ok().map(|v| v.max(0.0) as u64);
    }
    if let Some(raw) = value.strip_suffix('s') {
        return raw
            .trim()
            .parse::<f64>()
            .ok()
            .map(|v| (v * 1000.0).max(0.0) as u64);
    }
    if let Some(raw) = value.strip_suffix('t') {
        // TTML ticks are 10 000 per second in YouTube's output.
        return raw
            .trim()
            .parse::<f64>()
            .ok()
            .map(|v| (v / 10.0).max(0.0) as u64);
    }
    if value.contains(':') {
        let mut total = 0f64;
        for part in value.split(':') {
            total = total * 60.0 + part.trim().parse::<f64>().ok()?;
        }
        return Some((total * 1000.0).max(0.0) as u64);
    }
    value
        .parse::<f64>()
        .ok()
        .map(|v| (v * 1000.0).max(0.0) as u64)
}

/// Removes markup and decodes the XML entities lyrics documents use.
fn strip_tags(input: &str) -> String {
    let without_tags = ANY_TAG.replace_all(input, "");
    without_tags
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Derives `end_ms` for lines and words that do not carry one.
fn fill_end_times(lines: &mut [LyricLine]) {
    for index in 0..lines.len() {
        let next_start = lines.get(index + 1).map(|line| line.start_ms);
        if lines[index].end_ms.is_none() {
            lines[index].end_ms = next_start;
        }
        let word_count = lines[index].words.len();
        for word_index in 0..word_count {
            if lines[index].words[word_index].end_ms.is_none() {
                lines[index].words[word_index].end_ms = lines[index]
                    .words
                    .get(word_index + 1)
                    .map(|word| word.start_ms)
                    .or(lines[index].end_ms);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_lrc() {
        let text = "[ar:Artist]\n[00:12.34]First line\n[01:02.50]Second line\n";
        let lines = parse_lrc(text);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "First line");
        assert_eq!(lines[0].start_ms, 12_340);
        assert_eq!(lines[1].start_ms, 62_500);
        // end times are inferred from the next line
        assert_eq!(lines[0].end_ms, Some(62_500));
    }

    #[test]
    fn parses_enhanced_lrc_words() {
        let text = "[00:01.00]<00:01.00>Hello <00:01.50>world\n";
        let lines = parse_lrc(text);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[0].words[0].text, "Hello ");
        assert_eq!(lines[0].words[1].start_ms, 1_500);
        assert_eq!(lines[0].words[0].end_ms, Some(1_500));
        assert_eq!(lines[0].text, "Hello world");
    }

    #[test]
    fn handles_repeated_timestamps() {
        let text = "[00:10.00][00:20.00]Chorus\n";
        let lines = parse_lrc(text);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "Chorus");
        assert_eq!(lines[1].start_ms, 20_000);
    }

    #[test]
    fn parses_ttml_lines_and_words() {
        let text = r#"<?xml version="1.0"?>
        <tt xmlns="http://www.w3.org/ns/ttml"><body><div>
          <p begin="00:00:12.340" end="00:00:15.000">Plain line</p>
          <p begin="00:00:15.000" end="00:00:18.000"><span begin="00:00:15.000" end="00:00:16.000">Hello </span><span begin="00:00:16.000" end="00:00:18.000">world</span></p>
        </div></body></tt>"#;
        let lines = parse_ttml(text);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "Plain line");
        assert_eq!(lines[0].start_ms, 12_340);
        assert_eq!(lines[1].words.len(), 2);
        assert_eq!(lines[1].text, "Hello world");
        assert_eq!(lines[1].words[1].start_ms, 16_000);
    }

    #[test]
    fn parses_various_time_values() {
        assert_eq!(parse_time_value("12.34"), Some(12_340));
        assert_eq!(parse_time_value("12.34s"), Some(12_340));
        assert_eq!(parse_time_value("12000ms"), Some(12_000));
        assert_eq!(parse_time_value("0:00:12.340"), Some(12_340));
        assert_eq!(parse_time_value(""), None);
    }

    #[test]
    fn detects_formats() {
        assert_eq!(detect_format("[00:01.00]hi"), LyricsFormat::Lrc);
        assert_eq!(detect_format("<tt><body></body></tt>"), LyricsFormat::Ttml);
        assert_eq!(detect_format("just words"), LyricsFormat::Plain);
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(strip_tags("<b>a &amp; b</b>"), "a & b");
    }
}
