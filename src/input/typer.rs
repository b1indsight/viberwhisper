use anyhow::Result;
use tracing::info;

pub trait TextTyper: Send + Sync {
    fn type_text(&self, text: &str) -> Result<()>;
}

#[allow(dead_code)]
pub struct MockTyper;

impl TextTyper for MockTyper {
    fn type_text(&self, text: &str) -> Result<()> {
        info!(text = %text, "Typing text to current window");
        Ok(())
    }
}
