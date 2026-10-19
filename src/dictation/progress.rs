//! One latest-text slot and one outstanding wake-up per recording, independent of UI speed.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct State {
    text: String,
    notified: bool,
    closed: bool,
}

/// A recording-scoped progress mailbox. Closing rejects late snapshots; cancellation
/// also interrupts recognition after its result receiver has left the orchestrator.
#[derive(Clone)]
pub(crate) struct Progress {
    state: Arc<Mutex<State>>,
    notify: Arc<dyn Fn() + Send + Sync>,
    pub(super) cancelled: CancellationToken,
}

impl Progress {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            state: Arc::default(),
            notify: Arc::new(notify),
            cancelled: CancellationToken::new(),
        }
    }

    pub(super) fn publish(&self, text: String) {
        let mut state = self.state.lock().unwrap();
        if state.closed || state.text == text {
            return;
        }
        state.text = text;
        let notify = !std::mem::replace(&mut state.notified, true);
        drop(state);
        if notify {
            (self.notify)();
        }
    }

    /// Consume a wake-up, retaining the latest snapshot for destination metadata
    /// that may arrive after the first progress notification.
    pub fn latest(&self) -> String {
        let mut state = self.state.lock().unwrap();
        state.notified = false;
        state.text.clone()
    }

    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.text.clear();
    }

    pub fn cancel(&self) {
        self.close();
        self.cancelled.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn slow_consumers_coalesce_updates_and_closed_recordings_cannot_reopen() {
        let notifications = Arc::new(AtomicUsize::new(0));
        let notified = notifications.clone();
        let progress = Progress::new(move || {
            notified.fetch_add(1, Ordering::Relaxed);
        });
        for index in 0..1000 {
            progress.publish(index.to_string());
        }
        assert_eq!(notifications.load(Ordering::Relaxed), 1);
        assert_eq!(progress.latest(), "999");
        progress.publish("final".into());
        assert_eq!(notifications.load(Ordering::Relaxed), 2);
        progress.cancel();
        progress.publish("late".into());
        assert!(progress.latest().is_empty());
        assert!(progress.cancelled.is_cancelled());
        assert_eq!(notifications.load(Ordering::Relaxed), 2);
    }
}
