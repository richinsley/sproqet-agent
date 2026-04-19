# Sproqet

A lean peer-to-peer multiplexed binary protocol, rebuilt from scratch in idiomatic Rust.

Two dependencies, one primitive: a 12-byte wire protocol with deterministic variant serialization, fragmented-stream reassembly, bounded-blocking backpressure, and native support for multidimensional arrays as first-class citizens.

---

Sproqet is a small, opinionated point-to-point transport for multiplexed binary messages over any byte stream. It keeps the core narrow on purpose: **1:1 links, many logical channels, and backpressure that actually flows end-to-end.**

It is intentionally small. There is no built-in connection negotiation, peer discovery, keepalive layer, or multi-hop routing. The `route_link_id` field is reserved on the wire and always zero.

## What's in the box

- **12-byte wire header** (little-endian, packed) — magic + flags + channel_id + data_size + route_link_id
- **13-type [`SproqetVariant`](src/variant.rs)** — bool, int/uint 32/64, double, string, buffer, array, string-map, uint-map, and `SproqetNDArray` (shape + dtype + data)
- **Multiplexed channels** — one transport, many logical streams, thread-safe sends from many threads
- **MTU-chunked frames** (1400 B default) — large payloads are split into small packets and reassembled on the receiver
- **Deterministic serialization** — `BTreeMap` internally means two equal variants produce byte-identical output
- **Bounded backpressure** — per-channel 512-packet FIFO using `std::sync::mpsc::sync_channel`; senders block naturally when receivers can't keep up
- **Transport-agnostic** — `SproqetRead` / `SproqetWrite` traits; built-in impls for `TcpStream` and `UnixStream`
- **Standalone lockless SPSC FIFO** (`src/fifo.rs`) — a fixed-capacity ring with relaxed/acquire/release atomics on the hot path, type-level SPSC enforcement via `PhantomData<Cell<()>>`, and condvar-based blocking variants. Not required by the link, but available for callers who want it.

## What's not in the box

- Connection setup / teardown negotiation (bring your own handshake)
- Reconnection, keepalives, heartbeats
- Multi-hop routing, mesh topology, service discovery
- Encryption (use TLS, Noise, or another secure transport underneath or around it)
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

See [`examples/tcp_echo.rs`](examples/tcp_echo.rs) for a full ping-pong, and [`examples/ndarray_pipeline.rs`](examples/ndarray_pipeline.rs) for a multidimensional-array pass-through.

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

Core is done and tested. The five classical robustness tests (fragmentation reassembly, multiplexing isolation under concurrent senders, deserializer bounds, backpressure, and NDArray pass-through) all pass in ~0.2 seconds. Next steps: higher-level integrations, wrapper APIs, and real-world adoption.

## Acknowledgments

Developed in close collaboration with AI assistants during the April 2026 rewrite. The protocol design, FIFO soundness work, disciplined 2-dependency scope, and final integrated implementation all benefited from that interactive back-and-forth.

## License

MIT. See [`LICENSE`](LICENSE).
