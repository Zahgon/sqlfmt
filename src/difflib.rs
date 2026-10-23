//! Sequence matching for `--diff` output and option suggestions.
//!
//! A longest-matching-block sequence matcher ("Ratcliff/Obershelp" with the popular
//! element heuristic for sequences of 200 items or more), the unified diff format built
//! on it, and close-match suggestions. Diff hunks and suggestions depend on exactly
//! which blocks the matcher picks, so the algorithm is followed step by step.

use std::collections::HashMap;
use std::hash::Hash;

/// Compares two sequences of hashable items.
pub(crate) struct SequenceMatcher<'a, T: Eq + Hash> {
    a: &'a [T],
    b: &'a [T],
    b2j: HashMap<&'a T, Vec<usize>>,
}

/// An opcode: (tag, i1, i2, j1, j2).
pub(crate) type Opcode = (&'static str, usize, usize, usize, usize);

impl<'a, T: Eq + Hash> SequenceMatcher<'a, T> {
    pub(crate) fn new(a: &'a [T], b: &'a [T]) -> SequenceMatcher<'a, T> {
        let mut b2j: HashMap<&'a T, Vec<usize>> = HashMap::new();
        for (i, elt) in b.iter().enumerate() {
            b2j.entry(elt).or_default().push(i);
        }
        // items that occur in more than 1% of a long sequence are "popular" and are
        // never used to start a match
        let n = b.len();
        if n >= 200 {
            let ntest = n / 100 + 1;
            b2j.retain(|_, idxs| idxs.len() <= ntest);
        }
        SequenceMatcher { a, b, b2j }
    }

    /// The longest matching block in `a[alo..ahi]` and `b[blo..bhi]`: (i, j, size),
    /// preferring the earliest start in `a`, then in `b`.
    fn find_longest_match(&self, alo: usize, ahi: usize, blo: usize, bhi: usize) -> (usize, usize, usize) {
        let (a, b) = (self.a, self.b);
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, item) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = self.b2j.get(item) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j.checked_sub(1).and_then(|p| j2len.get(&p)).copied().unwrap_or(0) + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        // extend the match with equal items on both sides (there are no junk items)
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi && bestj + bestsize < bhi && a[besti + bestsize] == b[bestj + bestsize] {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    }

    /// The matching blocks, sorted, adjacent blocks joined, ending with (len a, len b, 0).
    pub(crate) fn get_matching_blocks(&self) -> Vec<(usize, usize, usize)> {
        let (la, lb) = (self.a.len(), self.b.len());
        let mut queue = vec![(0, la, 0, lb)];
        let mut blocks = Vec::new();
        while let Some((alo, ahi, blo, bhi)) = queue.pop() {
            let (i, j, k) = self.find_longest_match(alo, ahi, blo, bhi);
            if k > 0 {
                blocks.push((i, j, k));
                if alo < i && blo < j {
                    queue.push((alo, i, blo, j));
                }
                if i + k < ahi && j + k < bhi {
                    queue.push((i + k, ahi, j + k, bhi));
                }
            }
        }
        blocks.sort_unstable();
        let (mut i1, mut j1, mut k1) = (0, 0, 0);
        let mut joined = Vec::new();
        for (i2, j2, k2) in blocks {
            if i1 + k1 == i2 && j1 + k1 == j2 {
                k1 += k2;
            } else {
                if k1 > 0 {
                    joined.push((i1, j1, k1));
                }
                (i1, j1, k1) = (i2, j2, k2);
            }
        }
        if k1 > 0 {
            joined.push((i1, j1, k1));
        }
        joined.push((la, lb, 0));
        joined
    }

    pub(crate) fn get_opcodes(&self) -> Vec<Opcode> {
        let (mut i, mut j) = (0, 0);
        let mut answer = Vec::new();
        for (ai, bj, size) in self.get_matching_blocks() {
            let tag = if i < ai && j < bj {
                "replace"
            } else if i < ai {
                "delete"
            } else if j < bj {
                "insert"
            } else {
                ""
            };
            if !tag.is_empty() {
                answer.push((tag, i, ai, j, bj));
            }
            i = ai + size;
            j = bj + size;
            if size > 0 {
                answer.push(("equal", ai, i, bj, j));
            }
        }
        answer
    }

    /// Opcodes grouped into hunks with up to `n` lines of context.
    pub(crate) fn get_grouped_opcodes(&self, n: usize) -> Vec<Vec<Opcode>> {
        let mut codes = self.get_opcodes();
        if codes.is_empty() {
            codes.push(("equal", 0, 1, 0, 1));
        }
        if codes[0].0 == "equal" {
            let (tag, i1, i2, j1, j2) = codes[0];
            codes[0] = (tag, i1.max(i2.saturating_sub(n)), i2, j1.max(j2.saturating_sub(n)), j2);
        }
        let last = codes.len() - 1;
        if codes[last].0 == "equal" {
            let (tag, i1, i2, j1, j2) = codes[last];
            codes[last] = (tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n));
        }
        let nn = n + n;
        let mut groups = Vec::new();
        let mut group = Vec::new();
        for (tag, mut i1, i2, mut j1, j2) in codes {
            if tag == "equal" && i2 - i1 > nn {
                group.push((tag, i1, i2.min(i1 + n), j1, j2.min(j1 + n)));
                groups.push(std::mem::take(&mut group));
                i1 = i1.max(i2.saturating_sub(n));
                j1 = j1.max(j2.saturating_sub(n));
            }
            group.push((tag, i1, i2, j1, j2));
        }
        if !group.is_empty() && !(group.len() == 1 && group[0].0 == "equal") {
            groups.push(group);
        }
        groups
    }

    /// 2.0 * matches / total length (1.0 for two empty sequences).
    pub(crate) fn ratio(&self) -> f64 {
        let matches: usize = self.get_matching_blocks().iter().map(|b| b.2).sum();
        calculate_ratio(matches, self.a.len() + self.b.len())
    }

    /// An upper bound on [`SequenceMatcher::ratio`] from item counts.
    pub(crate) fn quick_ratio(&self) -> f64 {
        let mut fullbcount: HashMap<&T, i64> = HashMap::new();
        for elt in self.b {
            *fullbcount.entry(elt).or_insert(0) += 1;
        }
        let mut avail: HashMap<&T, i64> = HashMap::new();
        let mut matches = 0;
        for elt in self.a {
            let numb = match avail.get(elt) {
                Some(&n) => n,
                None => fullbcount.get(elt).copied().unwrap_or(0),
            };
            avail.insert(elt, numb - 1);
            if numb > 0 {
                matches += 1;
            }
        }
        calculate_ratio(matches, self.a.len() + self.b.len())
    }

    /// An upper bound on [`SequenceMatcher::ratio`] from the lengths alone.
    pub(crate) fn real_quick_ratio(&self) -> f64 {
        let (la, lb) = (self.a.len(), self.b.len());
        calculate_ratio(la.min(lb), la + lb)
    }
}

