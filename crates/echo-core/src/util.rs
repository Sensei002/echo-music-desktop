//! Small, dependency-light helpers shared across the app.

/// Formats a duration in seconds as `m:ss` (or `h:mm:ss` for long content).
pub fn format_duration(seconds: u32) -> String {
    let h = seconds / 3600;
    let m = (seconds % 3600) / 60;
    let s = seconds % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Formats a millisecond position as `m:ss`.
pub fn format_position_ms(ms: u64) -> String {
    format_duration((ms / 1000) as u32)
}

/// Parses `"3:16"`, `"1:02:44"` or `"195"` into seconds.
pub fn parse_duration(input: &str) -> Option<u32> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if let Ok(plain) = input.parse::<u32>() {
        return Some(plain);
    }
    let mut total = 0u32;
    for part in input.split(':') {
        let value: u32 = part.trim().parse().ok()?;
        total = total * 60 + value;
    }
    Some(total)
}

/// Extracts a YouTube thumbnail URL for a given image id and desired width.
pub fn thumbnail_url(image_id: &str, size: u32) -> String {
    format!("https://i.ytimg.com/vi/{image_id}/hqdefault.jpg?w={size}")
}

/// Extracts a YouTube Music playlist/album artwork URL.
pub fn artwork_url(image_id: &str, size: u32) -> String {
    if image_id.starts_with("http") {
        return image_id.to_string();
    }
    let base = if image_id.starts_with("MPREb_") || image_id.starts_with("MPLAU") {
        "https://i.ytimg.com/vi"
    } else {
        "https://lh3.googleusercontent.com"
    };
    format!("{base}/{image_id}=w{size}-h{size}-l90-rj")
}

/// Truncates a string to `max` characters, appending an ellipsis when cut.
pub fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Normalises a human string for fuzzy lyrics matching (lowercase, alphanumeric).
pub fn normalize_key(input: &str) -> String {
    input
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Current unix timestamp in milliseconds.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Current unix timestamp in seconds.
pub fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_parses_durations() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(65), "1:05");
        assert_eq!(format_duration(3725), "1:02:05");
        assert_eq!(parse_duration("3:16"), Some(196));
        assert_eq!(parse_duration("1:02:44"), Some(3764));
        assert_eq!(parse_duration("195"), Some(195));
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
    }

    #[test]
    fn normalises_keys() {
        assert_eq!(normalize_key("  Hey,   JUDE! "), "hey jude");
    }
}
