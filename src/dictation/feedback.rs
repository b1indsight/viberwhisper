//! Single-use feedback keeps UI delivery independent of recognition metadata.
use anyhow::Result;
use tokio::sync::oneshot;

#[derive(Debug)]
pub(crate) struct Transcript {
    pub text: String,
    pub confirmation: Option<Confirmation>,
}

impl From<String> for Transcript {
    fn from(text: String) -> Self {
        Self {
            text,
            confirmation: None,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Confirmation(oneshot::Sender<Feedback>);

/// Accepted text plus an optional reply owned until feedback handling completes.
#[derive(Debug)]
pub(crate) struct Feedback {
    pub text: String,
    acknowledgment: Option<oneshot::Sender<u64>>,
}

impl Feedback {
    /// Acknowledge only after the owner has confirmed memory and attempted its save.
    pub fn acknowledge(self, at: u64) {
        if let Some(acknowledgment) = self.acknowledgment {
            let _ = acknowledgment.send(at);
        }
    }
}

impl Confirmation {
    pub(crate) fn channel() -> (Self, oneshot::Receiver<Feedback>) {
        let (sender, receiver) = oneshot::channel();
        (Self(sender), receiver)
    }

    /// Called once, after delivery succeeds, on the finalization worker. Dropping
    /// this handle cancels learning. The serial worker commits before its next job.
    pub fn confirm(self, text: String) -> Result<()> {
        self.0
            .send(Feedback {
                text,
                acknowledgment: None,
            })
            .map_err(|_| anyhow::anyhow!("dictation feedback worker stopped"))
    }

    /// Wait on a finalization thread for the exact Unix-seconds time used by memory.
    /// The acknowledgment covers feedback handling and a save attempt, not durable storage.
    pub fn confirm_with_ack(self, text: String) -> Result<u64> {
        let (acknowledgment, receiver) = oneshot::channel();
        self.0
            .send(Feedback {
                text,
                acknowledgment: Some(acknowledgment),
            })
            .map_err(|_| anyhow::anyhow!("dictation feedback worker stopped"))?;
        receiver
            .blocking_recv()
            .map_err(|_| anyhow::anyhow!("dictation feedback worker stopped before acknowledgment"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_does_not_wait_for_memory_io() {
        // The serial recognition worker orders memory before its next job. The
        // delivery worker only submits feedback and need not wait for a second ACK.
        let (confirmation, mut receiver) = Confirmation::channel();
        assert!(confirmation.confirm("最终输入".into()).is_ok());
        let feedback = receiver.try_recv().unwrap();
        assert_eq!(feedback.text, "最终输入");
        assert!(feedback.acknowledgment.is_none());
    }

    #[test]
    fn traced_confirmation_requires_an_owner_acknowledgment() {
        // A stopped owner must release the waiter with an error, never a made-up time.
        for handled in [true, false] {
            let (confirmation, receiver) = Confirmation::channel();
            let caller = std::thread::spawn(move || confirmation.confirm_with_ack("final".into()));
            let feedback = receiver.blocking_recv().unwrap();
            assert_eq!(feedback.text, "final");
            assert!(!caller.is_finished());
            if handled {
                feedback.acknowledge(200);
                assert_eq!(caller.join().unwrap().unwrap(), 200);
            } else {
                drop(feedback);
                assert!(caller.join().unwrap().is_err());
            }
        }
    }
}
