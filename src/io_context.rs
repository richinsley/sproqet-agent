//! Transport-agnostic I/O traits.
//!
//! Sproqet doesn't care whether it runs over TCP, Unix-domain sockets,
//! a local pipe, or an in-memory test fixture. It needs only:
//!
//! - a way to read some bytes (returning `0` if none are currently available,
//!   or an error for EOF / fatal transport failures);
//! - a way to write some bytes (returning `0` if the transport would block,
//!   or an error for closed / fatal transports).
//!
//! The three-state convention — `Ok(n > 0)` = progress, `Ok(0)` = try again
//! later, `Err(_)` = stop — keeps callers simple: `read_exactly` /
//! `write_exactly` loops sleep briefly on `Ok(0)` and surface `Err` up the
//! stack. EOF should be reported as an error, not `Ok(0)`, so the link can
//! terminate cleanly instead of spinning forever.
//!
//! Built-in impls are provided for [`std::net::TcpStream`] and (on Unix)
//! [`std::os::unix::net::UnixStream`]. Custom transports — `serialport`,
//! `nng`, a test pipe, whatever — can be plugged in by implementing
//! [`SproqetRead`] and [`SproqetWrite`].

use std::io;

/// A source of bytes for a [`SproqetLink`](crate::link::SproqetLink) read thread.
///
/// Implementations must be `Send` (a link moves its reader onto an internal
/// thread at `start()`) and should not panic on well-formed input.
///
/// ### Three-state return convention
///
/// | Return value   | Meaning                                                    |
/// |----------------|------------------------------------------------------------|
/// | `Ok(n)`, n > 0 | `n` bytes were placed in `buf` at the front                |
/// | `Ok(0)`        | Nothing available right now; try again (non-blocking empty)|
/// | `Err(e)`       | EOF or fatal transport error; the read thread will exit    |
pub trait SproqetRead: Send {
    /// Read up to `buf.len()` bytes. See the trait-level docs for the
    /// three-state return convention. EOF must be returned as `Err(_)`, not
    /// `Ok(0)`.
    fn io_read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error>;
}

/// A sink of bytes for [`SproqetLink`](crate::link::SproqetLink) writes.
///
/// Implementations must be `Send`. Mirrors [`SproqetRead`]'s three-state
/// return convention.
pub trait SproqetWrite: Send {
    /// Write up to `buf.len()` bytes. `Ok(0)` means the transport is
    /// temporarily full; the caller will retry. Closed transports should
    /// return `Err(_)`, not `Ok(0)`.
    fn io_write(&mut self, buf: &[u8]) -> Result<usize, io::Error>;
}

// ── TCP ──────────────────────────────────────────────────────────────

impl SproqetRead for std::net::TcpStream {
    fn io_read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        use std::io::Read;
        match self.read(buf) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "tcp stream closed")),
            Ok(n) => Ok(n),
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }
}

impl SproqetWrite for std::net::TcpStream {
    fn io_write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        use std::io::Write;
        match self.write(buf) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::WriteZero, "tcp stream closed")),
            Ok(n) => Ok(n),
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }
}

// ── Unix domain sockets ──────────────────────────────────────────────

#[cfg(unix)]
impl SproqetRead for std::os::unix::net::UnixStream {
    fn io_read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        use std::io::Read;
        match self.read(buf) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "unix stream closed")),
            Ok(n) => Ok(n),
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(unix)]
impl SproqetWrite for std::os::unix::net::UnixStream {
    fn io_write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        use std::io::Write;
        match self.write(buf) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::WriteZero, "unix stream closed")),
            Ok(n) => Ok(n),
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
            {
                Ok(0)
            }
            Err(e) => Err(e),
        }
    }
}
