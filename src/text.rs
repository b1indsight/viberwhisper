/// Merge ordered transcription segments with a language-aware separator.
///
/// Chinese languages are joined without a separator. All other languages use
/// a single space. Empty segments are ignored.
pub(crate) fn merge_texts(texts: &[String], language: Option<String>) -> String {
    let separator = match language {
        Some(lang) if lang.starts_with("zh") => "",
        _ => " ",
    };

    texts
        .iter()
        .filter(|text| !text.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(separator)
}

#[cfg(test)]
mod tests {
    use super::merge_texts;

    #[test]
    fn merges_segments_with_the_language_separator() {
        let segments = vec!["hello".to_string(), String::new(), "world".to_string()];

        for (language, expected) in [
            (None, "hello world"),
            (Some("en"), "hello world"),
            (Some("zh"), "helloworld"),
            (Some("zh-CN"), "helloworld"),
        ] {
            assert_eq!(merge_texts(&segments, language.map(String::from)), expected);
        }
    }
}
