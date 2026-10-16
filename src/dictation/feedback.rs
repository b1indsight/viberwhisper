//! Single-use feedback keeps UI delivery independent of recognition metadata.
use anyhow::{Context, Result};
use std::{sync::mpsc, time::Duration};
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

#[derive(Debug)]
pub(crate) struct Feedback {
    pub text: String,
    pub acknowledged: mpsc::SyncSender<Result<()>>,
}

impl Confirmation {
    pub(crate) fn channel() -> (Self, oneshot::Receiver<Feedback>) {
        let (sender, receiver) = oneshot::channel();
        (Self(sender), receiver)
    }

    /// Called once, after delivery succeeds, on the finalization worker. Dropping
    /// this handle cancels learning. An acknowledgement orders the next recording.
    pub fn confirm(self, text: String) -> Result<()> {
        let (acknowledged, response) = mpsc::sync_channel(1);
        self.0
            .send(Feedback { text, acknowledged })
            .map_err(|_| anyhow::anyhow!("dictation feedback worker stopped"))?;
        response
            .recv_timeout(Duration::from_secs(2))
            .context("dictation feedback acknowledgement unavailable")?
    }
}
