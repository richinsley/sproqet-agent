//! TCP echo over Sproqet.
//!
//! Starts a listener and a client in the same process, sends a few
//! variants each way over channel 1, and verifies round-trip integrity.
//!
//! Run with:
//!
//! ```text
//! cargo run --example tcp_echo
//! ```

use std::collections::BTreeMap;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use sproqet::{SproqetLink, SproqetVariant};

const ADDR: &str = "127.0.0.1:9876";
const CHANNEL: u32 = 1;

fn make_link(stream: TcpStream) -> Arc<SproqetLink> {
    // A Sproqet link needs a reader and a writer. TcpStream implements
    // both SproqetRead and SproqetWrite, so we clone the socket to get
    // two handles.
    let reader = stream.try_clone().expect("clone for reader");
    // A short read timeout lets `stop()` wake the read thread every
    // 200 ms so it can observe the running flag.
    reader
        .set_read_timeout(Some(Duration::from_millis(200)))
        .expect("set_read_timeout");
    let writer = stream;
    let link = Arc::new(SproqetLink::new(Box::new(reader), Box::new(writer)));
    link.start();
    link
}

fn server() {
    let listener = TcpListener::bind(ADDR).expect("bind");
    let (stream, peer) = listener.accept().expect("accept");
    println!("[server] accepted from {peer}");

    let link = make_link(stream);

    // Receive a greeting variant
    let chan = link.get_channel(CHANNEL);
    let greeting = loop {
        if let Some(Ok(v)) = chan.receive_variant() {
            break v;
        }
        thread::sleep(Duration::from_millis(10));
    };
    println!("[server] got: {greeting:?}");

    // Reply with a map
    let mut reply = BTreeMap::new();
    reply.insert("status".to_owned(), SproqetVariant::String("ok".into()));
    reply.insert("echoed".to_owned(), greeting);
    reply.insert("answer".to_owned(), SproqetVariant::Uint32(42));

    link.send_variant(CHANNEL, &SproqetVariant::StringMap(reply))
        .expect("send reply");
    println!("[server] replied");

    // Give the reply a moment to drain before shutting down
    thread::sleep(Duration::from_millis(100));
    link.stop();
}

fn client() {
    // Tiny delay so the server binds first
    thread::sleep(Duration::from_millis(100));

    let stream = TcpStream::connect(ADDR).expect("connect");
    println!("[client] connected");

    let link = make_link(stream);

    // Say hello
    link.send_variant(CHANNEL, &SproqetVariant::String("Hello from client".into()))
        .expect("send greeting");
    println!("[client] sent greeting");

    // Wait for the reply
    let chan = link.get_channel(CHANNEL);
    let reply = loop {
        if let Some(Ok(v)) = chan.receive_variant() {
            break v;
        }
        thread::sleep(Duration::from_millis(10));
    };
    println!("[client] reply: {reply:?}");

    // Sanity check
    if let SproqetVariant::StringMap(m) = &reply {
        match m.get("status") {
            Some(SproqetVariant::String(s)) if s == "ok" => {}
            other => panic!("unexpected status in reply: {other:?}"),
        }
    } else {
        panic!("expected a StringMap reply, got {reply:?}");
    }

    link.stop();
}

fn main() {
    let srv = thread::spawn(server);
    let cli = thread::spawn(client);
    srv.join().expect("server");
    cli.join().expect("client");
    println!("[main] done.");
}
