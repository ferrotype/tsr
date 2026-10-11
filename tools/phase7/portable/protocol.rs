//! Compiler-independent, byte-preserving single-file acceptance protocol.
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TranspileRequest {
    pub version: u32,
    pub declaration: bool,
    pub source: Vec<u8>,
    pub filename: Vec<u8>,
    /// Raw numeric-enum compiler-options JSON, preserving object entry order.
    pub compiler_options: Vec<u8>,
    pub report_diagnostics: bool,
}

impl TranspileRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_version(self.version)?;
        if self.source.len() > i32::MAX as usize || self.filename.len() > i32::MAX as usize {
            return Err("source and filename must fit signed source positions".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TranspileResponse {
    pub version: u32,
    pub output: Vec<u8>,
    pub source_map: Vec<u8>,
    pub diagnostics: Vec<DiagnosticRecord>,
    pub sources: Vec<SourceRecord>,
}

impl TranspileResponse {
    pub fn validate(&self) -> Result<(), String> {
        validate_version(self.version)?;
        if self
            .sources
            .iter()
            .any(|source| source.canonical_name.is_some())
        {
            return Err(
                "portable transpile does not support mapped or supplemental diagnostic sources"
                    .to_owned(),
            );
        }
        for diagnostic in &self.diagnostics {
            diagnostic.validate(self.sources.len())?;
        }
        Ok(())
    }
}

fn validate_version(version: u32) -> Result<(), String> {
    if version != VERSION {
        return Err(format!("unsupported portable transpile version: {version}"));
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceRecord {
    pub name: Vec<u8>,
    pub path: Vec<u8>,
    pub text: Vec<u8>,
    /// Mapped/supplemental sources are rejected in version 1.
    pub canonical_name: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticRecord {
    /// Index into the response's retained source snapshots, never an arena ID.
    pub file: Option<usize>,
    pub pos: i64,
    pub end: i64,
    pub code: i32,
    pub category: i32,
    pub source: Vec<u8>,
    pub key: Vec<u8>,
    pub text: Vec<u8>,
    pub args: Vec<Vec<u8>>,
    /// Arguments resolved against the retained source for display.
    pub display_args: Vec<Vec<u8>>,
    /// Reconstruct a pinned message by key only when the original had one.
    pub has_message: bool,
    pub ad_hoc_message: Option<Vec<u8>>,
    pub chain: Vec<DiagnosticRecord>,
    pub related: Vec<DiagnosticRecord>,
    pub reports_unnecessary: bool,
    pub reports_deprecated: bool,
    pub skipped_on_no_emit: bool,
    pub repopulate: Option<RepopulateRecord>,
}

impl DiagnosticRecord {
    fn validate(&self, sources: usize) -> Result<(), String> {
        if self.file.is_some_and(|index| index >= sources) {
            return Err("diagnostic file index outside source snapshots".to_owned());
        }
        for diagnostic in self.chain.iter().chain(&self.related) {
            diagnostic.validate(sources)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepopulateRecord {
    pub kind: i32,
    pub module_reference: Vec<u8>,
    pub mode: i32,
    pub package_name: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_payloads_and_option_order_survive_the_wire() {
        let request = TranspileRequest {
            version: VERSION,
            declaration: false,
            source: vec![0xff, 0xed, 0xa0, 0x80],
            filename: vec![b'/', 0xfe, b'.', b't', b's'],
            compiler_options: br#"{"paths":{"10":["a"],"2":["b"]}}"#.to_vec(),
            report_diagnostics: true,
        };
        let json = serde_json::to_vec(&request).unwrap();
        assert_eq!(
            serde_json::from_slice::<TranspileRequest>(&json).unwrap(),
            request
        );
    }

    #[test]
    fn malformed_request_shapes_and_versions_fail() {
        let request = br#"{"version":1,"declaration":false,"source":[],"filename":[],"compiler_options":[123,125],"report_diagnostics":false}"#;
        let mut value: serde_json::Value = serde_json::from_slice(request).unwrap();
        value["extra"] = true.into();
        assert!(serde_json::from_value::<TranspileRequest>(value).is_err());
        let duplicate = String::from_utf8(request.to_vec())
            .unwrap()
            .replace("\"version\":1", "\"version\":1,\"version\":1");
        assert!(serde_json::from_str::<TranspileRequest>(&duplicate).is_err());
        let mut parsed: TranspileRequest = serde_json::from_slice(request).unwrap();
        parsed.version = VERSION + 1;
        assert!(parsed.validate().is_err());
        assert!(serde_json::from_str::<TranspileRequest>("{}").is_err());
    }

    #[test]
    fn response_rejects_nested_foreign_file_indices() {
        let diagnostic = DiagnosticRecord {
            file: None,
            pos: -1,
            end: -1,
            code: 1,
            category: 1,
            source: vec![],
            key: vec![],
            text: vec![],
            args: vec![],
            display_args: vec![],
            has_message: false,
            ad_hoc_message: None,
            chain: vec![],
            related: vec![],
            reports_unnecessary: false,
            reports_deprecated: false,
            skipped_on_no_emit: false,
            repopulate: None,
        };
        let mut parent = diagnostic.clone();
        let mut child = diagnostic;
        child.file = Some(0);
        parent.related.push(child);
        let response = TranspileResponse {
            version: VERSION,
            output: vec![],
            source_map: vec![],
            diagnostics: vec![parent],
            sources: vec![],
        };
        assert!(response.validate().is_err());
    }
}
