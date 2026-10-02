use tsr_jsstring::JsString;

/// A text and its ECMAScript line starts (byte offsets, the pinned
/// `core.ECMALineStarts`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EcmaLineInfo {
    pub(crate) text: JsString,
    pub(crate) line_starts: Vec<i32>,
}

// port: tsc/internal/sourcemap/lineinfo.go:CreateECMALineInfo
pub fn create_ecma_line_info(text: JsString, line_starts: Vec<i32>) -> EcmaLineInfo {
    EcmaLineInfo { text, line_starts }
}

impl EcmaLineInfo {
    // port: tsc/internal/sourcemap/lineinfo.go:ECMALineInfo.LineCount
    pub fn line_count(&self) -> isize {
        self.line_starts.len() as isize
    }

    /// The line's text including its terminator. An out-of-range line panics,
    /// as the pinned slice index does.
    // port: tsc/internal/sourcemap/lineinfo.go:ECMALineInfo.LineText
    pub fn line_text(&self, line: isize) -> &[u8] {
        let pos = self.line_starts[line as usize];
        let end = if line + 1 < self.line_starts.len() as isize {
            self.line_starts[(line + 1) as usize]
        } else {
            self.text.len() as i32
        };
        &self.text.as_bytes()[pos as usize..end as usize]
    }

    /// The text the line starts index.
    pub fn text(&self) -> &[u8] {
        self.text.as_bytes()
    }

    /// The ECMAScript line starts.
    pub fn line_starts(&self) -> &[i32] {
        &self.line_starts
    }
}
