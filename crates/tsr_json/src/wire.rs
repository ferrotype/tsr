//! Incremental byte lexer. Structural state belongs to the encoder/decoder;
//! string decoding and number spelling validation are shared by both.
use crate::Error;
use std::{borrow::Cow, io::Read};
use tsr_jsstring::wtf8::{decode_utf8, RUNE_ERROR};

pub(crate) struct Input<'a> {
    pub reader: Box<dyn Read + 'a>,
    pub bytes: Vec<u8>,
    pub eof: bool,
}
impl<'a> Input<'a> {
    pub fn new(reader: impl Read + 'a) -> Self {
        Self {
            reader: Box::new(reader),
            bytes: Vec::new(),
            eof: false,
        }
    }
    pub fn at(&mut self, index: usize) -> Result<Option<u8>, Error> {
        while index >= self.bytes.len() && !self.eof {
            let mut chunk = [0; 512];
            let size = loop {
                match self.reader.read(&mut chunk) {
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    result => break result,
                }
            }
            .map_err(|e| Error::Io {
                action: "read",
                message: e.to_string(),
            })?;
            if size == 0 {
                self.eof = true;
            } else {
                self.bytes.extend_from_slice(&chunk[..size]);
            }
        }
        Ok(self.bytes.get(index).copied())
    }
    pub fn space(&mut self, mut index: usize) -> Result<usize, Error> {
        while self
            .at(index)?
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        {
            index += 1;
        }
        Ok(index)
    }
    pub fn invalid(&mut self, index: usize, pointer: &str, context: &str) -> Error {
        let byte = self.bytes.get(index).copied().unwrap_or_default();
        let q = match byte {
            b'\n' => "'\\n'".into(),
            b'\r' => "'\\r'".into(),
            b'\t' => "'\\t'".into(),
            b'\'' => "'\\\''".into(),
            b'\\' => "'\\\\'".into(),
            32..=126 => format!("'{}'", char::from(byte)),
            _ => format!("'\\x{byte:02x}'"),
        };
        Error::syntax(
            index,
            pointer.into(),
            format!("invalid character {q} {context}"),
        )
    }
    pub fn string(
        &mut self,
        start: usize,
        pointer: &str,
        allow_invalid: bool,
    ) -> Result<(Vec<u8>, usize), Error> {
        let mut out = Vec::new();
        let mut i = start + 1;
        loop {
            let Some(b) = self.at(i)? else {
                return Err(Error::truncated(i, pointer.into()));
            };
            match b {
                b'"' => return Ok((out, i + 1)),
                b'\\' => {
                    let slash = i;
                    let Some(escape) = self.at(i + 1)? else {
                        return Err(Error::truncated(i, pointer.into()));
                    };
                    i += 2;
                    match escape {
                        b'"' | b'/' | b'\\' => out.push(escape),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'n' => out.push(10),
                        b'r' => out.push(13),
                        b't' => out.push(9),
                        b'u' => {
                            let first = self.hex4(i, slash, pointer)?;
                            i += 4;
                            let mut rune = i32::from(first);
                            if (0xd800..=0xdfff).contains(&first) {
                                let mut pair = None;
                                if first <= 0xdbff
                                    && self.at(i)? == Some(b'\\')
                                    && self.at(i + 1)? == Some(b'u')
                                {
                                    if let Ok(second) = self.hex4(i + 2, i, pointer) {
                                        if (0xdc00..=0xdfff).contains(&second) {
                                            pair = Some(second);
                                        }
                                    }
                                }
                                if let Some(second) = pair {
                                    rune = 0x10000
                                        + ((i32::from(first) - 0xd800) << 10)
                                        + (i32::from(second) - 0xdc00);
                                    i += 6;
                                } else if allow_invalid {
                                    rune = RUNE_ERROR;
                                } else {
                                    self.at(slash + 11)?;
                                    let text = String::from_utf8_lossy(
                                        &self.bytes[slash..(slash + 12).min(self.bytes.len())],
                                    );
                                    return Err(Error::syntax(
                                        slash,
                                        pointer.into(),
                                        format!("invalid surrogate pair `{text}` in string"),
                                    ));
                                }
                            }
                            append_rune(&mut out, rune);
                        }
                        _ => return Err(self.invalid(i - 1, pointer, "in string escape sequence")),
                    }
                }
                0..=31 => return Err(self.invalid(i, pointer, "in string")),
                0x80..=0xff => {
                    self.at(i + 3)?;
                    let (rune, width) = decode_utf8(&self.bytes[i..]);
                    if rune == RUNE_ERROR && width == 1 && !allow_invalid {
                        return Err(Error::syntax(i, pointer.into(), "invalid UTF-8"));
                    }
                    append_rune(&mut out, rune);
                    i += width;
                }
                _ => {
                    out.push(b);
                    i += 1;
                }
            }
        }
    }
    fn hex4(&mut self, start: usize, slash: usize, pointer: &str) -> Result<u16, Error> {
        let mut value = 0;
        for i in start..start + 4 {
            let Some(b) = self.at(i)? else {
                return Err(Error::truncated(slash, pointer.into()));
            };
            let digit = match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                _ => return Err(self.invalid(i, pointer, "in string escape sequence")),
            };
            value = value * 16 + u16::from(digit);
        }
        Ok(value)
    }
    pub fn number(&mut self, start: usize, pointer: &str) -> Result<usize, Error> {
        let mut i = start;
        if self.at(i)? == Some(b'-') {
            i += 1;
        }
        match self.at(i)? {
            Some(b'0') => i += 1,
            Some(b'1'..=b'9') => {
                i += 1;
                while self.at(i)?.is_some_and(|b| b.is_ascii_digit()) {
                    i += 1;
                }
            }
            None => return Err(Error::truncated(i, pointer.into())),
            _ => return Err(self.invalid(i, pointer, "in number (expecting digit)")),
        }
        if self.at(i)? == Some(b'.') {
            i += 1;
            i = self.digits(i, pointer)?;
        }
        if self.at(i)?.is_some_and(|b| matches!(b, b'e' | b'E')) {
            i += 1;
            if self.at(i)?.is_some_and(|b| matches!(b, b'+' | b'-')) {
                i += 1;
            }
            i = self.digits(i, pointer)?;
        }
        Ok(i)
    }
    fn digits(&mut self, mut i: usize, pointer: &str) -> Result<usize, Error> {
        match self.at(i)? {
            None => return Err(Error::truncated(i, pointer.into())),
            Some(b) if !b.is_ascii_digit() => {
                return Err(self.invalid(i, pointer, "in number (expecting digit)"))
            }
            _ => {}
        }
        while self.at(i)?.is_some_and(|b| b.is_ascii_digit()) {
            i += 1;
        }
        Ok(i)
    }
    pub fn literal(&mut self, start: usize, word: &[u8], pointer: &str) -> Result<usize, Error> {
        for (offset, &b) in word.iter().enumerate() {
            match self.at(start + offset)? {
                Some(actual) if actual == b => {}
                None => return Err(Error::truncated(start + offset, pointer.into())),
                _ => {
                    return Err(self.invalid(
                        start + offset,
                        pointer,
                        &format!("in literal {}", String::from_utf8_lossy(word)),
                    ))
                }
            }
        }
        Ok(start + word.len())
    }
}

