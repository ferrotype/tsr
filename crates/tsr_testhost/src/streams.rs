//! Opaque mapper bytes. No mapper framing or method dispatch lives here.
use std::collections::{BTreeMap, VecDeque};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::{
    protocol::{decode, fits, notify, response_fits, success, Id, RequestResult},
    wire::{raw, wire, Json},
};

pub(crate) const CHUNK: usize = 32 * 1024;
pub(crate) const WINDOW: usize = 64 * 1024;
const MAX_STREAM_IDENTITIES: usize = 64;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Channel {
    Stdout,
    Stderr,
}
#[derive(Default)]
struct Input {
    bytes: VecDeque<u8>,
    ended: bool,
}
#[derive(Serialize)]
#[serde(tag = "state", content = "code", rename_all = "camelCase")]
enum ExitState {
    Running,
    Unavailable,
    Exited(i32),
}

pub(crate) struct Stream {
    pub(crate) plugin: usize,
    stdout: Input,
    stderr: Input,
    credit: usize,
    closed: bool,
    exit: ExitState,
}
impl Stream {
    fn channel(&mut self, channel: Channel) -> &mut Input {
        match channel {
            Channel::Stdout => &mut self.stdout,
            Channel::Stderr => &mut self.stderr,
        }
    }
}

/// Closed identities remain bounded tombstones, so late traffic can never
/// reach a replacement stream. A connection admits at most 64 stream identities.
#[derive(Default)]
pub(crate) struct Streams(BTreeMap<String, Stream>);
impl Streams {
    pub(crate) fn can_open(&self) -> bool {
        self.can_reserve(0)
    }
    pub(crate) fn can_reserve(&self, pending: usize) -> bool {
        self.0.len() + pending < MAX_STREAM_IDENTITIES
    }
    pub(crate) fn has_active(&self, plugin: usize) -> bool {
        self.0
            .values()
            .any(|s| s.plugin == plugin && !s.closed && matches!(s.exit, ExitState::Running))
    }
    pub(crate) fn open(&mut self, name: &str, plugin: usize) -> Result<Vec<Json>, String> {
        if !self.can_open() || name.is_empty() || name.len() > 256 || self.0.contains_key(name) {
            return Err("stream identity must be new and contain 1..256 UTF8 bytes".into());
        }
        self.0.insert(
            name.to_owned(),
            Stream {
                plugin,
                stdout: Input::default(),
                stderr: Input::default(),
                credit: 0,
                closed: false,
                exit: ExitState::Running,
            },
        );
        Ok([Channel::Stdout, Channel::Stderr]
            .into_iter()
            .map(|channel| credit(name, channel, WINDOW))
            .collect())
    }

