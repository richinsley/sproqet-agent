//! Wire protocol constants and the 12-byte packet header.
//!
//! See `docs/PROTOCOL.md` for the authoritative specification.
//!
//! Layout:
//!
//! ```text
//!   magic(1) | flags(1) | channel_id(4 LE) | data_size(2 LE) | route_link_id(4 LE)
//!   flags = (delimiter << 5) | (frame_type & 0x1F)
//! ```

/// The Sproqet magic byte: `'s' | 0x80` = `0xF3`. High bit set so it
/// can never be confused with printable ASCII during resync scans.
pub const SPROQET_MAGIC: u8 = b's' | 0x80;

/// Size of the packet header in bytes.
pub const HEADER_SIZE: usize = 12;

/// Default maximum payload bytes per frame. Payloads larger than this
/// are split across multiple frames.
pub const MTU: usize = 1400;

/// Where a frame sits in the larger packet it belongs to.
///
/// A single logical packet may span one or many frames. Packets that fit
/// inside the MTU use [`Whole`](PacketDelimiter::Whole); larger ones use
/// [`Start`](PacketDelimiter::Start), any number of
/// [`Continuation`](PacketDelimiter::Continuation)s, and a final
/// [`Stop`](PacketDelimiter::Stop).
///
/// Note the bit pattern: bit 0 ⇔ "is start", bit 1 ⇔ "is stop".
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketDelimiter {
    /// Middle frame of a multi-frame packet.
    Continuation = 0x00,
    /// First frame of a multi-frame packet.
    Start = 0x01,
    /// Last frame of a multi-frame packet.
    Stop = 0x02,
    /// Self-contained packet; its payload fits in one frame.
    Whole = 0x03,
}

/// What kind of frame this is. Only [`Data`](FrameType::Data) is used today;
/// [`Helo`](FrameType::Helo) is reserved.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// Reserved (no negotiation in the current protocol).
    Helo = 0x00,
    /// Carries payload bytes. Current implementations emit only this type.
    Data = 0x05,
}

/// The 12-byte packet header.
///
/// Build one with [`SproqetPacketHeader::new`] or decode with
/// [`SproqetPacketHeader::from_bytes`]. Serialize with
/// [`SproqetPacketHeader::to_bytes`] — all multi-byte fields are
/// little-endian.
#[derive(Debug, Clone)]
pub struct SproqetPacketHeader {
    /// Must be [`SPROQET_MAGIC`]. Any other value on the wire is junk and
    /// the reader should resync by discarding bytes until it sees `0xF3`.
    pub magic: u8,
    /// Packed: bits 7–5 are the [`PacketDelimiter`], bits 4–0 the
    /// [`FrameType`]. Use [`delimiter`](Self::delimiter) /
    /// [`frame_type`](Self::frame_type) to unpack.
    pub flags: u8,
    /// Logical stream ID. Channels are created on first use.
    pub channel_id: u32,
    /// Number of payload bytes that follow this header. Capped at the
    /// MTU in the current implementation.
    pub data_size: u16,
    /// Reserved for mesh routing; always `0` in 1:1 P2P.
    pub route_link_id: u32,
}

impl SproqetPacketHeader {
    /// Construct a header with `magic` set to [`SPROQET_MAGIC`],
    /// `route_link_id` to `0`, and `flags` packed from
    /// `delimiter` and `frame_type`.
    pub fn new(
        delimiter: PacketDelimiter,
        frame_type: FrameType,
        channel_id: u32,
        data_size: u16,
    ) -> Self {
        Self {
            magic: SPROQET_MAGIC,
            flags: ((delimiter as u8) << 5) | (frame_type as u8 & 0x1F),
            channel_id,
            data_size,
            route_link_id: 0,
        }
    }

    /// Unpack the 2-bit delimiter from `flags`.
    pub fn delimiter(&self) -> u8 {
        (self.flags >> 5) & 0x03
    }

    /// Unpack the 5-bit frame type from `flags`.
    pub fn frame_type(&self) -> u8 {
        self.flags & 0x1F
    }

    /// Encode to 12 little-endian bytes suitable for writing directly to
    /// the transport.
    pub fn to_bytes(&self) -> [u8; HEADER_SIZE] {
        let mut b = [0u8; HEADER_SIZE];
        b[0] = self.magic;
        b[1] = self.flags;
        b[2..6].copy_from_slice(&self.channel_id.to_le_bytes());
        b[6..8].copy_from_slice(&self.data_size.to_le_bytes());
        b[8..12].copy_from_slice(&self.route_link_id.to_le_bytes());
        b
    }

    /// Decode a 12-byte header. Does not validate `magic` or
    /// `frame_type`; callers should check those.
    pub fn from_bytes(b: &[u8; HEADER_SIZE]) -> Self {
        Self {
            magic: b[0],
            flags: b[1],
            channel_id: u32::from_le_bytes([b[2], b[3], b[4], b[5]]),
            data_size: u16::from_le_bytes([b[6], b[7]]),
            route_link_id: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        }
    }
}
