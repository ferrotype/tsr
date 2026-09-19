//! JSDoc signature checks consume the parser's eager documentation and reparsed
//! annotation edges. They never trigger lazy documentation parsing.
use crate::{types::Set, CheckerState, Error};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{node_flags as nf, symbol_flags as sf, JsString, SyntaxKind as K};
use ts_diagnostics as d;

impl CheckerState {
    /// Parsed documentation is keyed by its logical source as well as its host.
    /// Constructed trees can instead provide the unscoped eager cache.
    pub(crate) fn eager_jsdoc_for_node(
        &self,
        node: NodeId,
    ) -> Result<Option<ts_ast::JSDocRoots>, Error> {
        let view = self.ast(node)?;
        if let Some(source) = ts_ast::utilities::get_source_file_of_node(view, Some(node))? {
            if let Some(roots) = view.source_eager_jsdoc(source, node)? {
                return Ok(Some(roots));
            }
        }
        Ok(view.eager_jsdoc(node)?)
    }

    // port: tsc/internal/checker/jsdoc.go:getAllJSDocTags
    pub(crate) fn all_signature_jsdoc_tags(&self, node: NodeId) -> Result<Vec<NodeId>, Error> {
        if self.node(node)?.flags() & nf::JS_DOC != 0 {
            return Ok(Vec::new());
        }
        let mut current = Some(node);
        while let Some(node) = current {
            if let Some(docs) = self.eager_jsdoc_for_node(node)? {
                if let Some(&last) = docs.last() {
                    let tags = self
                        .ast(last)?
                        .node(last)?
                        .data_source()
                        .as_js_doc()
                        .ok_or(Error::MissingLink("signature documentation"))?
                        .tags();
                    if tags.is_some() {
                        return self.source_list(last, tags);
                    }
                }
            }
            current =
                ts_ast::utilities_tail::get_next_js_doc_comment_location(self.ast(node)?, node)?;
        }
        Ok(Vec::new())
    }

    // port: tsc/internal/checker/jsdoc.go:Checker.checkUnmatchedJSDocParameters
    pub(crate) fn check_unmatched_jsdoc_parameters(&mut self, node: NodeId) -> Result<(), Error> {
        let mut documented = Vec::new();
        for tag in self.all_signature_jsdoc_tags(node)? {
            let read = self.node(tag)?;
            if read.kind() != K::JSDocParameterTag {
                continue;
            }
            let name = read
                .name()
                .ok_or(Error::MissingLink("documentation parameter name"))?;
            if self.node(name)?.kind() == K::Identifier
                && self.node_text(name)?.as_bytes().is_empty()
            {
                continue;
            }
            documented.push(tag);
        }
        if documented.is_empty() {
            return Ok(());
        }
        let js = self.node(node)?.flags() & nf::JAVA_SCRIPT_FILE != 0;
        let mut names = Set::default();
        let mut excluded = Set::default();
        for (index, parameter) in self
            .source_list(node, self.node(node)?.parameter_list())?
            .into_iter()
            .enumerate()
        {
            let name = self
                .ast(parameter)?
                .node(parameter)?
                .name()
                .ok_or(Error::MissingLink("signature parameter name"))?;
            if self.node(name)?.kind() == K::Identifier {
                names.insert(self.node_text(name)?.into_js_string());
            }
            if matches!(
                self.node(name)?.kind().known(),
                Some(K::ObjectBindingPattern | K::ArrayBindingPattern)
            ) {
                excluded.insert(index);
            }
        }
        if self.contains_arguments_reference(node)? {
            if !js {
                return Ok(());
            }
            let index = documented.len() - 1;
            let last = documented[index];
            let read = self.node(last)?;
            let data = read
                .data_source()
                .as_js_doc_parameter_or_property_tag()
                .ok_or(Error::MissingLink("last documentation parameter"))?;
            let name = read
                .name()
                .ok_or(Error::MissingLink("last documentation parameter name"))?;
            if self.node(name)?.kind() != K::Identifier {
                return Ok(());
            }
            let text = self.node_text(name)?.into_js_string();
            if excluded.contains(&index) || names.contains(&text) {
                return Ok(());
            }
            let Some(expression) = data.type_expression() else {
                return Ok(());
            };
            let Some(annotation) = self.node(expression)?.type_node() else {
                return Ok(());
            };
            let ty = self.get_type_from_type_node(annotation)?;
            if !self.is_array_type(ty)? {
                self.error_at(Some(name),d::JSDoc_param_tag_has_name_0_but_there_is_no_parameter_with_that_name_It_would_match_arguments_if_it_had_an_array_type,vec![text])?;
            }
            return Ok(());
        }
        for (index, tag) in documented.into_iter().enumerate() {
            let read = self.node(tag)?;
            let data = read
                .data_source()
                .as_js_doc_parameter_or_property_tag()
                .ok_or(Error::MissingLink("documentation parameter"))?;
            let name = read
                .name()
                .ok_or(Error::MissingLink("documentation parameter name"))?;
            let name_first = data.is_name_first();
            let kind = self.node(name)?.kind();
            if excluded.contains(&index)
                || kind == K::Identifier && names.contains(&self.node_text(name)?.into_js_string())
            {
                continue;
            }
            if kind == K::QualifiedName {
                if js {
                    let left = self
                        .ast(name)?
                        .node(name)?
                        .data_source()
                        .as_qualified_name()
                        .ok_or(Error::MissingLink("documentation qualified name"))?
                        .left()
                        .ok_or(Error::MissingLink("documentation name left"))?;
                    let text = self.entity_name_text(name)?;
                    let base = self.entity_name_text(left)?;
                    self.error_at(
                        Some(name),
                        d::Qualified_name_0_is_not_allowed_without_a_leading_param_object_1,
                        vec![text, base],
                    )?;
                }
            } else if !name_first {
                let text = self.node_text(name)?.into_js_string();
                let diagnostic = self.diagnostic_for_node(
                    Some(name),
                    d::JSDoc_param_tag_has_name_0_but_there_is_no_parameter_with_that_name,
                    vec![text],
                )?;
                if js {
                    self.add_diagnostic(diagnostic)?;
                } else {
                    self.add_suggestion_diagnostic(diagnostic)?;
                }
            }
        }
        Ok(())
    }

