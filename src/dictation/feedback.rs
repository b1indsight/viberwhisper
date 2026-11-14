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
pub(crate) struct Confirmation(oneshot::Sender<String>);

impl Confirmation {
    pub(crate) fn channel() -> (Self, oneshot::Receiver<String>) {
        let (sender, receiver) = oneshot::channel();
        (Self(sender), receiver)
    }

    /// Called once, after delivery succeeds, on the finalization worker. Dropping
    /// this handle cancels learning. The serial worker commits before its next job.
    pub fn confirm(self, text: String) -> Result<()> {
        self.0
            .send(text)
            .map_err(|_| anyhow::anyhow!("dictation feedback worker stopped"))
    }
}
