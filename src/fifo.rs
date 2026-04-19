//! Lockless single-producer / single-consumer circular FIFO.
//!
//! A fixed-capacity ring with relaxed/acquire/release atomics on the
//! hot path, and condvar-based blocking variants for back-pressure.
//!
//! ## Soundness
//!
//! SPSC is enforced **at the type level** by the `Producer` / `Consumer`
//! split returned from [`fifo`].  Each handle is `Send` (movable to a
//! thread) but `!Sync` (can't be shared by reference between threads),
//! so there can be at most one producer thread and one consumer thread
//! at any time.  This makes the lockless `head` / `tail` updates sound
//! without CAS.
//!
//! ## Notes
//!
//! - This module is *standalone*.  `SproqetChannel` itself uses the std
//!   `sync_channel`, which gives the same bounded-blocking semantics
//!   with zero custom code.  This FIFO is here for callers who want a
//!   single-allocation lockless ring — for example to wire Sproqet to a
//!   different transport, or to use the FIFO outside Sproqet entirely.

use std::alloc::{alloc, dealloc, handle_alloc_error, Layout};
use std::cell::{Cell, UnsafeCell};
use std::marker::PhantomData;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

struct Inner<T> {
    buf: *mut UnsafeCell<T>,
    capacity: usize, // requested size + 1, so empty != full
    head: AtomicUsize,
    tail: AtomicUsize,
    lock: Mutex<()>,
    cv_not_empty: Condvar,
    cv_not_full: Condvar,
}

// SAFETY: head/tail discipline (one thread updates each, with
// acquire/release ordering) means slot accesses don't overlap.  The
// raw pointer is owned, not aliased.
unsafe impl<T: Send> Send for Inner<T> {}
unsafe impl<T: Send> Sync for Inner<T> {}

impl<T> Inner<T> {
    fn new(size: usize) -> Self {
        assert!(size > 0, "FIFO size must be > 0");
        let capacity = size + 1;
        let layout = Layout::array::<UnsafeCell<T>>(capacity).expect("layout");
        let buf = unsafe { alloc(layout) as *mut UnsafeCell<T> };
        if buf.is_null() {
            handle_alloc_error(layout);
        }
        Self {
            buf,
            capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            lock: Mutex::new(()),
            cv_not_empty: Condvar::new(),
            cv_not_full: Condvar::new(),
        }
    }

    #[inline]
    fn next(&self, i: usize) -> usize {
        (i + 1) % self.capacity
    }

    fn try_push(&self, item: T) -> Result<(), T> {
        let tail = self.tail.load(Ordering::Relaxed);
        let next_tail = self.next(tail);
        let head = self.head.load(Ordering::Acquire);
        if next_tail == head {
            return Err(item); // full
        }
        // SAFETY: the producer alone writes this slot; the consumer can't
        // see it until tail is published with Release below.
        unsafe { ptr::write((*self.buf.add(tail)).get(), item) };
        self.tail.store(next_tail, Ordering::Release);
        self.cv_not_empty.notify_one();
        Ok(())
    }

    fn try_pop(&self) -> Option<T> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        if head == tail {
            return None; // empty
        }
        // SAFETY: producer published the value before bumping tail; we
        // (the sole consumer) own this slot until we bump head.
        let item = unsafe { ptr::read((*self.buf.add(head)).get()) };
        self.head.store(self.next(head), Ordering::Release);
        self.cv_not_full.notify_one();
        Some(item)
    }

    fn push_blocking(&self, mut item: T) {
        loop {
            match self.try_push(item) {
                Ok(()) => return,
                Err(it) => {
                    item = it;
                    let g = self.lock.lock().unwrap();
                    // Re-check under lock to avoid lost wakeups.
                    let tail = self.tail.load(Ordering::Relaxed);
                    let head = self.head.load(Ordering::Acquire);
                    if self.next(tail) != head {
                        drop(g);
                        continue;
                    }
                    let _guard = self.cv_not_full.wait(g).unwrap();
                }
            }
        }
    }

    fn pop_blocking(&self) -> T {
        loop {
            if let Some(item) = self.try_pop() {
                return item;
            }
            let g = self.lock.lock().unwrap();
            let head = self.head.load(Ordering::Relaxed);
            let tail = self.tail.load(Ordering::Acquire);
            if head != tail {
                drop(g);
                continue;
            }
            let _guard = self.cv_not_empty.wait(g).unwrap();
        }
    }

    fn len(&self) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        if tail >= head {
            tail - head
        } else {
            self.capacity - head + tail
        }
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        // Drop any items still in the ring.
        let mut head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        while head != tail {
            unsafe { ptr::drop_in_place((*self.buf.add(head)).get()) };
            head = (head + 1) % self.capacity;
        }
        let layout = Layout::array::<UnsafeCell<T>>(self.capacity).expect("layout");
        unsafe { dealloc(self.buf as *mut u8, layout) };
    }
}

