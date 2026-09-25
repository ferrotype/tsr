//! Group `class`: classes, heritage, decorators, modifiers.
//! Go: `tools/phase1/tables/go/class_columns.go`; spec:
//! `data/phase1/tables/class.json`.
use super::helpers::{all, int, is_kind, node_map, node_predicate, ref_of, refs_of};
use super::{children, parse_source, text, Column, Parsed};
use serde_json::{json, Value};
use tsr_arena::Error;
use tsr_ast::utilities_class::{self as class, AllAccessorDeclarations};
use tsr_ast::{AstView, NodeId, SyntaxKind as K};

pub const COLUMNS: &[&str] = &[
    "ast.Node.Decorators",
    "ast.ClassOrConstructorParameterIsDecorated",
    "ast.ClassElementOrClassElementParameterIsDecorated",
    "ast.NodeOrChildIsDecorated",
    "ast.NodeCanBeDecorated",
    "ast.GetAllAccessorDeclarations",
    "ast.GetAllAccessorDeclarationsForDeclaration",
    "ast.GetClassLikeDeclarationOfSymbol",
    "ast.GetHeritageClause",
    "ast.GetHeritageElements",
    "ast.GetClassExtendsHeritageElement",
    "ast.GetExtendsHeritageClauseElements",
    "ast.GetImplementsHeritageClauseElements",
    "ast.GetHeritageClauseElementName",
    "ast.GetFirstConstructorWithBody",
    "ast.GetThisParameter",
    "ast.HasModifier",
    "ast.HasAbstractModifier",
    "ast.HasAmbientModifier",
    "ast.IsClassOrTypeElement",
    "ast.IsExpressionWithTypeArgumentsInClassExtendsClause",
    "ast.IsInitializedProperty",
    "ast.IsNameOfHeritageClauseTypeReference",
    "ast.IsThisParameter",
    "ast.TryGetClassExtendingExpressionWithTypeArguments",
    "ast.TryGetClassImplementingOrExtendingHeritageClauseElement",
    "ast.ReplaceModifiers",
];

