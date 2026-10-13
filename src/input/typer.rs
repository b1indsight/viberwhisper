use anyhow::Result;
use tracing::info;

pub trait TextTyper: Send + Sync {
    fn type_text(&self, text: &str) -> Result<()>;

    /// Checks a captured destination immediately before writing. Native backends
    /// override this to place validation after their focus-settling delay.
    fn type_text_checked(&self, text: &str, validate: &dyn Fn() -> Result<()>) -> Result<()> {
        validate()?;
        self.type_text(text)
    }
}

#[derive(Debug)]
pub(crate) struct PartialTextDelivery;

impl std::fmt::Display for PartialTextDelivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("文字可能已部分输入，请先核对目标内容；可复制剩余文字手动输入")
    }
}
impl std::error::Error for PartialTextDelivery {}

#[allow(dead_code)]
pub struct MockTyper;

impl TextTyper for MockTyper {
    fn type_text(&self, text: &str) -> Result<()> {
        info!(text = %text, "Typing text to current window");
        Ok(())
    }
}
