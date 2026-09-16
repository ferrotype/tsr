use super::{ast, classify, Result, Row, Walker};
use serde_json::{json, Value};
use ts_arena::NodeId;
use ts_ast::SyntaxKind as K;
use ts_checker::{type_flags as tf, type_format_flags as ff, TypeRef};
use ts_printer::{EmitTextWriter, Printer, PrinterOptions, TextWriter};

const TYPE_FLAGS: u32 = ff::NO_TRUNCATION
    | ff::ALLOW_UNIQUE_ES_SYMBOL_TYPE
    | ff::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS;
// Keep the separate native builder flag domains visible in recorded requests.
const IGNORE_ERRORS: u32 = ts_nodebuilder::flags::IGNORE_ERRORS;
const ALLOW_UNRESOLVED_NAMES: i32 = ts_nodebuilder::internal_flags::ALLOW_UNRESOLVED_NAMES;

impl Walker<'_, '_> {
    pub(super) fn stamp(&self, source: NodeId, id: NodeId, operation: &str) -> Result<Value> {
        let view = ast(self.program, id)?;
        let node = view.node(id)?;
        let file = ast(self.program, source)?
            .source_file(source)?
            .parse_options()
            .file_name
            .clone();
        Ok(
            json!({"operation":operation,"file":std::str::from_utf8(file.as_bytes())?,"kind":node.kind().raw(),"pos":node.pos(),"end":node.end()}),
        )
    }
    fn typ(&mut self, source: NodeId, id: NodeId) -> Result<TypeRef> {
        let mut q = self.stamp(source, id, "GetTypeAtLocation")?;
        self.trace.active = q.clone();
        let typ = self.op.get_type_at_location(id)?;
        q["result_flags"] = json!(self.op.type_flags(typ)?);
        q["type_id"] = json!(typ.id());
        self.trace.queries.push(q);
        if self.trace.collect_type_strings {
            self.type_strings
                .push((self.stamp(source, id, "TypeToString")?, typ));
        }
        Ok(typ)
    }
    pub(super) fn write(
        &mut self,
        source: NodeId,
        id: NodeId,
        symbols: bool,
    ) -> Result<Option<Row>> {
        let program = self.program;
        let view = ast(program, id)?;
        let node = view.node(id)?;
        let parent = node.parent().ok_or("baseline node has no parent")?;
        let parent_node = view.node(parent)?;
        let source_view = ast(program, source)?;
        let source_file = source_view.source_file(source)?;
        let text = source_file.text().as_bytes();
        let actual_pos = ts_scanner::skip_trivia(text, i64::from(node.pos()));
        let line = usize::try_from(ts_jsstring::scanner_positions::compute_line_of_position(
            source_file.ecma_line_map(),
            isize::try_from(actual_pos)?,
        ))?;
        let source_text =
            ts_scanner::get_source_text_of_node_from_source_file(view, source, Some(id), false)?
                .as_bytes()
                .to_vec();
        if symbols {
            return self.symbol(source, id, parent, line, source_text);
        }
        if self.op.is_part_of_type_node(id)?
            || matches!(
                node.kind().known(),
                Some(K::AsExpression | K::SatisfiesExpression)
            ) && view
                .node(node.type_node().ok_or("assertion type")?)?
                .flags()
                & ts_ast::node_flags::REPARSED
                != 0
            || node.kind() == K::Identifier
                && !classify::declaration_has_value(view, parent)?
                && !(matches!(
                    parent_node.kind().known(),
                    Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
                ) && parent_node.name() == Some(id))
            || node.kind() == K::OmittedExpression
        {
            return Ok(None);
        }
        let mut typ = if classify::class_extends(view, parent)? {
            Some(self.typ(source, parent)?)
        } else {
            None
        };
        if typ
            .map(|t| self.op.type_flags(t).map(|f| f & tf::ANY != 0))
            .transpose()?
            .unwrap_or(true)
        {
            typ = Some(self.typ(source, id)?);
        }
        let typ = typ.ok_or("type query result")?;
        let plain_any = !self.had_errors
            && self.op.type_flags(typ)? & tf::ANY != 0
            && !matches!(
                parent_node.kind().known(),
                Some(
                    K::BindingElement
                        | K::PropertyAccessExpression
                        | K::QualifiedName
                        | K::MetaProperty
                )
            )
            && !ts_ast::utilities_middle::is_label_name(view, id)?
            && !ts_ast::utilities::is_global_scope_augmentation(&parent_node)
            && !classify::import_or_export_name(view, id, parent)?
            && !classify::intrinsic_jsx(view, id, parent, &source_text)?;
        let display = if plain_any {
            self.op.intrinsic_type_name(typ)?.as_bytes().to_vec()
        } else {
            let flags = (TYPE_FLAGS & ff::NODE_BUILDER_FLAGS_MASK) | IGNORE_ERRORS;
            let mut q = self.stamp(source, parent, "TypeToTypeNode")?;
            q["type_id"] = json!(typ.id());
            q["flags"] = json!(flags);
            q["internal_flags"] = json!(ALLOW_UNRESOLVED_NAMES);
            self.trace.active = q.clone();
            self.trace.queries.push(q.clone());
            let mut builder = self.op.node_builder();
            let mut generated =
                builder.type_to_type_node(typ, Some(parent), flags, ALLOW_UNRESOLVED_NAMES)?;
            let retry = if let Some(generated) = generated {
                node.kind() == K::Identifier
                    && parent_node.kind() == K::TypeAliasDeclaration
                    && parent_node.name() == Some(id)
                    && builder.view().node(generated)?.kind() == K::Identifier
                    && builder.view().node_text(generated)?.as_bytes()
                        == view.node_text(id)?.as_bytes()
            } else {
                false
            };
            if retry {
                let flags = flags | (ff::IN_TYPE_ALIAS & ff::NODE_BUILDER_FLAGS_MASK);
                q["flags"] = json!(flags);
                self.trace.active = q.clone();
                self.trace.queries.push(q);
                generated =
                    builder.type_to_type_node(typ, Some(parent), flags, ALLOW_UNRESOLVED_NAMES)?;
            }
            if let Some(generated) = generated {
                let mut writer = TextWriter::new(b"", 0);
                Printer::new(
                    PrinterOptions {
                        remove_comments: true,
                        ..Default::default()
                    },
                    builder.emit_context(),
                )
                .write(builder.view(), generated, Some(source), &mut writer)?;
                writer.text().to_vec()
            } else {
                vec![]
            }
        };
        Ok(Some(Row {
            line,
            source_text,
            display,
        }))
    }