// `PhantomData<Cell<()>>` makes the handle `Send` (movable across
// threads) but `!Sync` (cannot be shared by reference) — enforcing
// SPSC at the type level.
type NotSyncMarker = PhantomData<Cell<()>>;

pub struct Producer<T> {
    inner: Arc<Inner<T>>,
    _marker: NotSyncMarker,
}

pub struct Consumer<T> {
    inner: Arc<Inner<T>>,
    _marker: NotSyncMarker,
}

impl<T> Producer<T> {
    /// Non-blocking push.  Returns `Err(item)` if full.
    pub fn try_push(&self, item: T) -> Result<(), T> {
        self.inner.try_push(item)
    }

    /// Push, blocking until space is available.
    pub fn push(&self, item: T) {
        self.inner.push_blocking(item);
    }

    /// Approximate number of items currently in the ring.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the ring is currently empty.
    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }
}

impl<T> Consumer<T> {
    /// Non-blocking pop.  Returns `None` if empty.
    pub fn try_pop(&self) -> Option<T> {
        self.inner.try_pop()
    }

    /// Pop, blocking until an item is available.
    pub fn pop(&self) -> T {
        self.inner.pop_blocking()
    }

    /// Approximate number of items currently in the ring.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the ring is currently empty.
    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }
}

/// Create an SPSC ring with capacity `size`.
pub fn fifo<T>(size: usize) -> (Producer<T>, Consumer<T>) {
    let inner = Arc::new(Inner::new(size));
    (
        Producer {
            inner: inner.clone(),
            _marker: PhantomData,
        },
        Consumer {
            inner,
            _marker: PhantomData,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn try_push_pop_roundtrip() {
        let (p, c) = fifo::<u32>(4);
        assert_eq!(c.try_pop(), None);
        p.try_push(1).unwrap();
        p.try_push(2).unwrap();
        p.try_push(3).unwrap();
        p.try_push(4).unwrap();
        assert!(p.try_push(5).is_err()); // full
        assert_eq!(c.try_pop(), Some(1));
        assert_eq!(c.try_pop(), Some(2));
        p.try_push(5).unwrap();
        assert_eq!(c.try_pop(), Some(3));
        assert_eq!(c.try_pop(), Some(4));
        assert_eq!(c.try_pop(), Some(5));
        assert_eq!(c.try_pop(), None);
    }

    #[test]
    fn blocking_spsc_two_threads() {
        let (p, c) = fifo::<u64>(16);
        let producer = thread::spawn(move || {
            for i in 0..10_000u64 {
                p.push(i);
            }
        });
        let consumer = thread::spawn(move || {
            let mut sum = 0u64;
            for _ in 0..10_000 {
                sum += c.pop();
            }
            sum
        });
        producer.join().unwrap();
        let sum = consumer.join().unwrap();
        assert_eq!(sum, (0..10_000u64).sum::<u64>());
    }

    #[test]
    fn drop_drains_remaining_items() {
        // Use a payload with observable Drop side effect.
        use std::sync::atomic::{AtomicUsize, Ordering as Ord2};
        static DROPPED: AtomicUsize = AtomicUsize::new(0);
        struct Counted;
        impl Drop for Counted {
            fn drop(&mut self) {
                DROPPED.fetch_add(1, Ord2::SeqCst);
            }
        }
        DROPPED.store(0, Ord2::SeqCst);
        {
            let (p, _c) = fifo::<Counted>(8);
            p.try_push(Counted).ok().unwrap();
            p.try_push(Counted).ok().unwrap();
            p.try_push(Counted).ok().unwrap();
        } // both handles dropped → Inner dropped → 3 Counted dropped
        assert_eq!(DROPPED.load(Ord2::SeqCst), 3);
    }

    #[test]
    fn back_pressure_blocks_then_resumes() {
        let (p, c) = fifo::<u32>(2);
        // Fill it up
        p.try_push(1).unwrap();
        p.try_push(2).unwrap();
        // A blocking push from another thread will wait until consumer drains.
        let pj = thread::spawn(move || {
            p.push(3);
            p.push(4);
        });
        thread::sleep(Duration::from_millis(50));
        assert_eq!(c.pop(), 1);
        assert_eq!(c.pop(), 2);
        assert_eq!(c.pop(), 3);
        assert_eq!(c.pop(), 4);
        pj.join().unwrap();
    }
}
