//! High-level YouTube Music endpoints built on the InnerTube transport.

use crate::clients::WEB_REMIX;
use crate::http::InnerTube;
use crate::parse;
use crate::util::{run_text, runs_of, text_of};
use anyhow::Result;
use echo_core::{
    Album, AlbumPage, Artist, ArtistPage, HomePage, MediaItem, Playlist, PlaylistPage,
    SearchResults, Shelf, Song,
};
use serde_json::{json, Value};

/// The browse id of the personalised home feed.
pub const BROWSE_HOME: &str = "FEmusic_home";
/// The browse id of the explore page.
pub const BROWSE_EXPLORE: &str = "FEmusic_explore";
/// The browse id of the charts page.
pub const BROWSE_CHARTS: &str = "FEmusic_charts";
/// The browse id of the moods & genres page.
pub const BROWSE_MOODS: &str = "FEmusic_moods_and_genres";
/// The browse id of the new releases page.
pub const BROWSE_NEW_RELEASES: &str = "FEmusic_new_releases";
/// The browse id of the liked-music auto playlist.
pub const BROWSE_LIKED: &str = "FEmusic_liked_playlists";

/// Restricts a search to a single result type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchFilter {
    /// Mixed results across every type.
    #[default]
    All,
    Songs,
    Videos,
    Albums,
    Artists,
    Playlists,
}

impl SearchFilter {
    /// The protobuf `params` blob YouTube Music expects for this filter.
    fn params(self) -> Option<&'static str> {
        match self {
            SearchFilter::All => None,
            SearchFilter::Songs => Some("EgWKAQIIAWoKEAkQBRAKEAMQBA=="),
            SearchFilter::Videos => Some("EgWKAQIQAWoKEAkQChAFEAMQBA=="),
            SearchFilter::Albums => Some("EgWKAQIYAWoKEAkQChAFEAMQBA=="),
            SearchFilter::Artists => Some("EgWKAQIgAWoKEAkQChAFEAMQBA=="),
            SearchFilter::Playlists => Some("EgWKAQIoAWoKEAkQChAFEAMQBA=="),
        }
    }
}

/// Issues a `browse` request for a browse id.
pub fn browse(it: &InnerTube, browse_id: &str) -> Result<Value> {
    let body = it.body(&WEB_REMIX, json!({ "browseId": browse_id }));
    it.post("browse", &WEB_REMIX, &body)
}

/// The personalised home feed.
pub fn home(it: &InnerTube) -> Result<HomePage> {
    let response = browse(it, BROWSE_HOME)?;
    Ok(HomePage {
        chips: parse::collect_chips(&response),
        shelves: parse::collect_shelves(&response),
    })
}

/// The explore page (new releases, moods, charts teasers).
pub fn explore(it: &InnerTube) -> Result<HomePage> {
    let response = browse(it, BROWSE_EXPLORE)?;
    Ok(HomePage {
        chips: parse::collect_chips(&response),
        shelves: parse::collect_shelves(&response),
    })
}

/// The charts page.
pub fn charts(it: &InnerTube) -> Result<HomePage> {
    let response = browse(it, BROWSE_CHARTS)?;
    Ok(HomePage {
        chips: parse::collect_chips(&response),
        shelves: parse::collect_shelves(&response),
    })
}

/// Moods & genres.
pub fn moods_and_genres(it: &InnerTube) -> Result<Vec<Shelf>> {
    let response = browse(it, BROWSE_MOODS)?;
    Ok(parse::collect_shelves(&response))
}

/// Performs a search and groups the results by type.
pub fn search(it: &InnerTube, query: &str, filter: SearchFilter) -> Result<SearchResults> {
    let mut extra = json!({ "query": query });
    if let Some(params) = filter.params() {
        extra["params"] = json!(params);
    }
    let body = it.body(&WEB_REMIX, extra);
    let response = it.post("search", &WEB_REMIX, &body)?;
    Ok(group_search(&response))
}

/// Type-ahead suggestions for the search box.
pub fn search_suggestions(it: &InnerTube, query: &str) -> Result<Vec<String>> {
    let body = it.body(&WEB_REMIX, json!({ "input": query }));
    let response = it.post("music/get_search_suggestions", &WEB_REMIX, &body)?;
    Ok(parse::collect_suggestions(&response))
}

