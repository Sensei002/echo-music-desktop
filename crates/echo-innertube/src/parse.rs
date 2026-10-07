//! Mappers from InnerTube JSON into the shared [`echo_core`] models.
//!
//! YouTube Music nests its data deeply and reshuffles the tree regularly, so
//! the parsers here are written defensively: they walk the tree looking for
//! known renderer names instead of hard-coding a single path, and they never
//! panic on a missing field.

use crate::util::{
    best_thumbnail, browse_id_of, is_separator, page_type_of, parse_clock, run_text, runs_of,
    text_of, video_id_of,
};
use echo_core::{Album, Artist, MediaItem, Playlist, Shelf, Song};
use serde_json::Value;

/// Intermediate representation shared by every renderer flavour.
#[derive(Debug, Default)]
struct ItemParts {
    title: String,
    artists: Vec<String>,
    artist_ids: Vec<String>,
    album: Option<String>,
    album_id: Option<String>,
    duration: Option<u32>,
    year: Option<i32>,
    song_count: Option<u32>,
    subscribers: Option<String>,
    thumbnail: Option<String>,
    is_explicit: bool,
    video_id: Option<String>,
    browse_id: Option<String>,
    page_type: Option<String>,
}

impl ItemParts {
    fn into_item(self) -> Option<MediaItem> {
        if self.title.trim().is_empty() {
            return None;
        }
        match self.page_type.as_deref() {
            Some("MUSIC_PAGE_TYPE_ALBUM") => Some(MediaItem::Album(Album {
                id: self.browse_id?,
                title: self.title,
                artists: self.artists,
                thumbnail: self.thumbnail,
                year: self.year,
                song_count: self.song_count,
                duration: None,
            })),
            Some("MUSIC_PAGE_TYPE_ARTIST") => Some(MediaItem::Artist(Artist {
                id: self.browse_id?,
                name: self.title,
                thumbnail: self.thumbnail,
                subscribers: self.subscribers,
                description: None,
            })),
            Some("MUSIC_PAGE_TYPE_PLAYLIST")
            | Some("MUSIC_PAGE_TYPE_USER_CHANNEL")
            | Some("MUSIC_PAGE_TYPE_PODCAST_SHOW_DETAIL_PAGE") => {
                Some(MediaItem::Playlist(Playlist {
                    id: self.browse_id?,
                    title: self.title,
                    description: None,
                    thumbnail: self.thumbnail,
                    author: self.artists.first().cloned(),
                    song_count: self.song_count,
                    is_local: false,
                }))
            }
            _ => {
                let id = self.video_id.or(self.browse_id)?;
                Some(MediaItem::Song(Song {
                    id,
                    title: self.title,
                    artists: if self.artists.is_empty() {
                        vec!["Unknown artist".into()]
                    } else {
                        self.artists
                    },
                    artist_ids: self.artist_ids,
                    album: self.album,
                    album_id: self.album_id,
                    duration: self.duration,
                    thumbnail: self.thumbnail,
                    set_video_id: None,
                    is_explicit: self.is_explicit,
                    is_video: self.page_type.as_deref() == Some("MUSIC_PAGE_TYPE_TRACK_VIDEO"),
                    play_count: None,
                    year: self.year,
                    radio_playlist_id: None,
                }))
            }
        }
    }
}

