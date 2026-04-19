//! Error types for Sproqet.

use std::io;
use thiserror::Error;

/// Every fallible operation in Sproqet returns one of these.
#[derive(Error, Debug)]
pub enum SproqetError {
    /// The deserializer ran off the end of a buffer — a frame was
    /// truncated, or a length prefix lied about how much data follows.
    #[error("buffer underflow")]
    BufferUnderflow,

    /// A variant on the wire claims a type ID that this implementation
    /// doesn't recognise. Fatal — there's no way to skip past an
    /// unknown variant since we don't know its length.
    #[error("unknown type ID: {0:#04x}")]
    UnknownTypeId(u8),

    /// A wrapped transport-level I/O error.
    #[error("{0}")]
    Io(#[from] io::Error),

    /// The read thread was asked to stop, or the transport signalled EOF.
    #[error("connection closed")]
    ConnectionClosed,
}
