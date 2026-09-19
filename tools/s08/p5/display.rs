//! Direct public display requests. Expected bytes are never read by this adapter.
use serde_json::{json, Value};
use std::{ops::ControlFlow, sync::Arc};
use ts_arena::{CheckerIdentity, Counters, Generation, NodeId};
use ts_ast::{AstView, ChildVisitor, NodeListId, NodeSlice, SyntaxKind as K};
use ts_checker::CheckerOwner;
use ts_compiler::{FileCache, Program, ProgramCheckerHost, ProgramOptions};
use ts_core::{CompilerOptions, ModuleKind, ScriptTarget, Tristate};
use ts_jsstring::JsString;
use ts_printer::{EmitTextWriter, Printer, PrinterOptions, TextWriter};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn text(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(|| "expected string".into())
}

fn array(value: &Value) -> Result<&[Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "expected array".into())
}

fn decode_hex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return Err("odd source hex length".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |b: u8| char::from(b).to_digit(16).ok_or("invalid source hex");
            Ok(u8::try_from(digit(pair[0])? * 16 + digit(pair[1])?)?)
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|&byte| {
            [
                char::from(DIGITS[usize::from(byte >> 4)]),
                char::from(DIGITS[usize::from(byte & 15)]),
            ]
        })
        .collect()
}

