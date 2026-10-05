//! Blocking byte-stream adapter for S11's existing credit-controlled tunnel.
//! The connection pumps notifications while mapper threads wait on a condition.
use crate::{
    bridge::CallbackFs,
    streams::{CHUNK, WINDOW},
    wire::{wire, Json},
};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Read, Write},
    sync::{mpsc::Sender, Arc, Condvar, Mutex},
};
use tsr_ipc::{Closer, Stream};

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
#[derive(Default)]
enum ExitState {
    #[default]
    Running,
    Complete(Option<i32>),
}
#[derive(Default)]
struct State {
    stdout: VecDeque<u8>,
    stderr: VecDeque<u8>,
    ended: [bool; 2],
    credit: usize,
    closed: bool,
    exit: ExitState,
}
struct Pipe {
    name: String,
    output: Sender<Json>,
    state: Mutex<State>,
    ready: Condvar,
}
impl Pipe {
    fn send(&self, method: &str, params: &RawValue) -> io::Result<()> {
        self.output
            .send(wire!({"jsonrpc":"2.0", "method":method,"params":params}))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "mapper tunnel disconnected"))
    }
    fn credit(&self, channel: &str, bytes: usize) -> io::Result<()> {
        self.send(
            "testhost/streamCredit",
            &wire!({"stream":self.name,"channel":channel,"bytes":bytes}),
        )
    }
}
impl Closer for Pipe {
    fn close(&self) -> io::Result<()> {
        let mut state = self.state.lock().expect("mapper tunnel state");
        if state.closed {
            return Ok(());
        }
        state.closed = true;
        state.ended = [true; 2];
        state.stdout.clear();
        state.stderr.clear();
        self.ready.notify_all();
        drop(state);
        self.send("testhost/streamClose", &wire!({"stream":self.name}))
    }
    fn exit_code(&self) -> Option<i32> {
        match self.state.lock().expect("mapper tunnel state").exit {
            ExitState::Running => None,
            ExitState::Complete(code) => code,
        }
    }
}
struct Reader {
    pipe: Arc<Pipe>,
    channel: usize,
}
impl Read for Reader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let mut state = self.pipe.state.lock().expect("mapper tunnel state");
        loop {
            let ended = state.ended[self.channel];
            if state.closed {
                return Ok(0);
            }
            let input = if self.channel == 0 {
                &mut state.stdout
            } else {
                &mut state.stderr
            };
            if !input.is_empty() {
                let count = bytes.len().min(input.len()).min(CHUNK);
                for out in &mut bytes[..count] {
                    *out = input.pop_front().unwrap();
                }
                drop(state);
                if !ended {
                    self.pipe.credit(
                        if self.channel == 0 {
                            "stdout"
                        } else {
                            "stderr"
                        },
                        count,
                    )?;
                }
                return Ok(count);
            }
            if ended {
                return Ok(0);
            }
            state = self.pipe.ready.wait(state).expect("mapper tunnel state");
        }
    }
}
struct Writer(Arc<Pipe>);
impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let mut state = self.0.state.lock().expect("mapper tunnel state");
        while state.credit == 0 && !state.closed && matches!(state.exit, ExitState::Running) {
            state = self.0.ready.wait(state).expect("mapper tunnel state");
        }
        if state.closed || matches!(state.exit, ExitState::Complete(_)) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "mapper stream is closed",
            ));
        }
        let count = bytes.len().min(CHUNK).min(state.credit);
        state.credit -= count;
        self.0.send(
            "testhost/streamData",
            &wire!({"stream":self.0.name,"data":STANDARD.encode(&bytes[..count])}),
        )?;
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Registry {
    closed: bool,
    pipes: BTreeMap<String, Arc<Pipe>>,
}
pub(crate) struct Streams {
    output: Sender<Json>,
    state: Mutex<Registry>,
}
impl Streams {
    pub(crate) fn new(output: Sender<Json>) -> Self {
        Self {
            output,
            state: Mutex::new(Registry {
                closed: false,
                pipes: BTreeMap::new(),
            }),
        }
    }
    pub(crate) fn open(&self, name: &str) -> io::Result<()> {
        let mut state = self.state.lock().expect("mapper tunnel registry");
        if state.closed
            || name.is_empty()
            || name.len() > 256
            || state.pipes.len() >= 64
            || state.pipes.contains_key(name)
        {
            return Err(invalid("invalid or reused mapper stream identity"));
        }
        let pipe = Arc::new(Pipe {
            name: name.into(),
            output: self.output.clone(),
            state: Mutex::default(),
            ready: Condvar::new(),
        });
        state.pipes.insert(name.into(), pipe.clone());
        pipe.credit("stdout", WINDOW)?;
        pipe.credit("stderr", WINDOW)
    }
    pub(crate) fn close_all(&self) {
        let mut state = self.state.lock().expect("mapper tunnel registry");
        state.closed = true;
        for pipe in state.pipes.values() {
            let _ = pipe.close();
        }
    }
    pub(crate) fn notification(&self, method: &str, params: &RawValue) -> io::Result<bool> {
        let value: serde_json::Value =
            serde_json::from_str(params.get()).map_err(|e| invalid(e.to_string()))?;
        let Some(name) = value.get("stream").and_then(serde_json::Value::as_str) else {
            return Ok(false);
        };
        let Some(pipe) = self
            .state
            .lock()
            .expect("mapper tunnel registry")
            .pipes
            .get(name)
            .cloned()
        else {
            return Ok(false);
        };
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Chunk {
            stream: String,
            channel: String,
            data: String,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Credit {
            stream: String,
            bytes: usize,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct End {
            stream: String,
            channel: String,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Exit {
            stream: String,
            code: Option<i32>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Close {
            stream: String,
        }
        fn decode<T: serde::de::DeserializeOwned>(params: &RawValue) -> io::Result<T> {
            serde_json::from_str(params.get()).map_err(|e| invalid(e.to_string()))
        }
        fn channel(name: &str) -> io::Result<usize> {
            match name {
                "stdout" => Ok(0),
                "stderr" => Ok(1),
                _ => Err(invalid("invalid mapper channel")),
            }
        }
        let mut state = pipe.state.lock().expect("mapper tunnel state");
        match method {
            "test/streamData" => {
                let Chunk {
                    stream,
                    channel: name,
                    data,
                } = decode(params)?;
                debug_assert_eq!(stream, pipe.name);
                let ch = channel(&name)?;
                if data.is_empty() || data.len() > CHUNK.div_ceil(3) * 4 {
                    return Err(invalid("data outside stream chunk limit"));
                }
                let bytes = STANDARD.decode(&data).map_err(|e| invalid(e.to_string()))?;
                if bytes.len() > CHUNK {
                    return Err(invalid("data outside stream chunk limit"));
                }
                if !state.closed {
                    if state.ended[ch] {
                        return Err(invalid("data after EOF"));
                    }
                    let input = if ch == 0 {
                        &mut state.stdout
                    } else {
                        &mut state.stderr
                    };
                    if input.len() + bytes.len() > WINDOW {
                        return Err(invalid("data beyond stream credit"));
                    }
                    input.extend(bytes);
                }
            }
            "test/streamCredit" => {
                let Credit { stream, bytes } = decode(params)?;
                debug_assert_eq!(stream, pipe.name);
                if !state.closed {
                    if bytes == 0 || bytes > WINDOW - state.credit {
                        return Err(invalid("invalid stream credit"));
                    }
                    state.credit += bytes;
                }
            }
            "test/streamEnd" => {
                let End {
                    stream,
                    channel: name,
                } = decode(params)?;
                debug_assert_eq!(stream, pipe.name);
                let ch = channel(&name)?;
                if !state.closed {
                    if state.ended[ch] {
                        return Err(invalid("duplicate stream EOF"));
                    }
                    state.ended[ch] = true;
                }
            }
            "test/streamExit" => {
                if value.get("code").is_none() {
                    return Err(invalid("stream exit requires code or explicit null"));
                }
                let Exit { stream, code } = decode(params)?;
                debug_assert_eq!(stream, pipe.name);
                if matches!(state.exit, ExitState::Complete(_))
                    || !state.ended.iter().all(|&end| end)
                {
                    return Err(invalid("exit requires both EOFs and must be reported once"));
                }
                state.exit = ExitState::Complete(code);
            }
            "test/streamClose" => {
                let Close { stream } = decode(params)?;
                debug_assert_eq!(stream, pipe.name);
                state.closed = true;
                state.ended = [true; 2];
                state.stdout.clear();
                state.stderr.clear();
            }
            _ => return Ok(false),
        }
        pipe.ready.notify_all();
        Ok(true)
    }
}
pub(crate) struct Spawner {
    callbacks: CallbackFs,
    streams: Arc<Streams>,
}
impl Spawner {
    pub(crate) fn new(callbacks: CallbackFs, streams: Arc<Streams>) -> Self {
        Self { callbacks, streams }
    }
}
impl tsr_contentmapper::Spawner for Spawner {
    fn spawn(
        &self,
        command: &[tsr_jsstring::JsString],
        dir: &[u8],
        mut stderr: Box<dyn Write + Send>,
    ) -> Result<Stream, tsr_contentmapper::SpawnError> {
        let command = command
            .iter()
            .map(|part| std::str::from_utf8(part.as_bytes()).map(str::to_owned))
            .collect::<Result<Vec<_>, _>>()?;
        let name = command
            .first()
            .ok_or_else(|| invalid("empty mapper command"))?;
        let cwd = std::str::from_utf8(dir)?;
        let result = self
            .callbacks
            .spawn_mapper(&wire!({"name":name,"options":wire!({"command":command,"cwd":cwd})}))
            .map_err(|e| invalid(format!("mapper spawn callback: {e:?}")))?;
        let name = result
            .get("stream")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid("invalid mapper spawn response"))?;
        let pipe = self
            .streams
            .state
            .lock()
            .expect("mapper tunnel registry")
            .pipes
            .get(name)
            .cloned()
            .ok_or_else(|| invalid("missing mapper stream"))?;
        let mut error_reader = Reader {
            pipe: pipe.clone(),
            channel: 1,
        };
        if let Err(error) = std::thread::Builder::new()
            .name("mapper tunnel stderr".into())
            .spawn(move || {
                let _ = io::copy(&mut error_reader, &mut stderr);
            })
        {
            let _ = pipe.close();
            return Err(Box::new(error));
        }
        Ok(Stream {
            reader: Box::new(Reader {
                pipe: pipe.clone(),
                channel: 0,
            }),
            writer: Box::new(Writer(pipe.clone())),
            closer: pipe,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::{sync::mpsc, time::Duration};
    fn value(receive: &mpsc::Receiver<Json>) -> Value {
        serde_json::from_str(receive.recv_timeout(Duration::from_secs(5)).unwrap().get()).unwrap()
    }
    fn router() -> (crate::bridge::CallbackRouter, mpsc::Receiver<Json>) {
        let mut session = crate::Session::default();
        let message = json!({"jsonrpc":"2.0","id":0,"method":"test/initialize","params":{"version":2,"caseSensitive":true,"base":{},"symlinks":{},"callbacks":[],"plugins":[],"options":{}}});
        session
            .receive(&crate::framing::parse_json(message.to_string().as_bytes()).unwrap())
            .unwrap();
        let token = session.pending_options().unwrap().token;
        session.complete_options(token, Ok(())).unwrap();
        let (output, receive) = mpsc::channel();
        (session.callback_router(output).unwrap(), receive)
    }
    #[test]
    fn existing_tunnel_carries_bytes_credit_and_disconnect_unblocks_readers() {
        let (router, receive) = router();
        let spawner = router.mapper_spawner();
        let spawning = std::thread::spawn(move || {
            spawner
                .spawn(
                    &[tsr_jsstring::JsString::from_bytes(b"mapper".as_slice())],
                    b"/p",
                    Box::new(io::sink()),
                )
                .unwrap()
        });
        let spawn = value(&receive);
        assert_eq!(spawn["method"], "testhost/spawnPlugin");
        assert_eq!(spawn["params"]["options"]["cwd"], "/p");
        assert!(router.complete(
            spawn["id"].as_str().unwrap(),
            Ok(json!({"stream":"mapper:1"}))
        ));
        assert_eq!(value(&receive)["params"]["bytes"], WINDOW);
        assert_eq!(value(&receive)["params"]["bytes"], WINDOW);
        let mut stream = spawning.join().unwrap();
        let notify = |method: &str, params: Value| {
            router
                .mapper_notification(method, &crate::wire::raw(&params))
                .unwrap()
        };
        assert!(notify(
            "test/streamCredit",
            json!({"stream":"mapper:1","bytes":WINDOW})
        ));
        stream.writer.write_all(b"request").unwrap();
        assert_eq!(
            value(&receive)["params"]["data"],
            STANDARD.encode(b"request")
        );
        assert!(notify(
            "test/streamData",
            json!({"stream":"mapper:1","channel":"stdout","data":STANDARD.encode(b"reply")})
        ));
        let mut bytes = [0; 5];
        stream.reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"reply");
        assert_eq!(value(&receive)["params"]["bytes"], 5);
        let reader = std::thread::spawn(move || stream.reader.read(&mut [0; 1]));
        router.retire();
        assert_eq!(reader.join().unwrap().unwrap(), 0);
        assert_eq!(value(&receive)["method"], "testhost/streamClose");
    }
    #[test]
    fn reset_settles_spawn_callback() {
        let (router, receive) = router();
        let spawner = router.mapper_spawner();
        let spawning = std::thread::spawn(move || {
            spawner
                .spawn(
                    &[tsr_jsstring::JsString::from_bytes(b"mapper".as_slice())],
                    b"/",
                    Box::new(io::sink()),
                )
                .is_err()
        });
        let _ = value(&receive);
        router.retire();
        assert!(spawning.join().unwrap());
    }

    #[test]
    fn malformed_spawn_reply_settles_without_admitting_a_stream() {
        let (router, receive) = router();
        let spawner = router.mapper_spawner();
        let spawning = std::thread::spawn(move || {
            spawner
                .spawn(
                    &[tsr_jsstring::JsString::from_bytes(b"mapper".as_slice())],
                    b"/",
                    Box::new(io::sink()),
                )
                .is_err()
        });
        let spawn = value(&receive);
        assert!(router.complete(
            spawn["id"].as_str().unwrap(),
            Ok(json!({"stream":"mapper:invalid","unexpected":true}))
        ));
        assert!(spawning.join().unwrap());
        assert_eq!(router.pending_count(), 0);
        assert!(!router
            .mapper_notification(
                "test/streamClose",
                &crate::wire::raw(&json!({"stream":"mapper:invalid"}))
            )
            .unwrap());
    }

    #[test]
    fn process_exit_unblocks_credit_waiter_and_preserves_buffered_output() {
        let (output, receive) = mpsc::channel();
        let streams = Streams::new(output);
        streams.open("mapper:exit").unwrap();
        let _ = value(&receive);
        let _ = value(&receive);
        let pipe = streams.state.lock().unwrap().pipes["mapper:exit"].clone();
        let mut writer = Writer(pipe.clone());
        let (finished, result) = mpsc::channel();
        let write = std::thread::spawn(move || {
            finished
                .send(writer.write(b"waiting for credit").unwrap_err().kind())
                .unwrap();
        });
        let notify = |method: &str, params: Value| {
            streams
                .notification(method, &crate::wire::raw(&params))
                .unwrap()
        };
        notify(
            "test/streamData",
            json!({"stream":"mapper:exit","channel":"stdout","data":STANDARD.encode(b"last output")}),
        );
        for channel in ["stdout", "stderr"] {
            notify(
                "test/streamEnd",
                json!({"stream":"mapper:exit","channel":channel}),
            );
        }
        notify("test/streamExit", json!({"stream":"mapper:exit","code":9}));
        assert_eq!(
            result.recv_timeout(Duration::from_secs(5)).unwrap(),
            io::ErrorKind::BrokenPipe
        );
        write.join().unwrap();
        let mut reader = Reader {
            pipe: pipe.clone(),
            channel: 0,
        };
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"last output");
        assert_eq!(pipe.exit_code(), Some(9));
        assert!(streams
            .notification(
                "test/streamExit",
                &crate::wire::raw(&json!({"stream":"mapper:exit","code":9}))
            )
            .is_err());
    }

    #[test]
    fn terminal_streams_validate_late_traffic_and_cannot_reuse_identities() {
        let (output, receive) = mpsc::channel();
        let streams = Streams::new(output);
        streams.open("mapper:closed").unwrap();
        let _ = value(&receive);
        let _ = value(&receive);
        streams.close_all();
        assert_eq!(value(&receive)["method"], "testhost/streamClose");
        assert!(streams.notification("test/streamData", &crate::wire::raw(&json!({"stream":"mapper:closed","channel":"stdout","data":STANDARD.encode(b"late")}))).unwrap());
        assert!(streams
            .notification(
                "test/streamData",
                &crate::wire::raw(
                    &json!({"stream":"mapper:closed","channel":"stdout","data":"%%%"})
                )
            )
            .is_err());
        assert!(streams
            .notification(
                "test/streamExit",
                &crate::wire::raw(&json!({"stream":"mapper:closed","code":null}))
            )
            .unwrap());
        assert!(streams.open("mapper:closed").is_err());
        assert!(streams.open("mapper:new").is_err());
    }
}
