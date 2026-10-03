//! A seekable `Read` implementation over an HTTP(S) resource.
//!
//! `rodio`'s `Decoder` needs `Read + Seek`, but a remote stream is neither. This
//! adapter fetches the file lazily in fixed-size ranges and serves the decoder
//! from a small in-memory window, which is what makes gapless streaming of
//! `googlevideo` URLs possible without downloading the whole track first.

use anyhow::{anyhow, Context, Result};
use std::io::{self, Read, Seek, SeekFrom};

/// Size of each HTTP range request (256 KiB).
const WINDOW: u64 = 1 << 18;

/// A seekable reader over a remote audio file.
pub struct HttpRangeSource {
    client: reqwest::blocking::Client,
    url: String,
    /// Absolute offset of the next byte to be produced.
    position: u64,
    /// Total length when known (0 means "unknown").
    length: u64,
    window: Vec<u8>,
    window_pos: usize,
}

impl HttpRangeSource {
    /// Opens a remote resource, discovering its length with a probe request.
    pub fn open(client: reqwest::blocking::Client, url: impl Into<String>) -> Result<Self> {
        let url = url.into();
        let response = client
            .get(&url)
            .header("Range", "bytes=0-0")
            .send()
            .with_context(|| format!("failed to open {}", crate::truncate_url(&url)))?;

        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!(
                "stream probe returned HTTP {status} for {}",
                crate::truncate_url(&url)
            ));
        }

        let length = response
            .headers()
            .get("content-range")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit('/').next())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .or_else(|| {
                response
                    .headers()
                    .get("content-length")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.trim().parse::<u64>().ok())
            })
            .unwrap_or(0);

        let probe = response.bytes().context("failed to read the probe body")?;

        Ok(Self {
            client,
            url,
            position: 0,
            length,
            window: probe.to_vec(),
            window_pos: 0,
        })
    }

    /// Total length in bytes, or `None` when the server did not report one.
    pub fn length(&self) -> Option<u64> {
        if self.length > 0 {
            Some(self.length)
        } else {
            None
        }
    }

    /// Refills the window from the current position.
    fn refill(&mut self) -> io::Result<()> {
        self.window.clear();
        self.window_pos = 0;

        if self.length > 0 && self.position >= self.length {
            return Ok(());
        }

        let last = self.position.saturating_add(WINDOW - 1);
        let end = if self.length > 0 {
            last.min(self.length - 1)
        } else {
            last
        };

        let response = self
            .client
            .get(&self.url)
            .header("Range", format!("bytes={}-{}", self.position, end))
            .send()
            .map_err(|err| io::Error::new(io::ErrorKind::Other, format!("range request failed: {err}")))?;

        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("range request returned HTTP {status}"),
            ));
        }

        let bytes = response
            .bytes()
            .map_err(|err| io::Error::new(io::ErrorKind::Other, format!("range body failed: {err}")))?;
        self.window.extend_from_slice(&bytes);
        Ok(())
    }
}

impl Read for HttpRangeSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.window_pos >= self.window.len() {
            self.refill()?;
            if self.window.is_empty() {
                return Ok(0);
            }
        }
        let available = &self.window[self.window_pos..];
        let take = available.len().min(buf.len());
        buf[..take].copy_from_slice(&available[..take]);
        self.window_pos += take;
        self.position += take as u64;
        Ok(take)
    }
}

impl Seek for HttpRangeSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target: i64 = match pos {
            SeekFrom::Start(offset) => offset as i64,
            SeekFrom::Current(delta) => self.position as i64 + delta,
            SeekFrom::End(delta) => self.length as i64 + delta,
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot seek before the start of the stream",
            ));
        }
        self.position = target as u64;
        self.window.clear();
        self.window_pos = 0;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_before_start_is_rejected() {
        let client = reqwest::blocking::Client::new();
        let mut source = HttpRangeSource {
            client,
            url: "http://127.0.0.1:1/none".into(),
            position: 0,
            length: 100,
            window: Vec::new(),
            window_pos: 0,
        };
        assert!(source.seek(SeekFrom::Start(0)).is_ok());
        assert_eq!(source.seek(SeekFrom::End(0)).unwrap(), 100);
    }

    #[test]
    fn reads_from_the_probe_window_without_network() {
        let client = reqwest::blocking::Client::new();
        let mut source = HttpRangeSource {
            client,
            url: "http://127.0.0.1:1/none".into(),
            position: 0,
            length: 4,
            window: b"abcd".to_vec(),
            window_pos: 0,
        };
        let mut out = [0u8; 4];
        source.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"abcd");
        assert_eq!(source.position, 4);
    }
}
