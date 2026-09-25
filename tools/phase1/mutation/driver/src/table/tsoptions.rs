//! Group `tsoptions`: tsoptions ports (and `computeFn` through the bridge in
//! `tools/phase1/tables/go/bridges/tsoptions`).
//! Go: `tools/phase1/tables/go/tsoptions_columns.go`; spec:
//! `data/phase1/tables/tsoptions.json`.
use super::{decode, parse_config, Column};
use serde_json::{json, Value};

pub const COLUMNS: &[&str] = &["tsoptions.ParsedCommandLine.PossiblyMatchesDirectoryName"];

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "tsoptions.ParsedCommandLine.PossiblyMatchesDirectoryName" => {
            possibly_matches_directory_name(input)
        }
        _ => return None,
    })
}

/// One bool per directory path of `args`; the wildcard directories are
/// computed in setup.
fn possibly_matches_directory_name(input: &Value) -> Result<Column, String> {
    let (parsed, args) = parse_config(input)?;
    let paths: Vec<String> = decode(&args)?;
    let _ = parsed.wildcard_directories();
    Ok(Box::new(move || {
        Ok(Value::Array(
            paths
                .iter()
                .map(|path| {
                    json!(
                        parsed.possibly_matches_directory_name(&tsr_tspath::Path::from_bytes(
                            path.as_bytes().to_vec()
                        ))
                    )
                })
                .collect(),
        ))
    }))
}
