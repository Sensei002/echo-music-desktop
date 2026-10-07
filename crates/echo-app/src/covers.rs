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

/// Sniffs the leading bytes of an image so the cache file can carry the right
/// extension.
///
/// This matters because Slint decodes from a path with `image::open`, which
/// infers the format **from the file extension**. A perfectly valid PNG stored
/// as `.img` is rejected outright:
///
/// ```text
/// Error loading image from ...\artwork\<hash>.img:
/// The file extension `."img"` was not recognized as an image format
/// ```
///
/// So the extension has to reflect what the bytes actually are, not what the
/// server claimed in `Content-Type`. YouTube serves WebP to clients that
/// advertise support and JPEG otherwise, so we cannot assume either.
fn sniff_extension(bytes: &[u8]) -> Option<&'static str> {
    // JPEG: FF D8 FF
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("jpg");
    }
    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("png");
    }
    // GIF87a / GIF89a
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    // RIFF....WEBP
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    // BMP: "BM"
    if bytes.starts_with(b"BM") {
        return Some("bmp");
    }
    None
}

/// The cache file that holds `url`'s bytes.
///
/// `extension` should come from [`sniff_extension`] so Slint can decode the
/// file. Callers that only need to *locate* an already-cached entry should use
/// [`find_cached`], which searches the known extensions.
pub fn cache_path(dir: &Path, url: &str, extension: &str) -> PathBuf {
    dir.join(format!("{:016x}.{extension}", hash_url(url)))
}

/// Every extension [`sniff_extension`] can produce, for cache lookups.
const CACHE_EXTENSIONS: [&str; 5] = ["jpg", "png", "webp", "gif", "bmp"];

/// Finds the cached file for `url`, if it has already been downloaded.
///
/// The extension is not known until the bytes arrive, so an existing entry is
/// located by globbing the URL's stable hash across the supported extensions.
pub fn find_cached(dir: &Path, url: &str) -> Option<PathBuf> {
    let stem = format!("{:016x}", hash_url(url));
    CACHE_EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|candidate| candidate.is_file())
}

/// Downloads `url` into the cache unless it is already there.
///
/// Returns the cache path so the caller can load it on the UI thread.
pub fn ensure_cached(client: &reqwest::blocking::Client, dir: &Path, url: &str) -> Result<PathBuf> {
    if let Some(existing) = find_cached(dir, url) {
        return Ok(existing);
    }

    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let response = client
        .get(url)
        .send()
        .with_context(|| format!("failed to download artwork `{url}`"))?;
    if !response.status().is_success() {
        anyhow::bail!("artwork request returned HTTP {}", response.status());
    }
    let bytes = response
        .bytes()
        .context("failed to read the artwork body")?;

    // Without a recognised magic number there is no extension Slint can decode,
    // so fail here rather than caching a file the UI can never load.
    let extension = sniff_extension(&bytes).with_context(|| {
        format!(
            "artwork at `{url}` is not a supported image format ({} bytes, start: {:02x?})",
            bytes.len(),
            &bytes[..bytes.len().min(8)]
        )
    })?;
    let path = cache_path(dir, url, extension);

    // Write to a temporary name first so a partial download never looks cached.
    let temp = path.with_extension("part");
    std::fs::write(&temp, &bytes).with_context(|| format!("failed to write {}", temp.display()))?;
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
        let a = cache_path(dir, "https://example.com/a.jpg", "jpg");
        let b = cache_path(dir, "https://example.com/a.jpg", "jpg");
        let c = cache_path(dir, "https://example.com/b.jpg", "jpg");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.to_string_lossy().ends_with(".jpg"));
    }

    /// The cache file must carry an extension Slint can decode. Serving the
    /// same bytes under a different extension produces a different path, which
    /// is exactly why `find_cached` exists.
    #[test]
    fn cache_path_reflects_the_sniffed_format() {
        let dir = Path::new("/tmp/covers");
        let png = cache_path(dir, "https://example.com/a", "png");
        let webp = cache_path(dir, "https://example.com/a", "webp");
        assert!(png.to_string_lossy().ends_with(".png"));
        assert!(webp.to_string_lossy().ends_with(".webp"));
        assert_ne!(png, webp);
        // Same hash stem, only the extension differs.
        assert_eq!(png.with_extension(""), webp.with_extension(""));
    }

    #[test]
    fn sniffs_the_formats_youtube_actually_serves() {
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00];
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x00];
        assert_eq!(sniff_extension(&png), Some("png"));
        assert_eq!(sniff_extension(&jpeg), Some("jpg"));
    }

    #[test]
    fn sniffs_webp_which_needs_the_riff_header() {
        // A WebP file is "RIFF" + 4 size bytes + "WEBP", so a naive
        // starts_with("RIFF") would also match a WAV file.
        let webp = *b"RIFF\x24\x00\x00\x00WEBPVP8 ";
        let wav = *b"RIFF\x24\x00\x00\x00WAVEfmt ";
        assert_eq!(sniff_extension(&webp), Some("webp"));
        assert_eq!(sniff_extension(&wav), None);
    }

    #[test]
    fn rejects_bytes_that_are_not_an_image() {
        assert_eq!(sniff_extension(b"<html>nope</html>"), None);
        assert_eq!(sniff_extension(&[]), None);
        // Too short to contain a WebP signature at offset 8.
        assert_eq!(sniff_extension(b"RIFF"), None);
    }
}
