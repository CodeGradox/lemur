//! Work off the main thread. Rune keeps the app's state on the main thread and asks it to
//! `poll` before each frame; work done elsewhere comes back as a closure over that state,
//! queued here and run by the next poll, with the window woken to run it.

use super::Lemur;
use rune::Waker;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// A change to make to the app's state, from another thread.
pub type Job = Box<dyn FnOnce(&mut Lemur) + Send>;

pub struct Bg {
    tx: Sender<Job>,
    rx: Receiver<Job>,
    /// Wakes the window so it polls; none until the window's set up, and in tests, which
    /// run frames themselves.
    waker: Option<Waker>,
}

/// What sends jobs back from a thread.
#[derive(Clone)]
pub struct Back {
    tx: Sender<Job>,
    waker: Option<Waker>,
}

impl Back {
    /// Queues `job` for the next poll and wakes the window. False once the window's gone,
    /// so a thread working for it can stop.
    pub fn send(&self, job: impl FnOnce(&mut Lemur) + Send + 'static) -> bool {
        let sent = self.tx.send(Box::new(job)).is_ok();
        if sent && let Some(w) = &self.waker {
            w();
        }
        sent
    }
}

impl Default for Bg {
    fn default() -> Self {
        let (tx, rx) = channel();
        Bg { tx, rx, waker: None }
    }
}

impl Bg {
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    pub fn back(&self) -> Back {
        Back { tx: self.tx.clone(), waker: self.waker.clone() }
    }

    /// Runs `work` on a thread of its own, then `then` with its result on the main thread.
    pub fn run<R: Send + 'static>(
        &self,
        work: impl FnOnce() -> R + Send + 'static,
        then: impl FnOnce(&mut Lemur, R) + Send + 'static,
    ) {
        let back = self.back();
        std::thread::spawn(move || {
            let r = work();
            let _ = back.send(move |s| then(s, r));
        });
    }

    /// Runs `then` on the main thread in `after`.
    pub fn after(&self, after: Duration, then: impl FnOnce(&mut Lemur) + Send + 'static) {
        let back = self.back();
        std::thread::spawn(move || {
            std::thread::sleep(after);
            let _ = back.send(then);
        });
    }

    /// The jobs that came back since the last poll.
    pub fn take(&self) -> Vec<Job> {
        self.rx.try_iter().collect()
    }
}
