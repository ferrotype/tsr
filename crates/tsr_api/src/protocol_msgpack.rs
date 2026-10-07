//! The synchronous client's framing: a three-element msgpack array
//! `[type, method, payload]` whose method name doubles as the message id.
//! Only five markers occur: `0x93`, a positive fixint or `0xcc` for the type,
//! and `0xc4`/`0xc5`/`0xc6` for the two binaries. Responses may carry raw
//! bytes (`RawBinary`) that no JSON protocol ever sees.
//! port: tsc/internal/api/protocol_msgpack.go
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::sync::{Arc, Mutex};
use tsr_ipc::{Error, Protocol};
use tsr_json::{Encode, RawValue};
use tsr_jsonrpc::{FramingError, Id, Message, ResponseError, CODE_INTERNAL_ERROR};

/// The pin's `MessageType` of tsc/internal/api/protocol_msgpack.go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Request = 1,
    CallResponse = 2,
    CallError = 3,
    Response = 4,
    Error = 5,
    Call = 6,
}
impl MessageType {
    fn from_byte(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::Request,
            2 => Self::CallResponse,
            3 => Self::CallError,
            4 => Self::Response,
            5 => Self::Error,
            6 => Self::Call,
            _ => return None,
        })
    }
}

const FIXED_ARRAY_3: u8 = 0x93;
const BIN8: u8 = 0xc4;
const BIN16: u8 = 0xc5;
const BIN32: u8 = 0xc6;
const U8: u8 = 0xcc;

fn invalid(message: impl std::fmt::Display) -> Error {
    Error::Message(format!("api: invalid request: {message}"))
}

pub struct MessagePackProtocol {
    reader: Mutex<BufReader<Box<dyn Read + Send>>>,
    writer: Mutex<BufWriter<Box<dyn Write + Send>>>,
}

impl MessagePackProtocol {
    pub fn new(reader: Box<dyn Read + Send>, writer: Box<dyn Write + Send>) -> Arc<Self> {
        Arc::new(Self {
            reader: Mutex::new(BufReader::new(reader)),
            writer: Mutex::new(BufWriter::new(writer)),
        })
    }

    /// One tuple; a clean end of input before the first byte is `Eof`.
    /// port: tsc/internal/api/protocol_msgpack.go:MessagePackProtocol.readTuple
    pub fn read_tuple(&self) -> Result<(MessageType, String, Vec<u8>), Error> {
        let mut reader = self.reader.lock().expect("msgpack reader");
        let first = match read_byte(&mut *reader) {
            Ok(byte) => byte,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(Error::Framing(Arc::new(FramingError::Eof)));
            }
            Err(error) => return Err(error.into()),
        };
        if first != FIXED_ARRAY_3 {
            return Err(invalid(format!(
                "expected fixed 3-element array (0x93), received: 0x{first:02x}"
            )));
        }
        let marker = read_byte(&mut *reader)?;
        let raw_type = if marker <= 0x7f {
            marker
        } else if marker == U8 {
            read_byte(&mut *reader)?
        } else {
            return Err(invalid(format!(
                "expected positive fixint or uint8 marker, received: 0x{marker:02x}"
            )));
        };
        let message_type = MessageType::from_byte(raw_type)
            .ok_or_else(|| invalid(format!("unknown message type: {raw_type}")))?;
        let method = read_bin(&mut *reader)?;
        let payload = read_bin(&mut *reader)?;
        Ok((
            message_type,
            String::from_utf8_lossy(&method).into_owned(),
            payload,
        ))
    }

    /// port: tsc/internal/api/protocol_msgpack.go:MessagePackProtocol.writeTuple
    pub fn write_tuple(
        &self,
        message_type: MessageType,
        method: &str,
        payload: &[u8],
    ) -> Result<(), Error> {
        let mut writer = self.writer.lock().expect("msgpack writer");
        writer.write_all(&[FIXED_ARRAY_3, message_type as u8])?;
        write_bin(&mut *writer, method.as_bytes())?;
        write_bin(&mut *writer, payload)?;
        writer.flush()?;
        Ok(())
    }

    /// A response whose payload is not JSON (the pin's `RawBinary`).
    pub fn write_binary_response(&self, id: Option<&Id>, payload: &[u8]) -> Result<(), Error> {
        self.write_tuple(MessageType::Response, &id_method(id), payload)
    }
}

fn id_method(id: Option<&Id>) -> String {
    id.map(ToString::to_string).unwrap_or_default()
}

fn read_byte(reader: &mut impl BufRead) -> std::io::Result<u8> {
    let mut byte = [0u8; 1];
    reader.read_exact(&mut byte)?;
    Ok(byte[0])
}

