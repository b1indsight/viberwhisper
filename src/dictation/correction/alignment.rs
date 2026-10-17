use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

struct Edit {
    before: Range<usize>,
    after: Range<usize>,
}

/// Conservative grapheme alignment. Ambiguous common subsequences and expensive
/// rewrites are rejected rather than assigning a user's edit to a guessed term.
pub(super) struct Alignment {
    edits: Vec<Edit>,
    before_len: usize,
}

impl Alignment {
    pub fn new(before: &str, after: &str) -> Option<Self> {
        let a: Vec<_> = before.grapheme_indices(true).collect();
        let b: Vec<_> = after.grapheme_indices(true).collect();
        let mut prefix = 0;
        while prefix < a.len().min(b.len()) && a[prefix].1 == b[prefix].1 {
            prefix += 1;
        }
        let mut suffix = 0;
        while suffix < (a.len() - prefix).min(b.len() - prefix)
            && a[a.len() - suffix - 1].1 == b[b.len() - suffix - 1].1
        {
            suffix += 1;
        }
        let n = a.len() - prefix - suffix;
        let m = b.len() - prefix - suffix;
        let cells = (n + 1).checked_mul(m + 1)?;
        if cells > 262_144 {
            return None;
        }
        let mut lcs = vec![0u16; cells];
        let width = m + 1;
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * width + j] = if a[prefix + i].1 == b[prefix + j].1 {
                    1 + lcs[(i + 1) * width + j + 1]
                } else {
                    lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
                };
            }
        }
        let a_byte = |i: usize| a.get(prefix + i).map_or(before.len(), |(at, _)| *at);
        let b_byte = |j: usize| b.get(prefix + j).map_or(after.len(), |(at, _)| *at);
        let (mut i, mut j) = (0, 0);
        let mut edits = Vec::new();
        while i < n || j < m {
            if i < n && j < m && a[prefix + i].1 == b[prefix + j].1 {
                i += 1;
                j += 1;
                continue;
            }
            let (start_i, start_j) = (i, j);
            while i < n || j < m {
                if i < n && j < m && a[prefix + i].1 == b[prefix + j].1 {
                    break;
                }
                if lcs[i * width + j] == 0 {
                    i = n;
                    j = m;
                    break;
                }
                let delete = lcs[(i + 1) * width + j];
                let insert = lcs[i * width + j + 1];
                if delete == insert {
                    // Insert/delete order within one replacement is immaterial.
                    // Reject a tie only when it can choose different matching anchors.
                    let length = lcs[i * width + j];
                    let matches_a = (j + 1..m).any(|k| {
                        a[prefix + i].1 == b[prefix + k].1
                            && 1 + lcs[(i + 1) * width + k + 1] == length
                    });
                    let matches_b = (i + 1..n).any(|k| {
                        a[prefix + k].1 == b[prefix + j].1
                            && 1 + lcs[(k + 1) * width + j + 1] == length
                    });
                    if matches_a || matches_b {
                        return None;
                    }
                    i += 1;
                    j += 1;
                    continue;
                }
                if delete > insert {
                    i += 1;
                } else {
                    j += 1;
                }
            }
            edits.push(Edit {
                before: a_byte(start_i)..a_byte(i),
                after: b_byte(start_j)..b_byte(j),
            });
        }
        Some(Self {
            edits,
            before_len: before.len(),
        })
    }

    /// Project a complete source range; an edit crossing a boundary has no safe owner.
    pub fn project(&self, range: Range<usize>) -> Option<Range<usize>> {
        if range != (0..self.before_len)
            && self.edits.iter().any(|e| {
                e.before.is_empty()
                    && (e.before.start == range.start || e.before.start == range.end)
            })
        {
            return None;
        }
        self.project_segment(range)
    }

    /// Project a known segment including edge insertions. The caller rejects
    /// overlapping projections when an insertion has no unique segment owner.
    pub fn project_segment(&self, range: Range<usize>) -> Option<Range<usize>> {
        Some(self.boundary(range.start, false)?..self.boundary(range.end, true)?)
    }

    /// Include adjacent inserted letters/digits in a name, leaving separators and
    /// surrounding words outside it. The caller rejects overlapping projected names.
    pub fn project_noun(&self, range: Range<usize>, after: &str) -> Option<Range<usize>> {
        let mut projected = self.project_segment(range.clone())?;
        for edit in self.edits.iter().filter(|edit| edit.before.is_empty()) {
            let inserted = &after[edit.after.clone()];
            if edit.before.start == range.start {
                projected.start =
                    edit.after.start + inserted.trim_end_matches(char::is_alphanumeric).len();
            }
            if edit.before.start == range.end {
                projected.end =
                    edit.after.end - inserted.trim_start_matches(char::is_alphanumeric).len();
            }
        }
        Some(projected)
    }

    fn boundary(&self, at: usize, end: bool) -> Option<usize> {
        let mut offset = 0isize;
        for edit in &self.edits {
            if at < edit.before.start {
                break;
            }
            if at == edit.before.start {
                return Some(if end && edit.before.is_empty() {
                    edit.after.end
                } else {
                    edit.after.start
                });
            }
            if at < edit.before.end {
                return None;
            }
            offset = edit.after.end as isize - edit.before.end as isize;
        }
        at.checked_add_signed(offset)
    }
}
