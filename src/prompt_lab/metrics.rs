use jieba_rs::Jieba;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use super::ProperNounAnnotation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WerOperation {
    Equal,
    Substitution,
    Deletion,
    Insertion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WerEdit {
    pub(crate) operation: WerOperation,
    pub(crate) reference: Option<String>,
    pub(crate) hypothesis: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WerScore {
    pub(crate) reference_words: u64,
    pub(crate) substitutions: u64,
    pub(crate) deletions: u64,
    pub(crate) insertions: u64,
    pub(crate) wer_percent: f64,
    pub(crate) alignment: Vec<WerEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProperNounAnnotationScore {
    pub(crate) canonical: String,
    pub(crate) expected_occurrences: u32,
    pub(crate) matched_occurrences: u32,
    pub(crate) matched_forms: Vec<String>,
    pub(crate) missed_occurrences: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProperNounScore {
    pub(crate) matched_occurrences: u64,
    pub(crate) expected_occurrences: u64,
    pub(crate) accuracy_percent: f64,
    pub(crate) annotations: Vec<ProperNounAnnotationScore>,
}

pub(crate) fn score_wer(reference: &str, hypothesis: &str) -> WerScore {
    let reference = normalize(reference);
    let hypothesis = normalize(hypothesis);
    let use_jieba = reference.chars().any(is_han);
    let reference_tokens = tokenize(&reference, use_jieba);
    let hypothesis_tokens = tokenize(&hypothesis, use_jieba);
    align_tokens(&reference_tokens, &hypothesis_tokens)
}

fn tokenize(text: &str, use_jieba: bool) -> Vec<String> {
    let tokens = if use_jieba {
        static JIEBA: OnceLock<Jieba> = OnceLock::new();
        JIEBA
            .get_or_init(Jieba::new)
            .cut(text, false)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    } else {
        text.unicode_words().map(str::to_string).collect()
    };
    tokens
        .into_iter()
        .filter(|token| token.chars().any(is_scored_character))
        .map(|token| token.to_lowercase())
        .collect()
}

fn align_tokens(reference: &[String], hypothesis: &[String]) -> WerScore {
    let row_len = hypothesis.len() + 1;
    let mut costs = vec![0_usize; (reference.len() + 1) * row_len];
    for (index, cost) in costs.iter_mut().take(row_len).enumerate() {
        *cost = index;
    }
    for index in 1..=reference.len() {
        costs[index * row_len] = index;
    }
    for reference_index in 1..=reference.len() {
        for hypothesis_index in 1..=hypothesis.len() {
            let substitution = costs[(reference_index - 1) * row_len + hypothesis_index - 1]
                + usize::from(reference[reference_index - 1] != hypothesis[hypothesis_index - 1]);
            let deletion = costs[(reference_index - 1) * row_len + hypothesis_index] + 1;
            let insertion = costs[reference_index * row_len + hypothesis_index - 1] + 1;
            costs[reference_index * row_len + hypothesis_index] =
                substitution.min(deletion).min(insertion);
        }
    }

    let mut reference_index = reference.len();
    let mut hypothesis_index = hypothesis.len();
    let mut alignment = Vec::with_capacity(reference_index.max(hypothesis_index));
    let mut substitutions = 0_u64;
    let mut deletions = 0_u64;
    let mut insertions = 0_u64;
    while reference_index > 0 || hypothesis_index > 0 {
        let current = costs[reference_index * row_len + hypothesis_index];
        if reference_index > 0
            && hypothesis_index > 0
            && reference[reference_index - 1] == hypothesis[hypothesis_index - 1]
            && current == costs[(reference_index - 1) * row_len + hypothesis_index - 1]
        {
            alignment.push(WerEdit {
                operation: WerOperation::Equal,
                reference: Some(reference[reference_index - 1].clone()),
                hypothesis: Some(hypothesis[hypothesis_index - 1].clone()),
            });
            reference_index -= 1;
            hypothesis_index -= 1;
        } else if reference_index > 0
            && hypothesis_index > 0
            && current == costs[(reference_index - 1) * row_len + hypothesis_index - 1] + 1
        {
            alignment.push(WerEdit {
                operation: WerOperation::Substitution,
                reference: Some(reference[reference_index - 1].clone()),
                hypothesis: Some(hypothesis[hypothesis_index - 1].clone()),
            });
            substitutions += 1;
            reference_index -= 1;
            hypothesis_index -= 1;
        } else if reference_index > 0
            && current == costs[(reference_index - 1) * row_len + hypothesis_index] + 1
        {
            alignment.push(WerEdit {
                operation: WerOperation::Deletion,
                reference: Some(reference[reference_index - 1].clone()),
                hypothesis: None,
            });
            deletions += 1;
            reference_index -= 1;
        } else {
            alignment.push(WerEdit {
                operation: WerOperation::Insertion,
                reference: None,
                hypothesis: Some(hypothesis[hypothesis_index - 1].clone()),
            });
            insertions += 1;
            hypothesis_index -= 1;
        }
    }
    alignment.reverse();
    let reference_words = reference.len() as u64;
    let errors = substitutions + deletions + insertions;
    let wer_percent = if reference_words == 0 {
        0.0
    } else {
        errors as f64 * 100.0 / reference_words as f64
    };
    WerScore {
        reference_words,
        substitutions,
        deletions,
        insertions,
        wer_percent,
        alignment,
    }
}

pub(crate) fn score_proper_nouns(
    hypothesis: &str,
    annotations: &[ProperNounAnnotation],
) -> ProperNounScore {
    let matched_forms = match_proper_nouns(hypothesis, annotations, true);
    let annotations = annotations
        .iter()
        .zip(matched_forms)
        .map(|(annotation, matched_forms)| {
            let matched_occurrences = matched_forms.len() as u32;
            ProperNounAnnotationScore {
                canonical: annotation.canonical.clone(),
                expected_occurrences: annotation.expected_occurrences,
                matched_occurrences,
                matched_forms,
                missed_occurrences: annotation.expected_occurrences - matched_occurrences,
            }
        })
        .collect::<Vec<_>>();
    let matched_occurrences = annotations
        .iter()
        .map(|annotation| u64::from(annotation.matched_occurrences))
        .sum();
    let expected_occurrences = annotations
        .iter()
        .map(|annotation| u64::from(annotation.expected_occurrences))
        .sum();
    let accuracy_percent = if expected_occurrences == 0 {
        0.0
    } else {
        matched_occurrences as f64 * 100.0 / expected_occurrences as f64
    };
    ProperNounScore {
        matched_occurrences,
        expected_occurrences,
        accuracy_percent,
        annotations,
    }
}

pub(super) fn count_proper_noun_occurrences(
    text: &str,
    annotations: &[ProperNounAnnotation],
) -> Vec<usize> {
    match_proper_nouns(text, annotations, false)
        .into_iter()
        .map(|forms| forms.len())
        .collect()
}

fn match_proper_nouns(
    hypothesis: &str,
    annotations: &[ProperNounAnnotation],
    cap_at_expected: bool,
) -> Vec<Vec<String>> {
    let text = normalize(hypothesis);
    let text_chars = text.chars().collect::<Vec<_>>();
    let mut candidates = Vec::new();
    for (annotation_index, annotation) in annotations.iter().enumerate() {
        for form in std::iter::once(&annotation.canonical).chain(annotation.accepted.iter()) {
            let normalized_form = normalize(form);
            let form_chars = normalized_form.chars().collect::<Vec<_>>();
            if form_chars.is_empty() || form_chars.len() > text_chars.len() {
                continue;
            }
            for start in 0..=text_chars.len() - form_chars.len() {
                let end = start + form_chars.len();
                if form_matches(
                    &text_chars[start..end],
                    &form_chars,
                    annotation.case_sensitive,
                ) && has_required_boundaries(&text_chars, start, end, &form_chars)
                {
                    candidates.push(MatchCandidate {
                        annotation_index,
                        start,
                        end,
                        form: form.clone(),
                    });
                }
            }
        }
    }
    candidates.sort_by(|left, right| {
        (right.end - right.start)
            .cmp(&(left.end - left.start))
            .then(left.start.cmp(&right.start))
            .then(left.annotation_index.cmp(&right.annotation_index))
            .then(left.form.cmp(&right.form))
    });

    let mut occupied = vec![false; text_chars.len()];
    let mut matched_forms = vec![Vec::new(); annotations.len()];
    for candidate in candidates {
        let annotation = &annotations[candidate.annotation_index];
        if (cap_at_expected
            && matched_forms[candidate.annotation_index].len()
                >= annotation.expected_occurrences as usize)
            || occupied[candidate.start..candidate.end]
                .iter()
                .any(|occupied| *occupied)
        {
            continue;
        }
        occupied[candidate.start..candidate.end].fill(true);
        matched_forms[candidate.annotation_index].push(candidate.form);
    }
    matched_forms
}

struct MatchCandidate {
    annotation_index: usize,
    start: usize,
    end: usize,
    form: String,
}

fn form_matches(text: &[char], form: &[char], case_sensitive: bool) -> bool {
    text.iter().zip(form).all(|(left, right)| {
        left == right
            || (!case_sensitive
                && left
                    .to_lowercase()
                    .to_string()
                    .eq(&right.to_lowercase().to_string()))
    })
}

fn has_required_boundaries(text: &[char], start: usize, end: usize, form: &[char]) -> bool {
    let starts_at_boundary =
        has_word_boundary(form[0], start.checked_sub(1).map(|index| text[index]));
    let ends_at_boundary = has_word_boundary(form[form.len() - 1], text.get(end).copied());
    starts_at_boundary && ends_at_boundary
}

fn has_word_boundary(edge: char, neighbor: Option<char>) -> bool {
    !edge.is_alphanumeric()
        || neighbor.is_none_or(|neighbor| !characters_share_word_class(edge, neighbor))
}

fn characters_share_word_class(left: char, right: char) -> bool {
    if is_han(left) || is_han(right) {
        is_han(left) && is_han(right)
    } else {
        left.is_alphanumeric() && right.is_alphanumeric()
    }
}

fn normalize(text: &str) -> String {
    text.nfkc().collect()
}

fn is_scored_character(character: char) -> bool {
    character.is_alphanumeric() || is_han(character)
}

fn is_han(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x20000..=0x2FA1F
    )
}

#[cfg(test)]
mod tests {
    use crate::prompt_lab::ProperNounAnnotation;

    use super::{WerOperation, score_proper_nouns, score_wer};

    #[test]
    fn wer_aligns_substitutions_deletions_and_insertions() {
        let score = score_wer(
            "start deleted anchor replace old end",
            "start anchor replace new inserted end",
        );

        assert_eq!(score.reference_words, 6);
        assert_eq!(score.substitutions, 1);
        assert_eq!(score.deletions, 1);
        assert_eq!(score.insertions, 1);
        assert_eq!(score.wer_percent, 50.0);
        assert!(
            score
                .alignment
                .iter()
                .any(|edit| edit.operation == WerOperation::Substitution)
        );
    }

    #[test]
    fn wer_uses_nfkc_case_folding_and_han_segmentation() {
        let equivalent = score_wer("ＶｉｂｅｒWhisper 使用语音输入", "viberwhisper使用语音输入");
        let changed = score_wer("我喜欢自然语言处理", "我喜欢自然处理");

        assert_eq!(equivalent.wer_percent, 0.0);
        assert!(changed.deletions + changed.substitutions > 0);
    }

    #[test]
    fn wer_can_exceed_one_hundred_percent_for_insertions() {
        let score = score_wer("one", "one extra words");

        assert_eq!(score.insertions, 2);
        assert_eq!(score.wer_percent, 200.0);
    }

    #[test]
    fn proper_nouns_accept_aliases_case_policy_and_occurrence_caps() {
        let annotations = vec![ProperNounAnnotation {
            canonical: "ViberWhisper".to_string(),
            accepted: vec!["Viber Whisper".to_string()],
            case_sensitive: false,
            expected_occurrences: 2,
        }];

        let score = score_proper_nouns(
            "VIBERWHISPER and Viber Whisper and ViberWhisper",
            &annotations,
        );

        assert_eq!(score.matched_occurrences, 2);
        assert_eq!(score.expected_occurrences, 2);
        assert_eq!(score.accuracy_percent, 100.0);
        assert_eq!(score.annotations[0].matched_forms.len(), 2);
    }

    #[test]
    fn proper_noun_alphanumeric_forms_require_token_boundaries() {
        let annotations = vec![ProperNounAnnotation {
            canonical: "Codex".to_string(),
            accepted: Vec::new(),
            case_sensitive: true,
            expected_occurrences: 1,
        }];

        let embedded = score_proper_nouns("MyCodexTool", &annotations);
        let standalone = score_proper_nouns("Use Codex now", &annotations);

        assert_eq!(embedded.matched_occurrences, 0);
        assert_eq!(standalone.matched_occurrences, 1);
    }

    #[test]
    fn proper_nouns_match_at_han_latin_boundaries() {
        let annotations = vec![
            ProperNounAnnotation {
                canonical: "OpenAI".to_string(),
                accepted: vec!["Open AI".to_string()],
                case_sensitive: false,
                expected_occurrences: 2,
            },
            ProperNounAnnotation {
                canonical: "Cargo.toml".to_string(),
                accepted: Vec::new(),
                case_sensitive: true,
                expected_occurrences: 1,
            },
            ProperNounAnnotation {
                canonical: "Visual Studio Code".to_string(),
                accepted: Vec::new(),
                case_sensitive: true,
                expected_occurrences: 1,
            },
            ProperNounAnnotation {
                canonical: "V3".to_string(),
                accepted: Vec::new(),
                case_sensitive: true,
                expected_occurrences: 1,
            },
        ];

        // Chinese STT commonly omits spaces around Latin names while preserving their spelling.
        let score = score_proper_nouns(
            "测试OPENAI的流程和OPEN AI模型，在Visual Studio Code中打开Cargo.toml，版本V3发布",
            &annotations,
        );

        assert_eq!(score.matched_occurrences, 5);
        assert!(
            score
                .annotations
                .iter()
                .all(|annotation| annotation.matched_occurrences == annotation.expected_occurrences)
        );
    }

    #[test]
    fn punctuated_and_multiword_forms_reject_latin_token_embedding() {
        let annotations = vec![
            ProperNounAnnotation {
                canonical: "Cargo.toml".to_string(),
                accepted: Vec::new(),
                case_sensitive: true,
                expected_occurrences: 1,
            },
            ProperNounAnnotation {
                canonical: "Visual Studio Code".to_string(),
                accepted: Vec::new(),
                case_sensitive: true,
                expected_occurrences: 1,
            },
        ];

        let score = score_proper_nouns("MyCargo.tomlFile MyVisual Studio CodeTool", &annotations);

        assert_eq!(score.matched_occurrences, 0);
    }

    #[test]
    fn proper_noun_overlaps_are_consumed_longest_first() {
        let annotations = vec![
            ProperNounAnnotation {
                canonical: "OpenAI Codex".to_string(),
                accepted: Vec::new(),
                case_sensitive: true,
                expected_occurrences: 1,
            },
            ProperNounAnnotation {
                canonical: "Codex".to_string(),
                accepted: Vec::new(),
                case_sensitive: true,
                expected_occurrences: 1,
            },
        ];

        let score = score_proper_nouns("OpenAI Codex", &annotations);

        assert_eq!(score.matched_occurrences, 1);
        assert_eq!(score.expected_occurrences, 2);
        assert_eq!(score.annotations[0].matched_occurrences, 1);
        assert_eq!(score.annotations[1].matched_occurrences, 0);
    }
}
