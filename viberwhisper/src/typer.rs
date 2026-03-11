use tracing::info;

pub trait TextTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>>;
}

pub struct MockTyper;

impl TextTyper for MockTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        info!(text = %text, "向当前窗口输入文字");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mock_typer_succeeds() {
        let typer = MockTyper;
        assert!(typer.type_text("hello world").is_ok());
    }
}
