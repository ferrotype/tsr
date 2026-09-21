use crate::identifier_generated::{IDENTIFIER_PART, IDENTIFIER_START};
fn in_ranges(ch: i32, ranges: &[(u32, u32, u32)]) -> bool {
    let Ok(ch) = u32::try_from(ch) else {
        return false;
    };
    let index = ranges.partition_point(|&(_, end, _)| end < ch);
    ranges.get(index).is_some_and(|&(start, end, stride)| {
        ch >= start && ch <= end && (ch - start).is_multiple_of(stride)
    })
}
/// Unicode ID_Start only; scanner additions such as `$` remain in the scanner.
/// port: tsc/internal/stringutil/identifier.go:IsUnicodeIdentifierStart
pub fn is_unicode_identifier_start(ch: i32) -> bool {
    in_ranges(ch, IDENTIFIER_START)
}
/// port: tsc/internal/stringutil/identifier.go:IsUnicodeIdentifierPart
pub fn is_unicode_identifier_part(ch: i32) -> bool {
    in_ranges(ch, IDENTIFIER_PART)
}
