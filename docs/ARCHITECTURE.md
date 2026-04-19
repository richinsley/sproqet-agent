# Architecture

How the pieces fit together, why they're shaped the way they are, and when to reach for each primitive.

---

## Module map

```
src/
  error.rs         SproqetError                       (4 variants, thiserror-backed)
  wire_protocol.rs SproqetPacketHeader, constants     (12-byte LE header, magic 0xF3)
  variant.rs       SproqetVariant, SproqetNDArray       (13 types, BTreeMap for determinism)
  io_context.rs    SproqetRead, SproqetWrite            (3-state traits + TCP/Unix impls)
  channel.rs       SproqetChannel                     (per-channel frame assembly + 512-slot FIFO)
  link.rs          SproqetLink                        (read thread + multiplexed writes + MTU chunking)
  fifo.rs          fifo::{Producer, Consumer}       (standalone SPSC lockless ring)
```

Two dependencies total: `thiserror` for errors, `rand` for the test fixture only.

---

## The threading model

One `SproqetLink` owns:

- **Exactly one read thread**, spawned by `start()`, which reads headers + payloads from the transport and routes frames to per-channel assembly buffers.
- **A writer `Mutex`**, so many caller threads can `send_variant` / `send_buffer` concurrently. The mutex serializes header+payload pairs.
- **A channel map** (`Arc<Mutex<HashMap<u32, Arc<SproqetChannel>>>>`) so channels created on the sender side and the receiver side rendezvous by ID.

Each `SproqetChannel` owns:

- **An assembly buffer** (`Mutex<Vec<u8>>`) that accumulates fragments as `START … CONTINUATION … STOP` frames arrive.
- **A 512-slot `sync_channel`** that holds assembled packets until the application drains them.

The crucial observation: **no thread per channel**. Ten thousand logical channels cost ten thousand assembly buffers and FIFOs, not ten thousand threads.

---

## Backpressure — the end-to-end chain

The whole point of bounded FIFOs is that a slow receiver eventually slows down the sender. In Sproqet, the chain is:

```
Sender thread
    │
    ▼
SproqetLink::send_buffer              (writer mutex)
    │  chunks into MTU frames
    ▼
SproqetWrite::io_write                (transport; blocks if socket full)
    │
 =============  network / IPC  =============
    │
SproqetRead::io_read                  (transport)
    │
SproqetLink read thread
    │  parses header, reads payload
    ▼
SproqetChannel::push_frame
    │  assembles fragments
    ▼
sync_channel::send(...)             (blocks when 512 slots are full)
    │
    ▼
SproqetChannel::receive_variant       (application drains here)
```

If the application stops draining channel X:

1. `sync_channel` fills to 512. Next `send()` blocks.
2. Read thread now blocks inside `push_frame`. It can't read the next header.
3. Transport receive buffer fills up. OS-level flow control (TCP window, Unix socket water mark) kicks in.
4. Sender's `io_write` starts returning 0 (would-block). `write_exactly` spins with 100 µs sleeps.
5. Sender's `send_buffer` blocks, holding the writer mutex.

Application on the sender side that tries to push to channel X is now blocked. That is the desired outcome — data never gets lost, nothing is silently dropped, and an oversubscribed channel exerts backpressure all the way back to the upstream producer.

The read thread only ever blocks inside `push_frame` — never inside `io_read` — so sending on another channel still works as long as the transport itself is draining.

---

## Why `send_buffer` releases the writer mutex per chunk

A 6 MB NDArray is ~4,443 MTU-sized frames. If we held the writer mutex across the entire send, no other thread could emit even a small 17-byte variant on a different channel during that time. Since each frame carries its own `channel_id` in the header, **interleaving whole packets between chunks of a large packet is completely safe**: the receiver re-assembles per channel, and each channel has its own assembly buffer.

So the pattern is:

```rust
for chunk in data.chunks(MTU) {
    let mut writer = self.writer.lock().unwrap();   // lock
    write_exactly(&mut writer, &header)?;
    write_exactly(&mut writer, chunk)?;
    drop(writer);                                   // unlock
}
```