/// Reads the metadata carried by subtitle runs (`Artist • Album • 3:16`).
fn absorb_runs(parts: &mut ItemParts, runs: &[Value]) {
    for run in runs {
        let text = run_text(run).trim();
        if is_separator(text) {
            continue;
        }
        if let Some(seconds) = parse_clock(text) {
            parts.duration = Some(seconds);
            continue;
        }
        if text.len() == 4 && text.chars().all(|c| c.is_ascii_digit()) {
            parts.year = text.parse::<i32>().ok();
            continue;
        }
        let lower = text.to_lowercase();
        if lower.ends_with(" songs") || lower.ends_with(" song") {
            if let Some(count) = text
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u32>().ok())
            {
                parts.song_count = Some(count);
            }
            continue;
        }
        if lower.ends_with(" plays")
            || lower.ends_with(" views")
            || lower.ends_with("subscribers")
            || lower.ends_with(" monthly audience")
        {
            parts.subscribers = Some(text.to_string());
            continue;
        }

        match page_type_of(run).as_deref() {
            Some("MUSIC_PAGE_TYPE_ALBUM") => {
                parts.album = Some(text.to_string());
                parts.album_id = browse_id_of(run);
            }
            Some("MUSIC_PAGE_TYPE_ARTIST") => {
                parts.artists.push(text.to_string());
                if let Some(id) = browse_id_of(run) {
                    parts.artist_ids.push(id);
                }
            }
            _ => {
                if parts.artists.is_empty() && parts.album.is_none() {
                    parts.artists.push(text.to_string());
                } else if parts.album.is_none() {
                    parts.album = Some(text.to_string());
                }
            }
        }
    }
}

fn is_explicit(node: &Value) -> bool {
    node.get("badges")
        .map(|badges| badges.to_string().contains("EXPLICIT"))
        .unwrap_or(false)
}

/// Parses a `musicResponsiveListItemRenderer` (list rows in search/library).
pub fn parse_responsive_list_item(item: &Value) -> Option<MediaItem> {
    let flex = item.get("flexColumns").and_then(Value::as_array)?;
    let mut parts = ItemParts {
        thumbnail: best_thumbnail(item),
        is_explicit: is_explicit(item),
        video_id: video_id_of(item),
        browse_id: browse_id_of(item),
        page_type: page_type_of(item),
        ..Default::default()
    };

    if let Some(title_node) = flex
        .first()
        .and_then(|c| c.get("musicResponsiveListItemFlexColumnRenderer"))
        .and_then(|c| c.get("text"))
    {
        parts.title = text_of(title_node);
    }

    for column in flex.iter().skip(1) {
        if let Some(text) = column
            .get("musicResponsiveListItemFlexColumnRenderer")
            .and_then(|c| c.get("text"))
        {
            absorb_runs(&mut parts, runs_of(text));
        }
    }

    // Explicit rows sometimes carry the video id only in the play overlay.
    if parts.video_id.is_none() {
        parts.video_id = item
            .get("overlay")
            .and_then(|o| o.get("musicItemThumbnailOverlayRenderer"))
            .and_then(|o| o.get("content"))
            .and_then(|c| c.get("musicPlayButtonRenderer"))
            .and_then(|c| c.get("playNavigationEndpoint"))
            .and_then(|e| e.get("watchEndpoint"))
            .and_then(|e| e.get("videoId"))
            .and_then(Value::as_str)
            .map(str::to_string);
    }

    parts.into_item()
}

/// Parses a `musicTwoRowItemRenderer` (carousel / grid tiles).
pub fn parse_two_row_item(item: &Value) -> Option<MediaItem> {
    let mut parts = ItemParts {
        title: item.get("title").map(text_of).unwrap_or_default(),
        thumbnail: best_thumbnail(item),
        browse_id: browse_id_of(item),
        video_id: video_id_of(item),
        page_type: page_type_of(item),
        ..Default::default()
    };
    if let Some(subtitle) = item.get("subtitle") {
        absorb_runs(&mut parts, runs_of(subtitle));
    }
    parts.into_item()
}

/// Parses the header of a `musicCardShelfRenderer` (the "top result" tile).
fn parse_card_header(card: &Value) -> Option<MediaItem> {
    let mut parts = ItemParts {
        title: card.get("title").map(text_of).unwrap_or_default(),
        thumbnail: best_thumbnail(card),
        video_id: video_id_of(card.get("onTap").unwrap_or(&Value::Null)),
        browse_id: browse_id_of(card.get("onTap").unwrap_or(&Value::Null)),
        page_type: page_type_of(card.get("onTap").unwrap_or(&Value::Null)),
        ..Default::default()
    };
    if let Some(subtitle) = card.get("subtitle") {
        absorb_runs(&mut parts, runs_of(subtitle));
    }
    parts.into_item()
}

