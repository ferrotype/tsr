use super::{baseline, failure, file_name, hex, utf8, Error, Result};
use serde_json::{json, Value};
use ts_ast::Diagnostic;
use ts_checker::Operation;
use ts_compiler::Program;

pub fn payload(program: &Program, d: &Diagnostic) -> Result<Value> {
    let args = if d.message_args.is_empty() {
        Value::Null
    } else {
        json!(d
            .message_args
            .iter()
            .map(|a| utf8(a.as_bytes()))
            .collect::<Result<Vec<_>>>()?)
    };
    Ok(
        json!({"file":d.file.map(|id|utf8(file_name(program,id)?)).transpose()?,"pos":d.loc.pos(),"end":d.loc.end(),"code":d.code,"category":d.category,
        "key_hex":hex(d.message_key.as_bytes()),"text_hex":hex(d.message_text.as_bytes()),"source_hex":hex(d.source.as_bytes()),"args":args,
        "chain":d.message_chain.iter().map(|d|payload(program,d)).collect::<Result<Vec<_>>>()?,"related":d.related_information.iter().map(|d|payload(program,d)).collect::<Result<Vec<_>>>()?,
        "unnecessary":d.reports_unnecessary,"deprecated":d.reports_deprecated,"skipped_on_no_emit":d.skipped_on_no_emit}),
    )
}
// port: tsc/internal/compiler/program.go:SortAndDeduplicateDiagnostics
// Includes compactAndMergeRelatedInfos. Serialization is outside checker algorithms.
pub fn sorted(program: &Program, mut values: Vec<Diagnostic>) -> Result<Vec<Diagnostic>> {
    for d in &values {
        let _ = payload(program, d)?;
    }
    let names = |id| file_name(program, id);
    ts_core::sort_like_go(&mut values, &mut |a, b| {
        ts_ast::compare_diagnostics(a, b, &names).expect("validated diagnostic owners")
    });
    let mut out: Vec<Diagnostic> = Vec::new();
    let mut input = values.into_iter().peekable();
    while let Some(mut value) = input.next() {
        let mut merged = false;
        while input.peek().is_some_and(|next| {
            ts_ast::equal_diagnostics_no_related_info(&value, next, &names)
                .expect("validated diagnostic owners")
        }) {
            merged = true;
            value
                .related_information
                .extend(input.next().expect("peeked diagnostic").related_information);
        }
        if merged && !value.related_information.is_empty() {
            ts_core::sort_like_go(&mut value.related_information, &mut |a, b| {
                ts_ast::compare_diagnostics(a, b, &names).expect("validated diagnostic owners")
            });
            value.related_information.dedup_by(|a, b| {
                ts_ast::equal_diagnostics(a, b, &names).expect("validated diagnostic owners")
            });
        }
        out.push(value);
    }
    Ok(out)
}
pub fn all(program: &Program, op: &mut Operation<'_>) -> Result<Value> {
    all_mode(program, op, false)
}
pub fn all_mode(program: &Program, op: &mut Operation<'_>, program_mode: bool) -> Result<Value> {
    let syntactic = program.syntactic_diagnostics(None)?;
    let mut bind = Vec::new();
    let mut semantic = Vec::new();
    let mut semantic_error = None;
    for file in program.files() {
        let source = file.bound().view().source_file()?;
        bind.extend_from_slice(source.bind_diagnostics());
        // The only library-enabled protocol uses skipLibCheck. Match the
        // compiler's selection before entering checker.GetDiagnostics; lazy
        // library types reached by source queries still execute normally.
        if program.skip_type_checking(file, false)? {
            continue;
        }
        let values = if program_mode {
            program
                .semantic_diagnostics_with_checker(op, file)
                .map_err(Error::from)
        } else {
            op.semantic_diagnostics(file.source()).map_err(Error::from)
        };
        match values {
            Ok(values) => semantic.extend(values),
            Err(error) => {
                if semantic_error.is_none() {
                    semantic_error = Some(error);
                }
            }
        }
    }
    let globals = op.global_diagnostics()?;
    let mut phases = vec![
        ("config", program.config().config_file_parsing_diagnostics()),
        ("program", program.program_diagnostics()?.to_vec()),
        ("syntactic", syntactic),
        ("bind", bind),
        ("semantic", semantic),
        ("global", globals),
    ];
    let mut declaration_error = None;
    if program.options().emit_declarations() {
        let mut values = Vec::new();
        for file in program.files() {
            match program.declaration_diagnostics_with_checker(op, file) {
                Ok(diagnostics) => values.extend(diagnostics),
                Err(error) => {
                    if declaration_error.is_none() {
                        declaration_error = Some(Error::from(error));
                    }
                }
            }
        }
        phases.push(("declaration", values));
    }
    let mut output = json!({"state":"executed"});
    let mut combined = Vec::new();
    for (phase, values) in phases {
        let values = sorted(program, values)?;
        output[phase] = json!(values
            .iter()
            .map(|d| payload(program, d))
            .collect::<Result<Vec<_>>>()?);
        if !program_mode || phase != "bind" {
            combined.extend(values);
        }
    }
    if let Some(error) = &semantic_error {
        output["semantic"] = failure(error, "semantic_diagnostics");
    }
    if let Some(error) = &declaration_error {
        output["declaration"] = failure(error, "declaration_diagnostics");
    }
    if semantic_error.is_some() || declaration_error.is_some() {
        output["combined"] = failure(
            &Error::Unsupported("combined diagnostics require completed diagnostic phases"),
            "combined_diagnostics",
        );
        output["baseline"] = failure(
            &Error::Unsupported("error baseline requires completed diagnostic phases"),
            "error_baseline",
        );
    } else {
        let combined = sorted(program, combined)?;
        output["combined"] = json!(combined
            .iter()
            .map(|d| payload(program, d))
            .collect::<Result<Vec<_>>>()?);
        output["baseline"] = match baseline::render(program, &combined) {
            Ok(value) => value,
            Err(error) => failure(&error, "error_baseline"),
        };
    }
    Ok(output)
}
