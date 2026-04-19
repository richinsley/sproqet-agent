//! # Sproqet
//!
//! A lean P2P multiplexed binary protocol — sync, std-only core, with
//! a lockless SPSC FIFO available as a standalone module.
//!
//! ## Crate layout
//!
//! - [`error`]         — `SproqetError` (BufferUnderflow, UnknownTypeId, Io, ConnectionClosed)
//! - [`wire_protocol`] — 12-byte packed header, magic = `0xF3`
//! - [`variant`]       — 13-type `SproqetVariant` + serialization (BTreeMap → deterministic)
//! - [`io_context`]    — `SproqetRead` / `SproqetWrite` traits + TCP/Unix impls
//! - [`channel`]       — Per-channel frame assembly + bounded FIFO
//! - [`link`]          — Multiplexed link with read thread + MTU chunking
//! - [`fifo`]          — Standalone SPSC lockless ring, type-enforced

pub mod channel;
pub mod error;
pub mod fifo;
pub mod io_context;
pub mod link;
pub mod variant;
pub mod wire_protocol;

pub use channel::SproqetChannel;
pub use error::SproqetError;
pub use io_context::{SproqetRead, SproqetWrite};
pub use link::SproqetLink;
pub use variant::{SproqetNDArray, SproqetVariant};
pub use wire_protocol::{FrameType, PacketDelimiter, SPROQET_MAGIC};
