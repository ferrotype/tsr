//! The text of identifiers and literals (`getTextOfNode`,
//! `getLiteralTextOfNode` and `isFileLevelUniqueNameInCurrentFile` in
//! `tsc/internal/printer/printer.go`).
//!
//! Upstream wires these into the printer's `NameGenerator` as callbacks, and
//! `getTextOfNode` calls back into the generator for a generated name. Here
//! they read through a [`NodeText`], which the printer builds from its own
//! state and the generator's callbacks build from an owned copy
//! ([`NodeTextOwner`]); the generator and its host are passed in, so a
//! generated name inside a text source reaches the same generator.

use crate::literal_text::{get_literal_text, with_flag, LiteralTextFlags};
use crate::{emit_flags as ef, EmitContext, Error, NameGenerator, NameGeneratorHost};
use std::cell::Cell;
use std::rc::Rc;
use tsr_ast::{NodeId, SyntaxKind as K};
use tsr_core::ScriptTarget;
use tsr_jsstring::escape::{escape_jsx_attribute_string, escape_non_ascii_string, escape_string};
use tsr_jsstring::{JsString, LiteralEscapeFlags, QuoteChar};

/// What `getTextOfNode` reads from the printer: its emit context, its target
/// and the current source file.
#[derive(Clone, Copy)]
pub(crate) struct NodeText<'e> {
    pub(crate) emit_context: &'e EmitContext,
    pub(crate) target: ScriptTarget,
    pub(crate) current_source: Option<NodeId>,
}

/// The owned state the generator's callbacks read. The current source file is
/// shared with the write in progress, which updates it as upstream updates
/// `currentSourceFile`.
pub(crate) struct NodeTextOwner {
    pub(crate) emit_context: EmitContext,
    pub(crate) target: ScriptTarget,
    pub(crate) current_source: Rc<Cell<Option<NodeId>>>,
}

impl NodeTextOwner {
    pub(crate) fn reader(&self) -> NodeText<'_> {
        NodeText {
            emit_context: &self.emit_context,
            target: self.target,
            current_source: self.current_source.get(),
        }
    }

    /// Installs `GetTextOfNode` and `IsFileLevelUniqueNameInCurrentFile` on
    /// the generator, as `NewPrinter` wires them.
    pub(crate) fn install(self, generator: &mut NameGenerator<'static>) {
        let owner = Rc::new(self);
        let text_owner = Rc::clone(&owner);
        generator.get_text_of_node = Some(Rc::new(move |generator, host, node| {
            let text = text_owner
                .reader()
                .get_text_of_node(generator, host, node, false)?;
            Ok(JsString::from_bytes(text))
        }));
        generator.is_file_level_unique_name_in_current_file =
            Some(Rc::new(move |host, name, private_name| {
                owner
                    .reader()
                    .is_file_level_unique_name_in_current_file(host, name, private_name)
            }));
    }
}

