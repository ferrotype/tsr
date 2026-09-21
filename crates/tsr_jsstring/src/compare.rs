use crate::{
    equal_fold,
    helpers::{simple_lower, to_lower_go},
    wtf8::decode_utf8,
};
use std::cmp::Ordering;

/// port: tsc/internal/stringutil/compare.go:EquateStringCaseSensitive
pub fn equate_case_sensitive(a: &[u8], b: &[u8]) -> bool {
    a == b
}
/// port: tsc/internal/stringutil/compare.go:GetStringEqualityComparer
pub fn equality_comparer(ignore_case: bool) -> fn(&[u8], &[u8]) -> bool {
    if ignore_case {
        equal_fold
    } else {
        equate_case_sensitive
    }
}
/// port: tsc/internal/stringutil/compare.go:CompareStringsCaseSensitive
pub fn compare_case_sensitive(a: &[u8], b: &[u8]) -> Ordering {
    a.cmp(b)
}
/// port: tsc/internal/stringutil/compare.go:CompareStringsCaseInsensitive
pub fn compare_case_insensitive(mut a: &[u8], mut b: &[u8]) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    loop {
        let (ca, sa) = decode_utf8(a);
        let (cb, sb) = decode_utf8(b);
        if sa == 0 || sb == 0 {
            return sa.cmp(&sb);
        }
        let order = simple_lower(ca).cmp(&simple_lower(cb));
        if order != Ordering::Equal {
            return order;
        }
        a = &a[sa..];
        b = &b[sb..];
    }
}
/// port: tsc/internal/stringutil/compare.go:GetStringComparer
pub fn comparer(ignore_case: bool) -> fn(&[u8], &[u8]) -> Ordering {
    if ignore_case {
        compare_case_insensitive
    } else {
        compare_case_sensitive
    }
}
/// port: tsc/internal/stringutil/compare.go:CompareStringsCaseInsensitiveThenSensitive
pub fn compare_case_insensitive_then_sensitive(a: &[u8], b: &[u8]) -> Ordering {
    compare_case_insensitive(a, b).then_with(|| a.cmp(b))
}
/// port: tsc/internal/stringutil/compare.go:CompareStringsCaseInsensitiveEslintCompatible
pub fn compare_case_insensitive_eslint(a: &[u8], b: &[u8]) -> Ordering {
    if a == b {
        Ordering::Equal
    } else {
        to_lower_go(a).cmp(&to_lower_go(b))
    }
}
/// Affix lengths are byte windows even if a cut splits a UTF-8 rune.
/// port: tsc/internal/stringutil/compare.go:HasPrefix
pub fn has_prefix(text: &[u8], prefix: &[u8], case_sensitive: bool) -> bool {
    text.get(..prefix.len())
        .is_some_and(|start| equality_comparer(!case_sensitive)(start, prefix))
}
/// port: tsc/internal/stringutil/compare.go:HasSuffix
pub fn has_suffix(text: &[u8], suffix: &[u8], case_sensitive: bool) -> bool {
    suffix.len() <= text.len()
        && equality_comparer(!case_sensitive)(&text[text.len() - suffix.len()..], suffix)
}
/// port: tsc/internal/stringutil/compare.go:HasPrefixAndSuffixWithoutOverlap
pub fn has_prefix_and_suffix_without_overlap(
    text: &[u8],
    prefix: &[u8],
    suffix: &[u8],
    case_sensitive: bool,
) -> bool {
    prefix.len() <= text.len()
        && suffix.len() <= text.len() - prefix.len()
        && has_prefix(text, prefix, case_sensitive)
        && has_suffix(text, suffix, case_sensitive)
}