/// Loads a full album page.
pub fn album(it: &InnerTube, browse_id: &str) -> Result<AlbumPage> {
    let response = browse(it, browse_id)?;
    let shelves = parse::collect_shelves(&response);

    let album = detail_header(&response)
        .and_then(|header| {
            parse::parse_titled_item(header, Some("MUSIC_PAGE_TYPE_ALBUM"), Some(browse_id))
        })
        .and_then(|item| match item {
            MediaItem::Album(album) => Some(album),
            _ => None,
        })
        .unwrap_or_else(|| Album {
            id: browse_id.to_string(),
            ..Default::default()
        });

    let songs = songs_in(&shelves);
    let other_versions = shelves
        .iter()
        .filter(|shelf| {
            let title = shelf.title.to_lowercase();
            title.contains("version") || title.contains("more")
        })
        .flat_map(|shelf| shelf.items.iter())
        .filter_map(|item| match item {
            MediaItem::Album(album) => Some(album.clone()),
            _ => None,
        })
        .collect();

    Ok(AlbumPage {
        album,
        songs,
        other_versions,
    })
}

/// Loads a full artist page.
pub fn artist(it: &InnerTube, browse_id: &str) -> Result<ArtistPage> {
    let response = browse(it, browse_id)?;
    let artist = detail_header(&response)
        .and_then(|header| {
            parse::parse_titled_item(header, Some("MUSIC_PAGE_TYPE_ARTIST"), Some(browse_id))
        })
        .and_then(|item| match item {
            MediaItem::Artist(artist) => Some(artist),
            _ => None,
        })
        .unwrap_or_else(|| Artist {
            id: browse_id.to_string(),
            name: "Unknown artist".into(),
            ..Default::default()
        });

    Ok(ArtistPage {
        artist,
        sections: parse::collect_shelves(&response),
    })
}

/// Loads a full playlist page.
pub fn playlist(it: &InnerTube, playlist_id: &str) -> Result<PlaylistPage> {
    let browse_id = normalise_playlist_id(playlist_id);
    let response = browse(it, &browse_id)?;
    let shelves = parse::collect_shelves(&response);

    let playlist = detail_header(&response)
        .and_then(|header| {
            parse::parse_titled_item(header, Some("MUSIC_PAGE_TYPE_PLAYLIST"), Some(playlist_id))
        })
        .and_then(|item| match item {
            MediaItem::Playlist(playlist) => Some(playlist),
            _ => None,
        })
        .unwrap_or_else(|| Playlist {
            id: playlist_id.to_string(),
            title: "Playlist".into(),
            is_local: false,
            ..Default::default()
        });

    Ok(PlaylistPage {
        playlist,
        songs: songs_in(&shelves),
    })
}

/// Resolves the lyrics document for a track, when one exists.
pub fn lyrics(it: &InnerTube, video_id: &str) -> Result<Option<String>> {
    // 1. Ask `next` for the lyrics browse endpoint of this track.
    let body = it.body(
        &WEB_REMIX,
        json!({ "videoId": video_id, "isAudioOnly": true }),
    );
    let next = it.post("next", &WEB_REMIX, &body)?;
    let Some(lyrics_id) = find_lyrics_browse_id(&next) else {
        return Ok(None);
    };

    // 2. Load the lyrics document and flatten its description shelf.
    let response = browse(it, &lyrics_id)?;
    Ok(extract_description_text(&response))
}

/// Normalises a playlist id into its browsable form.
pub fn normalise_playlist_id(id: &str) -> String {
    if id.starts_with("VL") || id.starts_with("RD") || id.starts_with("MPRE") {
        id.to_string()
    } else {
        format!("VL{id}")
    }
}

/// Builds the radio/automix playlist id used to keep playing related tracks.
pub fn radio_playlist_id(video_id: &str) -> String {
    format!("RDAMVM{video_id}")
}

fn group_search(response: &Value) -> SearchResults {
    let mut results = SearchResults::default();
    for shelf in parse::collect_shelves(response) {
        let is_video_shelf = shelf.title.to_lowercase().contains("video");
        for item in shelf.items {
            match item {
                MediaItem::Song(mut song) => {
                    if is_video_shelf || song.is_video {
                        song.is_video = true;
                        results.videos.push(song);
                    } else {
                        results.songs.push(song);
                    }
                }
                MediaItem::Album(album) => results.albums.push(album),
                MediaItem::Artist(artist) => results.artists.push(artist),
                MediaItem::Playlist(playlist) => results.playlists.push(playlist),
            }
        }
    }
    results
}

