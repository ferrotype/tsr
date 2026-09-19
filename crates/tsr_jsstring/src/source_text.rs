//! BOM decoding for immutable compiler source bytes.

use std::ops::Range;
use std::sync::Arc;

use crate::jsstring::{JsString, Validity};

/// Immutable parser text, including raw malformed UTF-8.
#[derive(Clone, Debug, Default)]
pub struct SourceText(JsString);

impl SourceText {
    /// Already loaded text supplied directly to the Go parser. No BOM decoding.
    /// Use `from_bytes` at a physical/virtual file-loader boundary instead.
    pub fn from_loaded_bytes(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self(JsString::from_bytes(bytes))
    }

    /// port: tsc/internal/vfs/internal/internal.go:decodeBytes
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Self {
        let bytes = bytes.into();
        if bytes.starts_with(&[0xff, 0xfe]) {
            Self(JsString::from_bytes(decode_utf16(&bytes[2..], true)))
        } else if bytes.starts_with(&[0xfe, 0xff]) {
            Self(JsString::from_bytes(decode_utf16(&bytes[2..], false)))
        } else {
            let start = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
                3
            } else {
                0
            };
            let end = bytes.len();
            Self(JsString::from_shared_range(bytes, start..end))
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
    /// The whole shared backing allocation; slices of this text alias it.
    pub fn backing_bytes(&self) -> &[u8] {
        self.0.backing_bytes()
    }
    pub fn as_str(&self) -> Option<&str> {
        self.0.as_str()
    }
    pub fn validity(&self) -> Validity {
        self.0.validity()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn slice(&self, range: Range<usize>) -> Option<JsString> {
        self.0.slice(range)
    }
}

/// port: tsc/internal/vfs/internal/internal.go:decodeUtf16
fn decode_utf16(bytes: &[u8], little_endian: bool) -> Vec<u8> {
    // Go allocates len(bytes)/2 words and binary.Read reads that exact word count.
    // Consequently an odd final byte is discarded, including a one-byte payload.
    let units = bytes.chunks_exact(2).map(|pair| {
        let pair = [pair[0], pair[1]];
        if little_endian {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        }
    });
    char::decode_utf16(units)
        .map(|rune| rune.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect::<String>()
        .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::SourceText;

    #[test]
    fn parser_text_does_not_repeat_file_loading() {
        let double_bom = b"\xef\xbb\xbf\xef\xbb\xbfx";
        let loaded = SourceText::from_bytes(double_bom.as_slice());
        assert_eq!(loaded.as_bytes(), b"\xef\xbb\xbfx");
        let parser_text = SourceText::from_loaded_bytes(loaded.as_bytes());
        assert_eq!(parser_text.as_bytes(), loaded.as_bytes());
        assert_eq!(SourceText::from_bytes(loaded.as_bytes()).as_bytes(), b"x");

        for raw in [b"\xff\xfe\0\xd8\xff".as_slice(), b"\xfe\xff\xd8\0\xff"] {
            assert_eq!(SourceText::from_loaded_bytes(raw).as_bytes(), raw);
            assert_eq!(
                SourceText::from_bytes(raw).as_bytes(),
                "\u{fffd}".as_bytes()
            );
        }
        assert_eq!(
            SourceText::from_loaded_bytes(&b"\xffx"[..]).as_bytes(),
            b"\xffx"
        );
    }
}
