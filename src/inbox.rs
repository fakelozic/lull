//! Main's mailbox, with limits, so a busy main thread can't let a flood pile up in memory.
//! Rare, important news always fits. Floods either wait at their source or are dropped.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Waiting messages above which levels and other replaceable news are dropped.
const DROP_ABOVE: usize = 128;
/// Waiting messages above which the window and keys are made to wait.
const WAIT_ABOVE: usize = 256;

/// The sending side. Clone one for each source.
pub struct Sender<T> {
    inner: mpsc::Sender<T>,
    waiting: Arc<AtomicUsize>,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone(), waiting: Arc::clone(&self.waiting) }
    }
}

impl<T> Sender<T> {
    /// For rare news that must arrive (paused, lost, switched).
    pub fn send(&self, message: T) {
        self.waiting.fetch_add(1, Ordering::Relaxed);
        if self.inner.send(message).is_err() {
            // Main is exiting. Nothing is left to tell.
            self.waiting.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// For news the next message replaces (levels). Never waits, so the audio thread may use it.
    pub fn send_or_drop(&self, message: T) {
        if self.waiting.load(Ordering::Relaxed) < DROP_ABOVE {
            self.send(message);
        }
    }

    /// For input: holds the source back while main is behind. Never use on the audio or main thread.
    pub fn send_or_wait(&self, message: T) {
        let give_up = Instant::now() + Duration::from_secs(10);
        while self.waiting.load(Ordering::Relaxed) >= WAIT_ABOVE {
            if Instant::now() >= give_up {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.send(message);
    }
}

/// The receiving side.
pub struct Receiver<T> {
    inner: mpsc::Receiver<T>,
    waiting: Arc<AtomicUsize>,
}

impl<T> Receiver<T> {
    /// Waits up to `timeout` for a message.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        let message = self.inner.recv_timeout(timeout)?;
        self.waiting.fetch_sub(1, Ordering::Relaxed);
        Ok(message)
    }
}

/// A new mailbox.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (sender, receiver) = mpsc::channel();
    let waiting = Arc::new(AtomicUsize::new(0));
    (Sender { inner: sender, waiting: Arc::clone(&waiting) }, Receiver { inner: receiver, waiting })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floods_are_dropped_but_important_news_still_fits() {
        let (sender, receiver) = channel();
        (0..100_000).for_each(|_| sender.send_or_drop(0));
        sender.send(1);
        let mut got = Vec::new();
        while let Ok(message) = receiver.recv_timeout(Duration::ZERO) {
            got.push(message);
        }
        assert_eq!((got.len(), got.last()), (DROP_ABOVE + 1, Some(&1)));
    }
}
