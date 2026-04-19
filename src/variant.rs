//! Self-describing value type — the payload language of Sproqet.
//!
//! [`SproqetVariant`] is a tagged union of 13 primitive and container
//! types. Values are serialized into a length-prefixed byte sequence
//! that is deterministic (maps use [`BTreeMap`] internally, so equal
//! values produce byte-identical output) and self-describing (no schema
//! needed to parse — just walk the type tags).
//!
//! The wire format is specified in `docs/PROTOCOL.md`.

use crate::error::SproqetError;
use std::collections::BTreeMap;

// ── Type IDs (wire-compatible with the original C++) ─────────────────

/// Type tag: `NULL` — carries no payload.
pub const TYPE_NULL: u8 = 0x00;
/// Type tag: `VOID` — legacy alias for `NULL`; deserializes to [`SproqetVariant::Null`].
pub const TYPE_VOID: u8 = 0x01;
/// Type tag: `BOOL` — 1 byte, 0 = false, ≠0 = true.
pub const TYPE_BOOL: u8 = 0x02;
/// Type tag: `INT32` — 4 little-endian bytes.
pub const TYPE_INT32: u8 = 0x07;
/// Type tag: `UINT32` — 4 little-endian bytes.
pub const TYPE_UINT32: u8 = 0x08;
/// Type tag: `INT64` — 8 little-endian bytes.
pub const TYPE_INT64: u8 = 0x09;
/// Type tag: `UINT64` — 8 little-endian bytes.
pub const TYPE_UINT64: u8 = 0x0A;
/// Type tag: `DOUBLE` — IEEE-754 f64, stored via `to_bits` as 8 LE bytes.
pub const TYPE_DOUBLE: u8 = 0x0C;
/// Type tag: `BUFFER` — u32 length + raw bytes.
pub const TYPE_BUFFER: u8 = 0x0D;
/// Type tag: `STRING` — u32 length + UTF-8 bytes.
pub const TYPE_STRING: u8 = 0x0E;
/// Type tag: `ARRAY` — u32 count + that many serialized variants.
pub const TYPE_ARRAY: u8 = 0x0F;
/// Type tag: `STRING_MAP` — u32 count + (u32 key-len, key bytes, variant) ×.
pub const TYPE_STRING_MAP: u8 = 0x10;
/// Type tag: `UINT_MAP` — u32 count + (**u64 key**, variant) ×.
/// Keys are `u32` in memory but serialized as `u64` on the wire (a C++ quirk).
pub const TYPE_UINT_MAP: u8 = 0x11;
/// Type tag: `NDARRAY` — see [`SproqetNDArray`].
pub const TYPE_NDARRAY: u8 = 0x20;

/// A multidimensional array — the intended carrier for video frames,
/// tensors, and audio buffers in out-of-process media pipelines.
///
/// Shape and dtype semantics (row vs column major, endianness of multi-
/// byte pixel formats, etc.) are agreed out-of-band between sender and
/// receiver. `shape` is `Vec<u64>` so a single frame can be arbitrarily
/// high-dimensional without special-casing.
///
/// ```
/// # use sproqet::SproqetNDArray;
/// // An HD RGB frame (height, width, channels).
/// let frame = SproqetNDArray {
///     shape: vec![1080, 1920, 3],
///     data_type: "uint8".into(),
///     data: vec![0u8; 1920 * 1080 * 3],
/// };
/// ```
#[derive(Debug, Clone)]
pub struct SproqetNDArray {
    /// Dimensions of the array, outermost first. E.g. `[H, W, C]` for
    /// a standard image buffer.
    pub shape: Vec<u64>,
    /// Element type as a free-form string. Conventionally one of
    /// `"uint8"`, `"int16"`, `"float32"`, `"float64"`, but not enforced.
    pub data_type: String,
    /// The raw element bytes. Length must equal the product of `shape`
    /// times the element size of `data_type`.
    pub data: Vec<u8>,
}

/// A dynamically-typed value that can be sent across a Sproqet link.
///
/// Variants are self-describing: each one carries a one-byte type tag
/// followed by a type-specific payload. This is the unit of exchange
/// between peers — you serialize a [`SproqetVariant`] to bytes, the
/// receiver deserializes back to a [`SproqetVariant`], and the enum's
/// exhaustive match tells you what you got.
#[derive(Debug, Clone)]
pub enum SproqetVariant {
    /// No value.
    Null,
    /// A boolean flag.
    Bool(bool),
    /// A signed 32-bit integer.
    Int32(i32),
    /// An unsigned 32-bit integer.
    Uint32(u32),
    /// A signed 64-bit integer.
    Int64(i64),
    /// An unsigned 64-bit integer.
    Uint64(u64),
    /// A 64-bit IEEE-754 float.
    Double(f64),
    /// A raw byte blob (e.g. compressed payloads).
    Buffer(Vec<u8>),
    /// A UTF-8 string.
    String(String),
    /// A heterogeneous list of variants.
    Array(Vec<SproqetVariant>),
    /// A string-keyed map. Always serialized in ascending key order.
    StringMap(BTreeMap<String, SproqetVariant>),
    /// A u32-keyed map. Keys are `u32` here but are serialized as `u64`
    /// on the wire for C++ compatibility.
    UintMap(BTreeMap<u32, SproqetVariant>),
    /// A multidimensional array. See [`SproqetNDArray`].
    NDArray(SproqetNDArray),
}