struct Children<'a> {
    view: AstView<'a>,
    nodes: Vec<NodeId>,
    error: Option<ts_arena::Error>,
}
impl ChildVisitor for Children<'_> {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.nodes.push(node);
        ControlFlow::Continue(())
    }
    fn visit_list(&mut self, list: NodeListId) -> ControlFlow<()> {
        match self.view.list(list) {
            Ok(list) => self.visit_node_slice(list.nodes()),
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
    fn visit_node_slice(&mut self, slice: NodeSlice) -> ControlFlow<()> {
        match self.view.node_slice(slice) {
            Ok(slice) => {
                self.nodes.extend(slice.iter().flatten());
                ControlFlow::Continue(())
            }
            Err(error) => {
                self.error = Some(error);
                ControlFlow::Break(())
            }
        }
    }
}

fn declaration(program: &Program, name: &str) -> Result<(NodeId, NodeId, NodeId)> {
    let mut found = None;
    for file in program.files() {
        let view = file.bound().view().ast();
        let mut work = vec![file.source()];
        while let Some(id) = work.pop() {
            let read = view.node(id)?;
            if matches!(
                read.kind().known(),
                Some(
                    K::TypeAliasDeclaration
                        | K::InterfaceDeclaration
                        | K::VariableDeclaration
                        | K::NamespaceExport
                )
            ) {
                if let Some(n) = read.name() {
                    if view.node_text(n)?.as_bytes() == name.as_bytes()
                        && found.replace((id, n, file.source())).is_some()
                    {
                        return Err("ambiguous declaration".into());
                    }
                }
            }
            let mut children = Children {
                view,
                nodes: vec![],
                error: None,
            };
            let _ = read.for_each_child(&mut children);
            if let Some(error) = children.error {
                return Err(error.into());
            }
            work.extend(children.nodes.into_iter().rev());
        }
    }
    found.ok_or_else(|| "missing declaration".into())
}

pub fn observe(request: &Value) -> Result<Value> {
    if request["version"] != 1 {
        return Err("unknown request version".into());
    }
    let mut programs = Vec::new();
    for r in array(&request["programs"])? {
        let counters = Counters::new();
        let mut fs = ts_vfs::MemoryBuilder::new(b"/", true);
        for (path, content) in r["files"].as_object().ok_or("expected files")? {
            fs.insert_loaded(path.as_bytes(), text(content)?.as_bytes());
        }
        if let Some(files) = r.get("file_bytes") {
            for (path, content) in files.as_object().ok_or("expected hex files")? {
                if r["files"].get(path).is_some() {
                    return Err("duplicate source encoding".into());
                }
                fs.insert_loaded(path.as_bytes(), decode_hex(text(content)?)?);
            }
        }
        let roots = array(&r["roots"])?
            .iter()
            .map(|v| text(v).map(|s| JsString::from_bytes(s.as_bytes())))
            .collect::<Result<Vec<_>>>()?;
        let module = match r.get("module").map(text).transpose()?.unwrap_or("esnext") {
            "esnext" => ModuleKind::ESNEXT,
            "node16" => ModuleKind::NODE16,
            "nodenext" => ModuleKind::NODE_NEXT,
            _ => return Err("unknown display module option".into()),
        };
        let program = Arc::new(Program::load(
            ProgramOptions {
                config: ts_tsoptions::ParsedCommandLine::new(
                    CompilerOptions {
                        target: ScriptTarget::ESNEXT,
                        module,
                        strict: Tristate::TRUE,
                        no_lib: Tristate::TRUE,
                        ..Default::default()
                    },
                    roots,
                ),
                host: Arc::new(fs.finish()),
                current_directory: JsString::from_bytes(b"/".as_slice()),
                default_library_path: JsString::from_bytes(b"/no-default-lib".as_slice()),
                skip_module_resolution: false,
            },
            &mut FileCache::new(),
            &counters,
        )?);
        let owner = Arc::new(CheckerOwner::for_program(
            CheckerIdentity::new(Generation::new(&counters), &counters),
            &counters,
            Arc::new(ProgramCheckerHost::new(program.clone())),
        )?);
        let mut op = owner.operation()?;
        for file in program.files() {
            op.semantic_diagnostics(file.source())?;
        }
        op.global_diagnostics()?;
        let mut queries = Vec::new();
        for q in array(&r["queries"])? {
            let (decl, name, source) = declaration(&program, text(&q["declaration"])?)?;
            let (context_decl, _, context_source) = match q.get("enclosing_declaration") {
                Some(value) => declaration(&program, text(value)?)?,
                None => (decl, name, source),
            };
            let enclosing = match q["context"].as_str() {
                Some("declaration") => Some(context_decl),
                Some("source") => Some(context_source),
                None if q["context"].is_null() => None,
                _ => return Err("unknown context".into()),
            };
            let flags = u32::try_from(q["flags"].as_u64().ok_or("expected flags")?)?;
            let mut result = json!({"id":q["id"],"state":"content"});
            if q["operation"] == "symbol_string" {
                let meaning = u32::try_from(q["meaning"].as_u64().ok_or("expected meaning")?)?;
                if let Some(symbol) = op.get_symbol_at_location(name)? {
                    result["text_hex"] = json!(hex(op
                        .symbol_to_string_at(symbol, enclosing, meaning, flags)?
                        .as_bytes()));
                } else {
                    result["state"] = json!("absent");
                }
                queries.push(result);
                continue;
            }
            let typ = op.get_type_at_location(name)?;
            match text(&q["operation"])? {
                "type_string" => {
                    result["text_hex"] =
                        json!(hex(op.type_to_string_at(typ, enclosing, flags)?.as_bytes()));
                }
                "type_node" => {
                    let internal = i32::try_from(
                        q["internal_flags"]
                            .as_i64()
                            .ok_or("expected internal flags")?,
                    )?;
                    let mut builder = op.node_builder();
                    if let Some(node) =
                        builder.type_to_type_node(typ, enclosing, flags, internal)?
                    {
                        result["kind"] = json!(builder.view().node(node)?.kind().raw());
                        let mut writer = TextWriter::new(b"", 0);
                        Printer::new(
                            PrinterOptions {
                                remove_comments: true,
                                ..Default::default()
                            },
                            builder.emit_context(),
                        )
                        .write(
                            builder.view(),
                            node,
                            enclosing.map(|_| context_source),
                            &mut writer,
                        )?;
                        result["text_hex"] = json!(hex(writer.text()));
                    } else {
                        result["state"] = json!("absent");
                    }
                }
                _ => return Err("unknown operation".into()),
            }
            queries.push(result);
        }
        programs.push(json!({"id":r["id"],"queries":queries}));
    }
    Ok(json!({"version":1,"programs":programs}))
}

#[cfg(test)]
mod tests {
    #[test]
    fn cross_file_enclosing_contexts_match_native_display() {
        let request = serde_json::from_str(include_str!(
            "../../../data/s08/p5/display-cross-file/requests.json"
        ))
        .unwrap();
        let expected: serde_json::Value = serde_json::from_str(include_str!(
            "../../../data/s08/p5/display-cross-file/observations.json"
        ))
        .unwrap();
        let actual = super::observe(&request).unwrap();
        assert_eq!(actual["programs"], expected["programs"]);
    }
}
