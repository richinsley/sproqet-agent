# Sproqet

A lean peer-to-peer multiplexed binary protocol, rebuilt from scratch in idiomatic Rust.

Two dependencies, one primitive: a 12-byte wire protocol with deterministic variant serialization, fragmented-stream reassembly, bounded-blocking backpressure, and native support for multidimensional arrays (video frames, tensors) as first-class citizens.

---

## Origin

Sproqet started in 2016 as a personal experiment in lean P2P signalling — the kind of small, fast, opinionated control plane you'd want next to a media pipeline. It went through a tangled C++ prototype phase, then sat idle for years. This is the lean rewrite: **1:1 P2P multiplexing over any byte transport, with backpressure that actually flows end-to-end.**

It is intentionally small. No HELO, no ACKs, no keepalives, no mesh routing. The `route_link_id` field is reserved on the wire and always zero — if you want mesh, add it on top.

## What's in the box

- **12-byte wire header** (little-endian, packed) — magic + flags + channel_id + data_size + route_link_id
- **13-type [`SproqetVariant`](src/variant.rs)** — bool, int/uint 32/64, double, string, buffer, array, string-map, uint-map, and `SproqetNDArray` (shape + dtype + data) for media pipelines
- **Multiplexed channels** — one transport, many logical streams, thread-safe sends from many threads
- **MTU-chunked frames** (1400 B default) — a 6 MB HD frame is just 4,443 little packets to the receiver
- **Deterministic serialization** — `BTreeMap` internally means two equal variants produce byte-identical output
- **Bounded backpressure** — per-channel 512-packet FIFO using `std::sync::mpsc::sync_channel`; senders block naturally when receivers can't keep up
- **Transport-agnostic** — `SproqetRead` / `SproqetWrite` traits; built-in impls for `TcpStream` and `UnixStream`
- **Standalone lockless SPSC FIFO** (`src/fifo.rs`) — a fixed-capacity ring with relaxed/acquire/release atomics on the hot path, type-level SPSC enforcement via `PhantomData<Cell<()>>`, and condvar-based blocking variants. Not required by the link, but available for callers who want it.

## What's not in the box

- Connection setup / teardown negotiation (bring your own handshake)
- Reconnection, keepalives, heartbeats
- Multi-hop routing, mesh topology, service discovery
- Encryption (use TLS/noise on top, or route over SRT)
- Schema validation (variants are untyped on the wire; agree on shape out-of-band)

## Quick start

```rust
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use sproqet::{SproqetLink, SproqetVariant};

// listener side
let listener = TcpListener::bind("127.0.0.1:9999")?;
let (stream, _) = listener.accept()?;
let reader = stream.try_clone()?;
let writer = stream;
let link = Arc::new(SproqetLink::new(Box::new(reader), Box::new(writer)));
link.start();

// client side (in another process/thread)
let stream = TcpStream::connect("127.0.0.1:9999")?;
let reader = stream.try_clone()?;
let writer = stream;
let link = Arc::new(SproqetLink::new(Box::new(reader), Box::new(writer)));
link.start();

// send a variant on channel 1
link.send_variant(1, &SproqetVariant::String("hello".into()))?;

// receive on the peer
let chan = link.get_channel(1);
loop {
    if let Some(Ok(v)) = chan.receive_variant() {
        println!("got {:?}", v);
        break;
    }
    std::thread::sleep(std::time::Duration::from_millis(10));
}
```

See [`examples/tcp_echo.rs`](examples/tcp_echo.rs) for a full ping-pong, and [`examples/ndarray_pipeline.rs`](examples/ndarray_pipeline.rs) for a multidimensional-array (HD frame) pass-through.

## Wire protocol at a glance

```
  0       1       2       3       4       5       6       7       8       9      10      11
+-------+-------+-------+-------+-------+-------+-------+-------+-------+-------+-------+-------+
|magic  |flags  |      channel_id (u32 LE)      | data_size(LE) |       route_link_id (u32 LE)  |
+-------+-------+-------+-------+-------+-------+-------+-------+-------+-------+-------+-------+
  0xF3    ddttttt   4 bytes                          2 bytes          4 bytes (reserved, = 0)

  dd     = packet delimiter (2 bits): 00 continuation, 01 start, 10 stop, 11 whole
  ttttt  = frame type (5 bits): 0x05 = data
```

Payload follows the header. Frames ≤ 1400 B are sent as `whole`; larger payloads are chunked as `start … continuation … stop` and reassembled per-channel.

Full spec: [`docs/PROTOCOL.md`](docs/PROTOCOL.md).

## Architecture

- One read thread per link (spawned by `start()`) decodes headers, reads payloads, and routes frames by `channel_id` to per-channel assembly buffers
- Completed packets land in a 512-slot `sync_channel` per channel
- Many sender threads can call `send_variant` / `send_buffer` concurrently; a writer mutex serializes header+payload pairs, released per chunk so large payloads don't starve other channels
- Backpressure: receiver full → sender's `send()` blocks → read thread blocks in channel assembly → transport fills → writer's `send_buffer` blocks. End-to-end, no lost data.

Full writeup: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Building & testing

```bash
cargo build
cargo test                           # 5 integration tests + 4 FIFO unit tests
cargo test -- --nocapture            # see multiplexing progress output
cargo run --example tcp_echo
cargo run --example ndarray_pipeline
cargo doc --open
```

## Dependencies

- `thiserror = "2"` — ergonomic error types, zero runtime cost
- `rand = "0.8"` — **dev only**, for the fragmented-pipe test fixture

No tokio, no byteorder, no serde, no crossbeam. Just `std` and a macro crate.

## Status

Core is done and tested. The five classical robustness tests (fragmentation reassembly, multiplexing isolation under concurrent senders, deserializer bounds, backpressure, and HD-frame NDArray pass-through) all pass in ~0.2 seconds. Next steps: `pydagger` integration for out-of-process media pipelines.

## Acknowledgments

Developed collaboratively with **Claude** (Anthropic) and **Gemini** (Google) in April 2026. The protocol design is from a 2016 personal project of mine; the FIFO soundness work, the disciplined 2-dependency scope, and the final integrated implementation came out of an interactive back-and-forth between the two assistants.

## License

MIT. See [`LICENSE`](LICENSE).
