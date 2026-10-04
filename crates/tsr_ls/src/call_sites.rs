use crate::{
    call_declarations as decl, documentation::list, syntax::Syntax, LanguageService, Result,
};
use tsr_ast::{utilities as ast, utilities_positions as pos, NodeId, SyntaxKind as K};
use tsr_checker::Operation;
use tsr_core::TextRange;

pub(crate) struct Site {
    pub declaration: NodeId,
    pub source: NodeId,
    pub range: TextRange,
}

impl LanguageService<'_> {
    // port: tsc/internal/ls/callhierarchy.go:collectCallSites
    pub(crate) fn call_sites(&self, c: &mut Operation<'_>, node: NodeId) -> Result<Vec<Site>> {
        let view = self.view(node)?;
        let n = view.node(node)?;
        let mut initial = Vec::new();
        match n.kind().known() {
            Some(K::SourceFile) => initial.extend(list(view, n.statement_list())?),
            Some(K::ModuleDeclaration) => {
                if !ast::has_syntactic_modifier(view, node, tsr_ast::modifier_flags::AMBIENT)? {
                    if let Some(body) = n.body() {
                        if view.node(body)?.kind() == K::ModuleBlock {
                            initial.extend(list(view, view.node(body)?.statement_list())?);
                        }
                    }
                }
            }
            Some(
                K::FunctionDeclaration
                | K::FunctionExpression
                | K::ArrowFunction
                | K::MethodDeclaration
                | K::GetAccessor
                | K::SetAccessor,
            ) => {
                if let Some(implementation) = decl::implementation(self, c, node)? {
                    let n = c.node(implementation)?;
                    initial.extend(list(self.view(implementation)?, n.parameter_list())?);
                    initial.extend(n.body());
                }
            }
            Some(K::ClassDeclaration | K::ClassExpression) => {
                initial.extend(list(view, n.modifiers())?);
                if let Some(heritage) =
                    tsr_ast::utilities_class::get_class_extends_heritage_element(view, node)?
                {
                    initial.extend(view.node(heritage)?.expression());
                }
                for member in list(view, n.member_list())? {
                    let m = view.node(member)?;
                    initial.extend(list(view, m.modifiers())?);
                    if m.kind() == K::PropertyDeclaration {
                        initial.extend(m.initializer());
                    } else if m.kind() == K::Constructor && m.body().is_some() {
                        initial.extend(list(view, m.parameter_list())?);
                        initial.extend(m.body());
                    } else if m.kind() == K::ClassStaticBlockDeclaration {
                        initial.push(member);
                    }
                }
            }
            Some(K::ClassStaticBlockDeclaration) => initial.extend(
                n.data_source()
                    .as_class_static_block_declaration()
                    .and_then(|d| d.body()),
            ),
            _ => {
                return Err(tsr_astnav::Error::Assertion(
                    "Unexpected outgoing call declaration".into(),
                )
                .into())
            }
        }
        let mut stack = initial.into_iter().rev().collect::<Vec<_>>();
        let mut result = Vec::new();
        // Unlike the pin's recursive collector, the explicit stack is bounded
        // by syntax nodes and does not depend on the editor worker's stack size.
        while let Some(node) = stack.pop() {
            self.check_canceled()?;
            let view = self.view(node)?;
            let n = view.node(node)?;
            if n.flags() & tsr_ast::node_flags::AMBIENT != 0 {
                continue;
            }
            if decl::valid(view, node)? {
                if ast::is_class_like(&n) {
                    let mut names = Vec::new();
                    for member in list(view, n.member_list())? {
                        if let Some(name) = view.node(member)?.name() {
                            if view.node(name)?.kind() == K::ComputedPropertyName {
                                names.extend(view.node(name)?.expression());
                            }
                        }
                    }
                    stack.extend(names.into_iter().rev());
                }
                continue;
            }
            let mut children = Vec::new();
            let mut target = None;
            match n.kind().known() {
                Some(
                    K::Identifier
                    | K::ImportEqualsDeclaration
                    | K::ImportDeclaration
                    | K::ExportDeclaration
                    | K::InterfaceDeclaration
                    | K::TypeAliasDeclaration,
                ) => continue,
                Some(K::ClassStaticBlockDeclaration) => target = Some(node),
                Some(K::TypeAssertionExpression | K::AsExpression | K::SatisfiesExpression) => {
                    children.extend(n.expression());
                }
                Some(K::VariableDeclaration | K::Parameter) => {
                    children.extend(n.name());
                    children.extend(n.initializer());
                }
                Some(K::CallExpression | K::NewExpression) => {
                    target = n.expression();
                    children.extend(n.expression());
                    children.extend(list(view, n.argument_list())?);
                }
                Some(K::TaggedTemplateExpression) => {
                    let d = n.data_source().as_tagged_template_expression().unwrap();
                    target = d.tag();
                    children.extend(d.tag());
                    children.extend(d.template());
                }
                Some(K::JsxOpeningElement | K::JsxSelfClosingElement) => {
                    target = n.tag_name();
                    children.extend(n.tag_name());
                    children.extend(n.attributes());
                }
                Some(K::Decorator) => {
                    target = n.expression();
                    children.extend(n.expression());
                }
                Some(K::PropertyAccessExpression | K::ElementAccessExpression) => {
                    target = Some(node);
                    children.extend(
                        Syntax::new(
                            view,
                            ast::get_source_file_of_node(view, Some(node))?.unwrap(),
                        )?
                        .children(node)?,
                    );
                }
                _ => {
                    if !pos::is_part_of_type_node(view, node)? {
                        children.extend(
                            Syntax::new(
                                view,
                                ast::get_source_file_of_node(view, Some(node))?.unwrap(),
                            )?
                            .children(node)?,
                        );
                    }
                }
            }
            if let Some(target) = target {
                let declarations = decl::resolve(self, c, target)?;
                let source = ast::get_source_file_of_node(self.view(target)?, Some(target))?
                    .ok_or(tsr_arena::Error::InvalidGraph)?;
                let start = Syntax::new(self.view(target)?, source)?.start(target)?;
                let range = TextRange::new(start, i64::from(view.node(target)?.end()));
                for declaration in declarations {
                    result.push(Site {
                        declaration,
                        source,
                        range,
                    });
                }
            }
            stack.extend(children.into_iter().rev());
        }
        Ok(result)
    }
}
