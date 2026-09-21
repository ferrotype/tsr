use crate::state::{append_pointer, pointer, Frame};
use crate::wire::Input;
use crate::{Error, Kind, Options, Token};
use std::{borrow::Cow, io::Read};

/// Incremental decoder. Reads return owned tokens/values, so a caller can retain
/// them across later reads without referring into the mutable input buffer.
pub struct Decoder<'a> {
    input: Input<'a>,
    pos: usize,
    base: usize,
    stack: Vec<Frame>,
    options: Options<'a>,
    root_count: usize,
    pub(crate) base_depth: usize,
}
impl<'a> Decoder<'a> {
    pub fn new(reader: impl Read + 'a) -> Self {
        Self::with_options(reader, Options::default())
    }
    pub fn with_options(reader: impl Read + 'a, options: Options<'a>) -> Self {
        Self {
            input: Input::new(reader),
            pos: 0,
            base: 0,
            stack: Vec::new(),
            options,
            root_count: 0,
            base_depth: 0,
        }
    }
    pub fn from_slice(bytes: &'a [u8]) -> Self {
        Self::new(std::io::Cursor::new(bytes))
    }
    /// Per-call options are restored even when decoding fails. Namespace
    /// policy cannot change while the stream is positioned at an object name.
    pub fn unmarshal_decode<T: crate::Decode + ?Sized>(
        &mut self,
        value: &mut T,
        options: Options<'a>,
    ) -> Result<(), Error> {
        let original = self.options;
        self.options.join(&options);
        let reason = if self.stack.last().is_some_and(Frame::expects_name) {
            if original.allow_duplicate_names.unwrap_or(false)
                != self.options.allow_duplicate_names.unwrap_or(false)
            {
                Some("cannot change duplicate name checks after a JSON object has already begun processing")
            } else if original.allow_invalid_utf8.unwrap_or(false)
                != self.options.allow_invalid_utf8.unwrap_or(false)
            {
                Some("cannot change UTF-8 checks after a JSON object has already begun processing")
            } else {
                None
            }
        } else {
            None
        };
        let result = if let Some(reason) = reason {
            Err(crate::Error::Semantic(Box::new(crate::SemanticError {
                marshal: false,
                offset: self.input_offset(),
                pointer: pointer(&self.stack, true),
                kind: Kind::Invalid,
                value: None,
                type_name: T::type_name(),
                cause: Some(Error::Message(reason.into())),
            })))
        } else {
            self.value(value)
        };
        self.options = original;
        result
    }
    pub fn input_offset(&self) -> usize {
        self.base + self.pos
    }
    pub fn stack_depth(&self) -> usize {
        self.stack.len()
    }
    pub fn stack_pointer(&self) -> String {
        pointer(&self.stack, false)
    }
    pub(crate) fn depth_length(&self) -> (usize, usize) {
        (
            self.stack.len(),
            self.stack.last().map_or(self.root_count, |f| f.count),
        )
    }
    pub fn unread_buffer(&self) -> &[u8] {
        &self.input.bytes[self.pos..]
    }
    fn compact(&mut self) {
        if self.pos >= 4096 {
            self.input.bytes.drain(..self.pos);
            self.base += self.pos;
            self.pos = 0;
        }
    }
    fn absolute_error(&self, mut e: Error) -> Error {
        if let Error::Syntax(s) = &mut e {
            s.offset += self.base;
        }
        e
    }
    pub fn next_location(&mut self) -> Result<(usize, String), Error> {
        prepare(&mut self.input, self.pos, &self.stack)
            .map(|p| (self.base + p, pointer(&self.stack, true)))
            .map_err(|e| self.absolute_error(e))
    }
    pub fn peek_kind(&mut self) -> Kind {
        match prepare(&mut self.input, self.pos, &self.stack).and_then(|p| self.input.at(p)) {
            Ok(Some(b)) => Kind::from_byte(b),
            _ => Kind::Invalid,
        }
    }
    pub fn read_token(&mut self) -> Result<Token<'static>, Error> {
        self.compact();
        let at_root = self.stack.is_empty();
        let token = next(
            &mut self.input,
            &mut self.pos,
            &mut self.stack,
            &self.options,
            self.base_depth,
        )
        .map_err(|e| self.absolute_error(e))?;
        if at_root {
            self.root_count += 1;
        }
        Ok(token)
    }
    /// Read one complete value, retaining its original whitespace, number and
    /// escape spelling. A failed value does not advance the public position.
    pub fn read_value(&mut self) -> Result<Vec<u8>, Error> {
        let range = self.read_value_range()?;
        Ok(self.input.bytes[range].to_vec())
    }
    fn read_value_range(&mut self) -> Result<std::ops::Range<usize>, Error> {
        self.compact();
        let start =
            prepare(&mut self.input, self.pos, &self.stack).map_err(|e| self.absolute_error(e))?;
        let prefix = pointer(&self.stack, true);
        let mut cursor = start;
        let mut frames = Vec::new();
        let result = (|| {
            let first = next(
                &mut self.input,
                &mut cursor,
                &mut frames,
                &self.options,
                self.base_depth + self.stack.len(),
            )?;
            while !frames.is_empty() {
                next(
                    &mut self.input,
                    &mut cursor,
                    &mut frames,
                    &self.options,
                    self.base_depth + self.stack.len(),
                )?;
            }
            Ok(first)
        })();
        let first = result.map_err(|mut error| {
            if error == Error::Eof && !self.stack.is_empty() {
                error = Error::truncated(start, String::new());
            }
            if let Error::Syntax(e) = &mut error {
                e.pointer = prefix.clone() + &e.pointer;
            }
            self.absolute_error(error)
        })?;
        if let Some(parent) = self.stack.last_mut() {
            if parent.expects_name() {
                let Token::String(name) = &first else {
                    return Err(Error::syntax(
                        self.base + start,
                        prefix,
                        "object member name must be a string",
                    ));
                };
                if !self.options.allow_duplicate_names.unwrap_or(false)
                    && !parent.names.insert(name)
                {
                    let mut p = prefix;
                    append_pointer(&mut p, name);
                    return Err(Error::duplicate(self.base + start, p));
                }
                parent.name = name.to_vec();
            }
            parent.count += 1;
        } else {
            self.root_count += 1;
        }
        self.pos = cursor;
        Ok(start..cursor)
    }
    pub fn skip_value(&mut self) -> Result<(), Error> {
        self.read_value_range().map(|_| ())
    }
    pub fn finish(&mut self) -> Result<(), Error> {
        let p = self.input.space(self.pos)?;
        if self.input.at(p)?.is_some() {
            let error = self.input.invalid(p, "", "after top-level value");
            return Err(self.absolute_error(error));
        }
        Ok(())
    }
}

