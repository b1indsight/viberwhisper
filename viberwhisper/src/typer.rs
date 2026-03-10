pub trait TextTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>>;
}

pub struct MockTyper;

impl TextTyper for MockTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        println!("[Mock Typer] 向当前窗口输入文字: {}", text);
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
