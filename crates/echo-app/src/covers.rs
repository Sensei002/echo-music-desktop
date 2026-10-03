//! Cover-art download and on-disk cache.
//!
//! Slint images can only be constructed on the UI thread, so workers download
//! artwork to a cache directory and the UI thread turns the files into
//! `slint::Image` values. Each URL maps to a deterministic file name, which
//! makes the cache stable across restarts.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// FNV-1a — a tiny, dependency-free stable hash for cache file names.
fn hash_url(url: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in url.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The cache file that holds `url`'s bytes.
pub fn cache_path(dir: &Path, url: &str) -> PathBuf {
    dir.join(format!("{:016x}.img", hash_url(url)))
}

/// Downloads `url` into the cache unless it is already there.
///
/// Returns the cache path so the caller can load it on the UI thread.
pub fn ensure_cached(
    client: &reqwest::blocking::Client,
    dir: &Path,
    url: &str,
) -> Result<PathBuf> {
    let path = cache_path(dir, url);
    if path.exists() {
        return Ok(path);
    }

    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create {}", dir.display()))?;

    let response = client
        .get(url)
        .send()
        .with_context(|| format!("failed to download artwork `{url}`"))?;
    if !response.status().is_success() {
        anyhow::bail!("artwork request returned HTTP {}", response.status());
    }
    let bytes = response.bytes().context("failed to read the artwork body")?;

    // Write to a temporary name first so a partial download never looks cached.
    let temp = path.with_extension("part");
    std::fs::write(&temp, &bytes)
        .with_context(|| format!("failed to write {}", temp.display()))?;
    std::fs::rename(&temp, &path)
        .with_context(|| format!("failed to finalise {}", path.display()))?;
    Ok(path)
}

/// Removes every cached file (used by "clear cache" in settings).
pub fn clear(dir: &Path) -> Result<u64> {
    let mut removed = 0u64;
    if !dir.exists() {
        return Ok(0);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.path().is_file() {
            std::fs::remove_file(entry.path()).ok();
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_paths_are_deterministic_and_distinct() {
        let dir = Path::new("/tmp/covers");
        let a = cache_path(dir, "https://example.com/a.jpg");
        let b = cache_path(dir, "https://example.com/a.jpg");
        let c = cache_path(dir, "https://example.com/b.jpg");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.to_string_lossy().ends_with(".img"));
    }
}