pub fn build(column: &str, input: &Value) -> Option<Result<Column, String>> {
    Some(match column {
        "ast.Node.Decorators" => node_map("source", input, all, |parsed, view, node| {
            refs_of(parsed, class::decorators(view, node).map_err(text)?)
        }),
        "ast.ClassOrConstructorParameterIsDecorated" => {
            node_map("source", input, class_kinds, |_, view, node| {
                legacy_bits(|legacy| {
                    class::class_or_constructor_parameter_is_decorated(view, legacy, node)
                })
            })
        }
        "ast.ClassElementOrClassElementParameterIsDecorated" => {
            node_map("source", input, class_element_in_class, |_, view, node| {
                let parent = view.node(node).map_err(text)?.parent();
                legacy_bits(|legacy| {
                    class::class_element_or_class_element_parameter_is_decorated(
                        view, legacy, node, parent,
                    )
                })
            })
        }
        "ast.NodeOrChildIsDecorated" => {
            node_map("source", input, not_source_file, |_, view, node| {
                let (parent, grandparent) = parents(view, node).map_err(text)?;
                legacy_bits(|legacy| {
                    class::node_or_child_is_decorated(view, legacy, node, parent, grandparent)
                })
            })
        }
        "ast.NodeCanBeDecorated" => node_map("source", input, not_source_file, |_, view, node| {
            let (parent, grandparent) = parents(view, node).map_err(text)?;
            legacy_bits(|legacy| {
                class::node_can_be_decorated(view, legacy, node, parent, grandparent)
            })
        }),
        "ast.GetAllAccessorDeclarations" => node_map(
            "source",
            input,
            accessor_in_member_list,
            |parsed, view, node| {
                let parent = view
                    .node(node)
                    .map_err(text)?
                    .parent()
                    .ok_or("parentless accessor")?;
                let members: Vec<NodeId> = view
                    .node_slice(
                        view.node(parent)
                            .map_err(text)?
                            .members(view)
                            .map_err(text)?,
                    )
                    .map_err(text)?
                    .iter()
                    .flatten()
                    .collect();
                accessor_refs(
                    parsed,
                    class::get_all_accessor_declarations(view, &members, node).map_err(text)?,
                )
            },
        ),
        "ast.GetAllAccessorDeclarationsForDeclaration" => {
            node_map("bound", input, accessor_kinds, |parsed, view, node| {
                let bound = parsed.bound().ok_or("the input is not bound")?;
                let mut declarations = Vec::new();
                if let Some(symbol) = bound
                    .node_binding(node)
                    .map_err(text)?
                    .and_then(|binding| binding.symbol)
                {
                    let symbol = bound.symbol(symbol).map_err(text)?;
                    declarations = bound
                        .result()
                        .declarations()
                        .get(symbol.declarations())
                        .map_err(text)?
                        .iter()
                        .flatten()
                        .collect();
                }
                accessor_refs(
                    parsed,
                    class::get_all_accessor_declarations_for_declaration(view, node, &declarations)
                        .map_err(text)?,
                )
            })
        }
        "ast.GetClassLikeDeclarationOfSymbol" => {
            node_map("bound", input, all, |parsed, view, node| {
                let bound = parsed.bound().ok_or("the input is not bound")?;
                let Some(symbol) = bound
                    .node_binding(node)
                    .map_err(text)?
                    .and_then(|binding| binding.symbol)
                else {
                    return Ok(Value::Null);
                };
                ref_of(
                    parsed,
                    class::get_class_like_declaration_of_symbol(view, bound, symbol)
                        .map_err(text)?,
                )
            })
        }
        "ast.GetHeritageClause" => node_map("source", input, all, |parsed, view, node| {
            let extends =
                class::get_heritage_clause(view, node, K::ExtendsKeyword).map_err(text)?;
            let implements =
                class::get_heritage_clause(view, node, K::ImplementsKeyword).map_err(text)?;
            if extends.is_none() && implements.is_none() {
                return Ok(Value::Null);
            }
            Ok(json!([
                ref_of(parsed, extends)?,
                ref_of(parsed, implements)?
            ]))
        }),
        "ast.GetHeritageElements" => node_map("source", input, all, |parsed, view, node| {
            let extends =
                class::get_heritage_elements(view, node, K::ExtendsKeyword).map_err(text)?;
            let implements =
                class::get_heritage_elements(view, node, K::ImplementsKeyword).map_err(text)?;
            if extends.is_empty() && implements.is_empty() {
                return Ok(Value::Null);
            }
            Ok(json!([
                refs_of(parsed, extends)?,
                refs_of(parsed, implements)?
            ]))
        }),
        "ast.GetClassExtendsHeritageElement" => {
            node_map("source", input, all, |parsed, view, node| {
                ref_of(
                    parsed,
                    class::get_class_extends_heritage_element(view, node).map_err(text)?,
                )
            })
        }
        "ast.GetExtendsHeritageClauseElements" => {
            node_map("source", input, all, |parsed, view, node| {
                refs_of(
                    parsed,
                    class::get_extends_heritage_clause_elements(view, node).map_err(text)?,
                )
            })
        }
        "ast.GetImplementsHeritageClauseElements" => {
            node_map("source", input, all, |parsed, view, node| {
                refs_of(
                    parsed,
                    class::get_implements_heritage_clause_elements(view, node).map_err(text)?,
                )
            })
        }
        "ast.GetHeritageClauseElementName" => node_map(
            "source",
            input,
            heritage_element_kinds,
            |parsed, view, node| {
                ref_of(
                    parsed,
                    class::get_heritage_clause_element_name(view, node).map_err(text)?,
                )
            },
        ),
        "ast.GetFirstConstructorWithBody" => {
            node_map("source", input, member_list_kinds, |parsed, view, node| {
                ref_of(
                    parsed,
                    class::get_first_constructor_with_body(view, node).map_err(text)?,
                )
            })
        }
        "ast.GetThisParameter" => {
            node_map("source", input, function_like_data, |parsed, view, node| {
                ref_of(parsed, class::get_this_parameter(view, node).map_err(text)?)
            })
        }
        "ast.HasModifier" => node_map("source", input, all, |_, view, node| {
            let mut out = Vec::new();
            for (index, mask) in has_modifier_masks().into_iter().enumerate() {
                if tsr_ast::utilities::has_syntactic_modifier(view, node, mask).map_err(text)? {
                    out.push(json!(index));
                }
            }
            Ok(Value::Array(out))
        }),
        "ast.HasAbstractModifier" => node_predicate("source", input, all, |_, view, node| {
            class::has_abstract_modifier(view, node)
        }),
        "ast.HasAmbientModifier" => node_predicate("source", input, all, |_, view, node| {
            class::has_ambient_modifier(view, node)
        }),
        "ast.IsClassOrTypeElement" => node_predicate("source", input, all, |_, view, node| {
            Ok(class::is_class_or_type_element(&view.node(node)?))
        }),
        "ast.IsExpressionWithTypeArgumentsInClassExtendsClause" => {
            node_predicate("source", input, all, |_, view, node| {
                class::is_expression_with_type_arguments_in_class_extends_clause(view, node)
            })
        }
        "ast.IsInitializedProperty" => node_predicate("source", input, all, |_, view, node| {
            Ok(tsr_ast::utilities_middle::is_initialized_property(
                &view.node(node)?,
            ))
        }),
        "ast.IsNameOfHeritageClauseTypeReference" => {
            node_predicate("source", input, not_source_file, |_, view, node| {
                class::is_name_of_heritage_clause_type_reference(view, node)
            })
        }
        "ast.IsThisParameter" => node_predicate("source", input, all, |_, view, node| {
            class::is_this_parameter(view, node)
        }),
        "ast.TryGetClassExtendingExpressionWithTypeArguments" => {
            node_map("source", input, all, |parsed, view, node| {
                ref_of(
                    parsed,
                    class::try_get_class_extending_expression_with_type_arguments(view, node)
                        .map_err(text)?,
                )
            })
        }
        "ast.TryGetClassImplementingOrExtendingHeritageClauseElement" => {
            node_map("source", input, all, |parsed, view, node| {
                match class::try_get_class_implementing_or_extending_heritage_clause_element(
                    view, node,
                )
                .map_err(text)?
                {
                    (Some(class), is_implements) => {
                        Ok(json!([parsed.node_ref(Some(class))?, is_implements]))
                    }
                    (None, _) => Ok(Value::Null),
                }
            })
        }
        "ast.ReplaceModifiers" => replace_modifiers(input),
        _ => return None,
    })
}