// ── From impls ───────────────────────────────────────────────────────

impl From<bool> for SproqetVariant {
    fn from(v: bool) -> Self {
        SproqetVariant::Bool(v)
    }
}
impl From<i32> for SproqetVariant {
    fn from(v: i32) -> Self {
        SproqetVariant::Int32(v)
    }
}
impl From<u32> for SproqetVariant {
    fn from(v: u32) -> Self {
        SproqetVariant::Uint32(v)
    }
}
impl From<i64> for SproqetVariant {
    fn from(v: i64) -> Self {
        SproqetVariant::Int64(v)
    }
}
impl From<u64> for SproqetVariant {
    fn from(v: u64) -> Self {
        SproqetVariant::Uint64(v)
    }
}
impl From<f64> for SproqetVariant {
    fn from(v: f64) -> Self {
        SproqetVariant::Double(v)
    }
}
impl From<Vec<u8>> for SproqetVariant {
    fn from(v: Vec<u8>) -> Self {
        SproqetVariant::Buffer(v)
    }
}
impl From<&str> for SproqetVariant {
    fn from(v: &str) -> Self {
        SproqetVariant::String(v.to_owned())
    }
}
impl From<SproqetNDArray> for SproqetVariant {
    fn from(v: SproqetNDArray) -> Self {
        SproqetVariant::NDArray(v)
    }
}

// ── Binary helpers ───────────────────────────────────────────────────

fn write8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}
fn write32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn write64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn read8(data: &[u8], off: &mut usize) -> Result<u8, SproqetError> {
    if *off >= data.len() {
        return Err(SproqetError::BufferUnderflow);
    }
    let v = data[*off];
    *off += 1;
    Ok(v)
}
fn read32(data: &[u8], off: &mut usize) -> Result<u32, SproqetError> {
    if *off + 4 > data.len() {
        return Err(SproqetError::BufferUnderflow);
    }
    let v = u32::from_le_bytes(data[*off..*off + 4].try_into().unwrap());
    *off += 4;
    Ok(v)
}
fn read64(data: &[u8], off: &mut usize) -> Result<u64, SproqetError> {
    if *off + 8 > data.len() {
        return Err(SproqetError::BufferUnderflow);
    }
    let v = u64::from_le_bytes(data[*off..*off + 8].try_into().unwrap());
    *off += 8;
    Ok(v)
}
fn read_bytes<'a>(data: &'a [u8], off: &mut usize, len: usize) -> Result<&'a [u8], SproqetError> {
    if *off + len > data.len() {
        return Err(SproqetError::BufferUnderflow);
    }
    let s = &data[*off..*off + len];
    *off += len;
    Ok(s)
}

// ── Serialization ────────────────────────────────────────────────────

impl SproqetVariant {
    /// Append the wire encoding of `self` to `buf`.
    ///
    /// Serialization is deterministic: two equal variants always produce
    /// byte-identical output. Maps iterate in ascending key order.
    pub fn serialize(&self, buf: &mut Vec<u8>) {
        match self {
            SproqetVariant::Null => write8(buf, TYPE_NULL),
            SproqetVariant::Bool(v) => {
                write8(buf, TYPE_BOOL);
                write8(buf, u8::from(*v));
            }
            SproqetVariant::Int32(v) => {
                write8(buf, TYPE_INT32);
                write32(buf, *v as u32);
            }
            SproqetVariant::Uint32(v) => {
                write8(buf, TYPE_UINT32);
                write32(buf, *v);
            }
            SproqetVariant::Int64(v) => {
                write8(buf, TYPE_INT64);
                write64(buf, *v as u64);
            }
            SproqetVariant::Uint64(v) => {
                write8(buf, TYPE_UINT64);
                write64(buf, *v);
            }
            SproqetVariant::Double(v) => {
                write8(buf, TYPE_DOUBLE);
                write64(buf, v.to_bits());
            }
            SproqetVariant::Buffer(v) => {
                write8(buf, TYPE_BUFFER);
                write32(buf, v.len() as u32);
                buf.extend_from_slice(v);
            }
            SproqetVariant::String(v) => {
                write8(buf, TYPE_STRING);
                write32(buf, v.len() as u32);
                buf.extend_from_slice(v.as_bytes());
            }
            SproqetVariant::Array(v) => {
                write8(buf, TYPE_ARRAY);
                write32(buf, v.len() as u32);
                for item in v {
                    item.serialize(buf);
                }
            }
            SproqetVariant::StringMap(v) => {
                write8(buf, TYPE_STRING_MAP);
                write32(buf, v.len() as u32);
                for (k, val) in v {
                    write32(buf, k.len() as u32);
                    buf.extend_from_slice(k.as_bytes());
                    val.serialize(buf);
                }
            }
            SproqetVariant::UintMap(v) => {
                write8(buf, TYPE_UINT_MAP);
                write32(buf, v.len() as u32);
                for (&k, val) in v {
                    // C++ quirk: u32 key serialized as u64.
                    write64(buf, k as u64);
                    val.serialize(buf);
                }
            }
            SproqetVariant::NDArray(nd) => {
                write8(buf, TYPE_NDARRAY);
                write32(buf, nd.shape.len() as u32);
                for &s in &nd.shape {
                    write64(buf, s);
                }
                write32(buf, nd.data_type.len() as u32);
                buf.extend_from_slice(nd.data_type.as_bytes());
                write32(buf, nd.data.len() as u32);
                buf.extend_from_slice(&nd.data);
            }
        }
    }

