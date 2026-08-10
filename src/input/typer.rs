use tracing::info;

pub trait TextTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>>;
}

#[allow(dead_code)]
pub struct MockTyper;

impl TextTyper for MockTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        info!(text = %text, "Typing text to current window");
        Ok(())
    }
}