/// Go's `legacyBits`: bit 0 without, bit 1 with legacy decorators.
fn legacy_bits(f: impl Fn(bool) -> Result<bool, Error>) -> Result<Value, String> {
    let mut bits = 0;
    if f(false).map_err(text)? {
        bits |= 1;
    }
    if f(true).map_err(text)? {
        bits |= 2;
    }
    Ok(int(bits))
}

/// Go's `parentOf` and `grandparentOf`.
fn parents(view: AstView<'_>, node: NodeId) -> Result<(Option<NodeId>, Option<NodeId>), Error> {
    let parent = view.node(node)?.parent();
    let grandparent = match parent {
        Some(parent) => view.node(parent)?.parent(),
        None => None,
    };
    Ok((parent, grandparent))
}

/// Go's `accessorRefs`.
fn accessor_refs(parsed: &Parsed, all: AllAccessorDeclarations) -> Result<Value, String> {
    Ok(json!([
        ref_of(parsed, all.first_accessor)?,
        ref_of(parsed, all.second_accessor)?,
        ref_of(parsed, all.set_accessor)?,
        ref_of(parsed, all.get_accessor)?,
    ]))
}

fn not_source_file(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    Ok(view.node(node)?.kind() != K::SourceFile)
}

fn class_kinds(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(view, node, &[K::ClassDeclaration, K::ClassExpression])
}

fn accessor_kinds(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(view, node, &[K::GetAccessor, K::SetAccessor])
}

fn heritage_element_kinds(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[K::ExpressionWithTypeArguments, K::TypeReference],
    )
}

/// Go's `memberListKinds`.
fn member_list_kinds(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[
            K::ClassDeclaration,
            K::ClassExpression,
            K::InterfaceDeclaration,
            K::EnumDeclaration,
            K::TypeLiteral,
            K::MappedType,
        ],
    )
}