fn songs_in(shelves: &[Shelf]) -> Vec<Song> {
    let mut songs = Vec::new();
    for shelf in shelves {
        for item in &shelf.items {
            if let MediaItem::Song(song) = item {
                if !songs.iter().any(|existing: &Song| existing.id == song.id) {
                    songs.push(song.clone());
                }
            }
        }
    }
    songs
}

fn detail_header(response: &Value) -> Option<&Value> {
    for path in [
        "/header/musicDetailHeaderRenderer",
        "/header/musicImmersiveHeaderRenderer",
        "/header/musicVisualHeaderRenderer",
    ] {
        if let Some(header) = response.pointer(path) {
            return Some(header);
        }
    }
    None
}

/// Recursively finds a `browseId` that looks like a lyrics document.
fn find_lyrics_browse_id(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(id) = map
                .get("browseEndpoint")
                .and_then(|b| b.get("browseId"))
                .and_then(Value::as_str)
            {
                if id.starts_with("MPLYt") {
                    return Some(id.to_string());
                }
            }
            for (key, child) in map {
                if key == "browseEndpoint" {
                    continue;
                }
                if let Some(found) = find_lyrics_browse_id(child) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(find_lyrics_browse_id),
        _ => None,
    }
}

/// Joins every `musicDescriptionShelfRenderer` body into one string.
fn extract_description_text(value: &Value) -> Option<String> {
    let mut chunks = Vec::new();
    collect_descriptions(value, &mut chunks);
    let joined = chunks.join("\n").trim().to_string();
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

fn collect_descriptions(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "musicDescriptionShelfRenderer" {
                    if let Some(description) = child.get("description") {
                        let text = text_of(description);
                        if !text.trim().is_empty() {
                            out.push(text);
                        }
                    }
                    continue;
                }
                collect_descriptions(child, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_descriptions(item, out);
            }
        }
        _ => {}
    }
}

/// Extracts the first subtitle line of a header (used for artist descriptions).
pub fn header_subtitle(header: &Value) -> String {
    header
        .get("subtitle")
        .map(|s| {
            runs_of(s)
                .iter()
                .map(|run| run_text(run).trim())
                .filter(|t| !t.is_empty() && *t != "•")
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_playlist_ids() {
        assert_eq!(normalise_playlist_id("PL123"), "VLPL123");
        assert_eq!(normalise_playlist_id("VLPL123"), "VLPL123");
        assert_eq!(normalise_playlist_id("RDCLAK5uy"), "RDCLAK5uy");
    }

    #[test]
    fn search_filters_have_params() {
        assert!(SearchFilter::All.params().is_none());
        assert!(SearchFilter::Songs.params().is_some());
        assert!(SearchFilter::Playlists.params().is_some());
    }

    #[test]
    fn groups_search_results_by_type() {
        let response = json!({
            "contents": {"tabbedSearchResultsRenderer": {"tabs": [{"tabRenderer": {"content": {
                "sectionListRenderer": {"contents": [
                    {"musicShelfRenderer": {
                        "title": {"runs": [{"text": "Songs"}]},
                        "contents": [{"musicResponsiveListItemRenderer": {
                            "flexColumns": [
                                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Track"}]}}},
                                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Artist"}]}}}
                            ],
                            "playlistItemData": {"videoId": "v1"}
                        }}]
                    }},
                    {"musicShelfRenderer": {
                        "title": {"runs": [{"text": "Videos"}]},
                        "contents": [{"musicResponsiveListItemRenderer": {
                            "flexColumns": [
                                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Clip"}]}}}
                            ],
                            "playlistItemData": {"videoId": "v2"}
                        }}]
                    }}
                ]}
            }}}]}}
        });
        let results = group_search(&response);
        assert_eq!(results.songs.len(), 1);
        assert_eq!(results.songs[0].id, "v1");
        assert_eq!(results.videos.len(), 1);
        assert!(results.videos[0].is_video);
    }

    #[test]
    fn finds_lyrics_browse_id() {
        let next = json!({
            "contents": {"a": {"b": {"browseEndpoint": {"browseId": "MPLYt_ABC"}}}}
        });
        assert_eq!(find_lyrics_browse_id(&next).as_deref(), Some("MPLYt_ABC"));
        assert!(find_lyrics_browse_id(&json!({})).is_none());
    }

    #[test]
    fn extracts_description_shelf() {
        let response = json!({
            "contents": {"musicDescriptionShelfRenderer": {
                "description": {"runs": [{"text": "Line one"}, {"text": "\nLine two"}]}
            }}
        });
        assert_eq!(
            extract_description_text(&response).as_deref(),
            Some("Line one\nLine two")
        );
    }
}