    // port: tsc/internal/checker/checker.go:Checker.containsArgumentsReference
    pub(crate) fn contains_arguments_reference(&mut self, node: NodeId) -> Result<bool, Error> {
        let Some(body) = self.node(node)?.body() else {
            return Ok(false);
        };
        if let Some(&value) = self.body_checks.arguments_referenced.get(&node) {
            return Ok(value);
        }
        let mut pending = vec![body];
        let mut result = false;
        while let Some(node) = pending.pop() {
            let read = self.node(node)?;
            match read.kind().known() {
                Some(K::Identifier) => {
                    if self.node_text(node)?.as_bytes() == b"arguments"
                        && self.resolved_value_symbol(node)? == self.builtins.arguments_symbol
                    {
                        result = true;
                        break;
                    }
                    continue;
                }
                Some(
                    K::PropertyDeclaration | K::MethodDeclaration | K::GetAccessor | K::SetAccessor,
                ) => {
                    if let Some(name) = read.name() {
                        if self.node(name)?.kind() == K::ComputedPropertyName {
                            pending.push(name);
                            continue;
                        }
                    }
                }
                Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                    pending.extend(read.expression());
                    continue;
                }
                Some(K::PropertyAssignment) => {
                    pending.extend(read.initializer());
                    continue;
                }
                _ => {}
            }
            if matches!(
                read.kind().known(),
                Some(
                    K::Constructor
                        | K::FunctionExpression
                        | K::FunctionDeclaration
                        | K::ArrowFunction
                        | K::MethodDeclaration
                        | K::GetAccessor
                        | K::SetAccessor
                        | K::ModuleDeclaration
                        | K::SourceFile
                )
            ) || self.arguments_type_part(node)?
            {
                continue;
            }
            pending.extend(self.source_children(node)?.into_iter().rev());
        }
        self.body_checks.arguments_referenced.insert(node, result);
        Ok(result)
    }
}

impl CheckerState {
    /// `Node.JSDoc(nil)`: eager roots when already parsed, otherwise the host
    /// parses the file's lazy JSDoc for this node.
    // port: tsc/internal/ast/ast.go:Node.JSDoc
    pub(crate) fn jsdoc_for_node(&self, node: NodeId) -> Result<Option<ts_ast::JSDocRoots>, Error> {
        let view = self.ast(node)?;
        if view.node(node)?.flags() & nf::HAS_JS_DOC == 0 {
            return Ok(None);
        }
        if let Some(roots) = self.eager_jsdoc_for_node(node)? {
            return Ok(Some(roots));
        }
        let Some(source) = ts_ast::utilities::get_source_file_of_node(view, Some(node))? else {
            return Ok(None);
        };
        if !view.source_file(source)?.has_lazy_jsdoc {
            return Ok(None);
        }
        Ok(Some(self.program()?.host.jsdoc(view, source, node)?))
    }

