use crate::state::{append_pointer, pointer, Frame};
use crate::wire::{append_string, Input};
use crate::{Decoder, Encode, Error, Kind, Options, SemanticError, Token};
use std::{borrow::Cow, io::Write};

/// One grammar/state machine serves typed and token writes. By default completed
/// top-level values end in a newline; the one-shot marshal wrappers omit it.
pub struct Encoder<'a> {
    bytes: Vec<u8>,
    base: usize,
    root_count: usize,
    flushed: usize,
    stack: Vec<Frame>,
    pub(crate) options: Options<'a>,
    sink: Option<Box<dyn Write + 'a>>,
    pub(crate) omit_top_level_newline: bool,
}
impl<'a> Encoder<'a> {
    pub fn new(options: Options<'a>) -> Result<Self, Error> {
        if !options
            .prefix
            .unwrap_or_default()
            .bytes()
            .all(|b| matches!(b, b' ' | b'\t'))
            || options
                .indent
                .is_some_and(|s| !s.bytes().all(|b| matches!(b, b' ' | b'\t')))
        {
            return Err(Error::InvalidIndent);
        }
        Ok(Self {
            bytes: Vec::new(),
            base: 0,
            root_count: 0,
            flushed: 0,
            stack: Vec::new(),
            options,
            sink: None,
            omit_top_level_newline: false,
        })
    }
    pub fn with_writer(writer: impl Write + 'a, options: Options<'a>) -> Result<Self, Error> {
        let mut this = Self::new(options)?;
        this.sink = Some(Box::new(writer));
        Ok(this)
    }
    pub fn output_offset(&self) -> usize {
        self.base + self.bytes.len()
    }
    pub fn stack_depth(&self) -> usize {
        self.stack.len()
    }
    pub fn stack_pointer(&self) -> String {
        pointer(&self.stack, false)
    }
    /// Bytes committed to the in-memory stream; an incomplete top-level value
    /// remains buffered. With a writer, committed bytes belong to that writer.
    pub fn written_bytes(&self) -> &[u8] {
        &self.bytes[..self.flushed]
    }
    pub fn buffered_bytes(&self) -> &[u8] {
        &self.bytes[self.flushed..]
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
    pub fn value(&mut self, value: &(impl Encode + ?Sized)) -> Result<(), Error> {
        let before = self.depth_length();
        value.encode_guarded(self)?;
        let after = self.depth_length();
        if after != (before.0, before.1 + 1) {
            return Err(self.semantic(
                value.type_name(),
                Error::Message("must read or write exactly one value".into()),
            ));
        }
        Ok(())
    }
    pub fn null(&mut self) -> Result<(), Error> {
        self.write_token(Token::Null)
    }
    pub fn boolean(&mut self, value: bool) -> Result<(), Error> {
        self.write_token(Token::Boolean(value))
    }
    pub fn string(&mut self, value: &[u8]) -> Result<(), Error> {
        self.write_token(Token::String(Cow::Borrowed(value)))
    }
    pub fn int(&mut self, value: i64) -> Result<(), Error> {
        self.write_token(Token::int(value))
    }
    pub fn uint(&mut self, value: u64) -> Result<(), Error> {
        self.write_token(Token::uint(value))
    }
    pub fn number(&mut self, value: f64) -> Result<(), Error> {
        if !value.is_finite() {
            let spelling = if value.is_nan() {
                "NaN"
            } else if value.is_sign_negative() {
                "-Inf"
            } else {
                "+Inf"
            };
            return Err(self.semantic(
                "float64",
                Error::Message(format!("unsupported value: {spelling}")),
            ));
        }
        let text = if value == 0.0 && value.is_sign_negative() {
            "-0".into()
        } else {
            tsr_jsnum::Number::new(value).to_string()
        };
        self.write_token(Token::Number(Cow::Owned(text.into_bytes())))
    }
    pub(crate) fn semantic(&self, type_name: &'static str, cause: Error) -> Error {
        Error::Semantic(Box::new(SemanticError {
            marshal: true,
            offset: self.output_offset() + self.prefix(Kind::Null).len(),
            pointer: pointer(&self.stack, true),
            kind: Kind::Invalid,
            value: None,
            type_name,
            cause: Some(cause),
        }))
    }
    fn depth_length(&self) -> (usize, usize) {
        (
            self.stack.len(),
            self.stack.last().map_or(self.root_count, |f| f.count),
        )
    }
    fn prefix(&self, kind: Kind) -> Prefix<'a> {
        let mut prefix = Prefix {
            delimiter: None,
            space: false,
            indent: None,
            depth: 0,
            line_start: self.options.prefix.unwrap_or_default(),
        };
        if let Some(f) = self.stack.last() {
            if !kind.is_end() {
                if f.kind == Kind::BeginObject && !f.expects_name() {
                    prefix.delimiter = Some(b':');
                } else if f.count > 0 {
                    prefix.delimiter = Some(b',');
                }
            }
            if let Some(indent) = self.options.indent {
                if f.kind == Kind::BeginObject && !f.expects_name() && !kind.is_end() {
                    prefix.space = true;
                } else if !kind.is_end() || f.count > 0 {
                    prefix.indent = Some(indent);
                    prefix.depth = self.stack.len() - usize::from(kind.is_end());
                }
            }
        }
        prefix
    }
    #[allow(
        clippy::needless_pass_by_value,
        reason = "token ownership follows the streaming API, accepting owned and borrowed text alike"
    )]
    pub fn write_token(&mut self, token: Token<'_>) -> Result<(), Error> {
        let start = self.bytes.len();
        let result = self.append_token(&token);
        if result.is_err() {
            self.bytes.truncate(start);
        }
        result?;
        if self.stack.is_empty() {
            if !self.omit_top_level_newline {
                self.bytes.push(b'\n');
            }
            self.flush()?;
        } else if self.bytes.len() - self.flushed >= 4096 {
            self.flush()?;
        }
        Ok(())
    }
    fn append_token(&mut self, token: &Token<'_>) -> Result<(), Error> {
        let kind = token.kind();
        let offset = self.output_offset() + self.prefix(kind).len();
        let expects_name = self.stack.last().is_some_and(Frame::expects_name);
        let fail = |message| Error::syntax(offset, pointer(&self.stack, true), message);
        if kind.is_end() {
            let required = if kind == Kind::EndObject {
                Kind::BeginObject
            } else {
                Kind::BeginArray
            };
            if !self.stack.last().is_some_and(|f| f.kind == required) {
                return Err(fail("mismatching structural token for object or array"));
            }
            if required == Kind::BeginObject && !expects_name {
                return Err(fail("missing value after object name"));
            }
        } else if expects_name && kind != Kind::String {
            return Err(fail("object member name must be a string"));
        }
        if matches!(kind, Kind::BeginObject | Kind::BeginArray) && self.stack.len() >= 10000 {
            return Err(fail("exceeded max depth"));
        }
        // Prefix borrows only the immutable option strings, not the output.
        let prefix = self.prefix(kind);
        // Use a helper with split field borrows instead of allocating a staging buffer.
        append_prefix(
            &mut self.bytes,
            prefix.delimiter,
            prefix.space,
            prefix.indent.map(str::as_bytes),
            prefix.depth,
            prefix.line_start.as_bytes(),
        );
        let decoded = match token {
            Token::String(bytes) => Some(
                append_string(
                    &mut self.bytes,
                    bytes,
                    self.options.allow_invalid_utf8.unwrap_or(false),
                )
                .map_err(|at| {
                    Error::syntax(offset + 1 + at, pointer(&self.stack, true), "invalid UTF-8")
                })?,
            ),
            Token::Number(bytes) => {
                if !crate::wire::valid_number(bytes) {
                    let mut input = Input::new(std::io::Cursor::new(bytes.as_ref()));
                    let end = input.number(0, "").map_err(|mut e| {
                        if let Error::Syntax(e) = &mut e {
                            e.offset += offset;
                            e.pointer = pointer(&self.stack, true);
                        }
                        e
                    })?;
                    return Err(Error::syntax(
                        offset + end,
                        pointer(&self.stack, true),
                        "invalid number token",
                    ));
                }
                self.bytes.extend_from_slice(bytes);
                None
            }
            _ => {
                self.bytes.extend_from_slice(token.text());
                None
            }
        };
        if expects_name && !kind.is_end() {
            let name = decoded.as_deref().expect("name token is a string");
            let frame = self.stack.last_mut().expect("name has an object frame");
            if !self.options.allow_duplicate_names.unwrap_or(false) && !frame.names.insert(name) {
                let mut p = pointer(&self.stack, true);
                append_pointer(&mut p, name);
                return Err(Error::duplicate(offset, p));
            }
            frame.name.clear();
            frame.name.extend_from_slice(name);
        }
        // A syntax failure above preserves both grammar state and output bytes.
        if kind.is_end() {
            self.stack.pop();
        } else {
            if let Some(f) = self.stack.last_mut() {
                f.count += 1;
            } else {
                self.root_count += 1;
            }
            if matches!(kind, Kind::BeginObject | Kind::BeginArray) {
                self.stack.push(Frame::new(kind));
            }
        }
        Ok(())
    }
    pub fn flush(&mut self) -> Result<(), Error> {
        if let Some(sink) = &mut self.sink {
            // Keep accepted-but-not-written bytes on a short write/error. A
            // retry starts at the first unwritten byte, not at the token start.
            while self.flushed < self.bytes.len() {
                let n = match sink.write(&self.bytes[self.flushed..]) {
                    Ok(0) => {
                        return Err(Error::Io {
                            action: "write",
                            message: "failed to write whole buffer".into(),
                        })
                    }
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        return Err(Error::Io {
                            action: "write",
                            message: e.to_string(),
                        })
                    }
                };
                self.flushed += n;
            }
            self.base += self.bytes.len();
            self.bytes.clear();
            self.flushed = 0;
        } else {
            self.flushed = self.bytes.len();
        }
        Ok(())
    }
    pub fn array<'b, T: Encode + ?Sized + 'b>(
        &mut self,
        values: impl IntoIterator<Item = &'b T>,
    ) -> Result<(), Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.write_token(Token::BeginArray)?;
            for value in values {
                self.value(value)?;
            }
            self.write_token(Token::EndArray)
        })
    }
    pub fn object<'b, T: Encode + ?Sized + 'b>(
        &mut self,
        entries: impl IntoIterator<Item = (&'b [u8], &'b T)>,
    ) -> Result<(), Error> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.write_token(Token::BeginObject)?;
            for (key, value) in entries {
                self.string(key)?;
                self.value(value)?;
            }
            self.write_token(Token::EndObject)
        })
    }
    /// Raw values are validated before any emission, preserving transactional
    /// token state even when malformed input fails deep inside the value.
    pub fn write_value(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let mut decoder = Decoder::with_options(std::io::Cursor::new(bytes), self.options);
        decoder.base_depth = self.stack.len();
        decoder
            .skip_value()
            .and_then(|()| decoder.finish())
            .map_err(|mut error| {
                if error == Error::Eof {
                    error = Error::truncated(bytes.len(), String::new());
                }
                if let Error::Syntax(e) = &mut error {
                    e.offset += self.output_offset()
                        + self
                            .prefix(Kind::from_byte(bytes.first().copied().unwrap_or_default()))
                            .len();
                    e.pointer = pointer(&self.stack, true) + &e.pointer;
                }
                error
            })?;
        let mut decoder = Decoder::with_options(std::io::Cursor::new(bytes), self.options);
        loop {
            match decoder.read_token() {
                Ok(t) => self.write_token(t)?,
                Err(Error::Eof) => break,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
    /// Options override the stream only for this call. Changing the namespace
    /// policy while writing an object name, or changing formatting, is refused.
    pub fn marshal_encode(
        &mut self,
        value: &(impl Encode + ?Sized),
        mut options: Options<'a>,
    ) -> Result<(), Error> {
        options.allow_invalid_utf8 = Some(options.allow_invalid_utf8.unwrap_or(true));
        let old = self.options;
        self.options.join(&options);
        let reason = if self.stack.last().is_some_and(Frame::expects_name) {
            if old.allow_duplicate_names.unwrap_or(false)
                != self.options.allow_duplicate_names.unwrap_or(false)
            {
                Some("cannot change duplicate name checks after a JSON object has already begun processing")
            } else if old.allow_invalid_utf8.unwrap_or(false)
                != self.options.allow_invalid_utf8.unwrap_or(false)
            {
                Some("cannot change UTF-8 checks after a JSON object has already begun processing")
            } else {
                None
            }
        } else {
            None
        };
        let reason = reason.or_else(|| {
            (old.indent != self.options.indent
                || old.prefix.unwrap_or_default() != self.options.prefix.unwrap_or_default())
            .then_some("cannot change whitespace formatting within a MarshalEncode call")
        });
        let result = if let Some(reason) = reason {
            Err(self.semantic(value.type_name(), Error::Message(reason.into())))
        } else {
            self.value(value)
        };
        self.options = old;
        result
    }
}

struct Prefix<'a> {
    delimiter: Option<u8>,
    space: bool,
    indent: Option<&'a str>,
    depth: usize,
    line_start: &'a str,
}
impl Prefix<'_> {
    fn len(&self) -> usize {
        usize::from(self.delimiter.is_some())
            + usize::from(self.space)
            + self
                .indent
                .map_or(0, |s| 1 + self.line_start.len() + self.depth * s.len())
    }
}
fn append_prefix(
    out: &mut Vec<u8>,
    delimiter: Option<u8>,
    space: bool,
    indent: Option<&[u8]>,
    depth: usize,
    line_start: &[u8],
) {
    if let Some(delimiter) = delimiter {
        out.push(delimiter);
    }
    if space {
        out.push(b' ');
    }
    if let Some(indent) = indent {
        out.push(b'\n');
        out.extend_from_slice(line_start);
        for _ in 0..depth {
            out.extend_from_slice(indent);
        }
    }
}
