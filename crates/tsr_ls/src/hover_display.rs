//! Quick-info display follows hover.go. All type/signature syntax comes from
//! the production checker and printer, including classified Visual Studio text.
use crate::{display_parts::DisplayParts, symbols::container_node, LanguageService, Result};
use std::collections::HashSet;
use tsr_ast::{
    check_flags as cf, node_flags, symbol_flags as sf, utilities as ast,
    utilities_middle as middle, utilities_positions as pos, NodeId, SyntaxKind as K,
};
use tsr_checker::{
    symbol_format_flags as sff, type_flags as tf, type_format_flags as tff, Operation,
    SignatureRef, SymbolRef, TypeRef, VerbosityContext,
};
use tsr_printer::{EmitTextWriter, Printer, PrinterOptions};

pub(crate) const SYMBOL_FLAGS: u32 = sff::WRITE_TYPE_PARAMETERS_OR_ARGUMENTS
    | sff::USE_ONLY_EXTERNAL_ALIASING
    | sff::ALLOW_ANY_NODE_KIND
    | sff::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE;
pub(crate) const TYPE_FLAGS: u32 =
    tff::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE | tff::USE_INSTANTIATION_EXPRESSIONS;
const CLASSIFIED: u32 = tsr_nodebuilder::flags::IGNORE_ERRORS
    | tsr_nodebuilder::flags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE
    | tsr_nodebuilder::flags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME;

// port: tsc/internal/ls/hover.go:getCallOrNewExpression
pub(crate) fn call_or_new(view: tsr_ast::AstView<'_>, mut node: NodeId) -> Result<Option<NodeId>> {
    let Some(parent) = view.node(node)?.parent() else {
        return Ok(None);
    };
    if view.node(parent)?.kind() == K::PropertyAccessExpression
        && view.node(parent)?.name() == Some(node)
    {
        node = parent;
    }
    let Some(parent) = view.node(node)?.parent() else {
        return Ok(None);
    };
    let read = view.node(parent)?;
    Ok((matches!(
        read.kind().known(),
        Some(K::CallExpression | K::NewExpression)
    ) && read.expression() == Some(node))
    .then_some(parent))
}
// port: tsc/internal/ls/hover.go:getSignaturesAtLocation
pub(crate) fn signatures_at(
    checker: &mut Operation<'_>,
    view: tsr_ast::AstView<'_>,
    symbol: SymbolRef,
    construct: bool,
    node: NodeId,
) -> Result<Vec<SignatureRef>> {
    let ty = checker.get_type_of_symbol(symbol)?;
    let ty = checker.remove_missing_or_undefined_type(ty)?;
    let sigs = checker.get_signatures_of_type(
        ty,
        if construct {
            tsr_checker::SignatureKind::Construct
        } else {
            tsr_checker::SignatureKind::Call
        },
    )?;
    if sigs.len() > 1 || sigs.len() == 1 && !checker.signature_type_parameters(sigs[0])?.is_empty()
    {
        if let Some(call) = call_or_new(view, node)? {
            return Ok(vec![checker.get_resolved_signature(call)?]);
        }
    }
    Ok(sigs)
}

