/// Merge ordered transcription segments with a language-aware separator.
///
/// Chinese languages are joined without a separator. All other languages use
/// a single space. Empty segments are ignored.
pub(crate) fn merge_texts(texts: &[String], language: Option<&str>) -> String {
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
    fn merges_non_chinese_segments_with_spaces() {
        let segments = vec!["hello".to_string(), "world".to_string()];

        assert_eq!(merge_texts(&segments, Some("en")), "hello world");
    }
}
