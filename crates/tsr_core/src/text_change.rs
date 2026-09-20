//! Text edits (`core/textchange.go`).

use crate::TextRange;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub range: TextRange,
    pub new_text: Vec<u8>,
}

/// An edit list that cannot be applied in order: an edit starts before the
/// previous one ended, or reaches outside the text. Upstream slices without
/// checking and panics; the bounds are kept so a caller can report the same
/// failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnappliableEdits {
    pub low: i64,
    pub high: i64,
    /// The length of the text when the high bound is past it.
    pub capacity: Option<usize>,
}

impl std::fmt::Display for UnappliableEdits {
    /// The Go runtime's message for the slice expression upstream evaluates.
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.capacity {
            Some(length) => write!(
                output,
                "runtime error: slice bounds out of range [:{}] with length {length}",
                self.high
            ),
            None => write!(
                output,
                "runtime error: slice bounds out of range [{}:{}]",
                self.low, self.high
            ),
        }
    }
}

impl std::error::Error for UnappliableEdits {}

fn slice(text: &[u8], low: i64, high: i64) -> Result<&[u8], UnappliableEdits> {
    let length = text.len();
    if high < 0 || high as u64 > length as u64 {
        return Err(UnappliableEdits {
            low,
            high,
            capacity: Some(length),
        });
    }
    if low < 0 || low > high {
        return Err(UnappliableEdits {
            low,
            high,
            capacity: None,
        });
    }
    Ok(&text[low as usize..high as usize])
}

impl TextChange {
    // port: tsc/internal/core/textchange.go:TextChange.ApplyTo
    pub fn apply_to(&self, text: &[u8]) -> Result<Vec<u8>, UnappliableEdits> {
        let mut out = slice(text, 0, self.range.pos())?.to_vec();
        out.extend_from_slice(&self.new_text);
        out.extend_from_slice(slice(text, self.range.end(), text.len() as i64)?);
        Ok(out)
    }
}

/// Applies edits that are sorted and do not overlap.
// port: tsc/internal/core/textchange.go:ApplyBulkEdits
pub fn apply_bulk_edits(text: &[u8], edits: &[TextChange]) -> Result<Vec<u8>, UnappliableEdits> {
    let mut out = Vec::with_capacity(text.len());
    let mut last_end = 0i64;
    for edit in edits {
        let start = edit.range.pos();
        if start != last_end {
            out.extend_from_slice(slice(text, last_end, start)?);
        }
        out.extend_from_slice(&edit.new_text);
        last_end = edit.range.end();
    }
    out.extend_from_slice(slice(text, last_end, text.len() as i64)?);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(pos: i64, end: i64, text: &[u8]) -> TextChange {
        TextChange {
            range: TextRange::new(pos, end),
            new_text: text.to_vec(),
        }
    }

    #[test]
    fn ordered_edits_apply_and_a_single_edit_applies_alone() {
        let text = b"var x= ( { } ) ;";
        let edits = [
            edit(5, 5, b" "),
            edit(8, 9, b""),
            edit(10, 11, b""),
            edit(12, 13, b""),
        ];
        assert_eq!(apply_bulk_edits(text, &edits).unwrap(), b"var x = ({}) ;");
        assert_eq!(edit(3, 4, b"__").apply_to(b"var x").unwrap(), b"var__x");
        assert_eq!(apply_bulk_edits(text, &[]).unwrap(), text);
    }

    #[test]
    fn overlapping_edits_report_the_slice_upstream_panics_on() {
        // The pinned formatter emits these under semicolon removal.
        let error =
            apply_bulk_edits(b"0123456789", &[edit(2, 6, b""), edit(4, 8, b"")]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "runtime error: slice bounds out of range [6:4]"
        );
        let error = apply_bulk_edits(b"0123", &[edit(2, 9, b"")]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "runtime error: slice bounds out of range [9:4]"
        );
    }
}
