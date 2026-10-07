//! Local IPC transport for Discord's Rich Presence protocol.
//!
//! Discord does not expose a TCP socket that can simply be connected to. It
//! publishes a small "handshake" socket into a per-user runtime directory and
//! the client is expected to connect to it:
//!
//! * Windows  — `%TEMP%\discord-ipc-<n>` (a Unix-domain socket)
//! * Linux    — `$XDG_RUNTIME_DIR/discord-ipc-<n>` or `$TMPDIR/discord-ipc-<n>`
//! * macOS    — `$TMPDIR/discord-ipc-<n>`
//!
//! The files are numbered `0..=9`, one per running Discord client (stable, PTB,
//! Canary, …). The first one that accepts a connection wins.
//!
//! Writing this directly rather than pulling in a crate keeps the dependency
//! surface of a media player small, which matters because every dependency is
//! also a supply-chain and build-time cost.

use anyhow::{anyhow, Context, Result};
use std::io::{Read, Write};
use std::path::PathBuf;

/// The opcode Discord expects for a normal payload.
pub const OP_HANDSHAKE: u32 = 0;

/// The opcode used for `SET_ACTIVITY`.
pub const OP_FRAME: u32 = 1;

/// A connected Discord IPC socket.
pub struct IpcStream {
    /// The byte stream, whatever the platform calls it.
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
}

impl IpcStream {
    /// Finds a running Discord client and connects to it.
    ///
    /// An error here is the normal case, not an exceptional one — Discord is
    /// often simply not running — so callers should treat it as "presence
    /// currently unavailable" and retry later rather than surfacing it.
    pub fn connect() -> Result<Self> {
        let mut tried = 0usize;
        let mut last_error = None;

        for index in 0..10 {
            let name = format!("discord-ipc-{index}");
            for path in candidate_paths(&name) {
                tried += 1;
                match connect_path(&path) {
                    Ok((reader, writer)) => return Ok(Self { reader, writer }),
                    Err(err) => last_error = Some(err),
                }
            }
        }

        let detail = last_error
            .map(|err| format!(": {err:#}"))
            .unwrap_or_default();
        Err(anyhow!(
            "no Discord client is reachable ({tried} socket paths tried){detail}"
        ))
    }

    /// Sends one framed JSON payload: `[opcode u32le][len u32le][payload]`.
    pub fn send(&mut self, opcode: u32, payload: &str) -> Result<()> {
        let mut frame = Vec::with_capacity(8 + payload.len());
        frame.extend_from_slice(&opcode.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(payload.as_bytes());
        self.writer
            .write_all(&frame)
            .context("failed to write to the Discord socket")?;
        self.writer
            .flush()
            .context("failed to flush the Discord socket")
    }

    /// Reads one framed JSON payload.
    pub fn recv(&mut self) -> Result<String> {
        let mut header = [0u8; 8];
        self.reader
            .read_exact(&mut header)
            .context("failed to read the Discord frame header")?;
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        // A malformed or hostile length would otherwise ask for a huge
        // allocation, so refuse anything implausible up front.
        if length > 1024 * 1024 {
            return Err(anyhow!(
                "Discord sent an implausibly large frame ({length} bytes)"
            ));
        }
        let mut body = vec![0u8; length];
        self.reader
            .read_exact(&mut body)
            .context("failed to read the Discord frame body")?;
        String::from_utf8(body).context("Discord sent a frame that was not valid UTF-8")
    }
}

/// Candidate filesystem paths for a given socket name, most likely first.
fn candidate_paths(name: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        if let Some(temp) = std::env::var_os("TEMP").or_else(|| std::env::var_os("TMP")) {
            paths.push(PathBuf::from(temp).join(name));
        }
        // Some Discord builds publish under a nested directory.
        if let Some(temp) = std::env::var_os("TEMP").or_else(|| std::env::var_os("TMP")) {
            paths.push(PathBuf::from(temp).join("discord").join(name));
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(tmp) = std::env::var_os("TMPDIR") {
            paths.push(PathBuf::from(tmp).join(name));
        }
        paths.push(PathBuf::from("/tmp").join(name));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
            paths.push(PathBuf::from(runtime).join(name));
        }
        // Flatpak and Snap hide the runtime directory from the host.
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
            paths.push(
                PathBuf::from(runtime)
                    .join("app/com.discordapp.Discord")
                    .join(name),
            );
            paths.push(PathBuf::from(runtime).join("snap.discord").join(name));
        }
        for dir in ["/tmp", "/var/run/user"] {
            paths.push(PathBuf::from(dir).join(name));
        }
        if let Ok(uid) = std::env::var("UID") {
            let prefix = format!("discord-{uid}");
            if let Some(tmp) = std::env::var_os("TMPDIR") {
                paths.push(PathBuf::from(tmp).join(format!("{prefix}/{name}")));
            }
        }
    }

    // Deduplicate without pulling in a HashSet import.
    let mut unique: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    unique
}

