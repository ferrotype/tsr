//! The printer factory's transform helpers (`printer/factory.go`): common
//! expressions, comma inlining, entity-name and label restoration, prologue
//! splitting, declaration names and the calls to emit helpers.
//!
//! Upstream evaluates call arguments left to right; constructions keep that
//! order so nodes are created in the same sequence. The generated-name
//! constructors are another unit's; `NewGeneratedNameForNode` is reached
//! through `new_generated_name_for_node_ex` with default options, which is its
//! whole body upstream.

use super::environment::{is_prologue_directive, new_node_list};
use super::{AutoGenerateOptions, EmitContext};
use crate::emit_flags::{self, EmitFlags};
use crate::emit_helpers as helpers;
use tsr_ast::evaluator::OuterExpressionKinds;
use tsr_ast::{
    node_flags, token_flags, AstBuilder, Factory, FactoryMethods, JsString, NodeId, NodeListId,
    RuntimeFactory, SyntaxKind as K,
};
use tsr_core::TextRange;

/// Go's `NameOptions`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NameOptions {
    /// Indicates whether comments may be emitted for the name.
    pub allow_comments: bool,
    /// Indicates whether source maps may be emitted for the name.
    pub allow_source_maps: bool,
}

/// Go's `AssignedNameOptions`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AssignedNameOptions {
    /// Indicates whether comments may be emitted for the name.
    pub allow_comments: bool,
    /// Indicates whether source maps may be emitted for the name.
    pub allow_source_maps: bool,
    /// Indicates whether the assigned name of a declaration shouldn't be considered.
    pub ignore_assigned_name: bool,
}

/// Go's `PrivateIdentifierKind`, a string type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrivateIdentifierKind {
    Field,
    Method,
    Accessor,
    Untransformed,
}
impl PrivateIdentifierKind {
    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Field => b"f",
            Self::Method => b"m",
            Self::Accessor => b"a",
            Self::Untransformed => b"untransformed",
        }
    }
}

fn text(bytes: &'static [u8]) -> JsString {
    JsString::from_bytes(bytes)
}

/// `Node.Text()` for the kinds `NewStringLiteralFromNode` reads.
fn literal_like_text(factory: &dyn Factory, node: NodeId) -> JsString {
    let read = factory.node(node);
    if let Some(data) = read.as_identifier() {
        return data.text_owned();
    }
    if let Some(data) = read.as_private_identifier() {
        return data.text_owned();
    }
    if let Some(data) = read.as_string_literal() {
        return data.text_owned();
    }
    if let Some(data) = read.as_numeric_literal() {
        return data.text_owned();
    }
    if let Some(data) = read.as_big_int_literal() {
        return data.text_owned();
    }
    if let Some(data) = read.as_no_substitution_template_literal() {
        return data.text_owned();
    }
    if let Some(data) = read.as_template_head() {
        return data.text_owned();
    }
    if let Some(data) = read.as_template_middle() {
        return data.text_owned();
    }
    if let Some(data) = read.as_template_tail() {
        return data.text_owned();
    }
    if let Some(data) = read.as_regular_expression_literal() {
        return data.text_owned();
    }
    if let Some(data) = read.as_jsx_namespaced_name() {
        let (namespace, name) = (data.namespace(), data.name());
        drop(read);
        let namespace = literal_like_text(
            factory,
            namespace.expect("runtime error: invalid memory address or nil pointer dereference"),
        );
        let name = literal_like_text(
            factory,
            name.expect("runtime error: invalid memory address or nil pointer dereference"),
        );
        let mut bytes = Vec::with_capacity(namespace.len() + 1 + name.len());
        bytes.extend_from_slice(namespace.as_bytes());
        bytes.push(b':');
        bytes.extend_from_slice(name.as_bytes());
        return JsString::from_bytes(bytes);
    }
    panic!("Unhandled case in Node.Text: {:?}", read.kind())
}

impl EmitContext {
    /// Allocates a new StringLiteral whose source text is derived from the
    /// provided node, often an Identifier or NumericLiteral.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewStringLiteralFromNode
    pub fn new_string_literal_from_node(
        &mut self,
        factory: &mut dyn Factory,
        text_source_node: NodeId,
    ) -> NodeId {
        let text = match factory.node(text_source_node).kind().known() {
            Some(
                K::Identifier
                | K::PrivateIdentifier
                | K::JsxNamespacedName
                | K::StringLiteral
                | K::NumericLiteral
                | K::BigIntLiteral
                | K::NoSubstitutionTemplateLiteral
                | K::TemplateHead
                | K::TemplateMiddle
                | K::TemplateTail
                | K::RegularExpressionLiteral,
            ) => literal_like_text(factory, text_source_node),
            _ => JsString::default(),
        };
        let node = factory.new_string_literal(text, token_flags::NONE);
        self.set_text_source(node, text_source_node);
        node
    }

    //
    // Common Tokens
    //

