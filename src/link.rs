//! The `SproqetLink` — multiplexed point-to-point link over any
//! [`SproqetRead`] / [`SproqetWrite`] transport.
//!
//! See `docs/ARCHITECTURE.md` for the threading model and back-pressure
//! chain.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::channel::SproqetChannel;
use crate::error::SproqetError;
use crate::io_context::{SproqetRead, SproqetWrite};
use crate::variant::SproqetVariant;
use crate::wire_protocol::*;

/// Bidirectional, multiplexed point-to-point link.
///
/// A link wraps one reader and one writer (typically two clones of the
/// same socket) and turns them into an arbitrary number of logical
/// channels. Call [`start`](SproqetLink::start) to spawn the read thread;
/// from then on, any thread can call
/// [`send_variant`](SproqetLink::send_variant) /
/// [`send_buffer`](SproqetLink::send_buffer) concurrently, and any thread
/// can poll any channel's receive methods.
///
/// All public methods take `&self`, so the common pattern is to wrap
/// the link in an `Arc` and clone it into worker threads.
///
/// ```no_run
/// # use std::net::TcpStream;
/// # use std::sync::Arc;
/// # use sproqet::{SproqetLink, SproqetVariant};
/// let stream = TcpStream::connect("127.0.0.1:9000")?;
/// let reader = stream.try_clone()?;
/// let writer = stream;
/// let link = Arc::new(SproqetLink::new(Box::new(reader), Box::new(writer)));
/// link.start();
///
/// link.send_variant(1, &SproqetVariant::String("hi".into()))?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct SproqetLink {
    channels: Arc<Mutex<HashMap<u32, Arc<SproqetChannel>>>>,
    writer: Arc<Mutex<Box<dyn SproqetWrite>>>,
    running: Arc<AtomicBool>,
    read_thread: Mutex<Option<thread::JoinHandle<()>>>,
    reader: Mutex<Option<Box<dyn SproqetRead>>>,
}

impl SproqetLink {
    /// Construct a link from a reader and a writer.
    ///
    /// The link takes ownership of both. For a TCP or Unix socket, call
    /// `try_clone()` on the stream and pass one clone as the reader and
    /// the other as the writer.
    ///
    /// This doesn't spawn anything yet — call [`start`](Self::start) to
    /// get the read thread running.
    pub fn new(reader: Box<dyn SproqetRead>, writer: Box<dyn SproqetWrite>) -> Self {
        Self {
            channels: Arc::new(Mutex::new(HashMap::new())),
            writer: Arc::new(Mutex::new(writer)),
            running: Arc::new(AtomicBool::new(false)),
            read_thread: Mutex::new(None),
            reader: Mutex::new(Some(reader)),
        }
    }

    /// Spawn the internal read thread. From this point on, any frames
    /// arriving on the reader will be parsed, reassembled per-channel,
    /// and made available through [`get_channel`](Self::get_channel).
    ///
    /// # Panics
    /// Panics if called twice on the same link.
    pub fn start(&self) {
        let reader = self
            .reader
            .lock()
            .unwrap()
            .take()
            .expect("start() called twice");

        self.running.store(true, Ordering::SeqCst);

        let running = self.running.clone();
        let channels = self.channels.clone();

        let handle = thread::spawn(move || Self::run(reader, running, channels));

        *self.read_thread.lock().unwrap() = Some(handle);
    }

