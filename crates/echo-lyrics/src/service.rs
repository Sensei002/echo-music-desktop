//! Lyrics orchestration: provider ordering, caching and translation hooks.

use crate::model::Lyrics;
use crate::providers::{Provider, RawLyrics};
use crate::parse::parse_lyrics;
use anyhow::Result;
use lru::LruCache;
use parking_lot::Mutex;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;

/// Everything the providers need to look a track up.
#[derive(Debug, Clone, Default)]
pub struct LyricsQuery {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_secs: Option<u32>,
}

impl LyricsQuery {
    /// Stable cache key for this query.
    pub fn cache_key(&self) -> String {
        format!(
            "{}|{}|{}",
            self.video_id,
            self.title.to_lowercase(),
            self.artist.to_lowercase()
        )
    }
}

/// Fetches lyrics from a prioritised list of providers.
pub struct LyricsService {
    client: reqwest::blocking::Client,
    order: Mutex<Vec<Provider>>,
    cache: Arc<Mutex<LruCache<String, Option<Lyrics>>>>,
}

impl LyricsService {
    /// Builds a service from the provider ids stored in settings.
    pub fn new(provider_order: &[String]) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(12))
            .connect_timeout(Duration::from_secs(6))
            .user_agent(concat!("EchoMusicDesktop/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());

        let service = Self {
            client,
            order: Mutex::new(Vec::new()),
            cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(256).expect("non-zero"),
            ))),
        };
        service.set_order(provider_order);
        service
    }

    /// The active provider order.
    pub fn order(&self) -> Vec<Provider> {
        self.order.lock().clone()
    }

    /// Replaces the provider order, falling back to the default when the given
    /// ids are all unknown. Cached lookups are kept.
    pub fn set_order(&self, provider_order: &[String]) {
        let mut order: Vec<Provider> = provider_order
            .iter()
            .filter_map(|id| Provider::from_id(id))
            .collect();
        if order.is_empty() {
            order = Provider::all().to_vec();
        }
        *self.order.lock() = order;
    }

    /// Looks up lyrics, trying each provider in order.
    ///
    /// Results (including misses) are cached per query, so scrolling the lyrics
    /// view never re-hits the network.
    pub fn fetch(&self, query: &LyricsQuery) -> Result<Option<Lyrics>> {
        let key = query.cache_key();
        if let Some(cached) = self.cache.lock().get(&key).cloned() {
            return Ok(cached);
        }

        let mut result = None;
        let order = self.order.lock().clone();
        for provider in &order {
            match provider.fetch(&self.client, query) {
                Ok(Some(raw)) => {
                    let lyrics = materialise(raw, provider.id());
                    if !lyrics.is_empty() {
                        log::debug!("lyrics for `{}` from {}", query.title, provider.display_name());
                        result = Some(lyrics);
                        break;
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    log::debug!("{} lyrics failed: {err:#}", provider.display_name());
                }
            }
        }

        self.cache.lock().put(key, result.clone());
        Ok(result)
    }

    /// Fetches from one specific provider (used by the "source" switcher).
    pub fn fetch_from(
        &self,
        provider: Provider,
        query: &LyricsQuery,
    ) -> Result<Option<Lyrics>> {
        Ok(provider
            .fetch(&self.client, query)?
            .map(|raw| materialise(raw, provider.id()))
            .filter(|lyrics| !lyrics.is_empty()))
    }

    /// Drops every cached lookup.
    pub fn clear_cache(&self) {
        self.cache.lock().clear();
    }
}

/// Turns a provider payload into a [`Lyrics`] document.
fn materialise(raw: RawLyrics, provider: &str) -> Lyrics {
    match raw.lines {
        Some(lines) => Lyrics::new(lines, provider),
        None => parse_lyrics(&raw.text, raw.format, provider),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_default_order_from_unknown_ids() {
        let service = LyricsService::new(&["nope".to_string()]);
        assert_eq!(service.order().len(), Provider::all().len());
    }

    #[test]
    fn honours_a_configured_order() {
        let service = LyricsService::new(&["kugou".to_string(), "lrclib".to_string()]);
        assert_eq!(service.order(), vec![Provider::Kugou, Provider::Lrclib]);
    }

    #[test]
    fn order_can_be_changed_at_runtime() {
        let service = LyricsService::new(&[]);
        service.set_order(&["paxsenix".to_string(), "lrclib".to_string()]);
        assert_eq!(service.order(), vec![Provider::Paxsenix, Provider::Lrclib]);
        // Unknown ids fall back to the default order.
        service.set_order(&["bogus".to_string()]);
        assert_eq!(service.order().len(), Provider::all().len());
    }

    #[test]
    fn cache_keys_are_stable_and_case_insensitive() {
        let a = LyricsQuery {
            video_id: "v1".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            ..Default::default()
        };
        let b = LyricsQuery {
            title: "song".into(),
            artist: "artist".into(),
            ..a.clone()
        };
        assert_eq!(a.cache_key(), b.cache_key());
    }
}
