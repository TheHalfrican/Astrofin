//! One single-slot coalescing mailbox, shared by every actor in the workspace
//! that hands work to a dedicated thread.

use parking_lot::{Condvar, Mutex};
use std::sync::Arc;

/// Shared, cloneable handle to one actor's state: a `parking_lot` mutex plus
/// the condvar its consumer blocks on. Coalescing is the state's own business
/// — the mailbox owns only the lock, the wake, and the blocking wait.
pub struct Mailbox<S> {
    inner: Arc<Shared<S>>,
}

struct Shared<S> {
    state: Mutex<S>,
    cv: Condvar,
}

impl<S> Clone for Mailbox<S> {
    fn clone(&self) -> Mailbox<S> {
        Mailbox {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S> Mailbox<S> {
    pub fn new(state: S) -> Mailbox<S> {
        Mailbox {
            inner: Arc::new(Shared {
                state: Mutex::new(state),
                cv: Condvar::new(),
            }),
        }
    }

    /// Run `f` under the lock, then wake every blocked waiter. The wake is a
    /// broadcast: several waiters can be parked on distinct predicates, and
    /// waking only one would let it swallow another's wakeup.
    pub fn update<R>(&self, f: impl FnOnce(&mut S) -> R) -> R {
        let mut state = self.inner.state.lock();
        let out = f(&mut state);
        drop(state);
        self.inner.cv.notify_all();
        out
    }

    /// Run `f` under the lock and wake nobody.
    pub fn peek<R>(&self, f: impl FnOnce(&S) -> R) -> R {
        f(&self.inner.state.lock())
    }

    /// Block until `ready` holds of the state, then run `take` under the same
    /// lock without releasing it in between.
    pub fn wait<R>(&self, ready: impl Fn(&S) -> bool, take: impl FnOnce(&mut S) -> R) -> R {
        let mut state = self.inner.state.lock();
        while !ready(&state) {
            self.inner.cv.wait(&mut state);
        }
        take(&mut state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_is_visible_to_peek() {
        let mb = Mailbox::new(0u32);
        mb.update(|s| *s = 7);
        assert_eq!(mb.peek(|s| *s), 7);
    }

    #[test]
    fn wait_returns_immediately_when_already_ready() {
        let mb = Mailbox::new(Some(3u32));
        assert_eq!(mb.wait(|s| s.is_some(), Option::take), Some(3));
        assert_eq!(mb.peek(|s| *s), None);
    }

    #[test]
    fn wait_wakes_on_update_from_another_thread() {
        let mb = Mailbox::new(None::<u32>);
        let producer = mb.clone();
        let t = std::thread::spawn(move || {
            producer.update(|s| *s = Some(9));
        });
        assert_eq!(mb.wait(|s| s.is_some(), Option::take), Some(9));
        t.join().expect("producer thread");
    }

    #[test]
    fn update_wakes_every_waiter() {
        let mb = Mailbox::new(false);
        let waiters: Vec<_> = (0..2)
            .map(|_| {
                let mb = mb.clone();
                std::thread::spawn(move || mb.wait(|s| *s, |_| ()))
            })
            .collect();
        mb.update(|s| *s = true);
        for w in waiters {
            w.join().expect("waiter thread");
        }
    }

    #[test]
    fn new_holds_the_state_it_was_given() {
        let mb = Mailbox::new(String::from("seed"));
        assert_eq!(mb.peek(|s| s.clone()), "seed");
    }

    #[test]
    fn clones_share_one_slot() {
        let mb = Mailbox::new(0u32);
        let other = mb.clone();
        other.update(|s| *s += 5);
        assert_eq!(mb.peek(|s| *s), 5);
        mb.update(|s| *s += 1);
        assert_eq!(other.peek(|s| *s), 6);
    }

    #[test]
    fn update_returns_the_closures_value() {
        let mb = Mailbox::new(vec![1u8, 2, 3]);
        let taken = mb.update(std::mem::take);
        assert_eq!(taken, vec![1, 2, 3]);
        assert!(mb.peek(Vec::is_empty));
    }

    #[test]
    fn peek_sees_the_slot_without_taking_it() {
        let mb = Mailbox::new(Some(4u32));
        assert_eq!(mb.peek(|s| *s), Some(4));
        assert_eq!(mb.peek(|s| *s), Some(4));
    }

    #[test]
    fn updates_the_consumer_never_observed_coalesce_into_the_last_one() {
        let mb = Mailbox::new(None::<u32>);
        for v in 1..=3 {
            mb.update(|s| *s = Some(v));
        }
        assert_eq!(mb.wait(|s| s.is_some(), Option::take), Some(3));
        assert_eq!(mb.peek(|s| *s), None);
    }

    #[test]
    fn wait_ignores_updates_until_its_predicate_holds() {
        let mb = Mailbox::new(0u32);
        let producer = mb.clone();
        let t = std::thread::spawn(move || {
            for v in 1..=5 {
                producer.update(|s| *s = v);
            }
        });
        // Only 5 satisfies the predicate, so no earlier value can be returned.
        assert_eq!(mb.wait(|s| *s == 5, |s| *s), 5);
        t.join().expect("producer thread");
    }

    #[test]
    fn a_full_slot_hands_every_value_across_the_thread_in_order() {
        // `wait` mutates under the lock but wakes nobody, so both sides
        // publish their side of the handshake with `update`.
        let mb = Mailbox::new(None::<u32>);
        let producer = mb.clone();
        let t = std::thread::spawn(move || {
            for v in 1..=3 {
                // Park until the consumer has drained the previous value, so
                // nothing is coalesced away.
                producer.wait(Option::is_none, |_| ());
                producer.update(|s| *s = Some(v));
            }
        });
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(mb.wait(|s| s.is_some(), Option::take));
            mb.update(|_| ());
        }
        t.join().expect("producer thread");
        assert_eq!(seen, vec![Some(1), Some(2), Some(3)]);
    }
}
