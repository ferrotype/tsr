//! Shared P5 corpus observation, independent of its process wrapper.
use crate::{baseline, errors, executor};
use serde_json::{json, Value};
type InputBytes = (Vec<u8>, Vec<u8>);
fn unhex(value: &Value) -> Result<Vec<u8>, &'static str> {
    let text = value.as_str().ok_or("missing hex bytes")?;
    if !text.len().is_multiple_of(2) {
        return Err("odd hex length");
    }
    fn digit(b: u8) -> Result<u8, &'static str> {
        match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            _ => Err("noncanonical hex digit"),
        }
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|p| Ok((digit(p[0])? << 4) | digit(p[1])?))
        .collect()
}

fn diagnostic_presence(phase: &Value) -> Result<bool, &'static str> {
    if phase["state"] != "executed" {
        return Err("requested diagnostics did not complete before baseline walk");
    }
    if let Some(files) = phase["files"].as_array() {
        let mut present = false;
        for file in files {
            present |= diagnostic_presence(&file["result"])?;
        }
        return Ok(present);
    }
    phase["diagnostics"]
        .as_array()
        .map(|d| !d.is_empty())
        .ok_or("diagnostic phase lacks payload")
}

fn input_files(request: &Value, key: &str) -> Result<Vec<InputBytes>, &'static str> {
    request[key]
        .as_array()
        .ok_or("missing native input order")?
        .iter()
        .map(|f| Ok((unhex(&f["name_hex"])?, unhex(&f["content_hex"])?)))
        .collect()
}

pub fn observe(request: &Value) -> Value {
    let mut trace = baseline::Trace {
        collect_type_strings: request["public_type_strings"] == true,
        ..Default::default()
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        executor::observe(request, |program, op, phases, diagnostic_values| {
            let complete = phases
                .as_object()
                .ok_or("missing diagnostic phases")
                .and_then(|phases| {
                    phases.values().try_fold(false, |present, phase| {
                        Ok(present | diagnostic_presence(phase)?)
                    })
                });
            let had_errors = match complete {
                Ok(value) => value,
                Err(reason) => {
                    return executor::BaselineResults {
                        type_symbols: if request["type_baseline_requested"] == true {
                            executor::failure(reason, "baseline_prerequisite")
                        } else {
                            json!({"state":"not_requested"})
                        },
                        errors: executor::failure(reason, "baseline_prerequisite"),
                    }
                }
            };
            // Sorting/merging runs on actual typed diagnostics while the Program
            // and checker operation still own all referenced source identities.
            let sorted = diagnostic_values
                .map(|values| program.sort_and_deduplicate_diagnostics(values))
                .transpose();
            let errors = match &sorted {
                Err(error) => executor::failure(error, "diagnostic_aggregation"),
                Ok(None) => json!({"state":"not_requested"}),
                Ok(Some(values)) => {
                    let diagnostics =
                        executor::diagnostics::phase(program, values)["diagnostics"].take();
                    let rendered = (|| -> Result<Value, Box<dyn std::error::Error>> {
                        // Mapper execution is a named Program boundary. Do not
                        // let oracle-selected files silently supply this filter.
                        for file in program.files() {
                            if !file
                                .bound()
                                .view()
                                .source_file()?
                                .content_mapper()
                                .is_empty()
                            {
                                return Err("P5 native content-mapped error selection".into());
                            }
                        }
                        let contents = input_files(request, "error_inputs")?;
                        let inputs: Vec<_> = contents
                            .iter()
                            .map(|(name, content)| errors::InputFile { name, content })
                            .collect();
                        errors::render(program, &inputs, values, program.options().pretty.is_true())
                    })();
                    match rendered {
                        Ok(baseline) => {
                            json!({"state":"executed","diagnostics":diagnostics,"baseline":baseline,"emit":"not_executed",
                                "pretty":program.options().pretty.is_true(),"inputs":request["error_inputs"]})
                        }
                        Err(error) => {
                            json!({"state":"failed","class":"error_baseline","reason":error.to_string(),"diagnostics":diagnostics,"emit":"not_executed"})
                        }
                    }
                }
            };
            let type_symbols = if request["type_baseline_requested"] == true {
                let result = (|| -> Result<Value, Box<dyn std::error::Error>> {
                    let contents = input_files(request, "baseline_inputs")?;
                    let files: Vec<_> = contents
                        .iter()
                        .map(|(name, content)| baseline::InputFile { name, content })
                        .collect();
                    let header = request["baseline_header"]
                        .as_str()
                        .ok_or("missing baseline header")?;
                    Ok(baseline::generate(
                        program,
                        op,
                        &files,
                        header.as_bytes(),
                        had_errors,
                        &mut trace,
                    ))
                })();
                result.unwrap_or_else(|error| executor::failure(error, "baseline_prerequisite"))
            } else {
                json!({"state":"not_requested"})
            };
            executor::BaselineResults {
                type_symbols,
                errors,
            }
        })
    }));
    match result {
        Ok(row) => row,
        Err(payload) => {
            let reason = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("non-string panic payload");
            json!({"version":1,"id":request["id"],"acceptance_tier":request["acceptance_tier"],"fatal":executor::failure(reason,"panic"),"queries":trace.queries,"active_query":trace.active})
        }
    }
}
