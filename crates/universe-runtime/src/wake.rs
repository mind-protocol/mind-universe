//! The wake path: how anything at all reaches a resident Universe from outside.
//!
//! Nothing outside the resident process may mutate the world. An outside caller
//! may only PERCEIVE it, or PERTURB the field — add energy and walk away. That
//! second verb is this module: a [`FieldInjection`] is fire-and-forget, carries
//! no authority, and returns no result. Whether the perturbation crosses a
//! threshold, wakes a construct, produces an intent or commits anything is
//! decided inside, by the physics and the write-path — never by the caller.
//!
//! The inbox is also the loop's DORMANCY mechanism. When there is no work the
//! serial loop blocks in [`WakeInbox::recv_timeout`]. It does not spin, it does
//! not tick, it does not scan for something to do: an idle city consumes the CPU
//! of a sleeping thread, which is none.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use universe_core::EntityKey;

/// One perturbation of the field.
///
/// It names a target and, optionally, that the wake is an actor's TURN. It does
/// NOT name an effect, a mutation, or a verb — an injection is energy, not a
/// command.
#[derive(Clone, Debug)]
pub struct FieldInjection {
    /// The construct whose trigger receives the deposit. Resolving this to atoms
    /// is the field's business, not the injector's.
    pub target: EntityKey,
    /// Who perturbed the field. Attribution for the trace; never authority.
    pub origin: String,
    /// When present, this wake is an actor's turn and the loop runs exactly one
    /// inference with this already-serialised observation before the turn.
    /// Assembling it is Universe work; the runtime only carries it.
    pub turn: Option<String>,
}

impl FieldInjection {
    /// A bare perturbation: energy into the field, no turn, no authority.
    pub fn perturb(target: EntityKey, origin: impl Into<String>) -> Self {
        Self {
            target,
            origin: origin.into(),
            turn: None,
        }
    }

    /// A perturbation that is an actor's turn: the loop runs one inference on
    /// `prompt` before advancing.
    pub fn turn(target: EntityKey, origin: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            target,
            origin: origin.into(),
            turn: Some(prompt.into()),
        }
    }
}

/// The outside handle. Clonable and `Send`, so an observer port, a transport, or
/// another thread can perturb the running city without holding any part of it.
#[derive(Clone, Debug)]
pub struct Injector {
    tx: Sender<FieldInjection>,
}

impl Injector {
    /// Perturb the field. Fire-and-forget: the return value reports DELIVERY to
    /// the resident process, never an outcome in the world. There is deliberately
    /// no way to learn from here whether anything fired.
    pub fn inject(&self, injection: FieldInjection) -> Delivery {
        match self.tx.send(injection) {
            Ok(()) => Delivery::Delivered,
            Err(_) => Delivery::CityGone,
        }
    }
}

/// Whether a perturbation reached the resident process. Not an outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    Delivered,
    /// The resident process is no longer listening.
    CityGone,
}

/// The inside half: the queue the serial loop waits on.
#[derive(Debug)]
pub struct WakeInbox {
    rx: Receiver<FieldInjection>,
    received: u64,
}

impl WakeInbox {
    /// Take a pending injection if one is already there. Never blocks.
    pub fn try_recv(&mut self) -> Option<FieldInjection> {
        match self.rx.try_recv() {
            Ok(injection) => {
                self.received += 1;
                Some(injection)
            }
            Err(_) => None,
        }
    }

    /// BLOCK until an injection arrives or `timeout` elapses. This is the whole
    /// of dormancy: while the city has nothing to do, its thread is parked here.
    pub fn recv_timeout(&mut self, timeout: Duration) -> WaitOutcome {
        match self.rx.recv_timeout(timeout) {
            Ok(injection) => {
                self.received += 1;
                WaitOutcome::Woken(injection)
            }
            Err(RecvTimeoutError::Timeout) => WaitOutcome::TimedOut,
            Err(RecvTimeoutError::Disconnected) => WaitOutcome::NoInjectorsLeft,
        }
    }

    pub fn received(&self) -> u64 {
        self.received
    }
}

/// What a blocking wait produced.
#[derive(Debug)]
pub enum WaitOutcome {
    Woken(FieldInjection),
    TimedOut,
    /// Every [`Injector`] has been dropped: nothing can ever perturb this field
    /// again. Reported, not silently treated as idle.
    NoInjectorsLeft,
}

/// Build the wake path: the outside handle and the inside queue.
pub fn wake_channel() -> (Injector, WakeInbox) {
    let (tx, rx) = channel();
    (Injector { tx }, WakeInbox { rx, received: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A blocking wait on an empty inbox really blocks (it does not busy-return),
    /// and it wakes when something is injected.
    #[test]
    fn the_inbox_blocks_when_empty_and_wakes_on_injection() {
        let (injector, mut inbox) = wake_channel();
        assert!(inbox.try_recv().is_none());

        let started = Instant::now();
        assert!(matches!(
            inbox.recv_timeout(Duration::from_millis(120)),
            WaitOutcome::TimedOut
        ));
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "an empty wait must actually park the thread"
        );

        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            injector.inject(FieldInjection::perturb(EntityKey(0xD001), "test"))
        });
        match inbox.recv_timeout(Duration::from_secs(5)) {
            WaitOutcome::Woken(injection) => assert_eq!(injection.target, EntityKey(0xD001)),
            other => panic!("expected a wake, got {other:?}"),
        }
        assert_eq!(handle.join().unwrap(), Delivery::Delivered);
        assert_eq!(inbox.received(), 1);
    }
}