// ---------------------------------------------------------------------------
// Per-platform connect
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn connect_path(path: &std::path::Path) -> Result<(Box<dyn Read + Send>, Box<dyn Write + Send>)> {
    let stream = std::os::unix::net::UnixStream::connect(path)
        .with_context(|| format!("failed to connect to {}", path.display()))?;
    let reader = stream
        .try_clone()
        .context("failed to clone the Discord socket")?;
    Ok((Box::new(reader), Box::new(stream)))
}

/// Windows exposes Discord's Rich Presence socket as a Unix-domain socket too,
/// but `std` only stabilised `UnixStream` for it on Unix. A plain `CreateFileW`
/// on the socket path works identically and needs no extra crate.
#[cfg(target_os = "windows")]
fn connect_path(path: &std::path::Path) -> Result<(Box<dyn Read + Send>, Box<dyn Write + Send>)> {
    // `\\?\` bypasses MAX_PATH and, more importantly for a socket path, stops
    // the Win32 layer from normalising the name.
    let raw = path.to_string_lossy();
    let prefixed = if raw.starts_with(r"\\?\") {
        raw.to_string()
    } else {
        format!(r"\\?\{raw}")
    };

    let reader = win::SocketFile::open(&prefixed)?;
    let writer = win::SocketFile::open(&prefixed)?;
    Ok((Box::new(reader), Box::new(writer)))
}

#[cfg(target_os = "windows")]
mod win {
    use anyhow::{anyhow, Result};
    use std::io::{Read, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    /// A Win32 file handle usable as `Read + Write`.
    ///
    /// Owning the handle through `OwnedHandle` means the handle is closed on
    /// drop even if the caller panics, which matters because Discord will not
    /// accept a second connection from the same process otherwise.
    pub struct SocketFile {
        handle: OwnedHandle,
    }

    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const OPEN_EXISTING: u32 = 3;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security: *mut core::ffi::c_void,
            creation_disposition: u32,
            flags: u32,
            template: *mut core::ffi::c_void,
        ) -> *mut core::ffi::c_void;

        fn ReadFile(
            handle: *mut core::ffi::c_void,
            buffer: *mut u8,
            to_read: u32,
            read: *mut u32,
            overlapped: *mut core::ffi::c_void,
        ) -> i32;

        fn WriteFile(
            handle: *mut core::ffi::c_void,
            buffer: *const u8,
            to_write: u32,
            written: *mut u32,
            overlapped: *mut core::ffi::c_void,
        ) -> i32;
    }

    /// `INVALID_HANDLE_VALUE` is `(HANDLE)-1`, not null.
    const INVALID_HANDLE_VALUE: isize = -1;

    impl SocketFile {
        pub fn open(path: &str) -> Result<Self> {
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0, // no sharing: Discord accepts one client at a time
                    std::ptr::null_mut(),
                    OPEN_EXISTING,
                    // Discord's socket is a real byte stream, so it must not be
                    // opened in overlapped mode.
                    0,
                    std::ptr::null_mut(),
                )
            };
            if handle.is_null() || handle as isize == INVALID_HANDLE_VALUE {
                let err = std::io::Error::last_os_error();
                return Err(anyhow!("failed to open {path}: {err}"));
            }
            let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
            Ok(Self { handle })
        }

        fn raw(&self) -> *mut core::ffi::c_void {
            // `as_raw_handle` already yields `*mut c_void`; no cast needed.
            self.handle.as_raw_handle()
        }
    }

    impl Read for SocketFile {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut read = 0u32;
            let ok = unsafe {
                ReadFile(
                    self.raw(),
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                    &mut read,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(read as usize)
        }
    }

    impl Write for SocketFile {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut written = 0u32;
            let ok = unsafe {
                WriteFile(
                    self.raw(),
                    buf.as_ptr(),
                    buf.len() as u32,
                    &mut written,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(written as usize)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            // Sockets are unbuffered at this level; there is nothing to push.
            Ok(())
        }
    }
}