fn calculate_ratio(matches: usize, length: usize) -> f64 {
    if length > 0 {
        2.0 * matches as f64 / length as f64
    } else {
        1.0
    }
}

fn format_range_unified(start: usize, stop: usize) -> String {
    let mut beginning = start + 1;
    let length = stop - start;
    if length == 1 {
        return beginning.to_string();
    }
    if length == 0 {
        beginning -= 1;
    }
    format!("{beginning},{length}")
}

/// The lines of a unified diff of two lists of lines (each line keeps its line end).
pub(crate) fn unified_diff(
    a: &[&str],
    b: &[&str],
    fromfile: &str,
    tofile: &str,
    fromfiledate: &str,
    tofiledate: &str,
    n: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    let matcher = SequenceMatcher::new(a, b);
    for (index, group) in matcher.get_grouped_opcodes(n).into_iter().enumerate() {
        if index == 0 {
            let fromdate = if fromfiledate.is_empty() { String::new() } else { format!("\t{fromfiledate}") };
            let todate = if tofiledate.is_empty() { String::new() } else { format!("\t{tofiledate}") };
            out.push(format!("--- {fromfile}{fromdate}\n"));
            out.push(format!("+++ {tofile}{todate}\n"));
        }
        let (first, last) = (group[0], group[group.len() - 1]);
        out.push(format!(
            "@@ -{} +{} @@\n",
            format_range_unified(first.1, last.2),
            format_range_unified(first.3, last.4)
        ));
        for (tag, i1, i2, j1, j2) in group {
            if tag == "equal" {
                out.extend(a[i1..i2].iter().map(|l| format!(" {l}")));
                continue;
            }
            if tag == "replace" || tag == "delete" {
                out.extend(a[i1..i2].iter().map(|l| format!("-{l}")));
            }
            if tag == "replace" || tag == "insert" {
                out.extend(b[j1..j2].iter().map(|l| format!("+{l}")));
            }
        }
    }
    out
}

/// Up to `n` of `possibilities` that are close enough to `word` (similarity ratio at
/// least `cutoff`), best first.
pub(crate) fn get_close_matches(word: &str, possibilities: &[&str], n: usize, cutoff: f64) -> Vec<String> {
    let word_chars: Vec<char> = word.chars().collect();
    let mut result: Vec<(f64, String)> = Vec::new();
    for x in possibilities {
        let x_chars: Vec<char> = x.chars().collect();
        let s = SequenceMatcher::new(&x_chars, &word_chars);
        if s.real_quick_ratio() >= cutoff && s.quick_ratio() >= cutoff && s.ratio() >= cutoff {
            result.push((s.ratio(), x.to_string()));
        }
    }
    result.sort_by(|p, q| q.0.total_cmp(&p.0).then_with(|| q.1.cmp(&p.1)));
    result.truncate(n);
    result.into_iter().map(|(_, x)| x).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_diff_of_changed_line() {
        let a = ["select 1\n", "from a\n"];
        let b = ["select 2\n", "from a\n"];
        let diff = unified_diff(&a, &b, "x.sql", "x.sql", "(Source Query)", "(Formatted Query)", 3);
        assert_eq!(
            diff.concat(),
            "--- x.sql\t(Source Query)\n+++ x.sql\t(Formatted Query)\n@@ -1,2 +1,2 @@\n-select 1\n+select 2\n from a\n"
        );
    }

    #[test]
    fn identical_sequences_have_no_diff() {
        let a = ["a\n"];
        assert!(unified_diff(&a, &a, "f", "f", "", "", 3).is_empty());
    }

    #[test]
    fn close_matches() {
        let opts = ["--verbose", "--version", "--check", "--diff"];
        assert_eq!(get_close_matches("--bogus", &opts, 3, 0.6), vec!["--verbose"]);
        assert!(get_close_matches("--zzzzzzzz", &opts, 3, 0.6).is_empty());
    }
}
