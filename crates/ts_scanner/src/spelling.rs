//! The pinned Go suggestion distance and simple-fold comparison.

use ts_jsstring::{helpers::to_lower_go, wtf8::decode_utf8};

pub use ts_jsstring::equal_fold;

fn runes(mut bytes: &[u8]) -> Vec<i32> {
    let mut result = Vec::with_capacity(bytes.len());
    while !bytes.is_empty() {
        let (rune, width) = decode_utf8(bytes);
        result.push(rune);
        bytes = &bytes[width..];
    }
    result
}

/// port: tsc/internal/core/core.go:GetSpellingSuggestionForStrings
pub fn get_spelling_suggestion_for_strings<'candidate>(
    name: &[u8],
    candidates: impl IntoIterator<Item = &'candidate [u8]>,
) -> Option<&'candidate [u8]> {
    get_spelling_suggestion(name, candidates, |value| value, Ord::cmp, 0)
}

/// The same distance search with the caller's identity tie-breaker. A nonzero
/// candidate limit abandons the suggestion if the iterator exceeds the limit.
/// port: tsc/internal/core/core.go:GetSpellingSuggestionWithMaxCandidateCount
pub fn get_spelling_suggestion<'name, T: Copy>(
    name: &[u8],
    candidates: impl IntoIterator<Item = T>,
    get_name: impl Fn(T) -> &'name [u8],
    compare: impl Fn(T, T) -> std::cmp::Ordering,
    maximum_candidates: usize,
) -> Option<T> {
    let name_runes = runes(name);
    let name_lower = runes(&to_lower_go(name));
    let maximum_length_difference = 2.max((name_runes.len() as f64 * 0.34) as usize);
    let mut best_distance = (name_runes.len() as f64 * 0.4).floor() + 0.9;
    let mut best_candidate: Option<T> = None;
    let mut buffers = DistanceBuffers::default();
    for (index, candidate_value) in candidates.into_iter().enumerate() {
        if maximum_candidates != 0 && index >= maximum_candidates {
            return None;
        }
        let candidate = get_name(candidate_value);
        // The source deliberately compares candidate BYTES with input RUNES.
        if candidate.is_empty()
            || candidate.len().abs_diff(name_runes.len()) > maximum_length_difference
            || candidate == name
            || (candidate.len() < 3 && !equal_fold(candidate, name))
        {
            continue;
        }
        let distance = levenshtein_with_max(
            &mut buffers,
            &name_runes,
            &runes(candidate),
            &name_lower,
            &runes(&to_lower_go(candidate)),
            best_distance,
        );
        if distance < 0.0 {
            continue;
        }
        assert!(
            distance <= best_distance,
            "Debug failure. False expression."
        );
        if distance < best_distance {
            best_distance = distance;
            best_candidate = Some(candidate_value);
        } else if best_candidate.is_none_or(|best| compare(candidate_value, best).is_lt()) {
            best_candidate = Some(candidate_value);
        }
    }
    best_candidate
}

#[derive(Default)]
struct DistanceBuffers {
    previous: Vec<f64>,
    current: Vec<f64>,
}

/// port: tsc/internal/core/core.go:levenshteinWithMax
fn levenshtein_with_max(
    buffers: &mut DistanceBuffers,
    left: &[i32],
    right: &[i32],
    left_lower: &[i32],
    right_lower: &[i32],
    max_value: f64,
) -> f64 {
    buffers.previous.resize(right.len() + 1, 0.0);
    buffers.current.resize(right.len() + 1, 0.0);
    let big = max_value + 0.01;
    for (index, value) in buffers.previous.iter_mut().enumerate() {
        *value = index as f64;
    }
    for i in 1..=left.len() {
        let min_j = ((i as f64 - max_value).ceil() as i64).max(1) as usize;
        let max_j = ((max_value + i as f64).floor() as usize).min(right.len());
        let mut column_min = i as f64;
        buffers.current[0] = column_min;
        for j in 1..min_j {
            buffers.current[j] = big;
        }
        for j in min_j..=max_j {
            let substitution = buffers.previous[j - 1]
                + if left_lower[i - 1] == right_lower[j - 1] {
                    0.1
                } else {
                    2.0
                };
            let distance = if left[i - 1] == right[j - 1] {
                buffers.previous[j - 1]
            } else {
                (buffers.previous[j] + 1.0).min((buffers.current[j - 1] + 1.0).min(substitution))
            };
            buffers.current[j] = distance;
            column_min = column_min.min(distance);
        }
        for j in max_j + 1..=right.len() {
            buffers.current[j] = big;
        }
        if column_min > max_value {
            return -1.0;
        }
        std::mem::swap(&mut buffers.previous, &mut buffers.current);
    }
    let result = buffers.previous[right.len()];
    if result > max_value {
        -1.0
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{equal_fold, get_spelling_suggestion_for_strings};

    #[test]
    fn suggestions_use_caller_identity_and_abandon_an_over_limit_search() {
        let candidates = [(10, b"abYde".as_slice()), (20, b"abXde".as_slice())];
        let suggest = |limit| {
            super::get_spelling_suggestion(
                b"abcde",
                candidates,
                |entry| entry.1,
                |a, b| a.0.cmp(&b.0),
                limit,
            )
        };
        // Equal distances use the caller's identity order, not spelling order.
        assert_eq!(suggest(0), Some(candidates[0]));
        assert_eq!(suggest(2), Some(candidates[0]));
        // Exceeding the bound abandons even a previously selected candidate.
        assert_eq!(suggest(1), None);
    }

    type SuggestionCase<'a> = (&'a [u8], &'a [&'a [u8]], Option<&'a [u8]>);

    #[test]
    fn pinned_go_suggestion_discriminators() {
        // Independently observed through core.GetSpellingSuggestionForStrings.
        let cases: &[SuggestionCase<'_>] = &[
            (b"Scrpt", &[b"Script"], Some(b"Script")),
            (b"sc", &[b"SC"], Some(b"SC")),
            ("İ".as_bytes(), &[b"i"], None),
            ("\u{212a}".as_bytes(), &[b"k"], Some(b"k")),
            ("ſ".as_bytes(), &[b"s"], None),
            ("𐐀xxx".as_bytes(), &["𐐨xxx".as_bytes()], None),
            (b"abcde", &[b"abYde", b"abXde"], Some(b"abXde")),
            (&[0xff], &[&[0xfe]], Some(&[0xfe])),
        ];
        for &(name, candidates, expected) in cases {
            assert_eq!(
                get_spelling_suggestion_for_strings(name, candidates.iter().copied()),
                expected
            );
        }
    }

    #[test]
    fn simple_fold_is_distinct_from_lowercase_and_keeps_raw_inputs() {
        assert!(!equal_fold(b"i", "İ".as_bytes()));
        assert!(equal_fold(b"k", "\u{212a}".as_bytes()));
        assert!(equal_fold(b"s", "ſ".as_bytes()));
        assert!(equal_fold(&[0xff], &[0xfe]));
        assert!(!equal_fold(&[0xff], &[0xfe, 0xfd]));
    }
}
