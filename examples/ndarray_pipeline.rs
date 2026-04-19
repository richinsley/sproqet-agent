//! NDArray pass-through over a Unix-socket Sproqet link.
//!
//! Demonstrates the `SproqetNDArray` variant — the intended carrier for
//! video frames, tensors, and audio buffers in out-of-process media
//! pipelines. A synthetic "HD frame" (shape [1920, 1080, 3], dtype
//! "uint8") is pushed from a producer process over a Unix socket pair,
//! reassembled on the consumer side, and verified byte-for-byte.
//!
//! Run with:
//!
//! ```text
//! cargo run --example ndarray_pipeline --release
//! ```

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use sproqet::{SproqetLink, SproqetNDArray, SproqetVariant};

const CHANNEL: u32 = 42;

fn make_link(stream: UnixStream) -> Arc<SproqetLink> {
    let reader = stream.try_clone().expect("clone for reader");
    // Short read timeout so stop() can wake the read thread cleanly.
    reader
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("set_read_timeout");
    let writer = stream;
    let link = Arc::new(SproqetLink::new(Box::new(reader), Box::new(writer)));
    link.start();
    link
}

fn producer(stream: UnixStream) {
    let link = make_link(stream);

    // Synthesize a single HD frame: shape [H, W, C] = [1080, 1920, 3],
    // dtype "uint8", filled with a recognisable pattern.
    let (h, w, c) = (1080usize, 1920, 3);
    let mut data = vec![0u8; h * w * c];
    for y in 0..h {
        for x in 0..w {
            let idx = (y * w + x) * c;
            data[idx]     = (x & 0xFF) as u8;         // R = x
            data[idx + 1] = (y & 0xFF) as u8;         // G = y
            data[idx + 2] = ((x ^ y) & 0xFF) as u8;   // B = x ^ y
        }
    }

    let frame = SproqetNDArray {
        shape: vec![h as u64, w as u64, c as u64],
        data_type: "uint8".into(),
        data,
    };

    println!(
        "[producer] sending {} × {} × {} frame ({:.2} MiB)",
        frame.shape[0],
        frame.shape[1],
        frame.shape[2],
        frame.data.len() as f64 / (1024.0 * 1024.0),
    );
    let t0 = Instant::now();
    link.send_variant(CHANNEL, &SproqetVariant::NDArray(frame))
        .expect("send");
    println!("[producer] send_variant returned in {:?}", t0.elapsed());

    // Let the receiver pull it through before we tear down
    thread::sleep(Duration::from_millis(200));
    link.stop();
}

fn consumer(stream: UnixStream) {
    let link = make_link(stream);
    let chan = link.get_channel(CHANNEL);

    let t0 = Instant::now();
    let v = loop {
        if let Some(Ok(v)) = chan.receive_variant() {
            break v;
        }
        thread::sleep(Duration::from_millis(5));
    };
    let elapsed = t0.elapsed();

    match v {
        SproqetVariant::NDArray(nd) => {
            println!(
                "[consumer] received shape={:?} dtype={} bytes={} in {:?}",
                nd.shape,
                nd.data_type,
                nd.data.len(),
                elapsed
            );
            assert_eq!(nd.shape, vec![1080, 1920, 3]);
            assert_eq!(nd.data_type, "uint8");

            // Spot-check a few pixels matching the pattern
            let w = 1920;
            let c = 3;
            let idx = |y: usize, x: usize| (y * w + x) * c;
            let check = |y: usize, x: usize| {
                let i = idx(y, x);
                assert_eq!(nd.data[i],     (x & 0xFF) as u8);
                assert_eq!(nd.data[i + 1], (y & 0xFF) as u8);
                assert_eq!(nd.data[i + 2], ((x ^ y) & 0xFF) as u8);
            };
            check(0, 0);
            check(539, 959);
            check(1079, 1919);
            println!("[consumer] pixel checks passed");
        }
        other => panic!("expected NDArray, got {other:?}"),
    }

    link.stop();
}

fn main() {
    // A Unix-domain socketpair gives us a connected pair in-process —
    // no bind/listen dance needed.
    let (a, b) = UnixStream::pair().expect("socketpair");

    let t_prod = thread::spawn(move || producer(a));
    let t_cons = thread::spawn(move || consumer(b));

    t_prod.join().expect("producer");
    t_cons.join().expect("consumer");
    println!("[main] done.");
}
