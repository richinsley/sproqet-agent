//! All 5 robustness tests (sync, std-only).
//!
//! Ported from the C++ `sproqet_test_suite.cpp`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use sproqet::{
    FrameType, PacketDelimiter, SproqetError, SproqetLink, SproqetNDArray, SproqetRead,
    SproqetVariant, SproqetWrite, SPROQET_MAGIC,
};
use sproqet::wire_protocol::SproqetPacketHeader;

// ── In-memory pipe with intentional 16–128 byte fragmentation ────────

struct SharedBuffer {
    data: VecDeque<u8>,
}

struct FragmentedPipeReader {
    buffer: Arc<Mutex<SharedBuffer>>,
    rng: StdRng,
}

struct FragmentedPipeWriter {
    buffer: Arc<Mutex<SharedBuffer>>,
}

struct SinkWriter;

fn make_fragmented_pipe() -> (FragmentedPipeReader, FragmentedPipeWriter) {
    let buf = Arc::new(Mutex::new(SharedBuffer {
        data: VecDeque::new(),
    }));
    (
        FragmentedPipeReader {
            buffer: buf.clone(),
            rng: StdRng::seed_from_u64(42),
        },
        FragmentedPipeWriter { buffer: buf },
    )
}

impl SproqetRead for FragmentedPipeReader {
    fn io_read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        let max_to_read: usize = self.rng.gen_range(16..=128);
        let max_to_read = std::cmp::min(max_to_read, buf.len());

        let mut sb = self.buffer.lock().unwrap();
        let mut total = 0;
        while total < max_to_read {
            match sb.data.pop_front() {
                Some(b) => {
                    buf[total] = b;
                    total += 1;
                }
                None => break,
            }
        }
        Ok(total)
    }
}

impl SproqetWrite for FragmentedPipeWriter {
    fn io_write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        let mut sb = self.buffer.lock().unwrap();
        for &b in buf {
            sb.data.push_back(b);
        }
        Ok(buf.len())
    }
}

impl SproqetWrite for SinkWriter {
    fn io_write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        Ok(buf.len())
    }
}

fn make_link_pair() -> (Arc<SproqetLink>, Arc<SproqetLink>) {
    let (r_ab, w_ab) = make_fragmented_pipe();
    let (r_ba, w_ba) = make_fragmented_pipe();

    let link_a = Arc::new(SproqetLink::new(Box::new(r_ab), Box::new(w_ba)));
    let link_b = Arc::new(SproqetLink::new(Box::new(r_ba), Box::new(w_ab)));
    link_a.start();
    link_b.start();
    (link_a, link_b)
}

fn make_receive_only_link() -> (Arc<SproqetLink>, FragmentedPipeWriter) {
    let (reader, writer) = make_fragmented_pipe();
    let link = Arc::new(SproqetLink::new(Box::new(reader), Box::new(SinkWriter)));
    link.start();
    (link, writer)
}

// ── Test 1: stream fragmentation reassembly ──────────────────────────

