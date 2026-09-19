//! Source trace callbacks retain the diagnostic identity and typed arguments.
//! Rendering belongs to the host; source strings keep their original bytes.
use ts_diagnostics::Message;
use ts_jsstring::JsString;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceArg {
    Text(JsString),
    Bool(bool),
}
impl From<&[u8]> for TraceArg {
    fn from(value: &[u8]) -> Self {
        Self::Text(JsString::from_bytes(value))
    }
}
impl<const N: usize> From<&[u8; N]> for TraceArg {
    fn from(value: &[u8; N]) -> Self {
        Self::from(value.as_slice())
    }
}
impl From<Vec<u8>> for TraceArg {
    fn from(value: Vec<u8>) -> Self {
        Self::Text(JsString::from_bytes(value))
    }
}
impl From<&Vec<u8>> for TraceArg {
    fn from(value: &Vec<u8>) -> Self {
        Self::from(value.as_slice())
    }
}
impl From<JsString> for TraceArg {
    fn from(value: JsString) -> Self {
        Self::Text(value)
    }
}
impl From<&JsString> for TraceArg {
    fn from(value: &JsString) -> Self {
        Self::Text(value.clone())
    }
}
impl From<bool> for TraceArg {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}
impl From<&str> for TraceArg {
    fn from(value: &str) -> Self {
        Self::from(value.as_bytes())
    }
}

/// source: tsc/internal/module/resolver.go:DiagAndArgs
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagAndArgs {
    pub message: &'static Message,
    pub args: Vec<TraceArg>,
}
#[derive(Default)]
pub(super) struct Tracer {
    pub active: bool,
    traces: Vec<DiagAndArgs>,
}
impl Tracer {
    pub fn begin(&mut self, enabled: bool) {
        self.traces.clear();
        self.active = enabled;
    }
    /// port: tsc/internal/module/resolver.go:tracer.write
    pub fn write(&mut self, message: &'static Message, args: impl IntoIterator<Item = TraceArg>) {
        if self.active {
            self.traces.push(DiagAndArgs {
                message,
                args: args.into_iter().collect(),
            });
        }
    }
    /// port: tsc/internal/module/resolver.go:tracer.getTraces
    pub fn take(&mut self) -> Vec<DiagAndArgs> {
        std::mem::take(&mut self.traces)
    }
}

// Like the source nil-tracer branches, arguments are not built when tracing is
// disabled. In particular path joins and conditions strings stay lazy.
macro_rules! trace {
    ($resolver:expr,$message:expr $(,$argument:expr)* $(,)?) => {
        if $resolver.tracer.active {
            $resolver.tracer.write($message,[$($crate::trace::TraceArg::from($argument)),*]);
        }
    };
}
pub(crate) use trace;

pub(super) fn extensions_text(extensions: u8) -> Vec<u8> {
    let mut output = Vec::new();
    for (flag, name) in [
        (crate::resolver::TS, b"TypeScript".as_slice()),
        (crate::resolver::JS, b"JavaScript"),
        (crate::resolver::DTS, b"Declaration"),
        (crate::resolver::JSON, b"JSON"),
    ] {
        if extensions & flag != 0 {
            if !output.is_empty() {
                output.extend_from_slice(b", ");
            }
            output.extend_from_slice(name);
        }
    }
    output
}
pub(super) fn joined(values: &[JsString], separator: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            output.extend_from_slice(separator);
        }
        output.extend_from_slice(value.as_bytes());
    }
    output
}

pub(super) fn conditions(values: &[JsString]) -> Vec<u8> {
    let mut output = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.extend_from_slice(b", ");
        }
        output.push(b'\'');
        output.extend_from_slice(value.as_bytes());
        output.push(b'\'');
    }
    output
}
pub(super) fn package_id(package: &crate::PackageId) -> Vec<u8> {
    let mut output = package.name.as_bytes().to_vec();
    if !package.sub_module_name.is_empty() {
        output.push(b'/');
        output.extend_from_slice(package.sub_module_name.as_bytes());
    }
    output.push(b'@');
    output.extend_from_slice(package.version.as_bytes());
    output.extend_from_slice(package.peer_dependencies.as_bytes());
    output
}
impl crate::Resolver {
    // A source tracer belongs to one call. If its caller recovers a native
    // panic, later metadata reads must not append into that abandoned call.
    pub(super) fn trace_operation<T>(
        &mut self,
        work: impl FnOnce(&mut Self) -> Result<T, crate::Error>,
    ) -> Result<T, crate::Error> {
        if !self.tracer.active {
            return work(self);
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(self)));
        self.tracer.active = false;
        match outcome {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    pub(super) fn validate_package_field(
        &mut self,
        info: &crate::PackageJson,
        name: &str,
        expected: &str,
    ) -> bool {
        if let Some(field) = info.contents.field(name) {
            if field.state.is_present() {
                if field.state.valid {
                    return true;
                }
                trace!(
                    self,
                    ts_diagnostics::Expected_type_of_0_field_in_package_json_to_be_1_got_2,
                    name,
                    expected,
                    field.state.actual_type
                );
            }
        }
        trace!(
            self,
            ts_diagnostics::X_package_json_does_not_have_a_0_field,
            name
        );
        false
    }
    pub(super) fn package_json_path_field(
        &mut self,
        info: &crate::PackageJson,
        name: &str,
    ) -> Option<Vec<u8>> {
        if !self.validate_package_field(info, name, "string") {
            return None;
        }
        let field = info.string(name).unwrap_or_default();
        if field.is_empty() {
            trace!(
                self,
                ts_diagnostics::X_package_json_had_a_falsy_0_field,
                name
            );
            return None;
        }
        let path = ts_tspath::resolve(info.directory.as_bytes(), &[field]);
        trace!(
            self,
            ts_diagnostics::X_package_json_has_0_field_1_that_references_2,
            name,
            field,
            &path
        );
        Some(path)
    }
}

#[cfg(test)]
#[path = "trace_tests.rs"]
mod tests;
