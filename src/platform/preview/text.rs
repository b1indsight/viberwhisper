//! Word-aligned tail replacement shared by native preview editors.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use jieba_rs::Jieba;
use unicode_segmentation::UnicodeSegmentation;

/// Replace `old[start..]` with `replacement`; `start` is a UTF-8 byte boundary.
/// Native editors convert the preserved prefix to their own UTF-16/newline indices.
pub(crate) struct TextPatch<'a> {
    pub start: usize,
    pub replacement: &'a str,
}

impl<'a> TextPatch<'a> {
    pub fn between(old: &str, new: &'a str) -> Option<Self> {
        if old == new {
            return None;
        }
        let common = old
            .graphemes(true)
            .zip(new.graphemes(true))
            .take_while(|(old, new)| old == new)
            .map(|(part, _)| part.len())
            .sum::<usize>();
        let start = if common == old.len() {
            common
        } else {
            let old_words = boundaries(old);
            let new_words = boundaries(new);
            old_words
                .range(..=common)
                .rev()
                .copied()
                .find(|at| {
                    new_words.contains(at)
                        && grapheme_boundary(old, *at)
                        && grapheme_boundary(new, *at)
                })
                .unwrap_or(0)
        };
        Some(Self {
            start,
            replacement: &new[start..],
        })
    }
}

fn grapheme_boundary(text: &str, at: usize) -> bool {
    at == text.len() || text.grapheme_indices(true).any(|(index, _)| index == at)
}

fn boundaries(text: &str) -> BTreeSet<usize> {
    if text
        .chars()
        .any(|character| matches!(character, '\u{3400}'..='\u{9fff}' | '\u{20000}'..='\u{323af}'))
    {
        static JIEBA: OnceLock<Jieba> = OnceLock::new();
        let mut offset = 0;
        let mut result = BTreeSet::from([0]);
        for word in JIEBA.get_or_init(Jieba::new).cut(text, false) {
            offset += word.len();
            result.insert(offset);
        }
        result
    } else {
        text.split_word_bound_indices()
            .map(|(index, _)| index)
            .chain([text.len()])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::TextPatch;

    #[test]
    fn native_tail_patches_preserve_words_and_unicode() {
        for (old, new, prefix) in [
            ("I like coding", "I love coding", "I "),
            ("我们明天下午去北京。", "我们明天上午去上海。", "我们明天"),
            ("hello", "hello world", "hello"),
            ("hello world", "hello", "hello"),
            ("a 👩‍💻 old", "a 👩‍🔬 new", "a "),
            ("e\u{301} old", "e\u{300} new", ""),
            ("line\nold", "line\nnew", "line\n"),
            ("", "新文字", ""),
        ] {
            let patch = TextPatch::between(old, new).unwrap();
            assert_eq!(&old[..patch.start], prefix, "{old:?} -> {new:?}");
            assert_eq!(format!("{}{}", &old[..patch.start], patch.replacement), new);
        }
        assert!(TextPatch::between("相同 🎙️", "相同 🎙️").is_none());
    }
}