fn parse_items_from(contents: &[Value]) -> Vec<MediaItem> {
    contents
        .iter()
        .filter_map(|entry| {
            entry
                .get("musicResponsiveListItemRenderer")
                .and_then(parse_responsive_list_item)
                .or_else(|| {
                    entry
                        .get("musicTwoRowItemRenderer")
                        .and_then(parse_two_row_item)
                })
                .or_else(|| {
                    entry
                        .get("musicCardShelfRenderer")
                        .and_then(parse_card_header)
                })
        })
        .collect()
}

fn parse_shelf(shelf: &Value) -> Option<Shelf> {
    let contents = shelf.get("contents").and_then(Value::as_array)?;
    let items = parse_items_from(contents);
    if items.is_empty() {
        return None;
    }
    Some(Shelf {
        title: shelf.get("title").map(text_of).unwrap_or_default(),
        subtitle: None,
        browse_id: browse_id_of(shelf.get("bottomEndpoint").unwrap_or(&Value::Null))
            .or_else(|| browse_id_of(shelf.get("title").unwrap_or(&Value::Null))),
        items,
    })
}

fn parse_carousel(carousel: &Value) -> Option<Shelf> {
    let contents = carousel.get("contents").and_then(Value::as_array)?;
    let items = parse_items_from(contents);
    if items.is_empty() {
        return None;
    }
    let header = carousel
        .get("header")
        .and_then(|h| h.get("musicCarouselShelfBasicHeaderRenderer"));
    Some(Shelf {
        title: header
            .and_then(|h| h.get("title"))
            .map(text_of)
            .unwrap_or_default(),
        subtitle: header
            .and_then(|h| h.get("strapline"))
            .map(text_of)
            .filter(|s| !s.is_empty()),
        browse_id: header
            .and_then(|h| h.get("moreContentButton"))
            .and_then(browse_id_of),
        items,
    })
}

fn parse_card_shelf(card: &Value) -> Option<Shelf> {
    let title = card
        .get("header")
        .and_then(|h| h.get("musicCardShelfHeaderBasicRenderer"))
        .and_then(|h| h.get("title"))
        .map(text_of)
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "Top result".to_string());

    let mut items = Vec::new();
    if let Some(header_item) = parse_card_header(card) {
        items.push(header_item);
    }
    if let Some(contents) = card.get("contents").and_then(Value::as_array) {
        items.extend(parse_items_from(contents));
    }
    if items.is_empty() {
        return None;
    }
    Some(Shelf {
        title,
        subtitle: None,
        browse_id: None,
        items,
    })
}

/// Walks an arbitrary InnerTube response and collects every shelf in order.
pub fn collect_shelves(value: &Value) -> Vec<Shelf> {
    let mut shelves = Vec::new();
    walk(value, &mut shelves);
    shelves
}

fn walk(value: &Value, out: &mut Vec<Shelf>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                match key.as_str() {
                    "musicCarouselShelfRenderer" => {
                        if let Some(shelf) = parse_carousel(child) {
                            out.push(shelf);
                        }
                        continue;
                    }
                    "musicShelfRenderer" => {
                        if let Some(shelf) = parse_shelf(child) {
                            out.push(shelf);
                        }
                        continue;
                    }
                    "musicCardShelfRenderer" => {
                        if let Some(shelf) = parse_card_shelf(child) {
                            out.push(shelf);
                        }
                        continue;
                    }
                    _ => {}
                }
                walk(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, out);
            }
        }
        _ => {}
    }
}