#[test]
fn test_stream_fragmentation() {
    let (link_a, link_b) = make_link_pair();

    let large: Vec<u8> = (0u32..10_000).map(|i| (i % 256) as u8).collect();
    link_a.send_buffer(1, &large).unwrap();

    let chan = link_b.get_channel(1);
    let mut received = None;
    for _ in 0..500 {
        if let Some(buf) = chan.receive_buffer() {
            received = Some(buf);
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    let received = received.expect("did not receive within timeout");
    assert_eq!(received.len(), 10_000);
    assert_eq!(received, large);

    link_a.stop();
    link_b.stop();
}

// ── Test 2: multiplexing isolation + thread safety ───────────────────

#[test]
fn test_multiplexing_isolation() {
    let (link_a, link_b) = make_link_pair();

    let r10 = Arc::new(AtomicU32::new(0));
    let r20 = Arc::new(AtomicU32::new(0));

    let senders: Vec<_> = [(10u32, 0u32), (10, 10_000), (20, 20_000), (20, 30_000)]
        .iter()
        .map(|&(ch, base)| {
            let la = link_a.clone();
            thread::spawn(move || {
                for i in 0..100u32 {
                    la.send_variant(ch, &SproqetVariant::Uint32(base + i)).unwrap();
                }
            })
        })
        .collect();

    let receivers: Vec<_> = [(10u32, r10.clone()), (20, r20.clone())]
        .into_iter()
        .map(|(ch, counter)| {
            let lb = link_b.clone();
            thread::spawn(move || {
                let chan = lb.get_channel(ch);
                let mut count = 0u32;
                while count < 200 {
                    if let Some(Ok(_)) = chan.receive_variant() {
                        count += 1;
                        counter.fetch_add(1, Ordering::SeqCst);
                        #[allow(clippy::manual_is_multiple_of)]
                        if count % 20 == 0 {
                            eprintln!("  ch{ch}: {count}/200");
                        }
                    } else {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            })
        })
        .collect();

    for t in senders {
        t.join().unwrap();
    }
    for t in receivers {
        t.join().unwrap();
    }

    assert_eq!(r10.load(Ordering::SeqCst), 200);
    assert_eq!(r20.load(Ordering::SeqCst), 200);

    link_a.stop();
    link_b.stop();
}

// ── Test 3: deserializer bounds + bad type ID ────────────────────────

#[test]
fn test_variant_safety() {
    // truncated string (claims 5 bytes, only 2)
    let bad = vec![0x0E, 0x05, 0x00, 0x00, 0x00, b'a', b'b'];
    assert!(matches!(
        SproqetVariant::deserialize(&bad),
        Err(SproqetError::BufferUnderflow)
    ));

    // truncated map (claims 1 entry, supplies nothing)
    let bad = vec![0x10, 0x01, 0x00, 0x00, 0x00];
    assert!(matches!(
        SproqetVariant::deserialize(&bad),
        Err(SproqetError::BufferUnderflow)
    ));

    // unknown type ID
    let bad = vec![0xFF];
    assert!(matches!(
        SproqetVariant::deserialize(&bad),
        Err(SproqetError::UnknownTypeId(0xFF))
    ));
}

#[test]
fn test_header_resync_after_garbage_byte() {
    let (link, mut writer) = make_receive_only_link();

    writer.io_write(&[0x00]).unwrap();

    let payload = vec![0x08, 0x2A, 0x00, 0x00, 0x00];
    let header = SproqetPacketHeader::new(
        PacketDelimiter::Whole,
        FrameType::Data,
        7,
        payload.len() as u16,
    );
    writer.io_write(&header.to_bytes()).unwrap();
    writer.io_write(&payload).unwrap();

    let chan = link.get_channel(7);
    let mut received = None;
    for _ in 0..200 {
        if let Some(Ok(v)) = chan.receive_variant() {
            received = Some(v);
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    match received.expect("did not resync and receive variant") {
        SproqetVariant::Uint32(v) => assert_eq!(v, 42),
        other => panic!("expected Uint32(42), got {other:?}"),
    }

    link.stop();
}

#[test]
fn test_unknown_frame_type_payload_is_skipped() {
    let (link, mut writer) = make_receive_only_link();

    let unknown_payload = [0xAA, 0xBB, 0xCC];
    let unknown_header = SproqetPacketHeader {
        magic: SPROQET_MAGIC,
        flags: ((PacketDelimiter::Whole as u8) << 5) | 0x1F,
        channel_id: 9,
        data_size: unknown_payload.len() as u16,
        route_link_id: 0,
    };
    writer.io_write(&unknown_header.to_bytes()).unwrap();
    writer.io_write(&unknown_payload).unwrap();

    let payload = vec![0x08, 0x63, 0x00, 0x00, 0x00];
    let header = SproqetPacketHeader::new(
        PacketDelimiter::Whole,
        FrameType::Data,
        9,
        payload.len() as u16,
    );
    writer.io_write(&header.to_bytes()).unwrap();
    writer.io_write(&payload).unwrap();

    let chan = link.get_channel(9);
    let mut received = None;
    for _ in 0..200 {
        if let Some(Ok(v)) = chan.receive_variant() {
            received = Some(v);
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    match received.expect("did not skip unknown frame and receive next variant") {
        SproqetVariant::Uint32(v) => assert_eq!(v, 99),
        other => panic!("expected Uint32(99), got {other:?}"),
    }

    link.stop();
}

// ── Test 4: backpressure (sender + receiver concurrent) ──────────────

#[test]
fn test_backpressure() {
    let (link_a, link_b) = make_link_pair();
    let sender_done = Arc::new(AtomicBool::new(false));
    let sd = sender_done.clone();

    let la = link_a.clone();
    let sender = thread::spawn(move || {
        for i in 0..200u32 {
            la.send_variant(99, &SproqetVariant::Uint32(i)).unwrap();
        }
        sd.store(true, Ordering::SeqCst);
    });

    let lb = link_b.clone();
    let receiver = thread::spawn(move || {
        let chan = lb.get_channel(99);
        let mut count = 0u32;
        while count < 200 {
            if let Some(Ok(_)) = chan.receive_variant() {
                count += 1;
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        count
    });

    sender.join().unwrap();
    let count = receiver.join().unwrap();
    assert_eq!(count, 200);
    assert!(sender_done.load(Ordering::SeqCst));

    link_a.stop();
    link_b.stop();
}

// ── Test 5: NDArray pass-through (HD frame) ──────────────────────────

#[test]
fn test_ndarray_passthrough() {
    let (link_a, link_b) = make_link_pair();

    let nd = SproqetNDArray {
        shape: vec![1920, 1080, 3],
        data_type: "uint8".to_string(),
        data: vec![0x55; 1920 * 1080 * 3],
    };

    link_a
        .send_variant(5, &SproqetVariant::NDArray(nd.clone()))
        .unwrap();

    let chan = link_b.get_channel(5);
    let mut received = None;
    for _ in 0..1000 {
        if let Some(Ok(v)) = chan.receive_variant() {
            received = Some(v);
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    match received.expect("did not receive NDArray within timeout") {
        SproqetVariant::NDArray(got) => {
            assert_eq!(got.shape, vec![1920, 1080, 3]);
            assert_eq!(got.data_type, "uint8");
            assert_eq!(got.data.len(), 1920 * 1080 * 3);
            assert!(got.data.iter().all(|&b| b == 0x55));
        }
        other => panic!("expected NDArray, got {other:?}"),
    }

    link_a.stop();
    link_b.stop();
}