This gives real-time small-message latency even during bulk transfers, at the cost of one mutex acquire per 1,400 bytes (negligible).

---

## The two FIFOs

Sproqet actually ships two bounded FIFO implementations. They serve different roles.

### The channel FIFO (`std::sync::mpsc::sync_channel(512)`)

Used by `SproqetChannel` as the handoff between the read thread and the application. Pros:

- Zero custom code
- Provably correct (it's in `std`)
- `send()` blocks when full — exactly the backpressure semantic we need
- Multi-producer / multi-consumer safe via internal sync

This is the right tool for this job. Don't replace it.

### The standalone lockless SPSC FIFO (`src/fifo.rs`)

A separate module, not used internally by the link. A fixed-capacity ring with relaxed/acquire/release atomics on the hot path, condvars for blocking variants, and **type-enforced SPSC** via a `Producer`/`Consumer` split plus `PhantomData<Cell<()>>` to make each handle `!Sync`.

Use it when:

- You want a single-allocation lockless ring in your own code (outside Sproqet)
- You've profiled and `sync_channel` is the bottleneck (unlikely)
- You specifically want lockless atomics on the push/pop fast path

The handles cannot be cloned. Each goes to one thread. Cloning is the canonical SPSC violation; preventing it at the type level is what makes the `unsafe` internals sound.

```rust
use sproqet::fifo;

let (producer, consumer) = fifo::fifo::<u32>(1024);
std::thread::spawn(move || {
    for i in 0..10_000 { producer.push(i); }   // blocks if full
});
for _ in 0..10_000 {
    let v = consumer.pop();                    // blocks if empty
}
```

The module is self-contained; you can copy `src/fifo.rs` into another project as-is (dual with the MIT header from the repo root).

---

## Why no async

Tokio is great. So is `smol`, `async-std`, etc. Sproqet deliberately doesn't use any of them.

1. **One dependency** (`thiserror`) vs tokio's transitive tree. Build times, surface area, auditability all benefit.
2. **One thread per link** is fine for this domain. Media pipelines typically have dozens of links, not thousands. A thread-per-link scales to tens of thousands before the OS notices.
3. **Backpressure is trivially correct with blocking sends.** The async equivalents require `Notify` / `AtomicWaker` / custom future state machines — more surface area for subtle bugs.
4. **Any caller can wrap it.** If you want to drive Sproqet from an async runtime, `tokio::task::spawn_blocking` the sender and drain the receive channel with a `tokio::sync::mpsc` bridge. Conversely, you can't easily unwrap an async API into a sync one.

An async wrapper is a ~50-line exercise behind a feature flag if the need arises. Doing it inside the core would give up the zero-runtime-dep story.

---

## What the 5 robustness tests prove

1. **Stream fragmentation** (`test_stream_fragmentation`) — a 10 KB payload round-trips over a pipe that returns 16–128 random bytes per read. Proves the assembly buffer + `read_exactly` retry loop reassemble correctly under torn I/O.
2. **Multiplexing isolation** (`test_multiplexing_isolation`) — 4 sender threads on 2 channels, 2 receivers. 200 variants per channel, zero crosstalk. Proves the channel map and writer mutex are race-free.
3. **Variant safety** (`test_variant_safety`) — fuzz-style bad inputs (truncated string, truncated map, unknown type ID) all return typed errors instead of panicking or reading past the buffer.
4. **Backpressure** (`test_backpressure`) — concurrent sender and receiver, 200 variants, no deadlock, no lost messages.
5. **NDArray pass-through** (`test_ndarray_passthrough`) — a 6.2 MB HD frame (shape `[1920, 1080, 3]`, dtype `uint8`) survives the 4,443-frame round trip byte-perfect.

Plus the 4 FIFO unit tests (roundtrip, SPSC two-threads, Drop drains remaining items, backpressure blocks then resumes) exercise the standalone ring directly.

Total: 9 tests, ~0.24 seconds wall clock.