/// Collects the labels of every mood/genre chip in the response.
pub fn collect_chips(value: &Value) -> Vec<String> {
    let mut chips = Vec::new();
    collect_chips_into(value, &mut chips);
    chips.dedup();
    chips
}

fn collect_chips_into(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "chipCloudChipRenderer" {
                    let text = child.get("text").map(text_of).unwrap_or_default();
                    let text = text.trim();
                    if !text.is_empty() && !out.iter().any(|c| c == text) {
                        out.push(text.to_string());
                    }
                    continue;
                }
                collect_chips_into(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_chips_into(item, out);
            }
        }
        _ => {}
    }
}

/// Parses a detail header (`musicDetailHeaderRenderer` and friends) into a
/// model, forcing the page type when the header omits one.
pub fn parse_titled_item(
    node: &Value,
    page_type: Option<&str>,
    browse_id: Option<&str>,
) -> Option<MediaItem> {
    let mut parts = ItemParts {
        title: node.get("title").map(text_of).unwrap_or_default(),
        thumbnail: best_thumbnail(node),
        browse_id: browse_id.map(str::to_string).or_else(|| browse_id_of(node)),
        page_type: page_type.map(str::to_string).or_else(|| page_type_of(node)),
        ..Default::default()
    };
    if let Some(subtitle) = node.get("subtitle") {
        absorb_runs(&mut parts, runs_of(subtitle));
    }
    parts.into_item()
}

/// Extracts the search-suggestion strings from a suggestions response.
pub fn collect_suggestions(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_suggestions_into(value, &mut out);
    out
}

fn collect_suggestions_into(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "searchSuggestionRenderer" || key == "musicSuggestionRenderer" {
                    let text = child
                        .get("suggestion")
                        .or_else(|| {
                            child
                                .get("navigationEndpoint")
                                .and_then(|_| child.get("suggestion"))
                        })
                        .map(text_of)
                        .unwrap_or_default();
                    let text = text.trim();
                    if !text.is_empty() && !out.iter().any(|s| s == text) {
                        out.push(text.to_string());
                    }
                    continue;
                }
                collect_suggestions_into(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_suggestions_into(item, out);
            }
        }
        _ => {}
    }
}

/// Flattens every shelf item into a single list.
pub fn flatten_items(shelves: &[Shelf]) -> Vec<MediaItem> {
    shelves
        .iter()
        .flat_map(|shelf| shelf.items.iter().cloned())
        .collect()
}