    // port: tsc/internal/printer/factory.go:NodeFactory.NewThisExpression
    pub fn new_this_expression(&self, factory: &mut dyn Factory) -> NodeId {
        factory.new_keyword_expression(K::ThisKeyword.into())
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.NewTrueExpression
    pub fn new_true_expression(&self, factory: &mut dyn Factory) -> NodeId {
        factory.new_keyword_expression(K::TrueKeyword.into())
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.NewFalseExpression
    pub fn new_false_expression(&self, factory: &mut dyn Factory) -> NodeId {
        factory.new_keyword_expression(K::FalseKeyword.into())
    }

    //
    // Common Operators
    //

    fn new_binary(factory: &mut dyn Factory, left: NodeId, operator: K, right: NodeId) -> NodeId {
        let token = factory.new_token(operator.into());
        factory.new_binary_expression(None, Some(left), None, Some(token), Some(right))
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.NewCommaExpression
    pub fn new_comma_expression(
        &self,
        factory: &mut dyn Factory,
        left: NodeId,
        right: NodeId,
    ) -> NodeId {
        Self::new_binary(factory, left, K::CommaToken, right)
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.NewAssignmentExpression
    pub fn new_assignment_expression(
        &self,
        factory: &mut dyn Factory,
        left: NodeId,
        right: NodeId,
    ) -> NodeId {
        Self::new_binary(factory, left, K::EqualsToken, right)
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.NewLogicalORExpression
    pub fn new_logical_or_expression(
        &self,
        factory: &mut dyn Factory,
        left: NodeId,
        right: NodeId,
    ) -> NodeId {
        Self::new_binary(factory, left, K::BarBarToken, right)
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.NewLogicalANDExpression
    pub fn new_logical_and_expression(
        &self,
        factory: &mut dyn Factory,
        left: NodeId,
        right: NodeId,
    ) -> NodeId {
        Self::new_binary(factory, left, K::AmpersandAmpersandToken, right)
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.NewStrictEqualityExpression
    pub fn new_strict_equality_expression(
        &self,
        factory: &mut dyn Factory,
        left: NodeId,
        right: NodeId,
    ) -> NodeId {
        Self::new_binary(factory, left, K::EqualsEqualsEqualsToken, right)
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.NewStrictInequalityExpression
    pub fn new_strict_inequality_expression(
        &self,
        factory: &mut dyn Factory,
        left: NodeId,
        right: NodeId,
    ) -> NodeId {
        Self::new_binary(factory, left, K::ExclamationEqualsEqualsToken, right)
    }

    //
    // Compound Nodes
    //

    // port: tsc/internal/printer/factory.go:NodeFactory.NewVoidZeroExpression
    pub fn new_void_zero_expression(&self, factory: &mut dyn Factory) -> NodeId {
        let zero = factory.new_numeric_literal(text(b"0"), token_flags::NONE);
        factory.new_void_expression(Some(zero))
    }

    /// Converts a slice of expressions into a single comma-delimited
    /// expression. Returns `None` if `expressions` is empty.
    // port: tsc/internal/printer/factory.go:NodeFactory.InlineExpressions
    pub fn inline_expressions(
        &self,
        factory: &mut dyn Factory,
        expressions: &[NodeId],
    ) -> Option<NodeId> {
        if expressions.is_empty() {
            return None;
        }
        if expressions.len() == 1 {
            return Some(expressions[0]);
        }
        let expressions = flatten_comma_elements(factory, expressions);
        let mut expression = expressions[0];
        for &next in &expressions[1..] {
            expression = self.new_comma_expression(factory, expression, next);
        }
        Some(expression)
    }

    //
    // Utilities
    //

    // port: tsc/internal/printer/factory.go:NodeFactory.CreateExpressionFromEntityName
    #[allow(clippy::self_only_used_in_recursion)] // upstream's NodeFactory method; the receiver is the context
    pub fn create_expression_from_entity_name(
        &self,
        factory: &mut dyn RuntimeFactory,
        node: NodeId,
    ) -> NodeId {
        let (loc, parent, qualified) = {
            let read = factory.node(node);
            (
                read.range(),
                read.parent(),
                read.as_qualified_name()
                    .map(|data| (data.left(), data.right())),
            )
        };
        if let Some((left, right)) = qualified {
            let left = self.create_expression_from_entity_name(
                factory,
                left.expect("runtime error: invalid memory address or nil pointer dereference"),
            );
            let original_right =
                right.expect("runtime error: invalid memory address or nil pointer dereference");
            let right = tsr_ast::clone_node(factory, original_right);
            let (right_loc, right_parent) = {
                let read = factory.node(original_right);
                (read.range(), read.parent())
            };
            factory.set_node_range(right, right_loc);
            // TODO(rbuckton): Does this need to be parented?
            factory.set_node_parent(right, right_parent);
            let prop_access = factory.new_property_access_expression(
                Some(left),
                None,
                Some(right),
                node_flags::NONE,
            );
            factory.set_node_range(prop_access, loc);
            return prop_access;
        }
        let res = tsr_ast::clone_node(factory, node);
        factory.set_node_range(res, loc);
        // TODO(rbuckton): Does this need to be parented?
        factory.set_node_parent(res, parent);
        res
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.RestoreEnclosingLabel
    #[allow(clippy::self_only_used_in_recursion)] // upstream's NodeFactory method; the receiver is the context
    pub fn restore_enclosing_label(
        &self,
        factory: &mut dyn Factory,
        node: NodeId,
        outermost_labeled_statement: Option<NodeId>,
    ) -> NodeId {
        let Some(outermost_labeled_statement) = outermost_labeled_statement else {
            return node;
        };
        let (label, statement) = {
            let read = factory.node(outermost_labeled_statement);
            let data = read
                .as_labeled_statement()
                .expect("interface conversion: ast.nodeData is not *ast.LabeledStatement");
            (data.label(), data.statement())
        };
        let statement =
            statement.expect("runtime error: invalid memory address or nil pointer dereference");
        let mut inner_label = node;
        if factory.node(statement).kind() == K::LabeledStatement {
            inner_label = self.restore_enclosing_label(factory, node, Some(statement));
        }
        factory.update_labeled_statement(outermost_labeled_statement, label, Some(inner_label))
    }

    /// Creates a statement to bind the iteration value.
    // port: tsc/internal/printer/factory.go:NodeFactory.CreateForOfBindingStatement
    pub fn create_for_of_binding_statement(
        &self,
        factory: &mut dyn RuntimeFactory,
        node: NodeId,
        bound_value: NodeId,
    ) -> NodeId {
        let (kind, loc, flags) = {
            let read = factory.node(node);
            (read.kind(), read.range(), read.flags())
        };
        if kind == K::VariableDeclarationList {
            let declarations = factory
                .node(node)
                .as_variable_declaration_list()
                .expect("interface conversion: ast.nodeData is not *ast.VariableDeclarationList")
                .declarations()
                .expect("runtime error: invalid memory address or nil pointer dereference");
            let first_declaration = {
                let nodes = factory.read_list(declarations).nodes();
                let nodes = factory.read_nodes(nodes);
                assert!(
                    !nodes.is_empty(),
                    "runtime error: index out of range [0] with length 0"
                );
                nodes
                    .at(0)
                    .expect("runtime error: invalid memory address or nil pointer dereference")
            };
            let name = factory.node(first_declaration).name();
            let updated_declaration = factory.update_variable_declaration(
                first_declaration,
                name,
                None,
                None,
                Some(bound_value),
            );
            let list = new_node_list(factory, vec![updated_declaration]);
            let declaration_list =
                factory.update_variable_declaration_list(node, Some(list), flags);
            let statement = factory.new_variable_statement(None, Some(declaration_list));
            factory.set_node_range(statement, loc);
            return statement;
        }
        let updated_expression = self.new_assignment_expression(factory, node, bound_value);
        factory.set_node_range(updated_expression, loc);
        let statement = factory.new_expression_statement(Some(updated_expression));
        factory.set_node_range(statement, loc);
        statement
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewTypeCheck
    pub fn new_type_check(&self, factory: &mut dyn Factory, value: NodeId, tag: &[u8]) -> NodeId {
        if tag == b"null" {
            let null = factory.new_keyword_expression(K::NullKeyword.into());
            self.new_strict_equality_expression(factory, value, null)
        } else if tag == b"undefined" {
            let void_zero = self.new_void_zero_expression(factory);
            self.new_strict_equality_expression(factory, value, void_zero)
        } else {
            let type_of = factory.new_type_of_expression(Some(value));
            let tag = factory.new_string_literal(JsString::from_bytes(tag), token_flags::NONE);
            self.new_strict_equality_expression(factory, type_of, tag)
        }
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewMethodCall
    pub fn new_method_call(
        &self,
        factory: &mut dyn RuntimeFactory,
        object: NodeId,
        method_name: NodeId,
        arguments_list: Vec<NodeId>,
    ) -> NodeId {
        // Preserve the optionality of `object`.
        let read = factory.node(object);
        let optional =
            read.kind() == K::CallExpression && read.flags() & node_flags::OPTIONAL_CHAIN != 0;
        drop(read);
        let property_access = factory.new_property_access_expression(
            Some(object),
            None,
            Some(method_name),
            node_flags::NONE,
        );
        let arguments = new_node_list(factory, arguments_list);
        factory.new_call_expression(
            Some(property_access),
            None,
            None,
            Some(arguments),
            if optional {
                node_flags::OPTIONAL_CHAIN
            } else {
                node_flags::NONE
            },
        )
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewGlobalMethodCall
    pub fn new_global_method_call(
        &self,
        factory: &mut dyn RuntimeFactory,
        global_object_name: &[u8],
        method_name: &[u8],
        arguments_list: Vec<NodeId>,
    ) -> NodeId {
        let object = factory.new_identifier(JsString::from_bytes(global_object_name));
        let method = factory.new_identifier(JsString::from_bytes(method_name));
        self.new_method_call(factory, object, method, arguments_list)
    }

    /// `this_arg` is required; upstream panics on nil.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewFunctionCallCall
    pub fn new_function_call_call(
        &self,
        factory: &mut dyn RuntimeFactory,
        target: NodeId,
        this_arg: Option<NodeId>,
        arguments_list: Vec<NodeId>,
    ) -> NodeId {
        let this_arg = this_arg
            .expect("Attempted to construct function call call without this argument expression");
        let mut args = vec![this_arg];
        args.extend(arguments_list);
        let call = factory.new_identifier(text(b"call"));
        self.new_method_call(factory, target, call, args)
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewArraySliceCall
    pub fn new_array_slice_call(
        &self,
        factory: &mut dyn RuntimeFactory,
        array: NodeId,
        start: i64,
    ) -> NodeId {
        let mut args = Vec::new();
        if start != 0 {
            args.push(factory.new_numeric_literal(
                JsString::from_bytes(start.to_string().into_bytes()),
                token_flags::NONE,
            ));
        }
        let slice = factory.new_identifier(text(b"slice"));
        self.new_method_call(factory, array, slice, args)
    }

    /// Determines whether a node is a parenthesized expression that can be
    /// ignored when recreating outer expressions: its range is synthesized and
    /// it has no custom source map or comment range. (Upstream leaves the
    /// synthetic-comment conditions commented out.)
    // port: tsc/internal/printer/factory.go:NodeFactory.isIgnorableParen
    fn is_ignorable_paren(&self, factory: &dyn Factory, node: NodeId) -> bool {
        let read = factory.node(node);
        read.kind() == K::ParenthesizedExpression
            && tsr_ast::utilities::node_is_synthesized(&read)
            && tsr_ast::utilities::range_is_synthesized(self.source_map_range(factory, node))
            && tsr_ast::utilities::range_is_synthesized(self.comment_range_of(factory, node))
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.updateOuterExpression
    fn update_outer_expression(
        factory: &mut dyn RuntimeFactory,
        outer_expression: NodeId,
        expression: NodeId,
    ) -> NodeId {
        let read = factory.node(outer_expression);
        let kind = read.kind();
        match kind.known() {
            Some(K::ParenthesizedExpression) => {
                drop(read);
                factory.update_parenthesized_expression(outer_expression, Some(expression))
            }
            Some(K::TypeAssertionExpression) => {
                let type_node = read.type_node();
                drop(read);
                factory.update_type_assertion(outer_expression, type_node, Some(expression))
            }
            Some(K::AsExpression) => {
                let type_node = read.type_node();
                drop(read);
                factory.update_as_expression(outer_expression, Some(expression), type_node)
            }
            Some(K::SatisfiesExpression) => {
                let type_node = read.type_node();
                drop(read);
                factory.update_satisfies_expression(outer_expression, Some(expression), type_node)
            }
            Some(K::NonNullExpression) => {
                let flags = read.flags();
                drop(read);
                factory.update_non_null_expression(outer_expression, Some(expression), flags)
            }
            Some(K::ExpressionWithTypeArguments) => {
                let type_arguments = read.type_argument_list();
                drop(read);
                factory.update_expression_with_type_arguments(
                    outer_expression,
                    Some(expression),
                    type_arguments,
                )
            }
            Some(K::PartiallyEmittedExpression) => {
                drop(read);
                factory.update_partially_emitted_expression(outer_expression, Some(expression))
            }
            _ => panic!("Unexpected outer expression kind: {kind:?}"),
        }
    }

    /// `factory` is the builder so the outer-expression test can read the
    /// JSDoc type-assertion shape through its view.
    // port: tsc/internal/printer/factory.go:NodeFactory.RestoreOuterExpressions
    pub fn restore_outer_expressions(
        &self,
        factory: &mut AstBuilder,
        outer_expression: Option<NodeId>,
        inner_expression: NodeId,
        kinds: OuterExpressionKinds,
    ) -> Result<NodeId, crate::Error> {
        if let Some(outer_expression) = outer_expression {
            if tsr_ast::utilities::is_outer_expression(factory.view(), outer_expression, kinds)?
                && !self.is_ignorable_paren(factory, outer_expression)
            {
                let expression = Factory::node(factory, outer_expression).expression();
                let restored = self.restore_outer_expressions(
                    factory,
                    expression,
                    inner_expression,
                    tsr_ast::evaluator::outer_expression_kinds::ALL,
                )?;
                return Ok(Self::update_outer_expression(
                    factory,
                    outer_expression,
                    restored,
                ));
            }
        }
        Ok(inner_expression)
    }

    /// Ensures `"use strict"` is the first statement of a slice of statements.
    // port: tsc/internal/printer/factory.go:NodeFactory.EnsureUseStrict
    pub fn ensure_use_strict(
        &self,
        factory: &mut dyn Factory,
        statements: Vec<NodeId>,
    ) -> Vec<NodeId> {
        if let Some(&statement) = statements.first() {
            if is_prologue_directive(factory, statement) {
                let expression = factory
                    .node(statement)
                    .expression()
                    .expect("runtime error: invalid memory address or nil pointer dereference");
                let is_use_strict = factory
                    .node(expression)
                    .as_string_literal()
                    .is_some_and(|literal| literal.text() == b"use strict");
                if is_use_strict {
                    return statements;
                }
            }
        }
        let literal = factory.new_string_literal(text(b"use strict"), token_flags::NONE);
        let use_strict_prologue = factory.new_expression_statement(Some(literal));
        let mut result = Vec::with_capacity(statements.len() + 1);
        result.push(use_strict_prologue);
        result.extend(statements);
        result
    }

    /// Splits a slice of statements into standard prologue statements and the
    /// rest of the statements.
    // port: tsc/internal/printer/factory.go:NodeFactory.SplitStandardPrologue
    pub fn split_standard_prologue<'s>(
        &self,
        factory: &dyn Factory,
        source: &'s [NodeId],
    ) -> (&'s [NodeId], &'s [NodeId]) {
        for (i, &statement) in source.iter().enumerate() {
            if !is_prologue_directive(factory, statement) {
                return (&source[..i], &source[i..]);
            }
        }
        (source, &[])
    }

    /// Splits a slice of statements into custom prologue statements (those with
    /// `CUSTOM_PROLOGUE` set) and the rest. As upstream, a slice that is all
    /// custom prologues comes back as an empty prologue and the whole slice as
    /// the rest.
    // port: tsc/internal/printer/factory.go:NodeFactory.SplitCustomPrologue
    pub fn split_custom_prologue<'s>(
        &self,
        factory: &dyn Factory,
        source: &'s [NodeId],
    ) -> (&'s [NodeId], &'s [NodeId]) {
        for (i, &statement) in source.iter().enumerate() {
            if is_prologue_directive(factory, statement)
                || self.emit_flags(statement) & emit_flags::CUSTOM_PROLOGUE == 0
            {
                return (&source[..i], &source[i..]);
            }
        }
        (&[], source)
    }

    //
    // Declaration Names
    //

    /// A nil `node` reaches upstream's `NewGeneratedNameForNode(nil)`, which
    /// the generated-name unit does not accept yet: that is
    /// `Error::Unsupported`.
    // port: tsc/internal/printer/factory.go:NodeFactory.getName
    fn get_name(
        &mut self,
        factory: &mut AstBuilder,
        node: Option<NodeId>,
        mut emit_flags: EmitFlags,
        opts: AssignedNameOptions,
    ) -> Result<NodeId, crate::Error> {
        let mut node_name = None;
        if let Some(node) = node {
            node_name = if opts.ignore_assigned_name {
                tsr_ast::get_non_assigned_name_of_declaration(factory.view(), node)?
            } else {
                tsr_ast::get_name_of_declaration(factory.view(), Some(node))?
            };
        }

        if let Some(node_name) = node_name {
            let name = tsr_ast::clone_node(factory, node_name);
            if !opts.allow_comments {
                emit_flags |= emit_flags::NO_COMMENTS;
            }
            if !opts.allow_source_maps {
                emit_flags |= emit_flags::NO_SOURCE_MAP;
            }
            self.add_emit_flags(name, emit_flags);
            return Ok(name);
        }

        let Some(node) = node else {
            return Err(crate::Error::Unsupported("NewGeneratedNameForNode(nil)"));
        };
        Ok(self.new_generated_name_for_node_ex(factory, node, AutoGenerateOptions::default()))
    }

    /// Gets the local name of a declaration: a name the declaration's
    /// immediate scope can refer to (classes, enums, namespaces), never
    /// prefixed with a module or namespace export like `exports.`.
    // port: tsc/internal/printer/factory.go:NodeFactory.GetLocalName
    pub fn get_local_name(
        &mut self,
        factory: &mut AstBuilder,
        node: Option<NodeId>,
    ) -> Result<NodeId, crate::Error> {
        self.get_local_name_ex(factory, node, AssignedNameOptions::default())
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.GetLocalNameEx
    pub fn get_local_name_ex(
        &mut self,
        factory: &mut AstBuilder,
        node: Option<NodeId>,
        opts: AssignedNameOptions,
    ) -> Result<NodeId, crate::Error> {
        self.get_name(factory, node, emit_flags::LOCAL_NAME, opts)
    }

    /// Gets the export name of a declaration, always prefixed with a module or
    /// namespace export like `exports.` when it names an exported symbol.
    // port: tsc/internal/printer/factory.go:NodeFactory.GetExportName
    pub fn get_export_name(
        &mut self,
        factory: &mut AstBuilder,
        node: Option<NodeId>,
    ) -> Result<NodeId, crate::Error> {
        self.get_export_name_ex(factory, node, AssignedNameOptions::default())
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.GetExportNameEx
    pub fn get_export_name_ex(
        &mut self,
        factory: &mut AstBuilder,
        node: Option<NodeId>,
        opts: AssignedNameOptions,
    ) -> Result<NodeId, crate::Error> {
        self.get_name(factory, node, emit_flags::EXPORT_NAME, opts)
    }

    /// Gets the name of a declaration to use during emit.
    // port: tsc/internal/printer/factory.go:NodeFactory.GetDeclarationName
    pub fn get_declaration_name(
        &mut self,
        factory: &mut AstBuilder,
        node: Option<NodeId>,
    ) -> Result<NodeId, crate::Error> {
        self.get_declaration_name_ex(factory, node, NameOptions::default())
    }
    // port: tsc/internal/printer/factory.go:NodeFactory.GetDeclarationNameEx
    pub fn get_declaration_name_ex(
        &mut self,
        factory: &mut AstBuilder,
        node: Option<NodeId>,
        opts: NameOptions,
    ) -> Result<NodeId, crate::Error> {
        self.get_name(
            factory,
            node,
            emit_flags::NONE,
            AssignedNameOptions {
                allow_comments: opts.allow_comments,
                allow_source_maps: opts.allow_source_maps,
                ignore_assigned_name: false,
            },
        )
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.GetNamespaceMemberName
    pub fn get_namespace_member_name(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        ns: NodeId,
        mut name: NodeId,
        opts: NameOptions,
    ) -> NodeId {
        if !self.has_auto_generate_info(name) {
            name = tsr_ast::clone_node(factory, name);
        }
        let qualified_name =
            factory.new_property_access_expression(Some(ns), None, Some(name), node_flags::NONE);
        self.assign_comment_and_source_map_ranges(factory, qualified_name, name);
        if !opts.allow_comments {
            self.add_emit_flags(qualified_name, emit_flags::NO_COMMENTS);
        }
        if !opts.allow_source_maps {
            self.add_emit_flags(qualified_name, emit_flags::NO_SOURCE_MAP);
        }
        qualified_name
    }

    /// Gets the export name of a declaration for use in expressions.
    // port: tsc/internal/printer/factory.go:NodeFactory.GetExternalModuleOrNamespaceExportName
    pub fn get_external_module_or_namespace_export_name(
        &mut self,
        factory: &mut AstBuilder,
        ns: Option<NodeId>,
        node: NodeId,
        allow_comments: bool,
        allow_source_maps: bool,
    ) -> Result<NodeId, crate::Error> {
        if let Some(ns) = ns {
            if tsr_ast::utilities::has_syntactic_modifier(
                factory.view(),
                node,
                tsr_ast::modifier_flags::EXPORT,
            )? {
                let name_opts = NameOptions {
                    allow_comments,
                    allow_source_maps,
                };
                let name = self.get_declaration_name_ex(factory, Some(node), name_opts)?;
                return Ok(self.get_namespace_member_name(factory, ns, name, name_opts));
            }
        }
        self.get_export_name_ex(
            factory,
            Some(node),
            AssignedNameOptions {
                allow_comments,
                allow_source_maps,
                ignore_assigned_name: false,
            },
        )
    }

    //
    // Emit Helpers
    //

    /// Allocates a new Identifier representing a reference to a helper function.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewUnscopedHelperName
    pub fn new_unscoped_helper_name(&mut self, factory: &mut dyn Factory, name: &[u8]) -> NodeId {
        let node = factory.new_identifier(JsString::from_bytes(name));
        self.set_emit_flags(node, emit_flags::HELPER_NAME);
        node
    }

    /// `__name(arguments...)` with the unscoped helper name created before the
    /// argument list, as upstream's argument evaluation does.
    fn new_helper_call(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        helper_name: &[u8],
        arguments: Vec<NodeId>,
    ) -> NodeId {
        let name = self.new_unscoped_helper_name(factory, helper_name);
        let arguments = new_node_list(factory, arguments);
        factory.new_call_expression(Some(name), None, None, Some(arguments), node_flags::NONE)
    }

    // TypeScript Helpers

    // port: tsc/internal/printer/factory.go:NodeFactory.NewDecorateHelper
    pub fn new_decorate_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        decorator_expressions: Vec<NodeId>,
        target: NodeId,
        member_name: Option<NodeId>,
        descriptor: Option<NodeId>,
    ) -> NodeId {
        self.request_emit_helper(&helpers::DECORATE_HELPER);

        let mut arguments_array = Vec::new();
        let decorators = new_node_list(factory, decorator_expressions);
        arguments_array.push(factory.new_array_literal_expression(Some(decorators), true));
        arguments_array.push(target);
        if let Some(member_name) = member_name {
            arguments_array.push(member_name);
            if let Some(descriptor) = descriptor {
                arguments_array.push(descriptor);
            }
        }

        self.new_helper_call(factory, b"__decorate", arguments_array)
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewMetadataHelper
    pub fn new_metadata_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        metadata_key: &[u8],
        metadata_value: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::METADATA_HELPER);
        let name = self.new_unscoped_helper_name(factory, b"__metadata");
        let key = factory.new_string_literal(JsString::from_bytes(metadata_key), token_flags::NONE);
        let arguments = new_node_list(factory, vec![key, metadata_value]);
        factory.new_call_expression(Some(name), None, None, Some(arguments), node_flags::NONE)
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewParamHelper
    pub fn new_param_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        expression: NodeId,
        parameter_offset: i64,
        location: TextRange,
    ) -> NodeId {
        self.request_emit_helper(&helpers::PARAM_HELPER);
        let name = self.new_unscoped_helper_name(factory, b"__param");
        let offset = factory.new_numeric_literal(
            JsString::from_bytes(parameter_offset.to_string().into_bytes()),
            token_flags::NONE,
        );
        let arguments = new_node_list(factory, vec![offset, expression]);
        let helper =
            factory.new_call_expression(Some(name), None, None, Some(arguments), node_flags::NONE);
        factory.set_node_range(helper, location);
        helper
    }

    // ESNext Helpers

    // port: tsc/internal/printer/factory.go:NodeFactory.NewAddDisposableResourceHelper
    pub fn new_add_disposable_resource_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        env_binding: NodeId,
        value: NodeId,
        is_async: bool,
    ) -> NodeId {
        self.request_emit_helper(&helpers::ADD_DISPOSABLE_RESOURCE_HELPER);
        let name = self.new_unscoped_helper_name(factory, b"__addDisposableResource");
        let keyword = factory.new_keyword_expression(
            if is_async {
                K::TrueKeyword
            } else {
                K::FalseKeyword
            }
            .into(),
        );
        let arguments = new_node_list(factory, vec![env_binding, value, keyword]);
        factory.new_call_expression(Some(name), None, None, Some(arguments), node_flags::NONE)
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewDisposeResourcesHelper
    pub fn new_dispose_resources_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        env_binding: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::DISPOSE_RESOURCES_HELPER);
        self.new_helper_call(factory, b"__disposeResources", vec![env_binding])
    }

    // Class Fields Helpers

    // port: tsc/internal/printer/factory.go:NodeFactory.NewClassPrivateFieldGetHelper
    pub fn new_class_private_field_get_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        receiver: NodeId,
        state: NodeId,
        kind: PrivateIdentifierKind,
        f: Option<NodeId>,
    ) -> NodeId {
        self.request_emit_helper(&helpers::CLASS_PRIVATE_FIELD_GET_HELPER);
        let kind = factory.new_string_literal(text(kind.as_bytes()), token_flags::NONE);
        let mut args = vec![receiver, state, kind];
        if let Some(f) = f {
            args.push(f);
        }
        self.new_helper_call(factory, b"__classPrivateFieldGet", args)
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewClassPrivateFieldSetHelper
    pub fn new_class_private_field_set_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        receiver: NodeId,
        state: NodeId,
        value: NodeId,
        kind: PrivateIdentifierKind,
        f: Option<NodeId>,
    ) -> NodeId {
        self.request_emit_helper(&helpers::CLASS_PRIVATE_FIELD_SET_HELPER);
        let kind = factory.new_string_literal(text(kind.as_bytes()), token_flags::NONE);
        let mut args = vec![receiver, state, value, kind];
        if let Some(f) = f {
            args.push(f);
        }
        self.new_helper_call(factory, b"__classPrivateFieldSet", args)
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewClassPrivateFieldInHelper
    pub fn new_class_private_field_in_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        state: NodeId,
        receiver: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::CLASS_PRIVATE_FIELD_IN_HELPER);
        self.new_helper_call(factory, b"__classPrivateFieldIn", vec![state, receiver])
    }

    /// `Global.method(arguments...)` with the property access built first.
    fn new_global_property_call(
        factory: &mut dyn RuntimeFactory,
        global: &'static [u8],
        method: &'static [u8],
        arguments: Vec<NodeId>,
    ) -> NodeId {
        let object = factory.new_identifier(text(global));
        let name = factory.new_identifier(text(method));
        let callee = factory.new_property_access_expression(
            Some(object),
            None,
            Some(name),
            node_flags::NONE,
        );
        let arguments = new_node_list(factory, arguments);
        factory.new_call_expression(Some(callee), None, None, Some(arguments), node_flags::NONE)
    }

    /// Creates `Object.defineProperty(target, name, descriptor)`.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewObjectDefinePropertyCall
    pub fn new_object_define_property_call(
        &self,
        factory: &mut dyn RuntimeFactory,
        target: NodeId,
        name: NodeId,
        descriptor: NodeId,
    ) -> NodeId {
        Self::new_global_property_call(
            factory,
            b"Object",
            b"defineProperty",
            vec![target, name, descriptor],
        )
    }

    /// Creates `Reflect.get(target, propertyKey, receiver)`.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewReflectGetCall
    pub fn new_reflect_get_call(
        &self,
        factory: &mut dyn RuntimeFactory,
        target: NodeId,
        property_key: NodeId,
        receiver: NodeId,
    ) -> NodeId {
        Self::new_global_property_call(
            factory,
            b"Reflect",
            b"get",
            vec![target, property_key, receiver],
        )
    }

    /// Creates `Reflect.set(target, propertyKey, value, receiver)`.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewReflectSetCall
    pub fn new_reflect_set_call(
        &self,
        factory: &mut dyn RuntimeFactory,
        target: NodeId,
        property_key: NodeId,
        value: NodeId,
        receiver: NodeId,
    ) -> NodeId {
        Self::new_global_property_call(
            factory,
            b"Reflect",
            b"set",
            vec![target, property_key, value, receiver],
        )
    }

    /// Creates `target.bind(thisArg, ...args)`.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewFunctionBindCall
    pub fn new_function_bind_call(
        &self,
        factory: &mut dyn RuntimeFactory,
        target: NodeId,
        this_arg: NodeId,
        arguments_list: Vec<NodeId>,
    ) -> NodeId {
        let mut args = Vec::with_capacity(1 + arguments_list.len());
        args.push(this_arg);
        args.extend(arguments_list);
        let bind = factory.new_identifier(text(b"bind"));
        self.new_method_call(factory, target, bind, args)
    }

    /// Creates `(() => { ...statements })()`.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewImmediatelyInvokedArrowFunction
    pub fn new_immediately_invoked_arrow_function(
        &self,
        factory: &mut dyn RuntimeFactory,
        statements: Vec<NodeId>,
    ) -> NodeId {
        let parameters = new_node_list(factory, Vec::new());
        let arrow_token = factory.new_token(K::EqualsGreaterThanToken.into());
        let statements = new_node_list(factory, statements);
        let body = factory.new_block(Some(statements), true);
        let arrow = factory.new_arrow_function(
            None,
            None,
            Some(parameters),
            None,
            None,
            Some(arrow_token),
            Some(body),
        );
        let callee = factory.new_parenthesized_expression(Some(arrow));
        let arguments = new_node_list(factory, Vec::new());
        factory.new_call_expression(Some(callee), None, None, Some(arguments), node_flags::NONE)
    }

    /// Creates `export default <expression>;`.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewExportDefault
    pub fn new_export_default(&self, factory: &mut dyn Factory, expression: NodeId) -> NodeId {
        factory.new_export_assignment(None, false, None, Some(expression))
    }

    /// Creates `export { <name> };`.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewExternalModuleExport
    pub fn new_external_module_export(
        &self,
        factory: &mut dyn RuntimeFactory,
        name: NodeId,
    ) -> NodeId {
        let specifier = factory.new_export_specifier(false, None, Some(name));
        let elements = new_node_list(factory, vec![specifier]);
        let named_exports = factory.new_named_exports(Some(elements));
        factory.new_export_declaration(None, false, Some(named_exports), None, None)
    }

    // ES2018 Helpers

    /// Chains a sequence of expressions with `Object.assign`; upstream takes
    /// and ignores the script target.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewAssignHelper
    pub fn new_assign_helper(
        &self,
        factory: &mut dyn RuntimeFactory,
        attributes_segments: Vec<NodeId>,
        _script_target: tsr_core::ScriptTarget,
    ) -> NodeId {
        Self::new_global_property_call(factory, b"Object", b"assign", attributes_segments)
    }

    // ES2018 Destructuring Helpers

    /// `factory` is the builder so a binding element's property name can be
    /// read through its view. `computed_temp_variables` is upstream's nilable
    /// slice; a computed name without it is upstream's failed assertion.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewRestHelper
    pub fn new_rest_helper(
        &mut self,
        factory: &mut AstBuilder,
        value: NodeId,
        elements: &[NodeId],
        computed_temp_variables: Option<&[NodeId]>,
        location: TextRange,
    ) -> Result<NodeId, crate::Error> {
        self.request_emit_helper(&helpers::REST_HELPER);
        let mut property_names = Vec::new();
        let mut computed_temp_variable_offset = 0;
        for (i, &element) in elements.iter().enumerate() {
            if i == elements.len() - 1 {
                break;
            }
            let property_name =
                tsr_ast::utilities_positions::try_get_property_name_of_binding_or_assignment_element(
                    factory.view(),
                    element,
                )?;
            if let Some(property_name) = property_name {
                if Factory::node(factory, property_name).kind() == K::ComputedPropertyName {
                    let computed_temp_variables = computed_temp_variables.expect(
                        "Encountered computed property name but 'computedTempVariables' argument was not provided.",
                    );
                    let temp = computed_temp_variables[computed_temp_variable_offset];
                    computed_temp_variable_offset += 1;
                    // typeof _tmp === "symbol" ? _tmp : _tmp + ""
                    let condition = self.new_type_check(factory, temp, b"symbol");
                    let question_token = factory.new_token(K::QuestionToken.into());
                    let colon_token = factory.new_token(K::ColonToken.into());
                    let plus_token = factory.new_token(K::PlusToken.into());
                    let empty = factory.new_string_literal(JsString::default(), token_flags::NONE);
                    let when_false = factory.new_binary_expression(
                        None,
                        Some(temp),
                        None,
                        Some(plus_token),
                        Some(empty),
                    );
                    property_names.push(factory.new_conditional_expression(
                        Some(condition),
                        Some(question_token),
                        Some(temp),
                        Some(colon_token),
                        Some(when_false),
                    ));
                } else {
                    property_names.push(self.new_string_literal_from_node(factory, property_name));
                }
            }
        }
        let names = new_node_list(factory, property_names);
        let prop_names = factory.new_array_literal_expression(Some(names), false);
        factory.set_node_range(prop_names, location);
        Ok(self.new_helper_call(factory, b"__rest", vec![value, prop_names]))
    }

    // ES2018 Helpers

    /// Allocates a new Call expression to the `__await` helper.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewAwaitHelper
    pub fn new_await_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        expression: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::AWAIT_HELPER);
        self.new_helper_call(factory, b"__await", vec![expression])
    }

    /// Allocates a new Call expression to the `__asyncGenerator` helper.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewAsyncGeneratorHelper
    pub fn new_async_generator_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        generator_func: NodeId,
        has_lexical_this: bool,
    ) -> NodeId {
        self.request_emit_helper(&helpers::AWAIT_HELPER);
        self.request_emit_helper(&helpers::ASYNC_GENERATOR_HELPER);

        // Mark this node as originally an async function body
        self.add_emit_flags(
            generator_func,
            emit_flags::ASYNC_FUNCTION_BODY | emit_flags::REUSE_TEMP_VARIABLE_SCOPE,
        );

        let this_arg = if has_lexical_this {
            factory.new_keyword_expression(K::ThisKeyword.into())
        } else {
            self.new_void_zero_expression(factory)
        };

        let name = self.new_unscoped_helper_name(factory, b"__asyncGenerator");
        let arguments_identifier = factory.new_identifier(text(b"arguments"));
        let arguments = new_node_list(
            factory,
            vec![this_arg, arguments_identifier, generator_func],
        );
        factory.new_call_expression(Some(name), None, None, Some(arguments), node_flags::NONE)
    }

    /// Allocates a new Call expression to the `__asyncDelegator` helper.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewAsyncDelegatorHelper
    pub fn new_async_delegator_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        expression: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::AWAIT_HELPER);
        self.request_emit_helper(&helpers::ASYNC_DELEGATOR_HELPER);
        self.new_helper_call(factory, b"__asyncDelegator", vec![expression])
    }

    /// Allocates a new Call expression to the `__asyncValues` helper.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewAsyncValuesHelper
    pub fn new_async_values_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        expression: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::ASYNC_VALUES_HELPER);
        self.new_helper_call(factory, b"__asyncValues", vec![expression])
    }

    /// Allocates a new Call expression to the `__awaiter` helper.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewAwaiterHelper
    pub fn new_awaiter_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        has_lexical_this: bool,
        arguments_expression: Option<NodeId>,
        parameters: Option<NodeListId>,
        body: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::AWAITER_HELPER);

        let params = match parameters {
            Some(parameters) => parameters,
            None => new_node_list(factory, Vec::new()),
        };

        let asterisk = factory.new_token(K::AsteriskToken.into());
        let generator_func = factory.new_function_expression(
            None,
            Some(asterisk),
            None,
            None,
            Some(params),
            None,
            None,
            Some(body),
        );

        // Mark this node as originally an async function body
        self.add_emit_flags(
            generator_func,
            emit_flags::ASYNC_FUNCTION_BODY | emit_flags::REUSE_TEMP_VARIABLE_SCOPE,
        );

        let this_arg = if has_lexical_this {
            factory.new_keyword_expression(K::ThisKeyword.into())
        } else {
            self.new_void_zero_expression(factory)
        };

        let args_arg = match arguments_expression {
            Some(arguments_expression) => arguments_expression,
            None => self.new_void_zero_expression(factory),
        };

        let name = self.new_unscoped_helper_name(factory, b"__awaiter");
        let void_zero = self.new_void_zero_expression(factory);
        let arguments = new_node_list(factory, vec![this_arg, args_arg, void_zero, generator_func]);
        factory.new_call_expression(Some(name), None, None, Some(arguments), node_flags::NONE)
    }

