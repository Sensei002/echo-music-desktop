//! Small parsing helpers shared by the response mappers.

/// Truncates a string for log output, appending an ellipsis when cut.
pub fn snippet(input: &str, max: usize) -> String {
    let cleaned = input.replace(['\n', '\r'], " ");
    if cleaned.chars().count() <= max {
        return cleaned;
    }
    let mut out: String = cleaned.chars().take(max).collect();
    out.push('…');
    out
}

/// Extracts the plain text from a `runs`/`simpleText` node.
pub fn text_of(value: &serde_json::Value) -> String {
    if let Some(text) = value.get("simpleText").and_then(|v| v.as_str()) {
        return text.to_string();
    }
    if let Some(runs) = value.get("runs").and_then(|v| v.as_array()) {
        return runs
            .iter()
            .filter_map(|run| run.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

/// Returns the `runs` array of a text node, or an empty slice.
pub fn runs_of(value: &serde_json::Value) -> &[serde_json::Value] {
    value
        .get("runs")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Reads the `text` field of a single run.
pub fn run_text(run: &serde_json::Value) -> &str {
    run.get("text").and_then(|v| v.as_str()).unwrap_or("")
}

/// Pulls the first `browseId` found in an endpoint-like node.
pub fn browse_id_of(value: &serde_json::Value) -> Option<String> {
    value
        .get("browseEndpoint")
        .and_then(|v| v.get("browseId"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            value
                .get("navigationEndpoint")
                .and_then(|v| v.get("browseEndpoint"))
                .and_then(|v| v.get("browseId"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
}

/// Pulls the first `videoId` found in an endpoint-like node.
pub fn video_id_of(value: &serde_json::Value) -> Option<String> {
    value
        .get("watchEndpoint")
        .and_then(|v| v.get("videoId"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            value
                .get("playlistItemData")
                .and_then(|v| v.get("videoId"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .or_else(|| {
            value
                .get("navigationEndpoint")
                .and_then(|v| v.get("watchEndpoint"))
                .and_then(|v| v.get("videoId"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
}

/// Reads the `MUSIC_PAGE_TYPE_*` marker from a run or endpoint.
pub fn page_type_of(value: &serde_json::Value) -> Option<String> {
    let endpoint = value
        .get("navigationEndpoint")
        .or_else(|| value.get("browseEndpoint").map(|_| value))
        .unwrap_or(value);
    endpoint
        .get("browseEndpoint")
        .and_then(|b| b.get("browseEndpointContextSupportedConfigs"))
        .and_then(|c| c.get("browseEndpointContextMusicConfig"))
        .and_then(|c| c.get("pageType"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Collects every thumbnail URL inside a renderer, largest last.
pub fn thumbnails_of(value: &serde_json::Value) -> Vec<String> {
    let mut urls = Vec::new();
    let candidates = [
        ["thumbnail", "musicThumbnailRenderer", "thumbnail", "thumbnails"],
        ["thumbnailRenderer", "musicThumbnailRenderer", "thumbnail", "thumbnails"],
        ["thumbnail", "thumbnails"],
    ];
    for path in candidates {
        if let Some(list) = value
            .get(path[0])
            .and_then(|v| v.get(path[1]))
            .and_then(|v| v.get(path[2]))
            .and_then(|v| v.get(path[3]))
            .and_then(|v| v.as_array())
        {
            for entry in list {
                if let Some(url) = entry.get("url").and_then(|v| v.as_str()) {
                    urls.push(url.to_string());
                }
            }
            if !urls.is_empty() {
                return urls;
            }
        }
    }
    // `musicTwoRowItemRenderer` nests one level deeper via `thumbnailRenderer`.
    if let Some(list) = value
        .get("thumbnailRenderer")
        .and_then(|v| v.get("musicThumbnailRenderer"))
        .and_then(|v| v.get("thumbnail"))
        .and_then(|v| v.get("thumbnails"))
        .and_then(|v| v.as_array())
    {
        for entry in list {
            if let Some(url) = entry.get("url").and_then(|v| v.as_str()) {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

/// Chooses the highest-resolution thumbnail available.
pub fn best_thumbnail(value: &serde_json::Value) -> Option<String> {
    let urls = thumbnails_of(value);
    urls.last().cloned().or_else(|| urls.first().cloned())
}

/// Parses `"3:16"` / `"1:02:44"` durations found in subtitle runs.
pub fn parse_clock(input: &str) -> Option<u32> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !trimmed.contains(':') {
        return None;
    }
    let mut total = 0u32;
    for part in trimmed.split(':') {
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        total = total.checked_mul(60)? + part.parse::<u32>().ok()?;
    }
    Some(total)
}

/// Detects the run separators YouTube Music inserts between subtitle fields.
pub fn is_separator(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty() || trimmed == "•" || trimmed == "·"
}
