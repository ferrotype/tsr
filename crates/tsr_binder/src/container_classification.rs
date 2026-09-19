//! One container rule inventory; each access path supplies only the selected fact.
use crate::ContainerFlags as C;
use ts_ast::{
    local_bind::{BindNode, LocalBind},
    utilities as u, NodeKind, SyntaxKind as K,
};

const FUNCTION: i32 =
    C::IS_CONTAINER | C::IS_CONTROL_FLOW_CONTAINER | C::HAS_LOCALS | C::IS_FUNCTION_LIKE;

#[derive(Clone, Copy)]
pub(crate) enum ContainerRule {
    Fixed(i32),
    MethodParent,
    PropertyInitializer,
    BlockParent,
}

impl ContainerRule {
    /// The fact is evaluated only for the selected role. In particular, a nil
    /// parent is harmless for fixed/property rules but retains its method/block
    /// contract failure when those roles actually require it.
    pub(crate) fn flags(self, fact: bool) -> C {
        C(match self {
            Self::Fixed(flags) => flags,
            Self::MethodParent => {
                FUNCTION
                    | C::IS_THIS_CONTAINER
                    | if fact {
                        C::IS_OBJECT_LITERAL_OR_CLASS_EXPRESSION_METHOD_OR_ACCESSOR
                    } else {
                        0
                    }
            }
            Self::PropertyInitializer => {
                if fact {
                    C::IS_CONTROL_FLOW_CONTAINER | C::IS_THIS_CONTAINER
                } else {
                    C::NONE
                }
            }
            Self::BlockParent => {
                if fact {
                    C::NONE
                } else {
                    C::IS_BLOCK_SCOPED_CONTAINER | C::HAS_LOCALS
                }
            }
        })
    }
}

// Shared with the public GetContainerFlags adapter. This is the pinned kind
// algorithm, independent of storage shape except for its selected initializer.
pub(crate) fn container_rule(kind: NodeKind) -> ContainerRule {
    use ContainerRule::{BlockParent, Fixed, MethodParent, PropertyInitializer};
    match kind.known() {
        Some(
            K::ClassExpression
            | K::ClassDeclaration
            | K::EnumDeclaration
            | K::ObjectLiteralExpression
            | K::TypeLiteral
            | K::JsxAttributes,
        ) => Fixed(C::IS_CONTAINER),
        Some(K::InterfaceDeclaration) => Fixed(C::IS_CONTAINER | C::IS_INTERFACE),
        Some(
            K::ModuleDeclaration
            | K::TypeAliasDeclaration
            | K::JSTypeAliasDeclaration
            | K::MappedType
            | K::IndexSignature,
        ) => Fixed(C::IS_CONTAINER | C::HAS_LOCALS),
        Some(K::SourceFile) => {
            Fixed(C::IS_CONTAINER | C::IS_CONTROL_FLOW_CONTAINER | C::HAS_LOCALS)
        }
        Some(_) if u::is_method_or_accessor_kind(kind) => MethodParent,
        Some(K::Constructor | K::FunctionDeclaration | K::ClassStaticBlockDeclaration) => {
            Fixed(FUNCTION | C::IS_THIS_CONTAINER)
        }
        Some(
            K::MethodSignature
            | K::CallSignature
            | K::FunctionType
            | K::ConstructSignature
            | K::ConstructorType,
        ) => Fixed(FUNCTION | C::PROPAGATES_THIS_KEYWORD),
        Some(K::FunctionExpression) => {
            Fixed(FUNCTION | C::IS_FUNCTION_EXPRESSION | C::IS_THIS_CONTAINER)
        }
        Some(K::ArrowFunction) => {
            Fixed(FUNCTION | C::IS_FUNCTION_EXPRESSION | C::PROPAGATES_THIS_KEYWORD)
        }
        Some(K::ModuleBlock) => Fixed(C::IS_CONTROL_FLOW_CONTAINER),
        Some(K::PropertyDeclaration) => PropertyInitializer,
        Some(
            K::CatchClause | K::ForStatement | K::ForInStatement | K::ForOfStatement | K::CaseBlock,
        ) => Fixed(C::IS_BLOCK_SCOPED_CONTAINER | C::HAS_LOCALS),
        Some(K::Block) => BlockParent,
        _ => Fixed(C::NONE),
    }
}

pub(crate) fn local_container_flags<'scope>(
    local: &LocalBind<'scope, '_>,
    node: BindNode<'scope>,
) -> C {
    let read = local.node(node);
    let rule = container_rule(read.kind());
    let fact = match rule {
        ContainerRule::Fixed(flags) => return C(flags),
        ContainerRule::MethodParent => {
            let parent = local.node(read.parent().expect("nil node in source AST utility"));
            u::is_object_literal_or_class_expression_kind(parent.kind())
        }
        ContainerRule::PropertyInitializer => {
            let Some(property) = read.as_property_declaration() else {
                // Constructed kind/shape mismatches retain the checked public
                // interface-conversion panic; ordinary fields stay borrowed.
                return crate::get_container_flags(local.view(), local.node_id(node))
                    .expect("binder graph belongs to its retained source");
            };
            property.initializer().is_some()
        }
        ContainerRule::BlockParent => {
            let parent = local.node(crate::need(read.parent()));
            u::is_function_like_kind(parent.kind())
                || parent.kind() == K::ClassStaticBlockDeclaration
        }
    };
    rule.flags(fact)
}

#[cfg(test)]
#[path = "container_classification_tests.rs"]
mod tests;
