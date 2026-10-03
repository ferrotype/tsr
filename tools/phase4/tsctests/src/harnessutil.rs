//! The two `testutil/harnessutil` pieces the command-line harness uses: the
//! fake version string and the baselining tracer.
use crate::execute::tsc::{write_all, Writer};
use crate::goutil::{cut_prefix, cut_suffix, replace, trim_prefix, trim_suffix, StringBuilder};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use tsr_diagnostics::{Argument, Message};
use tsr_jsstring::JsString;

/// `harnessutil.FakeTSVersion`.
pub const FAKE_TS_VERSION: &str = "FakeTSVersion";

/// `tspath.ComparePathsOptions`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComparePathsOptions {
    pub use_case_sensitive_file_names: bool,
    pub current_directory: Vec<u8>,
}

/// `harnessutil.TracerForBaselining`: rewrites module-resolution traces so a
/// baseline does not depend on whether a `package.json` lookup was cached.
pub struct TracerForBaselining {
    opts: ComparePathsOptions,
    /// The pin's map is unsynchronized; build tasks trace into their own
    /// writers with `usePackageJsonCache` false and never touch it.
    package_json_cache: Mutex<HashMap<JsString, bool>>,
    builder: Arc<StringBuilder>,
}

impl TracerForBaselining {
    // port: tsc/internal/testutil/harnessutil/harnessutil.go:NewTracerForBaselining
    pub fn new(opts: ComparePathsOptions, builder: Arc<StringBuilder>) -> Self {
        Self {
            opts,
            package_json_cache: Mutex::new(HashMap::new()),
            builder,
        }
    }

    // port: tsc/internal/testutil/harnessutil/harnessutil.go:TracerForBaselining.Trace
    pub fn trace(&self, msg: &Message, args: &[Argument]) {
        self.trace_with_writer(
            &*self.builder,
            &msg.localize(&tsr_locale::DEFAULT, args),
            true,
        );
    }

    // port: tsc/internal/testutil/harnessutil/harnessutil.go:TracerForBaselining.TraceWithWriter
    pub fn trace_with_writer(&self, w: &dyn Writer, msg: &[u8], use_package_json_cache: bool) {
        let mut line = self.sanitize_trace(msg, use_package_json_cache);
        line.push(b'\n');
        write_all(w, &line);
    }

    fn to_path(&self, file: &[u8]) -> JsString {
        tsr_tspath::to_path(
            file,
            &self.opts.current_directory,
            self.opts.use_case_sensitive_file_names,
        )
    }

    // port: tsc/internal/testutil/harnessutil/harnessutil.go:TracerForBaselining.sanitizeTrace
    pub fn sanitize_trace(&self, msg: &[u8], use_package_json_cache: bool) -> Vec<u8> {
        // Version
        let version = format!("'{}'", tsr_core::version());
        let fake = format!("'{FAKE_TS_VERSION}'");
        let replaced = replace(msg, version.as_bytes(), fake.as_bytes(), 1);
        if replaced != msg {
            return replaced;
        }
        let mut cache = self
            .package_json_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // caching of fs in trace to be replaces with non caching version
        if let (text, true) = cut_suffix(
            msg,
            b"' does not exist according to earlier cached lookups.",
        ) {
            let file = trim_prefix(text, b"File '");
            if use_package_json_cache {
                let file_path = self.to_path(file);
                if cache.contains_key(&file_path) {
                    return msg.to_vec();
                }
                cache.insert(file_path, false);
            }
            return [b"File '", file, b"' does not exist."].concat();
        }
        if let (text, true) = cut_suffix(msg, b"' exists according to earlier cached lookups.") {
            let file = trim_prefix(text, b"File '");
            if use_package_json_cache {
                let file_path = self.to_path(file);
                if cache.contains_key(&file_path) {
                    return msg.to_vec();
                }
                cache.insert(file_path, true);
            }
            return [b"Found 'package.json' at '", file, b"'."].concat();
        }
        if use_package_json_cache {
            if let (text, true) = cut_suffix(msg, b"' does not exist.") {
                let file = trim_prefix(text, b"File '");
                let file_path = self.to_path(file);
                if cache.contains_key(&file_path) {
                    return [
                        b"File '",
                        file,
                        b"' does not exist according to earlier cached lookups.",
                    ]
                    .concat();
                }
                cache.insert(file_path, false);
                return msg.to_vec();
            }
            if let (text, true) = cut_prefix(msg, b"Found 'package.json' at '") {
                let file = trim_suffix(text, b"'.");
                let file_path = self.to_path(file);
                if cache.contains_key(&file_path) {
                    return [
                        b"File '",
                        file,
                        b"' exists according to earlier cached lookups.",
                    ]
                    .concat();
                }
                cache.insert(file_path, true);
                return msg.to_vec();
            }
        }
        msg.to_vec()
    }

    // port: tsc/internal/testutil/harnessutil/harnessutil.go:TracerForBaselining.String
    pub fn string(&self) -> Vec<u8> {
        self.builder.string()
    }

    // port: tsc/internal/testutil/harnessutil/harnessutil.go:TracerForBaselining.Reset
    pub fn reset(&self) {
        self.package_json_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}