/// port: tsc/internal/api/protocol_msgpack.go:MessagePackProtocol.readBin
fn read_bin(reader: &mut impl BufRead) -> Result<Vec<u8>, Error> {
    let marker = read_byte(reader)?;
    let size = match marker {
        BIN8 => usize::from(read_byte(reader)?),
        BIN16 => {
            let mut size = [0u8; 2];
            reader.read_exact(&mut size)?;
            usize::from(u16::from_be_bytes(size))
        }
        BIN32 => {
            let mut size = [0u8; 4];
            reader.read_exact(&mut size)?;
            usize::try_from(u32::from_be_bytes(size)).expect("64-bit targets")
        }
        other => {
            return Err(invalid(format!(
                "expected binary data (0xc4-0xc6), received: 0x{other:02x}"
            )))
        }
    };
    let mut payload = vec![0u8; size];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

/// port: tsc/internal/api/protocol_msgpack.go:MessagePackProtocol.writeBin
fn write_bin(writer: &mut impl Write, data: &[u8]) -> Result<(), Error> {
    let length = data.len();
    if length < 256 {
        writer.write_all(&[BIN8, length as u8])?;
    } else if length < 1 << 16 {
        writer.write_all(&[BIN16])?;
        writer.write_all(&(length as u16).to_be_bytes())?;
    } else {
        let length = u32::try_from(length)
            .map_err(|_| Error::Message("binary data exceeds 4 GiB".into()))?;
        writer.write_all(&[BIN32])?;
        writer.write_all(&length.to_be_bytes())?;
    }
    writer.write_all(data)?;
    Ok(())
}

fn marshal(value: Option<&dyn Encode>) -> Result<Vec<u8>, Error> {
    match value {
        Some(value) => Ok(tsr_json::marshal(value, tsr_json::Options::default())?),
        None => Ok(b"null".to_vec()),
    }
}

impl Protocol for MessagePackProtocol {
    /// port: tsc/internal/api/protocol_msgpack.go:MessagePackProtocol.ReadMessage
    fn read_message(&self) -> Result<Message, Error> {
        let (message_type, method, payload) = self.read_tuple()?;
        let mut message = Message::default();
        match message_type {
            MessageType::Request => {
                // The method is the pseudo id: this protocol has no explicit ids.
                message.id = Some(Id::string(method.clone()));
                message.method = method;
                message.params = Some(RawValue(payload));
            }
            MessageType::CallResponse => {
                message.id = Some(Id::string(method));
                message.result = Some(RawValue(payload));
            }
            MessageType::CallError => {
                message.id = Some(Id::string(method));
                message.error = Some(ResponseError {
                    code: CODE_INTERNAL_ERROR,
                    message: String::from_utf8_lossy(&payload).into_owned(),
                    data: None,
                });
            }
            other => {
                return Err(Error::Message(format!(
                    "unexpected message type: {}",
                    other as u8
                )))
            }
        }
        Ok(message)
    }

    fn write_request(
        &self,
        _id: &Id,
        method: &str,
        params: Option<&dyn Encode>,
    ) -> Result<(), Error> {
        self.write_tuple(MessageType::Call, method, &marshal(params)?)
    }

    fn write_notification(&self, method: &str, params: Option<&dyn Encode>) -> Result<(), Error> {
        self.write_tuple(MessageType::Call, method, &marshal(params)?)
    }

    fn write_response(&self, id: Option<&Id>, result: Option<&dyn Encode>) -> Result<(), Error> {
        self.write_tuple(MessageType::Response, &id_method(id), &marshal(result)?)
    }

    fn write_error(&self, id: Option<&Id>, error: &ResponseError) -> Result<(), Error> {
        self.write_tuple(MessageType::Error, &id_method(id), error.message.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(input: &[u8]) -> (Arc<MessagePackProtocol>, Arc<Mutex<Vec<u8>>>) {
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let output = Arc::new(Mutex::new(Vec::new()));
        let protocol = MessagePackProtocol::new(
            Box::new(std::io::Cursor::new(input.to_vec())),
            Box::new(Sink(output.clone())),
        );
        (protocol, output)
    }

    #[test]
    fn requests_use_the_method_as_their_id() {
        let mut frame = vec![FIXED_ARRAY_3, 1, BIN8, 4];
        frame.extend_from_slice(b"ping");
        frame.extend_from_slice(&[BIN8, 0]);
        let (protocol, _) = make(&frame);
        let message = protocol.read_message().unwrap();
        assert!(message.is_request());
        assert_eq!(message.method, "ping");
        assert_eq!(message.id.unwrap().to_string(), "ping");
        assert_eq!(message.params, Some(RawValue(Vec::new())));
        assert!(protocol.read_message().unwrap_err().is_eof());
    }

    #[test]
    fn frames_choose_the_smallest_binary_marker_and_report_bad_bytes() {
        let (protocol, output) = make(b"");
        protocol
            .write_tuple(MessageType::Response, "m", &vec![b'x'; 300])
            .unwrap();
        let bytes = output.lock().unwrap().clone();
        assert_eq!(&bytes[..6], &[FIXED_ARRAY_3, 4, BIN8, 1, b'm', BIN16]);
        assert_eq!(&bytes[6..8], &300u16.to_be_bytes());
        assert_eq!(bytes.len(), 8 + 300);
        let (protocol, _) = make(&[0x92]);
        assert_eq!(
            protocol.read_message().unwrap_err().to_string(),
            "api: invalid request: expected fixed 3-element array (0x93), received: 0x92"
        );
        let (protocol, _) = make(&[FIXED_ARRAY_3, U8, 9, BIN8, 0, BIN8, 0]);
        assert_eq!(
            protocol.read_message().unwrap_err().to_string(),
            "api: invalid request: unknown message type: 9"
        );
    }

    #[test]
    fn errors_carry_their_message_as_the_payload() {
        let (protocol, output) = make(b"");
        protocol
            .write_error(
                Some(&Id::string("release")),
                &ResponseError {
                    code: CODE_INTERNAL_ERROR,
                    message: "boom".into(),
                    data: None,
                },
            )
            .unwrap();
        let mut expected = vec![FIXED_ARRAY_3, 5, BIN8, 7];
        expected.extend_from_slice(b"release");
        expected.extend_from_slice(&[BIN8, 4]);
        expected.extend_from_slice(b"boom");
        assert_eq!(*output.lock().unwrap(), expected);
    }
}