fn prepare(input: &mut Input<'_>, pos: usize, stack: &[Frame]) -> Result<usize, Error> {
    prepare_inner(input, pos, stack).map_err(|mut e| {
        if let Error::Syntax(s) = &mut e {
            s.pointer = pointer(stack, true) + &s.pointer;
        }
        e
    })
}
fn prepare_inner(input: &mut Input<'_>, pos: usize, stack: &[Frame]) -> Result<usize, Error> {
    let mut i = input.space(pos)?;
    let Some(frame) = stack.last() else {
        return Ok(i);
    };
    let ptr = String::new();
    let ch = input.at(i)?;
    if frame.kind == Kind::BeginObject && !frame.expects_name() {
        if ch != Some(b':') {
            return Err(if ch.is_none() {
                Error::truncated(i, ptr)
            } else {
                input.invalid(i, &ptr, "after object name (expecting ':')")
            });
        }
        i = input.space(i + 1)?;
    } else if frame.count > 0
        && ch
            != Some(if frame.kind == Kind::BeginObject {
                b'}'
            } else {
                b']'
            })
    {
        if ch != Some(b',') {
            return Err(if ch.is_none() {
                Error::truncated(i, ptr)
            } else {
                input.invalid(
                    i,
                    &ptr,
                    if frame.kind == Kind::BeginObject {
                        "after object value (expecting ',' or '}')"
                    } else {
                        "after array element (expecting ',' or ']')"
                    },
                )
            });
        }
        i = input.space(i + 1)?;
        if matches!(input.at(i)?, Some(b'}' | b']')) {
            return Err(input.invalid(i, &ptr, "at start of value"));
        }
    }
    Ok(i)
}
fn next(
    input: &mut Input<'_>,
    pos: &mut usize,
    stack: &mut Vec<Frame>,
    options: &Options<'_>,
    base_depth: usize,
) -> Result<Token<'static>, Error> {
    next_inner(input, pos, stack, options, base_depth).map_err(|mut e| {
        if let Error::Syntax(s) = &mut e {
            s.pointer = pointer(stack, true) + &s.pointer;
        }
        e
    })
}
fn next_inner(
    input: &mut Input<'_>,
    pos: &mut usize,
    stack: &mut Vec<Frame>,
    options: &Options<'_>,
    base_depth: usize,
) -> Result<Token<'static>, Error> {
    let start = prepare_inner(input, *pos, stack)?;
    let ptr = String::new();
    let Some(b) = input.at(start)? else {
        return Err(if stack.is_empty() {
            Error::Eof
        } else {
            Error::truncated(start, ptr)
        });
    };
    let kind = Kind::from_byte(b);
    let expects_name = stack.last().is_some_and(Frame::expects_name);
    if kind.is_end() {
        let required = if kind == Kind::EndObject {
            Kind::BeginObject
        } else {
            Kind::BeginArray
        };
        if !stack.last().is_some_and(|f| f.kind == required) {
            return Err(Error::syntax(
                start,
                ptr,
                "mismatching structural token for object or array",
            ));
        }
        if required == Kind::BeginObject && !expects_name {
            return Err(Error::syntax(start, ptr, "missing value after object name"));
        }
        stack.pop();
        *pos = start + 1;
        return Ok(if kind == Kind::EndObject {
            Token::EndObject
        } else {
            Token::EndArray
        });
    }
    if expects_name && kind != Kind::String {
        return Err(Error::syntax(
            start,
            ptr,
            "object member name must be a string",
        ));
    }
    if matches!(kind, Kind::BeginObject | Kind::BeginArray) && base_depth + stack.len() >= 10000 {
        return Err(Error::syntax(start, ptr, "exceeded max depth"));
    }
    let (token, end) = match kind {
        Kind::String => {
            let (value, end) =
                input.string(start, &ptr, options.allow_invalid_utf8.unwrap_or(false))?;
            (Token::String(Cow::Owned(value)), end)
        }
        Kind::Number => {
            let end = input.number(start, &ptr)?;
            (
                Token::Number(Cow::Owned(input.bytes[start..end].to_vec())),
                end,
            )
        }
        Kind::Null => (Token::Null, input.literal(start, b"null", &ptr)?),
        Kind::True => (Token::Boolean(true), input.literal(start, b"true", &ptr)?),
        Kind::False => (Token::Boolean(false), input.literal(start, b"false", &ptr)?),
        Kind::BeginObject => (Token::BeginObject, start + 1),
        Kind::BeginArray => (Token::BeginArray, start + 1),
        _ => return Err(input.invalid(start, &ptr, "at start of value")),
    };
    if let Some(parent) = stack.last_mut() {
        if expects_name {
            let Token::String(name) = &token else {
                unreachable!()
            };
            if !options.allow_duplicate_names.unwrap_or(false) && !parent.names.insert(name) {
                let mut p = ptr;
                append_pointer(&mut p, name);
                return Err(Error::duplicate(start, p));
            }
            parent.name = name.to_vec();
        }
        parent.count += 1;
    }
    if matches!(kind, Kind::BeginObject | Kind::BeginArray) {
        stack.push(Frame::new(kind));
    }
    *pos = end;
    Ok(token)
}
