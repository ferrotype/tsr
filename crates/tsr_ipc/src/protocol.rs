//! Reading and writing connection messages.
use crate::Error;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tsr_json::{Encode, RawValue};
use tsr_jsonrpc::{Id, Message, Reader, RequestMessage, ResponseError, ResponseMessage, Writer};

/// Reads and writes messages. A connection reads from its reader loop while
/// other threads write, so implementations synchronize their own halves.
pub trait Protocol: Send + Sync {
    /// Reads the next message.
    fn read_message(&self) -> Result<Message, Error>;
    /// Writes a request.
    fn write_request(
        &self,
        id: &Id,
        method: &str,
        params: Option<&dyn Encode>,
    ) -> Result<(), Error>;
    /// Writes a notification (no ID).
    fn write_notification(&self, method: &str, params: Option<&dyn Encode>) -> Result<(), Error>;
    /// Writes a successful response; `None` writes a `null` result.
    fn write_response(&self, id: Option<&Id>, result: Option<&dyn Encode>) -> Result<(), Error>;
    /// Writes an error response.
    fn write_error(&self, id: Option<&Id>, error: &ResponseError) -> Result<(), Error>;
    /// A response whose payload is raw bytes rather than JSON. Only the
    /// synchronous msgpack protocol carries one; JSON-RPC never sees it.
    fn write_binary_response(&self, _id: Option<&Id>, _payload: &[u8]) -> Result<(), Error> {
        Err(Error::Message(
            "ipc: this protocol cannot carry a binary response".into(),
        ))
    }
}

/// JSON-RPC 2.0 with the LSP base protocol framing (Content-Length headers).
pub struct JsonRpcProtocol {
    reader: Mutex<Reader<Box<dyn Read + Send>>>,
    writer: Mutex<Writer<Box<dyn Write + Send>>>,
}

fn marshal(value: &dyn Encode) -> Result<Vec<u8>, Error> {
    Ok(tsr_json::marshal(value, tsr_json::Options::default())?)
}

impl JsonRpcProtocol {
    /// port: tsc/internal/ipc/protocol_jsonrpc.go:NewJSONRPCProtocol
    pub fn new(reader: Box<dyn Read + Send>, writer: Box<dyn Write + Send>) -> Arc<Self> {
        Arc::new(Self {
            reader: Mutex::new(Reader::new(reader)),
            writer: Mutex::new(Writer::new(writer)),
        })
    }

    fn write(&self, data: &[u8]) -> Result<(), Error> {
        Ok(self.writer.lock().expect("protocol writer").write(data)?)
    }
}

impl Protocol for JsonRpcProtocol {
    /// port: tsc/internal/ipc/protocol_jsonrpc.go:JSONRPCProtocol.ReadMessage
    fn read_message(&self) -> Result<Message, Error> {
        let data = self
            .reader
            .lock()
            .expect("protocol reader")
            .read()
            .map_err(|error| Error::Framing(Arc::new(error)))?;
        let mut message = Message::default();
        tsr_json::unmarshal(&data, &mut message, tsr_json::Options::default())?;
        Ok(message)
    }

    /// port: tsc/internal/ipc/protocol_jsonrpc.go:JSONRPCProtocol.WriteRequest
    fn write_request(
        &self,
        id: &Id,
        method: &str,
        params: Option<&dyn Encode>,
    ) -> Result<(), Error> {
        self.write(&marshal(&RequestMessage {
            id: Some(id),
            method,
            params,
        })?)
    }

    /// port: tsc/internal/ipc/protocol_jsonrpc.go:JSONRPCProtocol.WriteNotification
    fn write_notification(&self, method: &str, params: Option<&dyn Encode>) -> Result<(), Error> {
        self.write(&marshal(&RequestMessage {
            id: None,
            method,
            params,
        })?)
    }

    /// port: tsc/internal/ipc/protocol_jsonrpc.go:JSONRPCProtocol.WriteResponse
    fn write_response(&self, id: Option<&Id>, result: Option<&dyn Encode>) -> Result<(), Error> {
        let null = RawValue(b"null".to_vec());
        self.write(&marshal(&ResponseMessage {
            id,
            result: Some(result.unwrap_or(&null)),
            error: None,
        })?)
    }

    /// port: tsc/internal/ipc/protocol_jsonrpc.go:JSONRPCProtocol.WriteError
    fn write_error(&self, id: Option<&Id>, error: &ResponseError) -> Result<(), Error> {
        self.write(&marshal(&ResponseMessage {
            id,
            result: None,
            error: Some(error),
        })?)
    }
}
