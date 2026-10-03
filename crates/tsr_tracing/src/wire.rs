use tsr_checker::{TraceArgs, TraceValue};
use tsr_json::{Encode, Encoder, Error, Token};
use tsr_jsstring::JsString;

pub(crate) struct Event<'a> {
    pub tid: i64,
    pub ph: &'a str,
    pub category: &'a str,
    pub ts: f64,
    pub name: &'a str,
    pub scope: &'a str,
    pub duration: Option<f64>,
    pub args: Option<&'a TraceArgs>,
}
impl<'a> Event<'a> {
    pub fn metadata(tid: i64, ts: f64, name: &'a str, args: Option<&'a TraceArgs>) -> Self {
        Self {
            tid,
            ph: "M",
            category: "__metadata",
            ts,
            name,
            scope: "",
            duration: None,
            args,
        }
    }
}
fn field(out: &mut Encoder<'_>, key: &[u8], value: &(impl Encode + ?Sized)) -> Result<(), Error> {
    out.string(key)?;
    out.value(value)
}
impl Encode for Event<'_> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_token(Token::BeginObject)?;
        field(out, b"pid", &1_i64)?;
        field(out, b"tid", &self.tid)?;
        field(out, b"ph", self.ph)?;
        field(out, b"cat", self.category)?;
        field(out, b"ts", &self.ts)?;
        if !self.name.is_empty() {
            field(out, b"name", self.name)?;
        }
        if !self.scope.is_empty() {
            field(out, b"s", self.scope)?;
        }
        if let Some(duration) = self.duration {
            field(out, b"dur", &duration)?;
        }
        if let Some(args) = self.args.filter(|args| !args.is_empty()) {
            field(out, b"args", &Arguments(args))?;
        }
        out.write_token(Token::EndObject)
    }
}
struct Arguments<'a>(&'a TraceArgs);
impl Encode for Arguments<'_> {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_token(Token::BeginObject)?;
        for (key, value) in self.0 {
            out.string(key.as_bytes())?;
            match value {
                TraceValue::Bool(value) => out.value(value)?,
                TraceValue::Int(value) => out.value(value)?,
                TraceValue::Str(value) => out.value(value)?,
                TraceValue::Strs(value) => out.value(value)?,
            }
        }
        out.write_token(Token::EndObject)
    }
}
// port: tsc/internal/tracing/tracing.go:writeEventTo
// Tracing passes its locked buffer directly, folding the forwarding wrapper.
// port: tsc/internal/tracing/tracing.go:Tracing.writeEvent
pub(crate) fn write_event(out: &mut Vec<u8>, event: &Event<'_>) {
    tsr_json::marshal_write(
        out,
        event,
        tsr_json::Options {
            deterministic: Some(true),
            ..Default::default()
        },
    )
    .expect("trace events contain only finite timestamps and JSON-representable values");
}
pub(crate) struct TraceRecord {
    pub config_file_path: JsString,
    pub trace_path: JsString,
    pub types_path: JsString,
    pub checker_id: usize,
}
impl Encode for TraceRecord {
    fn encode(&self, out: &mut Encoder<'_>) -> Result<(), Error> {
        out.write_token(Token::BeginObject)?;
        if !self.config_file_path.is_empty() {
            field(out, b"configFilePath", &self.config_file_path)?;
        }
        if !self.trace_path.is_empty() {
            field(out, b"tracePath", &self.trace_path)?;
        }
        if !self.types_path.is_empty() {
            field(out, b"typesPath", &self.types_path)?;
        }
        field(out, b"checkerId", &(self.checker_id as u64))?;
        out.write_token(Token::EndObject)
    }
}