    pub(crate) fn request(&mut self, id: &Id, method: &str, params: &RawValue) -> RequestResult {
        match method {
            "test/streamWrite" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Write {
                    stream: String,
                    data: String,
                }
                let input: Write = decode(params)?;
                let bytes = decode_data(&input.data).map_err(|e| (-32602, e))?;
                let stream = self.get(&input.stream).map_err(|e| (-32602, e))?;
                if stream.closed || !matches!(stream.exit, ExitState::Running) {
                    return Err((-32002, "stream is closed".into()));
                }
                if bytes.len() > stream.credit {
                    return Err((-32002, "stream write requires credit".into()));
                }
                // Validate envelopes before changing credit or emitting bytes.
                let frame = notify(
                    "testhost/streamData",
                    &wire!({"stream":input.stream,"data":input.data}),
                );
                let result = success(id, &raw(&()));
                if !fits(&frame) || !fits(&result) {
                    return Err((-32602, "stream frame exceeds limit".into()));
                }
                stream.credit -= bytes.len();
                Ok(vec![frame, result])
            }
            "test/streamRead" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields, rename_all = "camelCase")]
                struct Read {
                    stream: String,
                    channel: Channel,
                    max_bytes: usize,
                }
                let input: Read = decode(params)?;
                if input.max_bytes == 0 || input.max_bytes > CHUNK {
                    return Err((-32602, "read size outside chunk limit".into()));
                }
                let stream = self.get(&input.stream).map_err(|e| (-32602, e))?;
                let channel = stream.channel(input.channel);
                let count = input.max_bytes.min(channel.bytes.len());
                let bytes: Vec<_> = channel.bytes.iter().take(count).copied().collect();
                let data = if count == 0 && !channel.ended {
                    None
                } else {
                    Some(STANDARD.encode(&bytes))
                };
                let value =
                    wire!({"data":data,"eof":channel.ended && count == channel.bytes.len()});
                response_fits(id, &value).map_err(|e| (-32602, e))?;
                channel.bytes.drain(..count);
                let mut messages = vec![success(id, &value)];
                if count != 0 && !channel.ended {
                    messages.push(credit(&input.stream, input.channel, count));
                }
                Ok(messages)
            }
            "test/streamStatus" => {
                let input: StreamName = decode(params)?;
                let stream = self.get(&input.stream).map_err(|e| (-32602, e))?;
                Ok(vec![success(
                    id,
                    &wire!({"closed":stream.closed,"exit":stream.exit}),
                )])
            }
            "test/closeStream" => {
                let input: StreamName = decode(params)?;
                let stream = self.get(&input.stream).map_err(|e| (-32602, e))?;
                let mut messages = vec![success(id, &raw(&()))];
                if !stream.closed {
                    close(stream);
                    messages.insert(
                        0,
                        notify("testhost/streamClose", &wire!({"stream":input.stream})),
                    );
                }
                Ok(messages)
            }
            _ => unreachable!("stream request dispatch"),
        }
    }

    /// Returns the owning plugin on a terminal event. All notification payloads
    /// remain validated after local close, but buffered late data is discarded.
    pub(crate) fn notification(
        &mut self,
        method: &str,
        params: &RawValue,
    ) -> Result<Option<usize>, String> {
        match method {
            "test/streamData" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Chunk {
                    stream: String,
                    channel: Channel,
                    data: String,
                }
                let input: Chunk = serde_json::from_str(params.get()).map_err(|e| e.to_string())?;
                let bytes = decode_data(&input.data)?;
                let stream = self.get(&input.stream)?;
                if stream.closed {
                    return Ok(None);
                }
                let channel = stream.channel(input.channel);
                if channel.ended || channel.bytes.len() + bytes.len() > WINDOW {
                    return Err("data after EOF or beyond stream credit".into());
                }
                channel.bytes.extend(bytes);
            }
            "test/streamCredit" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Credit {
                    stream: String,
                    bytes: usize,
                }
                let input: Credit =
                    serde_json::from_str(params.get()).map_err(|e| e.to_string())?;
                let stream = self.get(&input.stream)?;
                if stream.closed {
                    return Ok(None);
                }
                if input.bytes == 0 || input.bytes > WINDOW - stream.credit {
                    return Err("invalid stream credit".into());
                }
                stream.credit += input.bytes;
            }
            "test/streamEnd" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct End {
                    stream: String,
                    channel: Channel,
                }
                let input: End = serde_json::from_str(params.get()).map_err(|e| e.to_string())?;
                let stream = self.get(&input.stream)?;
                if !stream.closed {
                    let channel = stream.channel(input.channel);
                    if channel.ended {
                        return Err("duplicate stream EOF".into());
                    }
                    channel.ended = true;
                }
            }
            "test/streamExit" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Exit {
                    stream: String,
                    code: Option<i32>,
                }
                // Missing is a malformed exit report; explicit null means unavailable.
                if !crate::wire::fields(params)
                    .map_err(|e| e.to_string())?
                    .contains_key("code")
                {
                    return Err("stream exit requires code or explicit null".into());
                }
                let input: Exit = serde_json::from_str(params.get()).map_err(|e| e.to_string())?;
                let stream = self.get(&input.stream)?;
                if !matches!(stream.exit, ExitState::Running)
                    || !stream.stdout.ended
                    || !stream.stderr.ended
                {
                    return Err("exit requires both EOFs and must be reported once".into());
                }
                stream.exit = input.code.map_or(ExitState::Unavailable, ExitState::Exited);
                return Ok(Some(stream.plugin));
            }
            "test/streamClose" => {
                let input: StreamName =
                    serde_json::from_str(params.get()).map_err(|e| e.to_string())?;
                let stream = self.get(&input.stream)?;
                close(stream);
                return Ok(Some(stream.plugin));
            }
            _ => unreachable!("stream notification dispatch"),
        }
        Ok(None)
    }
    pub(crate) fn plugin_for(&self, params: &RawValue) -> Result<usize, (i64, String)> {
        let input: StreamName = decode(params)?;
        self.0
            .get(&input.stream)
            .map(|s| s.plugin)
            .ok_or_else(|| (-32602, "unknown stream".into()))
    }
    pub(crate) fn shutdown(&mut self) -> Vec<Json> {
        self.0
            .iter_mut()
            .filter_map(|(id, stream)| {
                if stream.closed {
                    return None;
                }
                close(stream);
                Some(notify("testhost/streamClose", &wire!({"stream":id})))
            })
            .collect()
    }
    fn get(&mut self, name: &str) -> Result<&mut Stream, String> {
        self.0.get_mut(name).ok_or_else(|| "unknown stream".into())
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamName {
    stream: String,
}
fn close(stream: &mut Stream) {
    stream.closed = true;
    stream.credit = 0;
    for channel in [&mut stream.stdout, &mut stream.stderr] {
        channel.bytes = VecDeque::new();
        channel.ended = true;
    }
}
fn credit(stream: &str, channel: Channel, bytes: usize) -> Json {
    notify(
        "testhost/streamCredit",
        &wire!({"stream":stream,"channel":channel,"bytes":bytes}),
    )
}
fn decode_data(data: &str) -> Result<Vec<u8>, String> {
    if data.is_empty() || data.len() > CHUNK.div_ceil(3) * 4 {
        return Err("data outside stream chunk limit".into());
    }
    let bytes = STANDARD.decode(data).map_err(|e| e.to_string())?;
    if bytes.len() > CHUNK {
        return Err("data outside stream chunk limit".into());
    }
    Ok(bytes)
}