    fn symbol(
        &mut self,
        source: NodeId,
        id: NodeId,
        parent: NodeId,
        line: usize,
        source_text: Vec<u8>,
    ) -> Result<Option<Row>> {
        let mut q = self.stamp(source, id, "GetSymbolAtLocation")?;
        self.trace.active = q.clone();
        let symbol = self.op.get_symbol_at_location(id)?;
        if symbol.is_none() {
            q["absent"] = json!(true);
        }
        self.trace.queries.push(q);
        let Some(symbol) = symbol else {
            return Ok(None);
        };
        let mut q = self.stamp(source, parent, "SymbolToStringEx")?;
        q["flags"] = json!(ts_checker::symbol_format_flags::ALLOW_ANY_NODE_KIND);
        self.trace.active = q.clone();
        self.trace.queries.push(q);
        let text = self.op.symbol_to_string_at(
            symbol,
            Some(parent),
            ts_ast::symbol_flags::NONE,
            ts_checker::symbol_format_flags::ALLOW_ANY_NODE_KIND,
        )?;
        let mut display = b"Symbol(".to_vec();
        display.extend_from_slice(&ts_ast::escape_all_internal_symbol_names(text.as_bytes()));
        let declarations = self.op.symbol_declarations(symbol)?;
        for (index, declaration) in declarations.iter().enumerate() {
            if index >= 5 {
                display.extend_from_slice(
                    format!(" ... and {} more", declarations.len() - index).as_bytes(),
                );
                break;
            }
            let declaration = declaration.ok_or("nil symbol declaration")?;
            let view = ast(self.program, declaration)?;
            let source = ts_ast::utilities::get_source_file_of_node(view, Some(declaration))?
                .ok_or("declaration source")?;
            let file = view.source_file(source)?;
            let pos = view.node(declaration)?.pos();
            let starts = file.ecma_line_map();
            let decl_line = usize::try_from(
                ts_jsstring::scanner_positions::compute_line_of_position(starts, pos as isize),
            )?;
            let start = usize::try_from(starts[decl_line])?;
            let character = ts_jsstring::line_map::utf16_len(
                &file.text().as_bytes()[start..usize::try_from(pos)?],
            );
            let name = file.parse_options().file_name.as_bytes();
            let base = ts_tspath::base_name(name);
            display.extend_from_slice(b", Decl(");
            display.extend_from_slice(base);
            display.extend_from_slice(b", ");
            if base.starts_with(b"lib.") && base.ends_with(b".d.ts") {
                display.extend_from_slice(b"--, --)");
            } else {
                display.extend_from_slice(format!("{decl_line}, {character})").as_bytes());
            }
        }
        display.push(b')');
        Ok(Some(Row {
            line,
            source_text,
            display,
        }))
    }
}