pub(crate) fn append_string<'a>(
    out: &mut Vec<u8>,
    bytes: &'a [u8],
    allow_invalid: bool,
) -> Result<Cow<'a, [u8]>, usize> {
    let mut decoded = Cow::Borrowed(bytes);
    let mut offset = 0;
    out.push(b'"');
    while offset < bytes.len() {
        let (rune, width) = decode_utf8(&bytes[offset..]);
        if rune == RUNE_ERROR && width == 1 && !allow_invalid {
            return Err(offset);
        }
        if rune == RUNE_ERROR && width == 1 {
            if matches!(decoded, Cow::Borrowed(_)) {
                decoded = Cow::Owned(bytes[..offset].to_vec());
            }
            append_rune(decoded.to_mut(), rune);
        } else if let Cow::Owned(decoded) = &mut decoded {
            decoded.extend_from_slice(&bytes[offset..offset + width]);
        }
        match rune {
            8 => out.extend_from_slice(b"\\b"),
            9 => out.extend_from_slice(b"\\t"),
            10 => out.extend_from_slice(b"\\n"),
            12 => out.extend_from_slice(b"\\f"),
            13 => out.extend_from_slice(b"\\r"),
            34 => out.extend_from_slice(b"\\\""),
            92 => out.extend_from_slice(b"\\\\"),
            0..=31 => {
                const HEX: &[u8] = b"0123456789abcdef";
                out.extend_from_slice(b"\\u00");
                out.push(HEX[rune as usize / 16]);
                out.push(HEX[rune as usize % 16]);
            }
            _ => append_rune(out, rune),
        }
        offset += width;
    }
    out.push(b'"');
    Ok(decoded)
}

fn append_rune(out: &mut Vec<u8>, rune: i32) {
    let mut buf = [0; 4];
    out.extend_from_slice(
        char::from_u32(rune as u32)
            .unwrap_or(char::REPLACEMENT_CHARACTER)
            .encode_utf8(&mut buf)
            .as_bytes(),
    );
}

/// Fast validation for the token encoder; detailed lexer errors are computed
/// only on failure, without allocating an input buffer for valid numbers.
pub(crate) fn valid_number(bytes: &[u8]) -> bool {
    let mut i = usize::from(bytes.first() == Some(&b'-'));
    match bytes.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => {
            i += 1;
            while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
        _ => return false,
    }
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(bytes.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let start = i;
        while bytes.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == bytes.len()
}
