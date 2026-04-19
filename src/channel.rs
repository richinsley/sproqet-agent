//! Per-channel frame assembly and delivery.
//!
//! A [`SproqetChannel`] is one logical stream within a
//! [`SproqetLink`](crate::link::SproqetLink). Fragments delivered by the
//! link's read thread via [`push_frame`](SproqetChannel::push_frame) are
//! accumulated in an assembly buffer; on a `STOP` / `WHOLE` delimiter,
//! the completed packet moves into a bounded FIFO where the application
//! can retrieve it with [`receive_variant`](SproqetChannel::receive_variant)
//! or [`receive_buffer`](SproqetChannel::receive_buffer).
//!
//! The bounded FIFO is an [`std::sync::mpsc::sync_channel`] of size 512:
//! `send` blocks when full, providing back-pressure that propagates
//! through the read thread, the transport, and ultimately to the sender
//! on the peer.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Mutex;

use crate::error::SproqetError;
use crate::variant::SproqetVariant;

/// One logical stream carried by a [`SproqetLink`](crate::link::SproqetLink).
///
/// Obtain one with
/// [`SproqetLink::get_channel`](crate::link::SproqetLink::get_channel).
/// Channels are created on demand — the first send or the first arrival
/// of a frame with a given `channel_id` creates one, and both sides
/// rendezvous by ID.
///
/// Receive methods are non-blocking: they return `None` when nothing is
/// ready. Callers typically poll with a short sleep, or drive them from
/// a dedicated consumer thread.
pub struct SproqetChannel {
    channel_id: u32,
    sender: Mutex<SyncSender<Vec<u8>>>,
    receiver: Mutex<Receiver<Vec<u8>>>,
    assembly_buffer: Mutex<Vec<u8>>,
}

impl SproqetChannel {
    /// Create a fresh channel with a 512-slot packet FIFO. Normally
    /// called internally by `SproqetLink`; exposed for testing.
    pub fn new(channel_id: u32) -> Self {
        let (tx, rx) = sync_channel(512);
        Self {
            channel_id,
            sender: Mutex::new(tx),
            receiver: Mutex::new(rx),
            assembly_buffer: Mutex::new(Vec::new()),
        }
    }

    /// The numeric ID this channel was created with.
    pub fn channel_id(&self) -> u32 {
        self.channel_id
    }

    /// Called by the read thread to deliver a frame fragment.
    ///
    /// When a complete packet has been assembled (`is_stop == true`), it
    /// is pushed into the bounded channel — blocking if full.
    pub fn push_frame(&self, data: &[u8], is_start: bool, is_stop: bool) {
        let mut buf = self.assembly_buffer.lock().unwrap();
        if is_start {
            buf.clear();
        }
        buf.extend_from_slice(data);
        if is_stop {
            let packet = std::mem::take(&mut *buf);
            let tx = self.sender.lock().unwrap();
            let _ = tx.send(packet);
        }
    }

    /// Non-blocking: pop the next completed packet (if any) and
    /// deserialize it as a [`SproqetVariant`].
    ///
    /// - `None` — the FIFO is currently empty.
    /// - `Some(Ok(v))` — a variant was received.
    /// - `Some(Err(e))` — a packet was received but was malformed; the
    ///   packet is consumed.
    pub fn receive_variant(&self) -> Option<Result<SproqetVariant, SproqetError>> {
        let rx = self.receiver.lock().unwrap();
        match rx.try_recv() {
            Ok(buf) => Some(SproqetVariant::deserialize(&buf)),
            Err(_) => None,
        }
    }

    /// Non-blocking: pop the next completed packet (if any) as raw bytes.
    /// Useful when you want to carry opaque blobs without the variant layer.
    pub fn receive_buffer(&self) -> Option<Vec<u8>> {
        let rx = self.receiver.lock().unwrap();
        rx.try_recv().ok()
    }
}