/// Go's `classElementInClass`.
fn class_element_in_class(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    let Some(parent) = read.parent() else {
        return Ok(false);
    };
    Ok(tsr_ast::utilities::is_class_element(&read)
        && tsr_ast::utilities::is_class_like(&view.node(parent)?))
}

/// Go's `accessorInMemberList`.
fn accessor_in_member_list(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    let read = view.node(node)?;
    let Some(parent) = read.parent() else {
        return Ok(false);
    };
    Ok(tsr_ast::utilities::is_accessor(&read) && member_list_kinds(view, parent)?)
}

/// Go's `functionLikeData`: the kinds whose data embeds FunctionLikeBase.
fn function_like_data(view: AstView<'_>, node: NodeId) -> Result<bool, Error> {
    is_kind(
        view,
        node,
        &[
            K::GetAccessor,
            K::SetAccessor,
            K::ArrowFunction,
            K::CallSignature,
            K::ConstructSignature,
            K::Constructor,
            K::ConstructorType,
            K::FunctionDeclaration,
            K::FunctionExpression,
            K::FunctionType,
            K::IndexSignature,
            K::JSDocSignature,
            K::MethodDeclaration,
            K::MethodSignature,
        ],
    )
}

/// Go's `replaceModifiersKinds`.
const REPLACE_MODIFIERS_KINDS: &[K] = &[
    K::TypeParameter,
    K::Parameter,
    K::ConstructorType,
    K::PropertySignature,
    K::PropertyDeclaration,
    K::MethodSignature,
    K::MethodDeclaration,
    K::Constructor,
    K::GetAccessor,
    K::SetAccessor,
    K::IndexSignature,
    K::FunctionExpression,
    K::ArrowFunction,
    K::ClassExpression,
    K::VariableStatement,
    K::FunctionDeclaration,
    K::ClassDeclaration,
    K::InterfaceDeclaration,
    K::TypeAliasDeclaration,
    K::EnumDeclaration,
    K::ModuleDeclaration,
    K::ImportEqualsDeclaration,
    K::ImportDeclaration,
    K::ExportAssignment,
    K::ExportDeclaration,
];

/// Go's `ast.ReplaceModifiers` column: `[index, [with nil modifiers, with
/// the node's own]]`, each result projected by [`replaced`]. The factory is
/// the input's own builder; Go's is a fresh `NodeFactory`, and a created node
/// is projected by its fields in both.
fn replace_modifiers(input: &Value) -> Result<Column, String> {
    let mut parsed = parse_source(input)?;
    Ok(Box::new(move || {
        let mut out = Vec::new();
        for at in 0..parsed.nodes.len() {
            let node = parsed.nodes[at];
            let modifiers = {
                let view = parsed.view();
                if !is_kind(view, node, REPLACE_MODIFIERS_KINDS).map_err(text)? {
                    continue;
                }
                view.node(node).map_err(text)?.modifiers()
            };
            let without = class::replace_modifiers(parsed.builder_mut()?, node, None);
            let with = class::replace_modifiers(parsed.builder_mut()?, node, modifiers);
            out.push(json!([
                at,
                [
                    replaced(&parsed, node, without)?,
                    replaced(&parsed, node, with)?
                ]
            ]));
        }
        Ok(Value::Array(out))
    }))
}

/// Go's `replaced`.
fn replaced(parsed: &Parsed, node: NodeId, result: NodeId) -> Result<Value, String> {
    if result == node {
        return Ok(json!(true));
    }
    let view = parsed.view();
    let kind = view.node(result).map_err(text)?.kind().raw();
    let refs = children(view, result)?
        .into_iter()
        .map(|child| parsed.node_ref(Some(child)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!([kind, refs]))
}

/// Go's `hasModifierMasks`: each syntactic and JSDoc-only flag alone, then
/// three unions.
fn has_modifier_masks() -> Vec<u32> {
    use tsr_ast::modifier_flags as m;
    let mut masks: Vec<u32> = (0..=16).map(|bit| 1 << bit).collect();
    masks.extend([
        m::EXPORT | m::DEFAULT,
        m::PUBLIC | m::PRIVATE | m::PROTECTED,
        m::SYNTACTIC_MODIFIERS,
    ]);
    masks
}