    /// Signal the read thread to stop and block until it exits.
    ///
    /// Idempotent and safe to call from `Drop`. Any packets still in a
    /// channel's FIFO remain retrievable via `receive_*` afterwards.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        let handle = self.read_thread.lock().unwrap().take();
        if let Some(h) = handle {
            let _ = h.join();
        }
    }

    /// Retrieve a channel handle, creating one on first use.
    ///
    /// Both sides of a link can call this with the same `channel_id`
    /// independently; the channels rendezvous via their numeric ID,
    /// not by any handshake. Channels are kept alive for the lifetime
    /// of the link.
    pub fn get_channel(&self, channel_id: u32) -> Arc<SproqetChannel> {
        self.channels
            .lock()
            .unwrap()
            .entry(channel_id)
            .or_insert_with(|| Arc::new(SproqetChannel::new(channel_id)))
            .clone()
    }

    /// Serialize `variant` and transmit it on `channel_id`.
    ///
    /// Shorthand for `serialize + send_buffer`. Blocks under back-pressure
    /// (see [`send_buffer`](Self::send_buffer)).
    pub fn send_variant(
        &self,
        channel_id: u32,
        variant: &SproqetVariant,
    ) -> Result<(), SproqetError> {
        let mut buf = Vec::new();
        variant.serialize(&mut buf);
        self.send_buffer(channel_id, &buf)
    }

    /// Send a raw byte buffer on `channel_id`, splitting it into
    /// MTU-sized frames.
    ///
    /// The write mutex is released between chunks so other sender
    /// threads can interleave whole packets on different channels —
    /// safe, because each receiver re-assembles independently per
    /// channel ID.
    ///
    /// Blocks if the transport can't accept more data, or if the peer's
    /// matching channel has a full FIFO (end-to-end back-pressure).
    pub fn send_buffer(&self, channel_id: u32, data: &[u8]) -> Result<(), SproqetError> {
        let mut offset = 0;
        while offset < data.len() {
            let chunk = std::cmp::min(MTU, data.len() - offset);
            let delimiter = if data.len() <= MTU {
                PacketDelimiter::Whole
            } else if offset == 0 {
                PacketDelimiter::Start
            } else if offset + chunk >= data.len() {
                PacketDelimiter::Stop
            } else {
                PacketDelimiter::Continuation
            };

            let header =
                SproqetPacketHeader::new(delimiter, FrameType::Data, channel_id, chunk as u16);

            let mut writer = self.writer.lock().unwrap();
            Self::write_exactly(&mut **writer, &header.to_bytes())?;
            Self::write_exactly(&mut **writer, &data[offset..offset + chunk])?;
            drop(writer);

            offset += chunk;
        }
        Ok(())
    }

    // ── internals ────────────────────────────────────────────────────

    fn run(
        mut reader: Box<dyn SproqetRead>,
        running: Arc<AtomicBool>,
        channels: Arc<Mutex<HashMap<u32, Arc<SproqetChannel>>>>,
    ) {
        let mut header_buf = [0u8; HEADER_SIZE];
        while running.load(Ordering::Relaxed) {
            if Self::read_exactly(&mut *reader, &mut header_buf, &running).is_err() {
                break;
            }

            let header = SproqetPacketHeader::from_bytes(&header_buf);
            if header.magic != SPROQET_MAGIC {
                continue;
            }
            if header.frame_type() != FrameType::Data as u8 {
                continue;
            }

            let size = header.data_size as usize;
            let mut payload = vec![0u8; size];
            if size > 0 && Self::read_exactly(&mut *reader, &mut payload, &running).is_err() {
                break;
            }

            let delim = header.delimiter();
            let is_start =
                delim == PacketDelimiter::Start as u8 || delim == PacketDelimiter::Whole as u8;
            let is_stop =
                delim == PacketDelimiter::Stop as u8 || delim == PacketDelimiter::Whole as u8;

            let channel = {
                let mut map = channels.lock().unwrap();
                map.entry(header.channel_id)
                    .or_insert_with(|| Arc::new(SproqetChannel::new(header.channel_id)))
                    .clone()
            };

            channel.push_frame(&payload, is_start, is_stop);
        }
    }

    fn read_exactly(
        reader: &mut dyn SproqetRead,
        buf: &mut [u8],
        running: &AtomicBool,
    ) -> Result<(), SproqetError> {
        let mut done = 0;
        while done < buf.len() {
            if !running.load(Ordering::Relaxed) {
                return Err(SproqetError::ConnectionClosed);
            }
            match reader.io_read(&mut buf[done..]) {
                Ok(0) => thread::sleep(Duration::from_micros(100)),
                Ok(n) => done += n,
                Err(e) => return Err(SproqetError::Io(e)),
            }
        }
        Ok(())
    }

    fn write_exactly(writer: &mut dyn SproqetWrite, buf: &[u8]) -> Result<(), SproqetError> {
        let mut done = 0;
        while done < buf.len() {
            match writer.io_write(&buf[done..]) {
                Ok(0) => thread::sleep(Duration::from_micros(100)),
                Ok(n) => done += n,
                Err(e) => return Err(SproqetError::Io(e)),
            }
        }
        Ok(())
    }
}

impl Drop for SproqetLink {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.read_thread.get_mut().unwrap().take() {
            let _ = h.join();
        }
    }
}