impl NodeText<'_> {
    /// Upstream passes the printer's `HasGlobalName` handler, which nothing
    /// sets at the pin; it is nil here.
    // port: tsc/internal/printer/printer.go:Printer.isFileLevelUniqueNameInCurrentFile
    pub(crate) fn is_file_level_unique_name_in_current_file(
        self,
        host: &dyn NameGeneratorHost,
        name: &[u8],
        _private_name: bool,
    ) -> Result<bool, Error> {
        if let Some(current_source) = self.current_source {
            self.emit_context
                .is_file_level_unique_name(host.view(), current_source, name, None)
        } else {
            Ok(true)
        }
    }

    /// `node` must be one of Identifier | PrivateIdentifier | LiteralExpression
    /// | JsxNamespacedName.
    // port: tsc/internal/printer/printer.go:Printer.getTextOfNode
    pub(crate) fn get_text_of_node(
        self,
        generator: &mut NameGenerator<'_>,
        host: &dyn NameGeneratorHost,
        node: NodeId,
        include_trivia: bool,
    ) -> Result<Vec<u8>, Error> {
        let view = host.view();
        let read = view.node(node)?;
        if tsr_ast::utilities::is_member_name(&read)
            && self.emit_context.has_auto_generate_info(node)
        {
            return Ok(generator.generate_name(host, node)?.as_bytes().to_vec());
        }

        if read.kind() == K::StringLiteral {
            if let Some(text_source_node) = self.emit_context.text_source(node) {
                return self.get_text_of_node(generator, host, text_source_node, include_trivia);
            }
        }

        let current_source = self.current_source;
        let can_use_source_file = current_source.is_some()
            && read.parent().is_some()
            && !tsr_ast::utilities::node_is_synthesized(&read);

        match read.kind().known() {
            Some(K::Identifier | K::PrivateIdentifier | K::JsxNamespacedName) => {
                let in_current_source = match current_source {
                    Some(current_source) if can_use_source_file => {
                        tsr_ast::utilities::get_source_file_of_node(view, Some(node))?
                            == Some(self.emit_context.most_original(current_source))
                    }
                    _ => false,
                };
                if !in_current_source {
                    return Ok(view.node_text(node)?.to_vec());
                }
            }
            Some(
                K::StringLiteral
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::TemplateHead
                | K::TemplateMiddle
                | K::TemplateTail,
            ) => {
                return self.get_literal_text_of_node(
                    generator,
                    host,
                    node,
                    None, /*sourceFile*/
                    LiteralEscapeFlags::NONE,
                );
            }
            _ => {
                return Err(Error::UnexpectedKind {
                    context: "unexpected node",
                    kind: read.kind(),
                })
            }
        }
        let current_source = current_source.expect("checked above");
        let source_file = view.source_file(current_source)?;
        Ok(tsr_scanner::get_text_of_node_from_source_text(
            view,
            source_file.text().as_bytes(),
            Some(node),
            include_trivia,
        )?
        .as_bytes()
        .to_vec())
    }

    /// A string literal with a text source prints the source's text, quoted
    /// and escaped; a literal whose source text is reachable prints it as
    /// written.
    // port: tsc/internal/printer/printer.go:Printer.getLiteralTextOfNode
    pub(crate) fn get_literal_text_of_node(
        self,
        generator: &mut NameGenerator<'_>,
        host: &dyn NameGeneratorHost,
        node: NodeId,
        source_file: Option<NodeId>,
        mut flags: LiteralTextFlags,
    ) -> Result<Vec<u8>, Error> {
        let view = host.view();
        if view.node(node)?.kind() == K::StringLiteral {
            if let Some(text_source_node) = self.emit_context.text_source(node) {
                let text = match view.node(text_source_node)?.kind().known() {
                    Some(K::NumericLiteral) => view.node_text(text_source_node)?.to_vec(),
                    Some(K::Identifier | K::PrivateIdentifier | K::JsxNamespacedName) => {
                        self.get_text_of_node(generator, host, text_source_node, false)?
                    }
                    _ => {
                        let source_file = tsr_ast::utilities::get_source_file_of_node(
                            view,
                            Some(text_source_node),
                        )?;
                        return self.get_literal_text_of_node(
                            generator,
                            host,
                            text_source_node,
                            source_file,
                            flags,
                        );
                    }
                };

                let escaped = if flags.contains(LiteralEscapeFlags::JSX_ATTRIBUTE_ESCAPE) {
                    escape_jsx_attribute_string(&text, QuoteChar::Double)
                } else if flags.contains(LiteralEscapeFlags::NEVER_ASCII_ESCAPE)
                    || self.emit_context.emit_flags(node) & ef::NO_ASCII_ESCAPING != 0
                {
                    escape_string(&text, QuoteChar::Double)
                } else {
                    escape_non_ascii_string(&text, QuoteChar::Double)
                };
                let mut quoted = Vec::with_capacity(escaped.len() + 2);
                quoted.push(b'"');
                quoted.extend_from_slice(&escaped);
                quoted.push(b'"');
                return Ok(quoted);
            }
        }
        // !!! Printer option to control whether to terminate unterminated literals
        if self.emit_context.emit_flags(node) & ef::NO_ASCII_ESCAPING != 0 {
            flags = with_flag(flags, LiteralEscapeFlags::NEVER_ASCII_ESCAPE);
        }
        if self.target >= ScriptTarget::ES2021 {
            flags = with_flag(flags, LiteralEscapeFlags::ALLOW_NUMERIC_SEPARATOR);
        }
        let source_file = match source_file.or(self.current_source) {
            Some(source_file) => Some(view.source_file(source_file)?),
            None => None,
        };
        get_literal_text(
            view,
            node,
            source_file.as_ref().map(|file| file.text().as_bytes()),
            flags,
        )
    }
}