    /// Parse a wire-encoded variant from `data`.
    ///
    /// Returns [`SproqetError::BufferUnderflow`] if a length prefix promises
    /// more bytes than are available, or [`SproqetError::UnknownTypeId`] if
    /// the input uses a type tag this implementation doesn't know.
    ///
    /// Any trailing bytes after a successfully-parsed variant are
    /// silently ignored.
    pub fn deserialize(data: &[u8]) -> Result<SproqetVariant, SproqetError> {
        let mut off = 0;
        Self::deserialize_at(data, &mut off)
    }

    fn deserialize_at(data: &[u8], off: &mut usize) -> Result<SproqetVariant, SproqetError> {
        let t = read8(data, off)?;
        match t {
            TYPE_NULL | TYPE_VOID => Ok(SproqetVariant::Null),
            TYPE_BOOL => Ok(SproqetVariant::Bool(read8(data, off)? != 0)),
            TYPE_INT32 => Ok(SproqetVariant::Int32(read32(data, off)? as i32)),
            TYPE_UINT32 => Ok(SproqetVariant::Uint32(read32(data, off)?)),
            TYPE_INT64 => Ok(SproqetVariant::Int64(read64(data, off)? as i64)),
            TYPE_UINT64 => Ok(SproqetVariant::Uint64(read64(data, off)?)),
            TYPE_DOUBLE => Ok(SproqetVariant::Double(f64::from_bits(read64(data, off)?))),
            TYPE_BUFFER => {
                let len = read32(data, off)? as usize;
                Ok(SproqetVariant::Buffer(read_bytes(data, off, len)?.to_vec()))
            }
            TYPE_STRING => {
                let len = read32(data, off)? as usize;
                let bytes = read_bytes(data, off, len)?;
                Ok(SproqetVariant::String(
                    String::from_utf8_lossy(bytes).into_owned(),
                ))
            }
            TYPE_ARRAY => {
                let count = read32(data, off)? as usize;
                let mut arr = Vec::with_capacity(count);
                for _ in 0..count {
                    arr.push(Self::deserialize_at(data, off)?);
                }
                Ok(SproqetVariant::Array(arr))
            }
            TYPE_STRING_MAP => {
                let count = read32(data, off)? as usize;
                let mut m = BTreeMap::new();
                for _ in 0..count {
                    let klen = read32(data, off)? as usize;
                    let kbytes = read_bytes(data, off, klen)?;
                    let k = String::from_utf8_lossy(kbytes).into_owned();
                    let v = Self::deserialize_at(data, off)?;
                    m.insert(k, v);
                }
                Ok(SproqetVariant::StringMap(m))
            }
            TYPE_UINT_MAP => {
                let count = read32(data, off)? as usize;
                let mut m = BTreeMap::new();
                for _ in 0..count {
                    let k = read64(data, off)? as u32;
                    let v = Self::deserialize_at(data, off)?;
                    m.insert(k, v);
                }
                Ok(SproqetVariant::UintMap(m))
            }
            TYPE_NDARRAY => {
                let sc = read32(data, off)? as usize;
                let mut shape = Vec::with_capacity(sc);
                for _ in 0..sc {
                    shape.push(read64(data, off)?);
                }
                let dl = read32(data, off)? as usize;
                let dt = String::from_utf8_lossy(read_bytes(data, off, dl)?).into_owned();
                let len = read32(data, off)? as usize;
                let data_bytes = read_bytes(data, off, len)?.to_vec();
                Ok(SproqetVariant::NDArray(SproqetNDArray {
                    shape,
                    data_type: dt,
                    data: data_bytes,
                }))
            }
            _ => Err(SproqetError::UnknownTypeId(t)),
        }
    }
}
