//! Append-only string pairs preserve Go's wire order; equal strings are not interned.
use ts_ast::{NodeKind, SyntaxKind};

pub(crate) struct StringTable<'a> {
    text: &'a [u8],
    other: Vec<u8>,
    pub(crate) offsets: Vec<u32>,
}
impl<'a> StringTable<'a> {
    // port: tsc/internal/api/encoder/stringtable.go:newStringTable
    pub(crate) fn new(text: &'a [u8], string_count: i64) -> Self {
        let capacity = string_count.wrapping_mul(2);
        assert!(capacity >= 0, "runtime error: makeslice: cap out of range");
        Self {
            text,
            other: Vec::new(),
            offsets: Vec::with_capacity(capacity as usize),
        }
    }
    // port: tsc/internal/api/encoder/stringtable.go:stringTable.add
    pub(crate) fn add(&mut self, text: &[u8], kind: NodeKind, pos: i32, end: i32) -> u32 {
        let index = self.offsets.len() as u32;
        if kind == SyntaxKind::SourceFile {
            self.offsets.extend([pos as u32, end as u32]);
            return index;
        }
        let mut end = i64::from(end);
        if end - i64::from(pos) > 0 && end <= self.text.len() as i64 {
            if matches!(
                kind.known(),
                Some(
                    SyntaxKind::StringLiteral
                        | SyntaxKind::TemplateTail
                        | SyntaxKind::NoSubstitutionTemplateLiteral
                )
            ) {
                end -= 1;
            }
            let start = end - text.len() as i64;
            // Deliberate contract bounds panic when inferred coordinates are invalid.
            if &self.text[start as usize..end as usize] == text {
                self.offsets.extend([start as u32, end as u32]);
                return index;
            }
        }
        let offset = self.text.len() + self.other.len();
        self.other.extend_from_slice(text);
        self.offsets
            .extend([offset as u32, (offset + text.len()) as u32]);
        index
    }
    // port: tsc/internal/api/encoder/stringtable.go:stringTable.stringLength
    pub(crate) fn string_length(&self) -> usize {
        self.text.len() + self.other.len()
    }
    // port: tsc/internal/api/encoder/stringtable.go:stringTable.encodedLength
    pub(crate) fn encoded_length(&self) -> usize {
        self.offsets.len() * 4 + self.string_length()
    }
    // port: tsc/internal/api/encoder/stringtable.go:stringTable.encode
    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        for offset in &self.offsets {
            out.extend_from_slice(&offset.to_le_bytes());
        }
        out.extend_from_slice(self.text);
        out.extend_from_slice(&self.other);
    }
}