    // port: tsc/internal/ast/utilities.go:GetJSDocDeprecatedTag
    pub(crate) fn direct_deprecated_tag(&self, node: NodeId) -> Result<Option<NodeId>, Error> {
        if let Some(docs) = self.jsdoc_for_node(node)? {
            for &doc in docs.iter() {
                let list = self
                    .ast(doc)?
                    .node(doc)?
                    .data_source()
                    .as_js_doc()
                    .ok_or(Error::MissingLink("deprecated documentation"))?
                    .tags();
                for tag in self.source_list(doc, list)? {
                    if self.node(tag)?.kind() == K::JSDocDeprecatedTag {
                        return Ok(Some(tag));
                    }
                }
            }
        }
        Ok(None)
    }
    // port: tsc/internal/ast/utilities.go:IsDeprecatedDeclarationWithCachedFlags
    pub(crate) fn is_deprecated_declaration(&self, node: NodeId) -> Result<bool, Error> {
        if ts_ast::utilities::get_combined_node_flags(self.ast(node)?, node)?
            & nf::POSSIBLY_CONTAINS_DEPRECATED_TAG
            == 0
        {
            return Ok(false);
        }
        let mut current = Some(node);
        while let Some(node) = current {
            let read = self.node(node)?;
            if read.flags() & nf::POSSIBLY_CONTAINS_DEPRECATED_TAG != 0 {
                return Ok(self.direct_deprecated_tag(node)?.is_some());
            }
            current = read.parent();
        }
        Ok(false)
    }
    // port: tsc/internal/checker/checker.go:Checker.checkDeprecatedSignature
    pub(crate) fn check_deprecated_signature(
        &mut self,
        signature: crate::SignatureId,
        node: NodeId,
    ) -> Result<(), Error> {
        let data = self.signatures.get(signature)?;
        if data.flags & crate::signature_flags::IS_SIGNATURE_CANDIDATE_FOR_OVERLOAD_FAILURE != 0 {
            return Ok(());
        }
        let Some(declaration) = data.declaration else {
            return Ok(());
        };
        if !self.is_deprecated_declaration(declaration)? {
            return Ok(());
        }
        let location = self.deprecated_suggestion_node(node)?;
        let invoked = ts_ast::utilities_middle::get_invoked_expression(self.ast(node)?, node)?
            .ok_or(Error::MissingLink("deprecated invoked expression"))?;
        let name = self.invoked_name_text(invoked)?;
        let signature = self.signature_to_string(signature)?;
        let message = if name.as_bytes().is_empty() {
            d::X_0_is_deprecated
        } else {
            d::The_signature_0_of_1_is_deprecated
        };
        let mut diagnostic =
            self.diagnostic_for_node(Some(location), message, vec![signature, name])?;
        if let Some(tag) = self.direct_deprecated_tag(declaration)? {
            diagnostic
                .related_information
                .push(std::sync::Arc::new(self.diagnostic_for_node(
                    Some(tag),
                    d::The_declaration_was_marked_as_deprecated_here,
                    vec![],
                )?));
        }
        self.add_suggestion_diagnostic(diagnostic)?;
        Ok(())
    }
    // port: tsc/internal/checker/checker.go:Checker.getDeprecatedSuggestionNode
    pub(crate) fn deprecated_suggestion_node(&self, mut node: NodeId) -> Result<NodeId, Error> {
        loop {
            let read = self.node(node)?;
            match read.kind().known() {
                Some(
                    K::ParenthesizedExpression
                    | K::CallExpression
                    | K::NewExpression
                    | K::Decorator,
                ) => {
                    node = read
                        .expression()
                        .ok_or(Error::MissingLink("deprecated expression"))?;
                }
                Some(K::PropertyAccessExpression) => {
                    return read
                        .name()
                        .ok_or(Error::MissingLink("deprecated property name"))
                }
                Some(K::ElementAccessExpression) => {
                    return read
                        .data_source()
                        .as_element_access_expression()
                        .and_then(|data| data.argument_expression())
                        .ok_or(Error::MissingLink("deprecated element name"))
                }
                Some(K::TaggedTemplateExpression) => {
                    node = read
                        .data_source()
                        .as_tagged_template_expression()
                        .and_then(|data| data.tag())
                        .ok_or(Error::MissingLink("deprecated template tag"))?;
                }
                Some(K::TypeReference) => {
                    let name = read
                        .data_source()
                        .as_type_reference_node()
                        .and_then(|data| data.type_name())
                        .ok_or(Error::MissingLink("deprecated type name"))?;
                    if let Some(data) = self
                        .ast(name)?
                        .node(name)?
                        .data_source()
                        .as_qualified_name()
                    {
                        return data
                            .right()
                            .ok_or(Error::MissingLink("deprecated type right"));
                    }
                    return Ok(node);
                }
                _ => return Ok(node),
            }
        }
    }
    // port: tsc/internal/checker/utilities.go:tryGetPropertyAccessOrIdentifierToString
    fn invoked_name_text(&self, mut node: NodeId) -> Result<ts_ast::JsString, Error> {
        let mut suffixes: Vec<Vec<u8>> = Vec::new();
        loop {
            let read = self.node(node)?;
            match read.kind().known() {
                Some(K::Identifier) => {
                    let mut bytes = self.node_text(node)?.as_bytes().to_vec();
                    for name in suffixes.into_iter().rev() {
                        bytes.push(b'.');
                        bytes.extend_from_slice(&name);
                    }
                    return Ok(ts_ast::JsString::from_bytes(bytes));
                }
                Some(K::PropertyAccessExpression) => {
                    let name = read.name().ok_or(Error::MissingLink("invoked name"))?;
                    suffixes.push(self.entity_name_text(name)?.as_bytes().to_vec());
                    node = read
                        .expression()
                        .ok_or(Error::MissingLink("invoked object"))?;
                }
                Some(K::ElementAccessExpression) => {
                    let argument = read
                        .data_source()
                        .as_element_access_expression()
                        .and_then(|data| data.argument_expression())
                        .ok_or(Error::MissingLink("invoked element"))?;
                    let name = self.node(argument)?;
                    if !ts_ast::utilities::is_property_name(&name) {
                        return Ok(ts_ast::JsString::default());
                    }
                    suffixes.push(self.index_property_name_node(argument)?.as_bytes().to_vec());
                    node = read
                        .expression()
                        .ok_or(Error::MissingLink("invoked object"))?;
                }
                _ => return Ok(ts_ast::JsString::default()),
            }
        }
    }
}

