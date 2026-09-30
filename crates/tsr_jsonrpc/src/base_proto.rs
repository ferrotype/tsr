//! The LSP base protocol: JSON-RPC payloads framed by `Content-Length`
//! headers.
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};

/// A framing failure, rendered as the pinned errors render.
#[derive(Debug)]
pub enum Error {
    /// The stream ended before a header line started.
    Eof,
    ReadHeader(io::Error),
    /// A header line without a `:` separator.
    InvalidHeader(Vec<u8>),
    /// A `Content-Length` value that is not an integer, or is negative.
    InvalidContentLength(String),
    NoContentLength,
    ReadContent(io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eof => f.write_str("EOF"),
            Self::ReadHeader(error) => write!(f, "jsonrpc: read header: {error}"),
            Self::InvalidHeader(line) => {
                write!(
                    f,
                    "jsonrpc: invalid header: {}",
                    tsr_jsstring::go_quote(line)
                )
            }
            Self::InvalidContentLength(detail) => {
                write!(f, "jsonrpc: invalid content length: {detail}")
            }
            Self::NoContentLength => f.write_str("jsonrpc: no content length"),
            Self::ReadContent(error) => write!(f, "jsonrpc: read content: {error}"),
        }
    }
}

impl std::error::Error for Error {}

/// Reads JSON-RPC messages with Content-Length framing.
pub struct Reader<R> {
    reader: BufReader<R>,
}

impl<R: Read> Reader<R> {
    /// port: tsc/internal/jsonrpc/baseproto.go:NewReader
    pub fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
        }
    }

    /// The next message payload.
    /// port: tsc/internal/jsonrpc/baseproto.go:Reader.Read
    pub fn read(&mut self) -> Result<Vec<u8>, Error> {
        let mut content_length: i64 = 0;
        loop {
            let mut line = Vec::new();
            match self.reader.read_until(b'\n', &mut line) {
                Ok(_) if line.last() == Some(&b'\n') => {}
                // A partial line at the end of the stream is Go's ReadBytes
                // returning io.EOF with the bytes read so far.
                Ok(_) => return Err(Error::Eof),
                Err(error) => return Err(Error::ReadHeader(error)),
            }
            if line == b"\r\n" {
                break;
            }
            let Some(colon) = line.iter().position(|&b| b == b':') else {
                return Err(Error::InvalidHeader(line));
            };
            let (key, value) = (&line[..colon], &line[colon + 1..]);
            if key == b"Content-Length" {
                let text = String::from_utf8_lossy(value.trim_ascii());
                content_length = parse_int(&text).map_err(|error| {
                    Error::InvalidContentLength(format!("parse error: {error}"))
                })?;
                if content_length < 0 {
                    return Err(Error::InvalidContentLength(format!(
                        "negative value {content_length}"
                    )));
                }
            }
        }
        if content_length <= 0 {
            return Err(Error::NoContentLength);
        }
        let length = usize::try_from(content_length).map_err(|_| {
            Error::ReadContent(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "content length exceeds memory",
            ))
        })?;
        // io.ReadFull: EOF when nothing was read, unexpected EOF after a part.
        let mut data = vec![0; length];
        let mut filled = 0;
        while filled < length {
            match self.reader.read(&mut data[filled..]) {
                Ok(0) => {
                    let (kind, text) = if filled == 0 {
                        (io::ErrorKind::UnexpectedEof, "EOF")
                    } else {
                        (io::ErrorKind::UnexpectedEof, "unexpected EOF")
                    };
                    return Err(Error::ReadContent(io::Error::new(kind, text)));
                }
                Ok(read) => filled += read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(Error::ReadContent(error)),
            }
        }
        Ok(data)
    }
}

/// `strconv.ParseInt(text, 10, 64)` with its error text.
fn parse_int(text: &str) -> Result<i64, String> {
    text.parse::<i64>().map_err(|error| {
        let reason = match error.kind() {
            std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
                "value out of range"
            }
            _ => "invalid syntax",
        };
        format!("strconv.ParseInt: parsing {}: {reason}", quote(text))
    })
}

fn quote(text: &str) -> String {
    tsr_jsstring::go_quote(text.as_bytes())
}

/// Writes JSON-RPC messages with Content-Length framing.
pub struct Writer<W: Write> {
    writer: BufWriter<W>,
}

impl<W: Write> Writer<W> {
    /// port: tsc/internal/jsonrpc/baseproto.go:NewWriter
    pub fn new(writer: W) -> Self {
        Self {
            writer: BufWriter::new(writer),
        }
    }

    /// Writes one payload with its Content-Length header and flushes it.
    /// port: tsc/internal/jsonrpc/baseproto.go:Writer.Write
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        write!(self.writer, "Content-Length: {}\r\n\r\n", data.len())?;
        self.writer.write_all(data)?;
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_extra_headers_are_ignored() {
        let mut bytes = Vec::new();
        let mut writer = Writer::new(&mut bytes);
        writer.write(br#"{"a":1}"#).unwrap();
        writer.write(b"[]").unwrap();
        drop(writer);
        assert_eq!(
            &bytes[..],
            b"Content-Length: 7\r\n\r\n{\"a\":1}Content-Length: 2\r\n\r\n[]"
        );
        let mut reader = Reader::new(&bytes[..]);
        assert_eq!(reader.read().unwrap(), br#"{"a":1}"#);
        assert_eq!(reader.read().unwrap(), b"[]");
        assert!(matches!(reader.read(), Err(Error::Eof)));
        let mut typed =
            Reader::new(&b"Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"[..]);
        assert_eq!(typed.read().unwrap(), b"{}");
    }

    #[test]
    fn malformed_headers_render_as_the_pin_does() {
        let error = |input: &[u8]| Reader::new(input).read().unwrap_err().to_string();
        assert_eq!(
            error(b"Content-Length 2\r\n\r\n{}"),
            "jsonrpc: invalid header: \"Content-Length 2\\r\\n\""
        );
        assert_eq!(
            error(b"Content-Length: x\r\n\r\n"),
            "jsonrpc: invalid content length: parse error: strconv.ParseInt: parsing \"x\": invalid syntax"
        );
        assert_eq!(
            error(b"Content-Length: -1\r\n\r\n"),
            "jsonrpc: invalid content length: negative value -1"
        );
        assert_eq!(error(b"\r\n"), "jsonrpc: no content length");
        assert_eq!(
            error(b"Content-Length: 4\r\n\r\n{}"),
            "jsonrpc: read content: unexpected EOF"
        );
    }
}
