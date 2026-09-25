//! API byte/UTF-16 offsets use cumulative deltas without rounding or clamping.

use crate::wtf8::decode_rune;

#[derive(Clone, Copy, Debug)]
struct PositionMapEntry {
    byte_offset: isize,
    delta: isize,
}

#[derive(Clone, Debug)]
pub struct PositionMap {
    ascii_only: bool,
    entries: Vec<PositionMapEntry>,
}

impl PositionMap {
    /// Reserved backing storage; the record itself belongs to its file owner.
    pub fn structural_bytes(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<PositionMapEntry>()
    }
    /// ComputePositionMap: one entry after each non-ASCII sequence.
    pub fn new(text: &[u8]) -> Self {
        let mut entries = Vec::new();
        let mut delta = 0;
        let mut byte_offset = 0;
        while byte_offset < text.len() {
            // port: tsc/internal/ast/positionmap.go:ComputePositionMap
            if text[byte_offset].is_ascii() {
                byte_offset += 1;
                continue;
            }
            let (rune, width) = decode_rune(&text[byte_offset..]);
            let utf16_size = if rune >= 0x10000 { 2 } else { 1 };
            delta += width as isize - utf16_size;
            byte_offset += width;
            entries.push(PositionMapEntry {
                byte_offset: byte_offset as isize,
                delta,
            });
        }
        Self {
            ascii_only: entries.is_empty(),
            entries,
        }
    }

    /// port: tsc/internal/ast/positionmap.go:PositionMap.IsAsciiOnly
    pub fn is_ascii_only(&self) -> bool {
        self.ascii_only
    }

    /// The cumulative delta applies only after a complete sequence. No clamping
    /// or boundary rounding is performed, including on negative offsets.
    ///
    /// port: tsc/internal/ast/positionmap.go:PositionMap.UTF8ToUTF16
    pub fn utf8_to_utf16(&self, offset: isize) -> isize {
        let end = self
            .entries
            .partition_point(|entry| entry.byte_offset <= offset);
        if end == 0 {
            offset
        } else {
            offset.wrapping_sub(self.entries[end - 1].delta)
        }
    }

    /// port: tsc/internal/ast/positionmap.go:PositionMap.UTF16ToUTF8
    pub fn utf16_to_utf8(&self, offset: isize) -> isize {
        let end = self
            .entries
            .partition_point(|entry| entry.byte_offset - entry.delta <= offset);
        if end == 0 {
            offset
        } else {
            offset.wrapping_add(self.entries[end - 1].delta)
        }
    }
}