impl CheckerState {
    // port: tsc/internal/ast/utilities.go:IsPartOfTypeNode
    // Identifier and property-access cases have already returned in the caller.
    fn arguments_type_part(&self, node: NodeId) -> Result<bool, Error> {
        let read = self.node(node)?;
        let kind = read.kind();
        if kind.raw() >= K::TypePredicate as i16 && kind.raw() <= K::ImportType as i16 {
            return Ok(true);
        }
        match kind.known() {
            Some(
                K::AnyKeyword
                | K::UnknownKeyword
                | K::NumberKeyword
                | K::BigIntKeyword
                | K::StringKeyword
                | K::BooleanKeyword
                | K::SymbolKeyword
                | K::ObjectKeyword
                | K::UndefinedKeyword
                | K::NullKeyword
                | K::NeverKeyword,
            ) => Ok(true),
            Some(K::VoidKeyword) => match read.parent() {
                Some(parent) => Ok(self.node(parent)?.kind() != K::VoidExpression),
                None => Err(Error::MissingLink("void keyword parent")),
            },
            Some(K::ExpressionWithTypeArguments) => self.arguments_type_heritage(node),
            Some(K::TypeParameter) => match read.parent() {
                Some(parent) => Ok(matches!(
                    self.node(parent)?.kind().known(),
                    Some(K::MappedType | K::InferType)
                )),
                None => Err(Error::MissingLink("type parameter parent")),
            },
            Some(K::QualifiedName | K::ThisKeyword) => self.arguments_type_parent(node),
            _ => Ok(false),
        }
    }
    // port: tsc/internal/ast/utilities.go:isPartOfTypeNodeInParent
    fn arguments_type_parent(&self, node: NodeId) -> Result<bool, Error> {
        let parent = self
            .ast(node)?
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("type part parent"))?;
        let read = self.node(parent)?;
        let kind = read.kind();
        if kind == K::TypeQuery {
            return Ok(false);
        }
        if kind == K::ImportType {
            return Ok(!read
                .data_source()
                .as_import_type_node()
                .ok_or(Error::MissingLink("import type parent"))?
                .is_type_of());
        }
        if kind.raw() >= K::TypePredicate as i16 && kind.raw() <= K::ImportType as i16 {
            return Ok(true);
        }
        match kind.known() {
            Some(K::ExpressionWithTypeArguments) => self.arguments_type_heritage(parent),
            Some(K::TypeParameter) => Ok(read
                .data_source()
                .as_type_parameter_declaration()
                .ok_or(Error::MissingLink("type parameter parent"))?
                .constraint()
                == Some(node)),
            Some(
                K::VariableDeclaration
                | K::Parameter
                | K::PropertyDeclaration
                | K::PropertySignature
                | K::FunctionDeclaration
                | K::FunctionExpression
                | K::ArrowFunction
                | K::Constructor
                | K::MethodDeclaration
                | K::MethodSignature
                | K::GetAccessor
                | K::SetAccessor
                | K::CallSignature
                | K::ConstructSignature
                | K::IndexSignature
                | K::TypeAssertionExpression,
            ) => Ok(read.type_node() == Some(node)),
            Some(K::CallExpression | K::NewExpression | K::TaggedTemplateExpression) => Ok(self
                .source_list(parent, read.type_argument_list())?
                .contains(&node)),
            _ => Ok(false),
        }
    }
    // port: tsc/internal/ast/utilities.go:isPartOfTypeExpressionWithTypeArguments
    fn arguments_type_heritage(&self, node: NodeId) -> Result<bool, Error> {
        let parent = self
            .ast(node)?
            .node(node)?
            .parent()
            .ok_or(Error::MissingLink("type expression parent"))?;
        let read = self.node(parent)?;
        if matches!(
            read.kind().known(),
            Some(K::JSDocImplementsTag | K::JSDocAugmentsTag)
        ) {
            return Ok(true);
        }
        if read.kind() != K::HeritageClause {
            return Ok(false);
        }
        let owner = read.parent().ok_or(Error::MissingLink("heritage owner"))?;
        Ok(!matches!(
            self.node(owner)?.kind().known(),
            Some(K::ClassDeclaration | K::ClassExpression)
        ) || read
            .data_source()
            .as_heritage_clause()
            .ok_or(Error::MissingLink("heritage clause"))?
            .token()
            == K::ImplementsKeyword)
    }
}

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.isDeprecatedSymbol
    pub(crate) fn is_deprecated_symbol(&mut self, symbol: SymbolId) -> Result<bool, Error> {
        let declarations = self.symbol_declarations(symbol)?.to_vec();
        let parent = self.parent_of_symbol(symbol)?;
        if let Some(parent) = parent {
            if declarations.len() > 1 {
                let interface = self.symbol(parent)?.flags() & sf::INTERFACE != 0;
                let mut any = false;
                let mut all = true;
                for declaration in declarations.iter().flatten() {
                    let deprecated = self.is_deprecated_declaration(*declaration)?;
                    any |= deprecated;
                    all &= deprecated;
                }
                return Ok(if interface { any } else { all });
            }
        }
        if let Some(value_declaration) = self.symbol(symbol)?.value_declaration() {
            if self.is_deprecated_declaration(value_declaration)? {
                return Ok(true);
            }
        }
        if declarations.is_empty() {
            return Ok(false);
        }
        for declaration in declarations.iter().flatten() {
            if !self.is_deprecated_declaration(*declaration)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // port: tsc/internal/checker/checker.go:Checker.addDeprecatedSuggestion
    // port: tsc/internal/checker/checker.go:Checker.addDeprecatedSuggestionWorker
    pub(crate) fn add_deprecated_suggestion(
        &mut self,
        location: NodeId,
        declarations: &[Option<NodeId>],
        deprecated_entity: JsString,
    ) -> Result<(), Error> {
        let mut diagnostic = self.diagnostic_for_node(
            Some(location),
            d::X_0_is_deprecated,
            vec![deprecated_entity],
        )?;
        for declaration in declarations.iter().flatten() {
            if let Some(tag) = self.direct_deprecated_tag(*declaration)? {
                diagnostic.related_information.push(std::sync::Arc::new(
                    self.diagnostic_for_node(
                        Some(tag),
                        d::The_declaration_was_marked_as_deprecated_here,
                        vec![],
                    )?,
                ));
                break;
            }
        }
        self.add_suggestion_diagnostic(diagnostic)?;
        Ok(())
    }
}
