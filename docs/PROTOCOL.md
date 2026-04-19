# Wire Protocol

This document is the authoritative specification of the Sproqet wire format. Two implementations are byte-compatible if and only if they follow the rules in this file.

All multi-byte integers are **little-endian**. All lengths are `u32` unless otherwise stated.

---

## 1. Packet header (12 bytes)

Every frame begins with a 12-byte header. Payload (`data_size` bytes) follows immediately.

```
Offset  Size  Field            Notes
------  ----  ---------------  ------------------------------------
   0      1   magic            Must be 0xF3 ('s' | 0x80)
   1      1   flags            (delimiter << 5) | (frame_type & 0x1F)
   2      4   channel_id       u32 LE, identifies logical channel
   6      2   data_size        u16 LE, bytes of payload following
   8      4   route_link_id    u32 LE, reserved (always 0 in P2P)
```

### 1.1 Magic byte

`0xF3` = `'s' | 0x80`. The high bit is set so the header can never be confused with printable ASCII. A receiver that reads a byte that is not `0xF3` at a header position **must** discard it and keep scanning — this is the resync mechanism after a bad transport.

### 1.2 Flags field

```
  bit  7 6 5 4 3 2 1 0
       └─┬─┘ └───┬───┘
         │       └── frame_type (5 bits)
         └────────── packet_delimiter (2 bits)
       bit 7 is reserved (= 0 today)
```

**Encoding**: `flags = (delimiter << 5) | (frame_type & 0x1F)`
**Decoding**: `delimiter = (flags >> 5) & 0x03`, `frame_type = flags & 0x1F`

### 1.3 Packet delimiter (2 bits)

| Value | Name          | Meaning                                              |
|-------|---------------|------------------------------------------------------|
| 0x00  | CONTINUATION  | A middle frame of a multi-frame packet               |
| 0x01  | START         | First frame of a multi-frame packet                  |
| 0x02  | STOP          | Last frame of a multi-frame packet                   |
| 0x03  | WHOLE         | Payload fits in one frame — self-contained           |

Note the bit pattern: bit 0 = "is start", bit 1 = "is stop", WHOLE = both. Implementations may use the bit test directly:

```
is_start = (delimiter & 0x01) != 0
is_stop  = (delimiter & 0x02) != 0
```

### 1.4 Frame type (5 bits)

| Value | Name  | Meaning               |
|-------|-------|-----------------------|
| 0x00  | HELO  | Reserved (not used)   |
| 0x05  | DATA  | Carries payload bytes |

Receivers **must** ignore frames with unknown `frame_type` (still consuming `data_size` payload bytes to stay aligned).

### 1.5 Channel ID

A `u32` logical stream ID chosen by the sender. Channels are created on demand on both sides: the first frame with a new `channel_id` implicitly creates a `SproqetChannel` at the receiver.

### 1.6 Data size

A `u16`: the number of payload bytes that follow this header. Maximum 65,535 per frame, though the default MTU (see §2) caps this at 1,400.

### 1.7 Route link ID

Reserved for future mesh routing. In 1:1 P2P mode (the only mode implemented today), this field is always `0`. Receivers should ignore it.

---

## 2. MTU and fragmentation

Payloads larger than **1,400 bytes** (configurable, but fixed in current implementations) are split across multiple frames on the same `channel_id`:

```
  First frame:   delimiter = START         data_size ≤ 1400
  Middle frames: delimiter = CONTINUATION  data_size ≤ 1400   (any number)
  Last frame:    delimiter = STOP          data_size = remainder
```

Payloads ≤ 1,400 bytes use a single `WHOLE` frame and skip `START`/`STOP`.

The receiver maintains a per-channel assembly buffer: on `START` it's cleared, every frame appends its payload, and `STOP` (or `WHOLE`) delivers the complete packet to the application.

**Interleaving is permitted.** Because assembly state is per-channel, channel A may send `START … CONTINUATION …`, then channel B sends a `WHOLE`, then channel A finishes with `STOP`. Both reassemble correctly. This is what makes multiplexing work end-to-end.

---

## 3. Variant serialization

A **variant** is a self-describing value prefixed by a 1-byte type ID. Payloads on the wire are (almost always) serialized variants.

### 3.1 Type IDs