    // ES Decorator Helpers

    fn new_property_assignment_named(
        factory: &mut dyn Factory,
        name: &'static [u8],
        initializer: Option<NodeId>,
    ) -> NodeId {
        let name = factory.new_identifier(text(name));
        factory.new_property_assignment(None, Some(name), None, None, initializer)
    }

    /// `name: "value"`, with the name created before the literal.
    fn new_string_property_assignment(
        factory: &mut dyn Factory,
        name: &'static [u8],
        value: &[u8],
    ) -> NodeId {
        let name = factory.new_identifier(text(name));
        let value = factory.new_string_literal(JsString::from_bytes(value), 0);
        factory.new_property_assignment(None, Some(name), None, None, Some(value))
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewESDecorateClassContextObject
    pub fn new_es_decorate_class_context_object(
        &self,
        factory: &mut dyn RuntimeFactory,
        name_expr: Option<NodeId>,
        metadata: Option<NodeId>,
    ) -> NodeId {
        let kind = Self::new_string_property_assignment(factory, b"kind", b"class");
        let name = Self::new_property_assignment_named(factory, b"name", name_expr);
        let metadata = Self::new_property_assignment_named(factory, b"metadata", metadata);
        let props = new_node_list(factory, vec![kind, name, metadata]);
        factory.new_object_literal_expression(Some(props), false)
    }

    fn new_obj_accessor(
        factory: &mut dyn Factory,
        name_computed: bool,
        name_expr: Option<NodeId>,
    ) -> NodeId {
        let obj = factory.new_identifier(text(b"obj"));
        if name_computed {
            factory.new_element_access_expression(Some(obj), None, name_expr, node_flags::NONE)
        } else {
            factory.new_property_access_expression(Some(obj), None, name_expr, node_flags::NONE)
        }
    }

    fn new_simple_parameter(factory: &mut dyn Factory, name: &'static [u8]) -> NodeId {
        let name = factory.new_identifier(text(name));
        factory.new_parameter_declaration(None, None, Some(name), None, None, None)
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewESDecorateClassElementAccessGetMethod
    pub fn new_es_decorate_class_element_access_get_method(
        &self,
        factory: &mut dyn RuntimeFactory,
        name_computed: bool,
        name_expr: Option<NodeId>,
    ) -> NodeId {
        let accessor = Self::new_obj_accessor(factory, name_computed, name_expr);
        let obj_param = Self::new_simple_parameter(factory, b"obj");
        let parameters = new_node_list(factory, vec![obj_param]);
        let arrow_token = factory.new_token(K::EqualsGreaterThanToken.into());
        let arrow = factory.new_arrow_function(
            None,
            None,
            Some(parameters),
            None,
            None,
            Some(arrow_token),
            Some(accessor),
        );
        Self::new_property_assignment_named(factory, b"get", Some(arrow))
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewESDecorateClassElementAccessSetMethod
    pub fn new_es_decorate_class_element_access_set_method(
        &self,
        factory: &mut dyn RuntimeFactory,
        name_computed: bool,
        name_expr: Option<NodeId>,
    ) -> NodeId {
        let accessor = Self::new_obj_accessor(factory, name_computed, name_expr);
        let value = factory.new_identifier(text(b"value"));
        let assignment = self.new_assignment_expression(factory, accessor, value);
        let stmt = factory.new_expression_statement(Some(assignment));
        let statements = new_node_list(factory, vec![stmt]);
        let body = factory.new_block(Some(statements), false);

        let obj_param = Self::new_simple_parameter(factory, b"obj");
        let value_param = Self::new_simple_parameter(factory, b"value");

        let parameters = new_node_list(factory, vec![obj_param, value_param]);
        let arrow_token = factory.new_token(K::EqualsGreaterThanToken.into());
        let arrow = factory.new_arrow_function(
            None,
            None,
            Some(parameters),
            None,
            None,
            Some(arrow_token),
            Some(body),
        );
        Self::new_property_assignment_named(factory, b"set", Some(arrow))
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewESDecorateClassElementAccessHasMethod
    pub fn new_es_decorate_class_element_access_has_method(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        name_computed: bool,
        name_expr: Option<NodeId>,
    ) -> NodeId {
        // The property name for the "in" expression
        let property_name = match name_expr {
            Some(name) if !name_computed && factory.node(name).kind() == K::Identifier => {
                Some(self.new_string_literal_from_node(factory, name))
            }
            _ => name_expr,
        };

        let obj_param = Self::new_simple_parameter(factory, b"obj");
        let in_token = factory.new_token(K::InKeyword.into());
        let obj = factory.new_identifier(text(b"obj"));
        let in_expr =
            factory.new_binary_expression(None, property_name, None, Some(in_token), Some(obj));

        let parameters = new_node_list(factory, vec![obj_param]);
        let arrow_token = factory.new_token(K::EqualsGreaterThanToken.into());
        let arrow = factory.new_arrow_function(
            None,
            None,
            Some(parameters),
            None,
            None,
            Some(arrow_token),
            Some(in_expr),
        );
        Self::new_property_assignment_named(factory, b"has", Some(arrow))
    }

    /// Creates the "access" object for a class element decorator context
    /// (15.7.3 CreateDecoratorAccessObject): `has` always, `get` for fields,
    /// methods, accessors and getters, `set` for fields, accessors and setters.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewESDecorateClassElementAccessObject
    pub fn new_es_decorate_class_element_access_object(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        name_computed: bool,
        name_expr: Option<NodeId>,
        has_get: bool,
        has_set: bool,
    ) -> NodeId {
        // "has" method: obj => name in obj
        let mut access_props = vec![self.new_es_decorate_class_element_access_has_method(
            factory,
            name_computed,
            name_expr,
        )];

        // "get" method: obj => obj.name or obj => obj[name]
        if has_get {
            access_props.push(self.new_es_decorate_class_element_access_get_method(
                factory,
                name_computed,
                name_expr,
            ));
        }

        // "set" method: (obj, value) => { obj.name = value; } or (obj, value) => { obj[name] = value; }
        if has_set {
            access_props.push(self.new_es_decorate_class_element_access_set_method(
                factory,
                name_computed,
                name_expr,
            ));
        }

        let props = new_node_list(factory, access_props);
        factory.new_object_literal_expression(Some(props), false)
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewESDecorateClassElementContextObject
    #[allow(clippy::fn_params_excessive_bools, clippy::too_many_arguments)] // upstream's positional signature
    pub fn new_es_decorate_class_element_context_object(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        kind: &[u8],
        name_computed: bool,
        name_expr: Option<NodeId>,
        is_static: bool,
        is_private: bool,
        has_get: bool,
        has_set: bool,
        metadata: Option<NodeId>,
    ) -> NodeId {
        // Build the name value for the context's "name" property
        let name_value = match name_expr {
            Some(name)
                if !name_computed
                    && matches!(
                        factory.node(name).kind().known(),
                        Some(K::PrivateIdentifier | K::Identifier)
                    ) =>
            {
                Some(self.new_string_literal_from_node(factory, name))
            }
            _ => name_expr,
        };

        // Build the access object with has/get/set arrow functions
        let access_obj = self.new_es_decorate_class_element_access_object(
            factory,
            name_computed,
            name_expr,
            has_get,
            has_set,
        );

        let static_expr = if is_static {
            self.new_true_expression(factory)
        } else {
            self.new_false_expression(factory)
        };

        let private_expr = if is_private {
            self.new_true_expression(factory)
        } else {
            self.new_false_expression(factory)
        };

        let kind = Self::new_string_property_assignment(factory, b"kind", kind);
        let name = Self::new_property_assignment_named(factory, b"name", name_value);
        let static_prop =
            Self::new_property_assignment_named(factory, b"static", Some(static_expr));
        let private_prop =
            Self::new_property_assignment_named(factory, b"private", Some(private_expr));
        let access = Self::new_property_assignment_named(factory, b"access", Some(access_obj));
        let metadata = Self::new_property_assignment_named(factory, b"metadata", metadata);
        let props = new_node_list(
            factory,
            vec![kind, name, static_prop, private_prop, access, metadata],
        );
        factory.new_object_literal_expression(Some(props), false)
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewESDecorateHelper
    #[allow(clippy::too_many_arguments)] // upstream's positional signature
    pub fn new_es_decorate_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        ctor: NodeId,
        descriptor_in: NodeId,
        decorators: NodeId,
        context_in: NodeId,
        initializers: NodeId,
        extra_initializers: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::ES_DECORATE_HELPER);
        self.new_helper_call(
            factory,
            b"__esDecorate",
            vec![
                ctor,
                descriptor_in,
                decorators,
                context_in,
                initializers,
                extra_initializers,
            ],
        )
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewRunInitializersHelper
    pub fn new_run_initializers_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        this_arg: NodeId,
        initializers: NodeId,
        value: Option<NodeId>,
    ) -> NodeId {
        self.request_emit_helper(&helpers::RUN_INITIALIZERS_HELPER);
        let arguments = match value {
            Some(value) => vec![this_arg, initializers, value],
            None => vec![this_arg, initializers],
        };
        self.new_helper_call(factory, b"__runInitializers", arguments)
    }

    // ES2015 Helpers

    // port: tsc/internal/printer/factory.go:NodeFactory.NewTemplateObjectHelper
    pub fn new_template_object_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        cooked_array: NodeId,
        raw_array: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::MAKE_TEMPLATE_OBJECT_HELPER);
        self.new_helper_call(
            factory,
            b"__makeTemplateObject",
            vec![cooked_array, raw_array],
        )
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewPropKeyHelper
    pub fn new_prop_key_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        expr: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::PROP_KEY_HELPER);
        self.new_helper_call(factory, b"__propKey", vec![expr])
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewSetFunctionNameHelper
    pub fn new_set_function_name_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        f: NodeId,
        name: NodeId,
        prefix: &[u8],
    ) -> NodeId {
        self.request_emit_helper(&helpers::SET_FUNCTION_NAME_HELPER);
        let arguments = if prefix.is_empty() {
            vec![f, name]
        } else {
            let prefix =
                factory.new_string_literal(JsString::from_bytes(prefix), token_flags::NONE);
            vec![f, name, prefix]
        };
        self.new_helper_call(factory, b"__setFunctionName", arguments)
    }

    // ES Module Helpers

    /// Allocates a new Call expression to the `__importDefault` helper.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewImportDefaultHelper
    pub fn new_import_default_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        expression: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::IMPORT_DEFAULT_HELPER);
        self.new_helper_call(factory, b"__importDefault", vec![expression])
    }

    /// Allocates a new Call expression to the `__importStar` helper.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewImportStarHelper
    pub fn new_import_star_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        expression: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::IMPORT_STAR_HELPER);
        self.new_helper_call(factory, b"__importStar", vec![expression])
    }

    /// Allocates a new Call expression to the `__exportStar` helper.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewExportStarHelper
    pub fn new_export_star_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        module_expression: NodeId,
        exports_expression: NodeId,
    ) -> NodeId {
        self.request_emit_helper(&helpers::EXPORT_STAR_HELPER);
        self.new_helper_call(
            factory,
            b"__exportStar",
            vec![module_expression, exports_expression],
        )
    }

    // port: tsc/internal/printer/factory.go:NodeFactory.NewAssignmentTargetWrapper
    pub fn new_assignment_target_wrapper(
        &self,
        factory: &mut dyn RuntimeFactory,
        param_name: NodeId,
        expression: NodeId,
    ) -> NodeId {
        let value = factory.new_identifier(text(b"value"));
        let parameter =
            factory.new_parameter_declaration(None, None, Some(param_name), None, None, None);
        let parameters = new_node_list(factory, vec![parameter]);
        let statement = factory.new_expression_statement(Some(expression));
        let statements = new_node_list(factory, vec![statement]);
        let body = factory.new_block(Some(statements), false);
        let set_accessor = factory.new_set_accessor_declaration(
            None,
            Some(value),
            None,
            Some(parameters),
            None,
            None,
            Some(body),
        );
        let properties = new_node_list(factory, vec![set_accessor]);
        let obj_literal = factory.new_object_literal_expression(Some(properties), false);
        // Explicit parens required because of v8 regression (https://bugs.chromium.org/p/v8/issues/detail?id=9560)
        let parenthesized = factory.new_parenthesized_expression(Some(obj_literal));
        let value = factory.new_identifier(text(b"value"));
        factory.new_property_access_expression(
            Some(parenthesized),
            None,
            Some(value),
            node_flags::NONE,
        )
    }

    /// Allocates a new Call expression to the `__rewriteRelativeImportExtension` helper.
    // port: tsc/internal/printer/factory.go:NodeFactory.NewRewriteRelativeImportExtensionsHelper
    pub fn new_rewrite_relative_import_extensions_helper(
        &mut self,
        factory: &mut dyn RuntimeFactory,
        first_argument: NodeId,
        preserve_jsx: bool,
    ) -> NodeId {
        self.request_emit_helper(&helpers::REWRITE_RELATIVE_IMPORT_EXTENSIONS_HELPER);
        let arguments = if preserve_jsx {
            let true_token = factory.new_token(K::TrueKeyword.into());
            vec![first_argument, true_token]
        } else {
            vec![first_argument]
        };
        self.new_helper_call(factory, b"__rewriteRelativeImportExtension", arguments)
    }
}

// port: tsc/internal/printer/factory.go:flattenCommaElement
fn flatten_comma_element(factory: &dyn Factory, node: NodeId, expressions: &mut Vec<NodeId>) {
    let read = factory.node(node);
    if let Some(binary) = read.as_binary_expression() {
        let operator = binary
            .operator_token()
            .expect("runtime error: invalid memory address or nil pointer dereference");
        if tsr_ast::utilities::node_is_synthesized(&read)
            && factory.node(operator).kind() == K::CommaToken
        {
            let (left, right) = (binary.left(), binary.right());
            drop(read);
            flatten_comma_element(
                factory,
                left.expect("runtime error: invalid memory address or nil pointer dereference"),
                expressions,
            );
            flatten_comma_element(
                factory,
                right.expect("runtime error: invalid memory address or nil pointer dereference"),
                expressions,
            );
            return;
        }
    }
    expressions.push(node);
}

// port: tsc/internal/printer/factory.go:flattenCommaElements
fn flatten_comma_elements(factory: &dyn Factory, expressions: &[NodeId]) -> Vec<NodeId> {
    let mut result = Vec::new();
    for &expression in expressions {
        flatten_comma_element(factory, expression, &mut result);
    }
    result
}
