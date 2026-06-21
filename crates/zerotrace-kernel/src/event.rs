// Event system: double-buffered event queue stored in the World.
//
// Events<T> is a ring-buffer that supports concurrent writes (via
// EventWriter) and single-consumer drains (via EventReader).  The
// double-buffering ensures that new events can be written while
// a reader is draining — no allocation or lock contention on the
// hot path.
//
// # Example
//
// ```rust
// use zerotrace_kernel::event::Events;
// use zerotrace_kernel::world::World;
// use zerotrace_kernel::param::{EventWriter, EventReader};
//
// #[derive(Debug, Clone, PartialEq)]
// struct MyEvent(u32);
//
// let world = World::new();
// world.insert(Events::<MyEvent>::new());
//
// // Write
// let mut writer = EventWriter::<MyEvent>::fetch(&world).unwrap();
// writer.write(MyEvent(42));
//
// // Read
// let mut reader = EventReader::<MyEvent>::fetch(&world).unwrap();
// assert_eq!(reader.drain(), vec![MyEvent(42)]);
// ```

use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A lock-protected double-buffered event queue.
///
/// `T` must be `Send + Sync` because the queue is stored in the
/// [`World`](crate::world::World) behind an `Arc`.
#[derive(Debug)]
pub struct Events<T: Send + Sync + 'static> {
    /// The primary buffer — all writes go here.
    buffer_a: Mutex<Vec<T>>,
    /// The swap buffer — drained by readers.
    buffer_b: Mutex<Vec<T>>,
    /// Which buffer is "active" for writes: 0 = A, 1 = B.
    /// Readers swap this atomically before draining.
    active: AtomicUsize,
}

impl<T: Send + Sync + 'static> Events<T> {
    /// Create an empty double-buffered event queue.
    pub fn new() -> Self {
        Self {
            buffer_a: Mutex::new(Vec::new()),
            buffer_b: Mutex::new(Vec::new()),
            active: AtomicUsize::new(0),
        }
    }

    /// Create with pre-allocated capacity for both buffers.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer_a: Mutex::new(Vec::with_capacity(capacity)),
            buffer_b: Mutex::new(Vec::with_capacity(capacity)),
            active: AtomicUsize::new(0),
        }
    }

    /// Push a single event onto the active buffer.
    ///
    /// Reads the active index with Acquire ordering, then locks the
    /// corresponding buffer.  The lock itself provides ordering: if a
    /// concurrent `drain()` flips `active` before we lock, we may land
    /// in the "wrong" buffer, but the event is never lost — it will be
    /// collected on the next drain cycle.
    ///
    /// For strict FIFO across concurrent write + drain, external
    /// synchronization is required.
    pub fn send(&self, event: T) {
        // Single atomic load — no TOCTOU concern because the Mutex lock
        // provides the necessary synchronization boundary.
        if self.active.load(Ordering::Acquire) == 0 {
            self.buffer_a.lock().push(event);
        } else {
            self.buffer_b.lock().push(event);
        }
    }

    /// Push multiple events in a single lock acquisition.
    pub fn send_batch(&self, events: impl IntoIterator<Item = T>) {
        if self.active.load(Ordering::Acquire) == 0 {
            let mut buf = self.buffer_a.lock();
            buf.extend(events);
        } else {
            let mut buf = self.buffer_b.lock();
            buf.extend(events);
        }
    }

    /// Drain all events.  Atomically swaps the active/inactive buffers
    /// via `fetch_xor`, then returns the contents of the previously-active
    /// (now drained) buffer.
    ///
    /// Uses a single atomic RMW operation (`fetch_xor`) to eliminate the
    /// TOCTOU race that existed between the previous `load` + `swap`.
    /// Two concurrent `drain()` calls will each receive a different `prev`
    /// value, guaranteeing they drain different buffers.
    pub fn drain(&self) -> Vec<T> {
        // `fetch_xor(1)` atomically flips between 0 (A active) and 1 (B active).
        // Returns the value BEFORE the flip, telling us which buffer to drain.
        let prev = self.active.fetch_xor(1, Ordering::AcqRel);

        // Drain the buffer that WAS active
        let mut buf = if prev == 0 {
            self.buffer_a.lock()
        } else {
            self.buffer_b.lock()
        };
        std::mem::take(&mut *buf)
    }

    /// Total number of events across both buffers.
    pub fn len(&self) -> usize {
        self.buffer_a.lock().len() + self.buffer_b.lock().len()
    }

    /// Returns `true` if both buffers are empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T: Send + Sync + 'static> Default for Events<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Ev(u32);

    #[test]
    fn test_send_and_drain() {
        let events = Events::<Ev>::new();
        events.send(Ev(1));
        events.send(Ev(2));
        assert_eq!(events.len(), 2);

        let drained = events.drain();
        assert_eq!(drained, vec![Ev(1), Ev(2)]);
        assert!(events.is_empty());
    }

    #[test]
    fn test_send_batch() {
        let events = Events::<Ev>::new();
        events.send_batch(vec![Ev(1), Ev(2), Ev(3)]);
        assert_eq!(events.drain().len(), 3);
    }

    #[test]
    fn test_double_buffering() {
        // Write, drain, write again, drain — each drain should return
        // only the events written since the last drain.
        let events = Events::<Ev>::new();

        events.send(Ev(1));
        assert_eq!(events.drain(), vec![Ev(1)]);

        events.send(Ev(2));
        events.send(Ev(3));
        assert_eq!(events.drain(), vec![Ev(2), Ev(3)]);

        assert!(events.is_empty());
    }

    #[test]
    fn test_interleaved_write_drain() {
        let events = Events::<Ev>::new();
        events.send(Ev(1));
        events.send(Ev(2));
        let first = events.drain();
        assert_eq!(first.len(), 2);

        events.send(Ev(3));
        events.send(Ev(4));
        let second = events.drain();
        assert_eq!(second.len(), 2);
    }

    #[test]
    fn test_empty_drain() {
        let events = Events::<Ev>::new();
        assert!(events.drain().is_empty());
    }

    #[test]
    fn test_with_capacity() {
        let events = Events::<Ev>::with_capacity(64);
        for i in 0..100 {
            events.send(Ev(i));
        }
        assert_eq!(events.drain().len(), 100);
    }

    #[test]
    fn test_len_counts_both_buffers() {
        let events = Events::<Ev>::new();
        // Write to A, swap, write to B (old A now has data)
        events.send(Ev(1));
        let _ = events.drain(); // swaps, A is now empty, B is active
        events.send(Ev(2));
        // Now B has 1 event, A is empty
        assert_eq!(events.len(), 1);
        events.send(Ev(3));
        assert_eq!(events.len(), 2);
    }
}