struct QuickInfo<'a, 'op, 'p> {
    service: &'a LanguageService<'p>,
    c: &'a mut Operation<'op>,
    node: NodeId,
    enclosing: Option<NodeId>,
    source: NodeId,
    vc: &'a mut VerbosityContext,
    classified: bool,
    out: DisplayParts,
    declaration: Option<NodeId>,
    meaning: i32,
    aliases: HashSet<SymbolRef>,
    alias_level: usize,
    expanded: bool,
}
impl QuickInfo<'_, '_, '_> {
    fn set_decl(&mut self, decl: Option<NodeId>) {
        if self.declaration.is_none() {
            self.declaration = decl;
        }
    }
    fn new_line(&mut self) {
        if !self.out.text().is_empty() {
            self.out.write(b"\n");
        }
        if self.alias_level != 0 {
            self.parenthesized(b"alias");
        }
    }
    fn parenthesized(&mut self, text: &[u8]) {
        self.out.write_punctuation(b"(");
        self.out.write(text);
        self.out.write_punctuation(b") ");
    }
    fn symbol_name(&mut self, symbol: SymbolRef, enclosing: Option<NodeId>) -> Result<()> {
        let text = self
            .c
            .symbol_to_string_at(symbol, enclosing, sf::NONE, SYMBOL_FLAGS)?;
        if self.classified {
            self.out.write_symbol(text.as_bytes(), Some(symbol.id()));
        } else {
            self.out.write(text.as_bytes());
        }
        Ok(())
    }
    fn ty(&mut self, ty: TypeRef, enclosing: Option<NodeId>, flags: u32) -> Result<()> {
        let flags = flags | tff::MULTILINE_OBJECT_LITERALS;
        if self.classified {
            let mut builder = self.c.node_builder();
            let node = builder.type_to_type_node(
                ty,
                enclosing,
                flags & tff::NODE_BUILDER_FLAGS_MASK | CLASSIFIED,
                0,
            )?;
            if let Some(node) = node {
                let mut printer = Printer::new(PrinterOptions::default(), builder.emit_context());
                printer.id_to_symbol = Some(builder.identifier_symbols().collect());
                let mut out = DisplayParts::new(true);
                printer.write(builder.view(), node, Some(self.source), &mut out, None)?;
                self.out.append(out);
                return Ok(());
            }
        }
        self.out.write(
            self.c
                .type_to_string_ex(ty, enclosing, flags, Some(self.vc))?
                .as_bytes(),
        );
        Ok(())
    }
    fn signature(&mut self, sig: SignatureRef, flags: u32) -> Result<()> {
        let flags = flags | tff::MULTILINE_OBJECT_LITERALS;
        if self.classified {
            let construct = self.c.signature_flags(sig)? & tsr_checker::signature_flags::CONSTRUCT
                != 0
                && flags & tff::WRITE_CALL_STYLE_SIGNATURE == 0;
            let kind = match (flags & tff::WRITE_ARROW_STYLE_SIGNATURE != 0, construct) {
                (true, true) => K::ConstructorType,
                (true, false) => K::FunctionType,
                (false, true) => K::ConstructSignature,
                (false, false) => K::CallSignature,
            };
            let mut builder = self.c.node_builder();
            let node = builder.signature_to_signature_declaration(
                sig,
                kind,
                tsr_checker::BuilderRequest {
                    enclosing: self.enclosing,
                    flags: flags & tff::NODE_BUILDER_FLAGS_MASK | CLASSIFIED,
                    internal_flags: 0,
                },
            )?;
            if let Some(node) = node {
                let mut printer = Printer::new(PrinterOptions::default(), builder.emit_context());
                printer.id_to_symbol = Some(builder.identifier_symbols().collect());
                let mut out = DisplayParts::new(true);
                printer.write(builder.view(), node, Some(self.source), &mut out, None)?;
                self.out.append(out);
                return Ok(());
            }
        }
        self.out.write(
            self.c
                .signature_to_string_ex(sig, self.enclosing, flags, Some(self.vc))?
                .as_bytes(),
        );
        Ok(())
    }
    fn signatures(
        &mut self,
        sigs: &[SignatureRef],
        prefix: &[u8],
        parenthesized: bool,
        symbol: SymbolRef,
    ) -> Result<()> {
        for (index, sig) in sigs.iter().enumerate() {
            self.new_line();
            if index == 3 && sigs.len() >= 5 {
                self.out
                    .write_comment(format!("// +{} more overloads", sigs.len() - 3).as_bytes());
                break;
            }
            if parenthesized {
                self.parenthesized(prefix);
            } else {
                self.out.write_keyword(prefix);
            }
            self.symbol_name(symbol, self.enclosing)?;
            if self.c.symbol(symbol)?.flags() & sf::OPTIONAL != 0 {
                self.out.write_punctuation(b"?");
            }
            self.signature(
                *sig,
                TYPE_FLAGS
                    | tff::WRITE_CALL_STYLE_SIGNATURE
                    | tff::WRITE_TYPE_ARGUMENTS_OF_SIGNATURE,
            )?;
        }
        Ok(())
    }
    fn type_parameters(&mut self, parameters: &[TypeRef]) -> Result<()> {
        if parameters.is_empty() {
            return Ok(());
        }
        self.out.write_punctuation(b"<");
        for (index, tp) in parameters.iter().enumerate() {
            if index != 0 {
                self.out.write_punctuation(b", ");
            }
            if let Some(s) = self.c.type_symbol(*tp)? {
                let s = self.c.symbol_ref(s)?;
                self.symbol_name(s, None)?;
            }
            if let Some(constraint) = self.c.get_constraint_of_type_parameter(*tp)? {
                self.out.write_keyword(b" extends ");
                self.ty(constraint, None, TYPE_FLAGS)?;
            }
            if let Some(default) = self.c.get_default_from_type_parameter(*tp)? {
                self.out.write_operator(b" = ");
                self.ty(default, None, TYPE_FLAGS)?;
            }
        }
        self.out.write_punctuation(b">");
        Ok(())
    }
    fn expand(&mut self, symbol: SymbolRef, meaning: u32) -> Result<bool> {
        if self.expanded {
            return Ok(true);
        }
        let flags = self.c.symbol(symbol)?.flags();
        if flags & (sf::CLASS | sf::INTERFACE | sf::NAMESPACE) == 0 {
            return Ok(false);
        }
        let ty = if flags & (sf::CLASS | sf::INTERFACE) != 0 {
            self.c.get_declared_type_of_symbol(symbol)?
        } else {
            self.c
                .get_type_of_symbol_at_location(symbol, Some(self.node))?
        };
        if self.c.is_lib_type_for_hover_verbosity(ty)? {
            return Ok(false);
        }
        if self.vc.level <= 0 {
            self.vc.can_increase_verbosity = true;
            return Ok(false);
        }
        let mut vc = VerbosityContext {
            level: self.vc.level - 1,
            max_truncation_length: self.vc.max_truncation_length,
            ..VerbosityContext::default()
        };
        let text = self
            .c
            .expand_symbol_for_hover(symbol, meaning, Some(&mut vc))?;
        if text.is_empty() {
            return Ok(false);
        }
        self.vc.can_increase_verbosity |= vc.can_increase_verbosity;
        self.vc.truncated |= vc.truncated;
        self.out.write(text.as_bytes());
        self.expanded = true;
        Ok(true)
    }
    fn write_symbol(&mut self, symbol: SymbolRef) -> Result<()> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.write_symbol_inner(symbol)
        })
    }
    fn write_symbol_inner(&mut self, symbol: SymbolRef) -> Result<()> {
        self.service.check_canceled()?;
        if self.c.symbol(symbol)?.flags() & sf::ALIAS != 0 && self.aliases.insert(symbol) {
            let aliased = self.c.get_aliased_symbol(symbol)?;
            if !self.c.is_unknown_symbol(aliased)? {
                self.alias_level += 1;
                self.write_symbol(aliased)?;
                self.alias_level -= 1;
            }
        }
        let all_flags = self.c.symbol(symbol)?.flags();
        let mut flags = all_flags
            & match self.meaning {
                1 => sf::VALUE | sf::SIGNATURE,
                2 => sf::TYPE,
                4 => sf::NAMESPACE,
                _ => sf::VALUE | sf::SIGNATURE | sf::TYPE | sf::NAMESPACE,
            };
        if flags == 0 {
            if self.alias_level != 0 || !self.out.text().is_empty() {
                return Ok(());
            }
            flags = all_flags & (sf::VALUE | sf::SIGNATURE | sf::TYPE | sf::NAMESPACE);
            if flags == 0 {
                return Ok(());
            }
        }
        let value_decl = self.c.symbol(symbol)?.value_declaration();
        let declarations: Vec<_> = self
            .c
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect();
        if flags & sf::PROPERTY != 0
            && value_decl.is_some_and(|d| {
                self.c
                    .node(d)
                    .is_ok_and(|n| n.kind() == K::MethodDeclaration)
            })
        {
            flags = sf::METHOD;
        }
        let view = self.service.view(self.node)?;
        if flags & (sf::VARIABLE | sf::PROPERTY | sf::ACCESSOR) != 0 {
            self.new_line();
            if self.c.symbol(symbol)?.check_flags() & cf::INDEX_SYMBOL == 0 {
                if flags & sf::PROPERTY != 0 {
                    self.parenthesized(b"property");
                } else if flags & sf::ACCESSOR != 0 {
                    self.parenthesized(b"accessor");
                } else if let Some(decl) = value_decl {
                    let v = self.service.view(decl)?;
                    let decl = ast::get_root_declaration(v, decl)?;
                    if v.node(decl)?.kind() == K::Parameter {
                        self.parenthesized(b"parameter");
                    } else {
                        let combined =
                            ast::get_combined_node_flags(v, decl)? & node_flags::BLOCK_SCOPED;
                        if combined == node_flags::LET {
                            self.out.write_keyword(b"let ");
                        } else if combined == node_flags::CONST {
                            self.out.write_keyword(b"const ");
                        } else if combined == node_flags::USING {
                            self.out.write_keyword(b"using ");
                        } else if combined == node_flags::AWAIT_USING {
                            self.out.write_keyword(b"await ");
                            self.out.write_keyword(b"using ");
                        } else {
                            self.out.write_keyword(b"var ");
                        }
                    }
                }
                let parent = self.c.symbol(symbol)?.parent();
                if self.c.symbol(symbol)?.name_bytes() == b"export="
                    && parent.is_some_and(|p| {
                        self.c
                            .symbol_ref(p)
                            .and_then(|s| self.c.symbol(s))
                            .is_ok_and(|s| s.flags() & sf::MODULE != 0)
                    })
                {
                    self.out.write(b"exports");
                } else {
                    self.symbol_name(symbol, self.enclosing)?;
                }
                if all_flags & sf::OPTIONAL != 0 {
                    self.out.write_punctuation(b"?");
                }
                self.out.write_punctuation(b": ");
            }
            if let Some(call) = call_or_new(view, self.node)? {
                let mut flags = TYPE_FLAGS
                    | tff::WRITE_TYPE_ARGUMENTS_OF_SIGNATURE
                    | tff::WRITE_ARROW_STYLE_SIGNATURE;
                if view.node(call)?.kind() == K::CallExpression {
                    flags |= tff::WRITE_CALL_STYLE_SIGNATURE;
                }
                let sig = self.c.get_resolved_signature(call)?;
                self.signature(sig, flags)?;
            } else {
                let ty = self
                    .c
                    .get_type_of_symbol_at_location(symbol, Some(self.node))?;
                let constrained = if let Some(s) = self.c.type_symbol(ty)? {
                    self.c.symbol(self.c.symbol_ref(s)?)?.flags() & sf::TYPE_PARAMETER != 0
                        && self.c.get_constraint_of_type_parameter(ty)?.is_some()
                } else {
                    false
                };
                if constrained && self.vc.level > 0 {
                    let mut vc = VerbosityContext {
                        level: self.vc.level - 1,
                        max_truncation_length: self.vc.max_truncation_length,
                        ..VerbosityContext::default()
                    };
                    self.out.write(
                        self.c
                            .type_parameter_to_string_ex(ty, self.enclosing, Some(&mut vc))?
                            .as_bytes(),
                    );
                    self.vc.can_increase_verbosity |= vc.can_increase_verbosity;
                    self.vc.truncated |= vc.truncated;
                } else {
                    self.ty(ty, self.enclosing, TYPE_FLAGS)?;
                    if constrained {
                        self.vc.can_increase_verbosity = true;
                    }
                }
            }
            self.set_decl(value_decl.or(declarations.first().copied()));
        }
        if flags & sf::ENUM_MEMBER != 0 {
            self.new_line();
            self.parenthesized(b"enum member");
            let ty = self.c.get_type_of_symbol(symbol)?;
            self.ty(ty, self.enclosing, TYPE_FLAGS)?;
            if self.c.type_flags(ty)? & tf::LITERAL != 0 {
                self.out.write_operator(b" = ");
                self.out
                    .write_literal(self.c.literal_value_text(ty)?.as_bytes());
            }
            self.set_decl(value_decl);
        }
        let parent = view.node(self.node)?.parent();
        if flags & (sf::FUNCTION | sf::METHOD) != 0 {
            let method = flags & sf::METHOD != 0;
            let prefix = if method {
                b"method".as_slice()
            } else {
                b"function ".as_slice()
            };
            let sigs = if let Some(parent) = parent.filter(|p| declarations.contains(p)) {
                let read = view.node(parent)?;
                if view.node(self.node)?.kind() == K::Identifier
                    && (ast::is_function_like_declaration(Some(&read))
                        || read.kind() == K::MethodSignature)
                    && read.name() == Some(self.node)
                {
                    self.set_decl(Some(parent));
                    vec![self.c.get_signature_from_declaration(parent)?]
                } else {
                    signatures_at(self.c, view, symbol, false, self.node)?
                }
            } else {
                signatures_at(self.c, view, symbol, false, self.node)?
            };
            if sigs.len() == 1 {
                if let Some(d) = self.c.signature_declaration(sigs[0])? {
                    if self.c.node(d.id())?.flags() & node_flags::JS_DOC == 0 {
                        self.set_decl(Some(d.id()));
                    }
                }
            }
            self.signatures(&sigs, prefix, method, symbol)?;
            self.set_decl(value_decl);
        }
        if flags & (sf::CLASS | sf::INTERFACE) != 0 {
            if view.node(self.node)?.kind() == K::ThisKeyword
                || pos::is_this_in_type_query(view, self.node)?
            {
                self.new_line();
                self.out.write_keyword(b"this");
            } else if let Some(parent) = parent.filter(|_| {
                view.node(self.node)
                    .is_ok_and(|n| n.kind() == K::ConstructorKeyword)
            }) {
                if matches!(
                    view.node(parent)?.kind().known(),
                    Some(K::Constructor | K::ConstructSignature)
                ) {
                    self.set_decl(Some(parent));
                    let sig = self.c.get_signature_from_declaration(parent)?;
                    self.signatures(&[sig], b"constructor ", false, symbol)?;
                }
            } else {
                let sigs = if flags & sf::CLASS != 0 && call_or_new(view, self.node)?.is_some() {
                    signatures_at(self.c, view, symbol, true, self.node)?
                } else {
                    Vec::new()
                };
                if sigs.len() == 1 {
                    if let Some(d) = self.c.signature_declaration(sigs[0])? {
                        if self.c.node(d.id())?.flags() & node_flags::JS_DOC == 0 {
                            self.set_decl(Some(d.id()));
                        }
                    }
                    self.signatures(&sigs, b"constructor ", false, symbol)?;
                } else {
                    self.new_line();
                    let local = flags & sf::CLASS != 0
                        && declarations.iter().any(|d| {
                            self.c
                                .node(*d)
                                .is_ok_and(|n| n.kind() == K::ClassExpression)
                        });
                    if local {
                        self.parenthesized(b"local class");
                    }
                    if !self.expand(symbol, flags)? {
                        if !local {
                            if flags & sf::CLASS != 0 {
                                let mut abstract_class = false;
                                for &d in &declarations {
                                    let v = self.service.view(d)?;
                                    if v.node(d)?.kind() == K::ClassDeclaration
                                        && ast::get_combined_modifier_flags(v, d)?
                                            & tsr_ast::modifier_flags::ABSTRACT
                                            != 0
                                    {
                                        abstract_class = true;
                                    }
                                }
                                if abstract_class {
                                    self.out.write_keyword(b"abstract ");
                                }
                                self.out.write_keyword(b"class ");
                            } else {
                                self.out.write_keyword(b"interface ");
                            }
                        }
                        self.symbol_name(symbol, self.enclosing)?;
                        let params = self
                            .c
                            .get_local_type_parameters_of_class_or_interface_or_type_alias(
                                symbol,
                            )?;
                        self.type_parameters(&params)?;
                    }
                }
            }
            self.set_decl(if flags & sf::CLASS != 0 {
                value_decl
            } else {
                declarations.iter().copied().find(|d| {
                    self.c
                        .node(*d)
                        .is_ok_and(|n| n.kind() == K::InterfaceDeclaration)
                })
            });
        }
        if flags & sf::ENUM != 0 {
            self.new_line();
            if !self.expand(symbol, flags)? {
                let mut is_const = false;
                for &d in &declarations {
                    let v = self.service.view(d)?;
                    if v.node(d)?.kind() == K::EnumDeclaration
                        && ast::get_combined_modifier_flags(v, d)? & tsr_ast::modifier_flags::CONST
                            != 0
                    {
                        is_const = true;
                    }
                }
                if is_const {
                    self.out.write_keyword(b"const ");
                }
                self.out.write_keyword(b"enum ");
                self.symbol_name(symbol, self.enclosing)?;
            }
            self.set_decl(declarations.iter().copied().find(|d| {
                self.c
                    .node(*d)
                    .is_ok_and(|n| n.kind() == K::EnumDeclaration)
            }));
        }
        if flags & sf::MODULE != 0 {
            self.new_line();
            if !self.expand(symbol, flags)? {
                let module = if let Some(d) = value_decl {
                    let v = self.service.view(d)?;
                    v.node(d)?.kind() == K::SourceFile || tsr_ast::is_ambient_module(v, d)?
                } else {
                    false
                };
                self.out
                    .write_keyword(if module { b"module " } else { b"namespace " });
                self.symbol_name(symbol, self.enclosing)?;
                for &d in &declarations {
                    let v = self.service.view(d)?;
                    if let Some(attributes) = v
                        .node(d)?
                        .data_source()
                        .as_module_declaration()
                        .and_then(|d| d.attributes())
                    {
                        let mut emit = tsr_printer::EmitContext::default();
                        emit.set_emit_flags(attributes, tsr_printer::emit_flags::SINGLE_LINE);
                        let printer = Printer::new(PrinterOptions::default(), &emit);
                        let source = ast::get_source_file_of_node(v, Some(d))?;
                        let mut out = DisplayParts::new(self.classified);
                        printer.write(v, attributes, source, &mut out, None)?;
                        self.out.write_keyword(b" with ");
                        self.out.append(out);
                        break;
                    }
                }
            }
            self.set_decl(declarations.iter().copied().find(|d| {
                self.c
                    .node(*d)
                    .is_ok_and(|n| n.kind() == K::ModuleDeclaration)
            }));
        }
        if flags & sf::TYPE_PARAMETER != 0 {
            self.new_line();
            self.parenthesized(b"type parameter");
            let ty = self.c.get_declared_type_of_symbol(symbol)?;
            self.symbol_name(symbol, self.enclosing)?;
            if let Some(cons) = self.c.get_constraint_of_type_parameter(ty)? {
                self.out.write_keyword(b" extends ");
                self.ty(cons, self.enclosing, TYPE_FLAGS)?;
            }
            if let Some(parent) = self.c.symbol(symbol)?.parent() {
                self.out.write_keyword(b" in ");
                let parent = self.c.symbol_ref(parent)?;
                self.symbol_name(parent, self.enclosing)?;
                let params = self
                    .c
                    .get_local_type_parameters_of_class_or_interface_or_type_alias(parent)?;
                self.type_parameters(&params)?;
            } else if let Some(decl) = declarations
                .iter()
                .find(|d| self.c.node(**d).is_ok_and(|n| n.kind() == K::TypeParameter))
            {
                let v = self.service.view(*decl)?;
                if let Some(parent) = v.node(*decl)?.parent() {
                    let read = v.node(parent)?;
                    if ast::is_function_like(Some(&read)) {
                        self.out.write_keyword(b" in ");
                        if read.kind() == K::ConstructSignature {
                            self.out.write_keyword(b"new ");
                        } else if read.kind() != K::CallSignature && read.name().is_some() {
                            if let Some(sym) = self.service.bound_symbol(self.c, parent)? {
                                self.symbol_name(sym, self.enclosing)?;
                            }
                        }
                        let sig = self.c.get_signature_from_declaration(parent)?;
                        self.signature(sig, TYPE_FLAGS | tff::WRITE_TYPE_ARGUMENTS_OF_SIGNATURE)?;
                    } else if read.kind() == K::TypeAliasDeclaration {
                        self.out.write_keyword(b" in ");
                        self.out.write_keyword(b"type ");
                        if let Some(sym) = self.service.bound_symbol(self.c, parent)? {
                            self.symbol_name(sym, self.enclosing)?;
                            let params = self.c.get_type_alias_type_parameters(sym)?;
                            self.type_parameters(&params)?;
                        }
                    }
                }
            }
            self.set_decl(
                declarations
                    .iter()
                    .copied()
                    .find(|d| self.c.node(*d).is_ok_and(|n| n.kind() == K::TypeParameter)),
            );
        }
        if flags & sf::TYPE_ALIAS != 0 {
            self.new_line();
            self.out.write_keyword(b"type ");
            self.symbol_name(symbol, self.enclosing)?;
            let params = self.c.get_type_alias_type_parameters(symbol)?;
            self.type_parameters(&params)?;
            self.out.write_operator(b" = ");
            let ty = if let Some(parent) = parent.filter(|p| {
                view.node(*p)
                    .is_ok_and(|n| middle::is_const_type_reference(view, &n).unwrap_or(false))
            }) {
                self.c.get_type_at_location(parent)?
            } else {
                self.c.get_declared_type_of_symbol(symbol)?
            };
            self.ty(ty, self.enclosing, TYPE_FLAGS | tff::IN_TYPE_ALIAS)?;
            self.set_decl(declarations.iter().copied().find(|d| {
                self.c.node(*d).is_ok_and(|n| {
                    matches!(
                        n.kind().known(),
                        Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
                    )
                })
            }));
        }
        if flags & sf::SIGNATURE != 0 {
            self.new_line();
            let ty = self.c.get_type_of_symbol(symbol)?;
            self.ty(ty, self.enclosing, TYPE_FLAGS)?;
        }
        Ok(())
    }
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/hover.go:getQuickInfoAndDeclarationAtLocation
    pub(crate) fn quick_info(
        &self,
        checker: &mut Operation<'_>,
        symbol: Option<SymbolRef>,
        node: NodeId,
        vc: &mut VerbosityContext,
        classified: bool,
        meaning: i32,
    ) -> Result<(DisplayParts, Option<NodeId>)> {
        let view = self.view(node)?;
        let mut info = QuickInfo {
            service: self,
            c: checker,
            node,
            enclosing: container_node(view, node)?,
            source: ast::get_source_file_of_node(view, Some(node))?
                .ok_or(tsr_arena::Error::InvalidGraph)?,
            vc,
            classified,
            out: DisplayParts::new(classified),
            declaration: None,
            meaning,
            aliases: HashSet::new(),
            alias_level: 0,
            expanded: false,
        };
        if view.node(node)?.kind() == K::ThisKeyword && pos::is_in_expression_context(view, node)?
            || pos::is_this_in_type_query(view, node)?
        {
            info.out.write_keyword(b"this");
            info.out.write_punctuation(b": ");
            let ty = info.c.get_type_at_location(node)?;
            info.ty(ty, info.enclosing, TYPE_FLAGS)?;
        } else if let Some(symbol) = symbol {
            info.write_symbol(symbol)?;
        } else if should_get_type(view, node)? {
            let ty = info.c.get_type_at_location(node)?;
            info.ty(ty, info.enclosing, TYPE_FLAGS)?;
        }
        Ok((info.out, info.declaration))
    }
}
// port: tsc/internal/ls/hover.go:shouldGetType
fn should_get_type(view: tsr_ast::AstView<'_>, node: NodeId) -> Result<bool> {
    let read = view.node(node)?;
    Ok(match read.kind().known() {
        Some(K::Identifier) => {
            !(read.flags() & node_flags::JS_DOC != 0 && pos::is_declaration_name(view, node)?
                || middle::is_label_name(view, node)?
                || tsr_ast::utilities_tail::is_tag_name(view, node)?
                || read
                    .parent()
                    .map(|p| middle::is_const_type_reference(view, &view.node(p)?))
                    .transpose()?
                    .unwrap_or(false))
        }
        Some(K::ThisKeyword | K::ThisType | K::SuperKeyword | K::NamedTupleMember) => true,
        Some(K::MetaProperty) => crate::hover::is_import_meta(view, node)?,
        _ => false,
    })
}
