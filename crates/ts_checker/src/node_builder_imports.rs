//! Import-type mode selection and portability diagnostics. Path ranking stays
//! in the shared module-specifier generator over the retained program host.
use crate::{node_builder::NodeBuilder, Error};
use ts_ast::{FactoryMethods, JsString, NodeId, SymbolId, SyntaxKind as K};
use ts_core::{ModuleKind, ModuleResolutionKind, ResolutionMode as Mode};
use ts_nodebuilder::flags as nf;
use ts_printer::emit_resolver::DeclarationTrackerEvent as Event;

fn through_node_modules(specifier: &JsString) -> bool {
    specifier
        .as_bytes()
        .windows(b"/node_modules/".len())
        .any(|part| part == b"/node_modules/")
}

impl NodeBuilder<'_> {
    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.symbolToTypeNode
    pub(super) fn import_type_specifier(
        &mut self,
        module: SymbolId,
        symbol: SymbolId,
    ) -> Result<(JsString, Option<NodeId>), Error> {
        let enclosing = self.enclosing.map(|node| self.emit.most_original(node));
        let context = enclosing
            .map(|node| self.checker.module_source(node))
            .transpose()?;
        let host = self.checker.program()?.host.clone();
        let node_resolution = matches!(
            host.options().module_resolution_kind(),
            ModuleResolutionKind::NODE16 | ModuleResolutionKind::NODE_NEXT
        );
        let mut mode = Mode::NONE;
        let mut specifier = None;
        if node_resolution {
            if let (Some((_, context)), Some(target)) = (&context, self.module_source_file(module)?)
            {
                let target = self
                    .checker
                    .ast(target)?
                    .source_file(target)?
                    .file_name()
                    .to_vec();
                let target_mode = host.get_emit_module_format_of_file(&target)?;
                if target_mode == ModuleKind::ESNEXT
                    && target_mode != host.get_emit_module_format_of_file(context.as_bytes())?
                {
                    specifier = Some(self.module_specifier_with_context_and_mode(
                        module,
                        enclosing,
                        Mode::ESNEXT,
                    )?);
                    mode = Mode::ESNEXT;
                }
            }
        }
        let mut specifier = match specifier.filter(|name| !name.is_empty()) {
            Some(name) => name,
            None => self.module_specifier_with_context(module, enclosing)?,
        };
        if self.flags & nf::ALLOW_NODE_MODULES_RELATIVE_PATHS == 0
            && through_node_modules(&specifier)
        {
            let original = specifier.clone();
            if node_resolution {
                if let Some((_, context)) = context {
                    let swapped = if host.get_emit_module_format_of_file(context.as_bytes())?
                        == ModuleKind::ESNEXT
                    {
                        Mode::COMMON_JS
                    } else {
                        Mode::ESNEXT
                    };
                    let candidate =
                        self.module_specifier_with_context_and_mode(module, enclosing, swapped)?;
                    if !through_node_modules(&candidate) {
                        specifier = candidate;
                        mode = swapped;
                    }
                }
            }
            if mode == Mode::NONE {
                self.encountered_error = true;
                self.report(Event::LikelyUnsafeImportRequired {
                    specifier: original,
                    symbol_name: self.checker.symbol(symbol)?.name_to_owned(),
                });
            }
        }
        let attribute_type = if !self.name_has_declaration_kind(module, K::SourceFile)?
            && self.name_has_declaration_kind(module, K::ModuleDeclaration)?
        {
            Some(self.checker.module_import_attributes_type(module)?)
        } else {
            None
        };
        let attributes = self.import_type_attributes(attribute_type, mode)?;
        Ok((specifier, attributes))
    }

    // port: tsc/internal/checker/nodebuilderimpl.go:NodeBuilderImpl.createImportAttributesForModuleSpecifier
    fn import_type_attributes(
        &mut self,
        ty: Option<crate::TypeId>,
        mode: Mode,
    ) -> Result<Option<NodeId>, Error> {
        let mut properties = Vec::new();
        if let Some(ty) = ty {
            for property in self.checker.get_properties_of_type(ty)? {
                properties.push((self.checker.symbol(property)?.name_to_owned(), property));
            }
        }
        properties.sort_by(|(a, _), (b, _)| a.cmp(b));
        let mut attributes = Vec::new();
        if mode != Mode::NONE {
            let value = if mode == Mode::ESNEXT {
                b"import".as_slice()
            } else {
                b"require".as_slice()
            };
            let name = self.string_literal(JsString::from_bytes(b"resolution-mode".as_slice()));
            let text = self.string_literal(JsString::from_bytes(value));
            attributes.push(self.ast.new_import_attribute(Some(name), Some(text)));
            self.approximate_length += b"resolution-mode".len() + value.len() + 6;
        }
        for (name, property) in properties {
            let ty = self.checker.get_type_of_symbol(property)?;
            if self.checker.types.flags(ty)? & crate::type_flags::STRING_LITERAL == 0 {
                continue;
            }
            let crate::LiteralValue::String(value) = &self.checker.types.literal(ty)?.value else {
                return Err(Error::MissingLink("string import attribute literal"));
            };
            let value = value.clone();
            self.approximate_length += name.len() + value.len() + 4;
            let name = if ts_scanner::is_identifier_text(
                name.as_bytes(),
                ts_core::LanguageVariant::STANDARD,
            ) {
                self.ast.new_identifier(name)
            } else {
                self.approximate_length += 2;
                self.string_literal(name)
            };
            let value = self.string_literal(value);
            attributes.push(self.ast.new_import_attribute(Some(name), Some(value)));
        }
        if attributes.is_empty() {
            return Ok(None);
        }
        self.approximate_length += 16 + 2 * (attributes.len() - 1);
        let attributes = self.list(attributes)?;
        Ok(Some(self.ast.new_import_attributes(
            K::WithKeyword.into(),
            Some(attributes),
            false,
        )))
    }
}