/// Parses a `playlistPanelVideoRenderer` (an up-next / queue entry).
pub fn parse_queue_item(item: &Value) -> Option<Song> {
    let title = item.get("title").map(text_of).unwrap_or_default();
    if title.trim().is_empty() {
        return None;
    }
    let video_id = item
        .get("videoId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| video_id_of(item))?;

    let mut artists = Vec::new();
    let mut artist_ids = Vec::new();
    for key in ["longBylineText", "shortBylineText"] {
        if let Some(node) = item.get(key) {
            for run in runs_of(node) {
                let text = run_text(run).trim();
                if is_separator(text) {
                    continue;
                }
                artists.push(text.to_string());
                if let Some(id) = browse_id_of(run) {
                    artist_ids.push(id);
                }
            }
            if !artists.is_empty() {
                break;
            }
        }
    }

    Some(Song {
        id: video_id,
        title,
        artists,
        artist_ids,
        album: None,
        album_id: None,
        duration: item
            .get("lengthText")
            .map(text_of)
            .and_then(|t| parse_clock(&t)),
        thumbnail: best_thumbnail(item),
        set_video_id: None,
        is_explicit: is_explicit(item),
        is_video: false,
        play_count: None,
        year: None,
        radio_playlist_id: None,
    })
}

/// Recursively collects every queue entry present in a `next` response.
pub fn collect_queue(value: &Value) -> Vec<Song> {
    let mut out = Vec::new();
    collect_queue_into(value, &mut out);
    out
}

fn collect_queue_into(value: &Value, out: &mut Vec<Song>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "playlistPanelVideoRenderer" {
                    if let Some(song) = parse_queue_item(child) {
                        out.push(song);
                    }
                    continue;
                }
                collect_queue_into(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_queue_into(item, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_song_row() {
        let row = json!({
            "flexColumns": [
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Khalasi"}]}}},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                    {"text": "Aditya Gadhvi", "navigationEndpoint": {"browseEndpoint": {"browseId": "UC1",
                        "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ARTIST"}}}}},
                    {"text": " • "},
                    {"text": "Coke Studio", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPRE1",
                        "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ALBUM"}}}}},
                    {"text": " • "},
                    {"text": "3:16"}
                ]}}}
            ],
            "playlistItemData": {"videoId": "abc123"},
            "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "https://img/low.jpg"}, {"url": "https://img/high.jpg"}]}}}
        });
        let item = parse_responsive_list_item(&row).expect("row should parse");
        match item {
            MediaItem::Song(song) => {
                assert_eq!(song.id, "abc123");
                assert_eq!(song.title, "Khalasi");
                assert_eq!(song.artists, vec!["Aditya Gadhvi".to_string()]);
                assert_eq!(song.album.as_deref(), Some("Coke Studio"));
                assert_eq!(song.duration, Some(196));
                assert_eq!(song.thumbnail.as_deref(), Some("https://img/high.jpg"));
            }
            other => panic!("expected a song, got {other:?}"),
        }
    }

    #[test]
    fn parses_an_album_tile() {
        let tile = json!({
            "title": {"runs": [{"text": "Echoes"}]},
            "subtitle": {"runs": [{"text": "Pink Floyd"}, {"text": " • "}, {"text": "1971"}]},
            "thumbnailRenderer": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "https://img/a.jpg"}]}}},
            "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_1",
                "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ALBUM"}}}}
        });
        let item = parse_two_row_item(&tile).expect("tile should parse");
        match item {
            MediaItem::Album(album) => {
                assert_eq!(album.id, "MPREb_1");
                assert_eq!(album.year, Some(1971));
                assert_eq!(album.artists, vec!["Pink Floyd".to_string()]);
            }
            other => panic!("expected an album, got {other:?}"),
        }
    }

    #[test]
    fn collects_shelves_and_chips() {
        let response = json!({
            "contents": {
                "singleColumnBrowseResultsRenderer": {
                    "tabs": [{
                        "tabRenderer": {
                            "content": {
                                "sectionListRenderer": {
                                    "contents": [
                                        {"musicCarouselShelfRenderer": {
                                            "header": {"musicCarouselShelfBasicHeaderRenderer": {"title": {"runs": [{"text": "Trending"}]}}},
                                            "contents": [{"musicTwoRowItemRenderer": {
                                                "title": {"runs": [{"text": "Song A"}]},
                                                "navigationEndpoint": {"watchEndpoint": {"videoId": "v1"}}
                                            }}]
                                        }},
                                        {"chipCloudRenderer": {"chips": [
                                            {"chipCloudChipRenderer": {"text": {"runs": [{"text": "Feel good"}]}}}
                                        ]}}
                                    ]
                                }
                            }
                        }
                    }]
                }
            }
        });
        let shelves = collect_shelves(&response);
        assert_eq!(shelves.len(), 1);
        assert_eq!(shelves[0].title, "Trending");
        assert_eq!(shelves[0].items.len(), 1);
        assert_eq!(collect_chips(&response), vec!["Feel good".to_string()]);
    }

    #[test]
    fn missing_fields_do_not_panic() {
        assert!(parse_responsive_list_item(&json!({})).is_none());
        assert!(parse_two_row_item(&json!({"title": {"runs": []}})).is_none());
        assert!(parse_shelf(&json!({})).is_none());
    }
}