| ID     | Name         | Wire format (after type byte)                                       |
|--------|--------------|---------------------------------------------------------------------|
| `0x00` | `NULL`       | (nothing)                                                           |
| `0x01` | `VOID`       | (nothing) — deserializes to `NULL`                                  |
| `0x02` | `BOOL`       | `u8`: 0 = false, ≠0 = true                                          |
| `0x07` | `INT32`      | `i32` LE (bit-reinterpret of `u32` bytes)                           |
| `0x08` | `UINT32`     | `u32` LE                                                            |
| `0x09` | `INT64`      | `i64` LE                                                            |
| `0x0A` | `UINT64`     | `u64` LE                                                            |
| `0x0C` | `DOUBLE`     | `u64` LE = `f64::to_bits(v)`                                        |
| `0x0D` | `BUFFER`     | `u32` length, then that many raw bytes                              |
| `0x0E` | `STRING`     | `u32` length, then that many bytes (UTF-8, not NUL-terminated)      |
| `0x0F` | `ARRAY`      | `u32` count, then `count` serialized variants                       |
| `0x10` | `STRING_MAP` | `u32` count, then `count` × (`u32` key-len, key bytes, variant)     |
| `0x11` | `UINT_MAP`   | `u32` count, then `count` × (**`u64` key**, variant) — see §3.3     |
| `0x20` | `NDARRAY`    | See §3.4                                                            |

An unknown type ID is a fatal error — the deserializer must not attempt to skip it.

### 3.2 Deterministic map order

Maps serialize in **ascending key order** (BTreeMap semantics). Two logically-equal maps must produce byte-identical output. Hash-based maps (e.g. Rust's `HashMap`) must be sorted before serialization to preserve wire compatibility.

### 3.3 The UINT_MAP key-size quirk

`UINT_MAP` keys are `u32` in the API but are serialized as `u64` on the wire. This is a faithful port of a C++ implementation detail: the original used `memcpy` from an 8-byte stack slot. Implementations must write 8 bytes per key (zero-extended) and read 8 bytes per key (truncating the high 32 bits).

### 3.4 NDArray

Multidimensional arrays (intended for video frames, tensors, audio buffers). Laid out as:

```
Offset  Size   Field              Notes
------  -----  -----------------  -------------------------------------
   0      4    shape_count (u32)  number of dimensions
   4    8×N    shape (u64 each)   one u64 per dimension
   *      4    dtype_len (u32)    length of dtype string
   *      L    dtype bytes        UTF-8, e.g. "uint8", "float32"
   *      4    data_len (u32)     bytes of payload
   *     DL    data bytes         raw pixel/tensor data
```

**No padding.** Semantics of the shape and dtype (row-major vs column-major, byte order of multi-byte pixel types, etc.) are agreed out-of-band between sender and receiver.

**Example**: an HD RGB8 frame
```
shape_count = 3
shape       = [1920, 1080, 3]   (height, width, channels — by convention)
dtype       = "uint8"
data_len    = 1920 * 1080 * 3  = 6,220,800
data        = 6.2 MB of pixels
```

Wire overhead: 1 (type) + 4 + 24 + 4 + 5 + 4 = 42 bytes of framing around 6.2 MB. The frame is then MTU-chunked into ~4,443 `DATA` frames across one channel.

---

## 4. Worked example: `send_variant(1, Uint32(42))`

Serialize the variant (5 bytes):
```
08 2A 00 00 00
```
(type `UINT32`, then `42` as little-endian u32.)

Build a single `WHOLE` frame:
```
Header:
  F3                magic
  65                flags = (WHOLE=3 << 5) | DATA=5 = 0x60 | 0x05
  01 00 00 00       channel_id = 1
  05 00             data_size = 5
  00 00 00 00       route_link_id = 0

Payload:
  08 2A 00 00 00    UINT32(42)

Total on the wire: 17 bytes.
```

---

## 5. Resync after a bad transport

If a receiver reads a header byte that is not `0xF3`, it has lost frame alignment. Recovery: **discard the byte and keep reading**. The `0xF3` magic will reappear at the start of the next valid frame (possibly after junk), and deserialization resumes there. No state in any channel's assembly buffer needs to be cleared — a lost partial packet will simply never be delivered, and the receiver can detect this at the application layer via its own sequencing if it cares.

---

## 6. Versioning

This document describes **protocol version 1**. If an incompatible change is made (new type ID with a conflicting layout, different endianness, new required fields), implementations should negotiate out-of-band — there is no version byte in the wire format. Additive changes (new frame types with unique IDs, new variant types with unique IDs) do not require a version bump because unknown IDs are discarded (frames) or rejected (variants).
