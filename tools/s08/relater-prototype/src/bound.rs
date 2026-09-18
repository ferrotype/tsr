//! Type construction from bound syntax for the reference implementation.
//!
//! This layer consumes no production-checker types. Node/symbol links and all
//! instantiated types belong to this checker. A member table contains lazy
//! property-type links, so looking up the two roots does not resolve their graph.

use crate::bound_input::{BoundInput, InputError, SymbolGroup};
use crate::{
    flags as tf, object_flags as of, Checker, DiagnosticLocation, Error, IndexInfo, LiteralValue,
    Member, Signature, Structure, TypeCell, TypeLink,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, NodeListId, SyntaxKind as K};

mod compound_source;
mod conditional_source;
mod generic_source;
mod initialize;
mod instantiate;
mod mapped_source;
mod signature_source;
#[cfg(test)]
mod tests;
mod value_source;

impl From<InputError> for Error {
    fn from(error: InputError) -> Self {
        Self::Unsupported(error.to_string().into())
    }
}

impl From<ts_arena::Error> for Error {
    fn from(error: ts_arena::Error) -> Self {
        Self::Unsupported(format!("bound AST: {error:?}").into())
    }
}

fn missing<T>(what: &str) -> Result<T, Error> {
    Err(Error::Unsupported(
        format!("bound type construction: {what}").into(),
    ))
}

#[derive(Clone, Default)]
struct Environment(
    Vec<(NodeId, Weak<TypeCell>)>,
    Option<instantiate::Mapper>,
    Vec<(Weak<TypeCell>, Weak<TypeCell>)>,
);

impl Environment {
    fn key(&self) -> Result<EnvironmentKey, Error> {
        let nodes = self
            .0
            .iter()
            .map(|(node, ty)| Ok((*node, ty.upgrade().ok_or(Error::Released)?.id())))
            .collect::<Result<_, Error>>()?;
        let synthetic = if let Some(mapper) = &self.1 {
            mapper.synthetic_key()?
        } else {
            self.2
                .iter()
                .map(|(source, target)| {
                    Ok((
                        source.upgrade().ok_or(Error::Released)?.id(),
                        target.upgrade().ok_or(Error::Released)?.id(),
                    ))
                })
                .collect::<Result<_, Error>>()?
        };
        Ok((nodes, synthetic))
    }
    fn get(&self, node: NodeId) -> Result<Option<Rc<TypeCell>>, Error> {
        self.0
            .iter()
            .rev()
            .find(|(n, _)| *n == node)
            .map(|(_, ty)| ty.upgrade().ok_or(Error::Released))
            .transpose()
    }
}

type EnvironmentKey = (Vec<(NodeId, u32)>, Vec<(u32, u32)>);
type NodeKey = (NodeId, EnvironmentKey);

/// The reference checker and its bound input have one explicit lifetime owner.
/// Type-cell resolvers capture weak references to this owner, never a strong
/// edge back from the graph into itself.
pub struct BoundChecker {
    state: Rc<Construction>,
}

struct Construction {
    input: BoundInput,
    checker: Checker,
    initialization: initialize::Initialization,
    builtins: RefCell<HashMap<u32, Rc<TypeCell>>>,
    node_types: RefCell<HashMap<NodeKey, Rc<TypeCell>>>,
    declared_types: RefCell<HashMap<Vec<SymbolId>, Rc<TypeCell>>>,
    resolving_aliases: RefCell<Vec<Vec<SymbolId>>>,
    instantiated_types: RefCell<HashMap<NodeKey, Rc<TypeCell>>>,
    source_signatures: RefCell<HashMap<NodeKey, Signature>>,
    identities: RefCell<HashMap<NodeId, u64>>,
    value_types: RefCell<HashMap<(Vec<SymbolId>, EnvironmentKey), Rc<TypeCell>>>,
    return_types: RefCell<HashMap<NodeKey, Rc<TypeCell>>>,
    resolving_returns: RefCell<Vec<NodeKey>>,
    interface_this: RefCell<HashMap<u32, Rc<TypeCell>>>,
    tuple_targets: RefCell<HashMap<compound_source::TupleTargetKey, Rc<TypeCell>>>,
    tuple_instances: RefCell<HashMap<(u32, Vec<u32>), Rc<TypeCell>>>,
    tuple_bases: RefCell<HashMap<u32, TypeLink>>,
    interface_instances: RefCell<HashMap<(u32, Vec<u32>), Rc<TypeCell>>>,
    instantiation: instantiate::State,
    mapped_state: mapped_source::State,
    conditional: conditional_source::State,
    current_node: Cell<Option<NodeId>>,
    signatures: Cell<usize>,
    instantiations: Cell<usize>,
}

impl BoundChecker {
    pub fn new(input: BoundInput) -> Result<Self, Error> {
        let checker = Checker::new();
        let initialization = initialize::intrinsics(&checker, input.options())?;
        let builtins = initialization.primitives.clone();
        let signatures = initialization.signatures.len();
        let state = Rc::new(Construction {
            input,
            checker,
            initialization,
            builtins: RefCell::new(builtins),
            node_types: RefCell::new(HashMap::new()),
            declared_types: RefCell::new(HashMap::new()),
            resolving_aliases: RefCell::new(Vec::new()),
            instantiated_types: RefCell::new(HashMap::new()),
            source_signatures: RefCell::new(HashMap::new()),
            identities: RefCell::new(HashMap::new()),
            value_types: RefCell::new(HashMap::new()),
            return_types: RefCell::new(HashMap::new()),
            resolving_returns: RefCell::new(Vec::new()),
            interface_this: RefCell::new(HashMap::new()),
            tuple_targets: RefCell::new(HashMap::new()),
            tuple_instances: RefCell::new(HashMap::new()),
            tuple_bases: RefCell::new(HashMap::new()),
            interface_instances: RefCell::new(HashMap::new()),
            instantiation: instantiate::State::default(),
            mapped_state: mapped_source::State::default(),
            conditional: conditional_source::State::default(),
            current_node: Cell::new(None),
            signatures: Cell::new(signatures),
            instantiations: Cell::new(0),
        });
        state.initialize_globals()?;
        Ok(Self { state })
    }

    pub fn checker(&self) -> &Checker {
        &self.state.checker
    }
    pub fn input(&self) -> &BoundInput {
        &self.state.input
    }
    pub fn signatures_created(&self) -> usize {
        self.state.signatures.get()
    }
    pub fn instantiations(&self) -> usize {
        self.state.instantiations.get()
    }

    pub fn declared_type(&self, declaration: NodeId) -> Result<Rc<TypeCell>, Error> {
        self.state.reset_instantiation_count();
        let name = self
            .state
            .input
            .node(declaration)?
            .name()
            .ok_or_else(|| Error::Unsupported("declaration has no name".into()))?;
        let group = self
            .state
            .input
            .resolve_type_name(name)?
            .ok_or_else(|| Error::Unsupported("declaration has no bound symbol".into()))?;
        self.state.declared(&group)
    }

    pub fn declaration_location(&self, declaration: NodeId) -> Result<DiagnosticLocation, Error> {
        self.state.location(declaration)
    }
}

impl Construction {
    fn outer_environment(self: &Rc<Self>, node: NodeId) -> Result<Environment, Error> {
        let mut ancestors = Vec::new();
        let mut parent = self.input.node(node)?.parent();
        while let Some(node) = parent {
            ancestors.push(node);
            parent = self.input.node(node)?.parent();
        }
        let mut result = Environment::default();
        for node in ancestors.into_iter().rev() {
            let read = self.input.node(node)?;
            if matches!(
                read.kind().known(),
                Some(
                    K::InterfaceDeclaration
                        | K::ClassDeclaration
                        | K::TypeAliasDeclaration
                        | K::JSTypeAliasDeclaration
                        | K::FunctionDeclaration
                        | K::FunctionExpression
                        | K::ArrowFunction
                        | K::MethodSignature
                        | K::MethodDeclaration
                        | K::CallSignature
                        | K::ConstructSignature
                        | K::FunctionType
                        | K::ConstructorType
                )
            ) {
                for parameter in self.list(node, read.type_parameter_list())? {
                    result
                        .0
                        .push((parameter, Rc::downgrade(&self.type_parameter(parameter)?)));
                }
            }
            if read.kind() == K::InterfaceDeclaration {
                let group = self
                    .input
                    .resolve_type_name(read.name().ok_or(Error::ResolutionFailed)?)?
                    .ok_or(Error::ResolutionFailed)?;
                let target = self.declared(&group)?;
                if let Some(this) = self.interface_this.borrow().get(&target.id()) {
                    self.record_type_parameter(this, node);
                    result.2.push((Rc::downgrade(this), Rc::downgrade(this)));
                }
            }
            if let Some(data) = read.data_source().as_mapped_type_node() {
                let parameter = data.type_parameter().ok_or(Error::ResolutionFailed)?;
                result
                    .0
                    .push((parameter, Rc::downgrade(&self.type_parameter(parameter)?)));
            }
        }
        Ok(result)
    }

    fn alias_metadata(
        self: &Rc<Self>,
        node: NodeId,
        env: &Environment,
        name: Option<Rc<str>>,
    ) -> Result<Option<instantiate::Alias>, Error> {
        let mut parent = self.input.node(node)?.parent();
        while let Some(node) = parent {
            let read = self.input.node(node)?;
            if read.kind() == K::ParenthesizedType
                || read
                    .data_source()
                    .as_type_operator_node()
                    .is_some_and(|data| data.operator() == K::ReadonlyKeyword)
            {
                parent = read.parent();
            } else {
                break;
            }
        }
        let Some(parent) = parent else {
            return Ok(None);
        };
        let read = self.input.node(parent)?;
        if !matches!(
            read.kind().known(),
            Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration)
        ) {
            return Ok(None);
        }
        let name = if let Some(name) = name {
            name
        } else {
            self.name(read.name().ok_or(Error::ResolutionFailed)?)?
        };
        let mut arguments = Vec::new();
        for parameter in self.list(parent, read.type_parameter_list())? {
            let ty = if let Some(ty) = env.get(parameter)? {
                ty
            } else {
                self.type_parameter(parameter)?
            };
            arguments.push(Rc::downgrade(&ty));
        }
        Ok(Some(instantiate::Alias {
            declaration: parent,
            name,
            arguments,
        }))
    }

    fn instantiate_source(
        self: &Rc<Self>,
        ty: &Rc<TypeCell>,
        source: &instantiate::TypeSource,
        env: Environment,
        alias: Option<instantiate::Alias>,
    ) -> Result<Rc<TypeCell>, Error> {
        // Native instantiated objects have a target/argument/alias cache, not
        // type-node links keyed only by syntax and ordinary parameters. Do not
        // reuse the syntax worker's cache across distinct aliases or this args.
        let read = self.input.node(source.node)?;
        let name = alias
            .as_ref()
            .map_or_else(|| ty.name.clone(), |alias| alias.name.clone());
        let result = match read.kind().known() {
            Some(
                K::TypeLiteral
                | K::FunctionType
                | K::ConstructorType
                | K::MethodSignature
                | K::MethodDeclaration,
            ) => self.instantiated_anonymous(
                ty,
                env.1.as_ref().ok_or(Error::ResolutionFailed)?.clone(),
                name,
                alias.as_ref().map(|alias| self.identity(alias.declaration)),
            )?,
            Some(K::MappedType) => self.mapped_with_alias(source.node, &env, alias.clone())?,
            Some(K::TypeReference) => {
                self.source_reference_with_alias(source.node, &env, alias.clone())?
            }
            Some(K::TupleType) => self.tuple_with_alias(
                source.node,
                &env,
                ty.is_readonly_array_or_tuple(),
                alias.clone(),
            )?,
            Some(K::TypeOperator)
                if read
                    .data_source()
                    .as_type_operator_node()
                    .is_some_and(|operator| operator.operator() == K::ReadonlyKeyword) =>
            {
                let operand = read.type_node().ok_or(Error::ResolutionFailed)?;
                if self.input.node(operand)?.kind() != K::TupleType {
                    return missing("deferred readonly non-tuple source");
                }
                self.tuple_with_alias(operand, &env, true, alias.clone())?
            }
            _ => return missing("source-backed instantiation constructor"),
        };
        self.record_type_source(
            &result,
            instantiate::TypeSource {
                node: source.node,
                environment: env,
                alias,
            },
        );
        Ok(result)
    }

    fn instantiation_limit(&self, _: &TypeCell) -> Result<Rc<TypeCell>, Error> {
        let location = self
            .current_node
            .get()
            .map(|node| self.node_location(node))
            .transpose()?
            .unwrap_or_default();
        self.checker
            .diagnostics
            .borrow_mut()
            .push(crate::Diagnostic {
                location,
                message:
                    ts_diagnostics::Type_instantiation_is_excessively_deep_and_possibly_infinite,
                args: Vec::new(),
                chain: Vec::new(),
                related: Vec::new(),
            });
        self.initialization
            .named
            .borrow()
            .get("errorType")
            .cloned()
            .ok_or(Error::ResolutionFailed)
    }

    fn identity(&self, node: NodeId) -> u64 {
        let mut identities = self.identities.borrow_mut();
        let next = identities.len() as u64 + 1;
        *identities.entry(node).or_insert(next)
    }
    fn builtin(&self, flags: u32) -> Result<Rc<TypeCell>, Error> {
        self.builtins
            .borrow()
            .get(&flags)
            .cloned()
            .ok_or_else(|| Error::Unsupported(format!("uninitialized intrinsic {flags}").into()))
    }

    fn node_location(&self, node: NodeId) -> Result<DiagnosticLocation, Error> {
        let read = self.input.node(node)?;
        let source = self.input.source_file(node)?;
        let file = String::from_utf8(source.file_name().to_vec())
            .map_err(|_| Error::Unsupported("non-UTF8 diagnostic file name".into()))?;
        let pos = ts_scanner::skip_trivia(source.text().as_bytes(), i64::from(read.pos()));
        Ok(DiagnosticLocation {
            file: Some(file),
            pos: pos as isize,
            end: read.end() as isize,
        })
    }

    fn location(&self, declaration: NodeId) -> Result<DiagnosticLocation, Error> {
        let (pos, end) = self.input.declaration_name_span(declaration)?;
        let source = self.input.source_file(declaration)?;
        let file = String::from_utf8(source.file_name().to_vec())
            .map_err(|_| Error::Unsupported("non-UTF8 diagnostic file name".into()))?;
        Ok(DiagnosticLocation {
            file: Some(file),
            pos: pos as isize,
            end: end as isize,
        })
    }

    fn text(&self, node: NodeId) -> Result<Vec<u8>, Error> {
        Ok(self.input.ast(node)?.node_text(node)?.as_bytes().to_vec())
    }

    fn name(&self, node: NodeId) -> Result<Rc<str>, Error> {
        if !matches!(
            self.input.node(node)?.kind().known(),
            Some(K::Identifier | K::StringLiteral | K::NumericLiteral | K::PrivateIdentifier)
        ) {
            return missing(&format!(
                "name syntax {}",
                self.input.node(node)?.kind_string()
            ));
        }
        let bytes = self.text(node)?;
        String::from_utf8(bytes)
            .map(Rc::from)
            .map_err(|_| Error::Unsupported("non-UTF8 symbol display name".into()))
    }

    fn list(&self, owner: NodeId, list: Option<NodeListId>) -> Result<Vec<NodeId>, Error> {
        let Some(list) = list else {
            return Ok(Vec::new());
        };
        let ast = self.input.ast(owner)?;
        ast.node_slice(ast.list(list)?.nodes())?
            .iter()
            .map(|node| node.ok_or_else(|| Error::Unsupported("nil syntax-list entry".into())))
            .collect()
    }

    fn declarations(&self, group: &SymbolGroup) -> Result<Vec<NodeId>, Error> {
        let mut declarations = Vec::new();
        for &symbol in &group.symbols {
            declarations.extend(self.input.declarations(symbol)?.iter().flatten());
        }
        Ok(declarations)
    }

    // port: tsc/internal/checker/checker.go:Checker.getDeclaredTypeOfSymbol
    fn declared(self: &Rc<Self>, group: &SymbolGroup) -> Result<Rc<TypeCell>, Error> {
        if let Some(ty) = self.declared_types.borrow().get(&group.symbols).cloned() {
            return Ok(ty);
        }
        let declarations = self
            .declarations(group)?
            .into_iter()
            .filter(|&node| {
                self.input.node(node).is_ok_and(|read| {
                    matches!(
                        read.kind().known(),
                        Some(
                            K::TypeAliasDeclaration
                                | K::JSTypeAliasDeclaration
                                | K::InterfaceDeclaration
                                | K::TypeParameter
                        )
                    )
                })
            })
            .collect::<Vec<_>>();
        let &first = declarations
            .first()
            .ok_or_else(|| Error::Unsupported("symbol has no declarations".into()))?;
        let read = self.input.node(first)?;
        let name = self.name(
            read.name()
                .ok_or_else(|| Error::Unsupported("type declaration name".into()))?,
        )?;
        let ty = match read.kind().known() {
            Some(K::TypeAliasDeclaration | K::JSTypeAliasDeclaration) => {
                if self.resolving_aliases.borrow().contains(&group.symbols) {
                    // The declaration-resolution stack records a cycle, not a
                    // structurally recursive object. Never expand it forever.
                    return self
                        .initialization
                        .named
                        .borrow()
                        .get("errorType")
                        .cloned()
                        .ok_or(Error::ResolutionFailed);
                }
                self.resolving_aliases
                    .borrow_mut()
                    .push(group.symbols.clone());
                let result = (|| {
                    let environment = self.parameters(first, &Environment::default())?.0;
                    let annotation = read
                        .type_node()
                        .ok_or_else(|| Error::Unsupported("type alias annotation".into()))?;
                    self.type_node(annotation, &environment, Some(name))
                })();
                self.resolving_aliases.borrow_mut().pop();
                result?
            }
            Some(K::InterfaceDeclaration) => {
                let (environment, parameters) = self.parameters(first, &Environment::default())?;
                let has_this = !parameters.is_empty() || self.interface_has_this(&declarations)?;
                let target = self.object(
                    declarations.clone(),
                    environment,
                    name,
                    of::INTERFACE | if has_this { of::REFERENCE } else { 0 },
                )?;
                // Publish the target before resolving any bases or constraints.
                self.declared_types
                    .borrow_mut()
                    .insert(group.symbols.clone(), target.clone());
                if has_this {
                    let this = self.checker.graph.type_parameter("this", Some(&target));
                    self.interface_this.borrow_mut().insert(target.id(), this);
                }
                if !parameters.is_empty() {
                    let weak = Rc::downgrade(self);
                    let symbols = group.symbols.clone();
                    target.set_generic_target(&parameters, move |checker, target, args| {
                        let state = weak.upgrade().ok_or(Error::Released)?;
                        if !std::ptr::eq(checker, &state.checker) {
                            return missing("generic target used by another checker");
                        }
                        state.instantiate_interface(
                            &SymbolGroup {
                                symbols: symbols.clone(),
                            },
                            target,
                            args,
                        )
                    })?;
                    target.set_reference_shape(&target, &parameters)?;
                }
                target
            }
            Some(K::TypeParameter) => self.type_parameter(first)?,
            _ => return missing(&format!("declared type of {}", read.kind_string())),
        };
        self.declared_types
            .borrow_mut()
            .insert(group.symbols.clone(), ty.clone());
        Ok(ty)
    }

    // port: tsc/internal/checker/checker.go:Checker.getTypeFromTypeNodeWorker
    fn type_node(
        self: &Rc<Self>,
        node: NodeId,
        env: &Environment,
        alias: Option<Rc<str>>,
    ) -> Result<Rc<TypeCell>, Error> {
        struct CurrentNode<'a>(&'a Cell<Option<NodeId>>, Option<NodeId>);
        impl Drop for CurrentNode<'_> {
            fn drop(&mut self) {
                self.0.set(self.1)
            }
        }
        let _current = CurrentNode(&self.current_node, self.current_node.replace(Some(node)));
        if let Some(mapper) = &env.1 {
            let source = self.type_node_worker(node, &Environment::default(), None)?;
            let alias = self.alias_metadata(node, env, alias)?;
            return self.instantiate(&source, mapper, alias);
        }
        self.type_node_worker(node, env, alias)
    }

    fn type_node_worker(
        self: &Rc<Self>,
        node: NodeId,
        env: &Environment,
        alias: Option<Rc<str>>,
    ) -> Result<Rc<TypeCell>, Error> {
        let read = self.input.node(node)?;
        let intrinsic = match read.kind().known() {
            Some(K::AnyKeyword) => Some(tf::ANY),
            Some(K::UnknownKeyword) => Some(tf::UNKNOWN),
            Some(K::StringKeyword) => Some(tf::STRING),
            Some(K::NumberKeyword) => Some(tf::NUMBER),
            Some(K::BigIntKeyword) => Some(tf::BIG_INT),
            Some(K::SymbolKeyword) => Some(tf::ES_SYMBOL),
            Some(K::VoidKeyword) => Some(tf::VOID),
            Some(K::UndefinedKeyword) => Some(tf::UNDEFINED),
            Some(K::NullKeyword) => Some(tf::NULL),
            Some(K::NeverKeyword) => Some(tf::NEVER),
            Some(K::ObjectKeyword) => Some(tf::NON_PRIMITIVE),
            _ => None,
        };
        if let Some(flag) = intrinsic {
            return self.builtin(flag);
        }
        let key = (
            node,
            if env.1.is_none() {
                EnvironmentKey::default()
            } else {
                env.key()?
            },
        );
        if let Some(ty) = self.node_types.borrow().get(&key).cloned() {
            return Ok(ty);
        }
        let source_alias = self.alias_metadata(node, env, alias.clone())?;
        let alias = alias.or_else(|| source_alias.as_ref().map(|alias| alias.name.clone()));
        let before = self.checker.graph.len();
        let ty = match read.kind().known() {
            Some(K::BooleanKeyword) => {
                let a = self.checker.graph.intern_literal(
                    tf::BOOLEAN_LITERAL,
                    LiteralValue::Boolean(false),
                    "false",
                );
                let b = self.checker.graph.intern_literal(
                    tf::BOOLEAN_LITERAL,
                    LiteralValue::Boolean(true),
                    "true",
                );
                self.checker.graph.union(&[a, b])?
            }
            Some(K::ParenthesizedType) => self.type_node(
                read.type_node()
                    .ok_or_else(|| Error::Unsupported("parenthesized operand".into()))?,
                env,
                alias,
            )?,
            Some(K::LiteralType) => {
                let literal = read
                    .data_source()
                    .as_literal_type_node()
                    .and_then(|data| data.literal())
                    .ok_or_else(|| Error::Unsupported("literal operand".into()))?;
                self.literal(literal)?
            }
            Some(
                K::TypeLiteral
                | K::FunctionType
                | K::ConstructorType
                | K::MethodSignature
                | K::MethodDeclaration,
            ) => {
                let declarations = if matches!(
                    read.kind().known(),
                    Some(K::MethodSignature | K::MethodDeclaration)
                ) {
                    if let Some(symbol) =
                        self.input.binding(node)?.and_then(|binding| binding.symbol)
                    {
                        self.declarations(&SymbolGroup {
                            symbols: vec![symbol],
                        })?
                    } else {
                        vec![node]
                    }
                } else {
                    vec![node]
                };
                self.object_with_alias(
                    declarations,
                    env.clone(),
                    alias.unwrap_or_else(|| Rc::from("__type")),
                    of::ANONYMOUS | if env.1.is_some() { of::INSTANTIATED } else { 0 },
                    source_alias
                        .as_ref()
                        .map(|alias| self.identity(alias.declaration)),
                )?
            }
            Some(K::TypeReference) => self.source_reference(node, env, alias)?,
            Some(K::UnionType | K::IntersectionType) => {
                let data = read.data_source();
                let list = if let Some(data) = data.as_union_type_node() {
                    data.types()
                } else {
                    data.as_intersection_type_node()
                        .and_then(|data| data.types())
                };
                let types = self
                    .list(node, list)?
                    .into_iter()
                    .map(|node| self.type_node(node, env, None))
                    .collect::<Result<Vec<_>, _>>()?;
                if read.kind() == K::UnionType {
                    if let Some(named) = &source_alias {
                        let arguments = named
                            .arguments
                            .iter()
                            .map(|ty| ty.upgrade().map(|ty| ty.id()).ok_or(Error::Released))
                            .collect::<Result<Vec<_>, _>>()?;
                        self.checker.graph.union_named_arguments(
                            &types,
                            self.identity(named.declaration),
                            &named.name,
                            &arguments,
                        )?
                    } else if let Some(name) = alias {
                        self.checker
                            .graph
                            .union_named(&types, self.identity(node), &name)?
                    } else {
                        self.checker.graph.union(&types)?
                    }
                } else {
                    self.checker.graph.allocate_full(
                        tf::INTERSECTION,
                        0,
                        alias.unwrap_or_else(|| Rc::from("intersection")),
                        None,
                        source_alias
                            .as_ref()
                            .map(|alias| self.identity(alias.declaration)),
                        None,
                        false,
                        types.iter().map(Rc::downgrade).collect(),
                        false,
                        None,
                    )
                }
            }
            Some(K::ArrayType) => {
                let element = read
                    .data_source()
                    .as_array_type_node()
                    .and_then(|data| data.element_type())
                    .ok_or(Error::ResolutionFailed)?;
                self.array(node, self.type_node(element, env, None)?, false)?
            }
            Some(K::TupleType) => self.tuple(node, env, false, alias)?,
            Some(K::TypeOperator) => {
                let data = read.data_source();
                let data = data
                    .as_type_operator_node()
                    .ok_or(Error::ResolutionFailed)?;
                let operand = data.r#type().ok_or(Error::ResolutionFailed)?;
                if data.operator() == K::ReadonlyKeyword {
                    let read = self.input.node(operand)?;
                    if read.kind() == K::TupleType {
                        self.tuple(operand, env, true, alias)?
                    } else if let Some(data) = read.data_source().as_array_type_node() {
                        self.array(
                            node,
                            self.type_node(
                                data.element_type().ok_or(Error::ResolutionFailed)?,
                                env,
                                None,
                            )?,
                            true,
                        )?
                    } else {
                        return missing("readonly operand");
                    }
                } else if data.operator() == K::KeyOfKeyword {
                    self.keyof(node, self.type_node(operand, env, None)?)?
                } else if data.operator() == K::UniqueKeyword {
                    self.unique_symbol_type(node)?
                } else {
                    return missing("type operator");
                }
            }
            Some(K::MappedType) => self.mapped(node, env, alias)?,
            Some(K::ConditionalType) => self.conditional(node, env, alias)?,
            Some(K::InferType) => {
                let parameter = read
                    .data_source()
                    .as_infer_type_node()
                    .and_then(|data| data.type_parameter())
                    .ok_or(Error::ResolutionFailed)?;
                if let Some(ty) = env.get(parameter)? {
                    ty
                } else {
                    self.type_parameter(parameter)?
                }
            }
            Some(K::IndexedAccessType) => {
                let data = read.data_source();
                let data = data
                    .as_indexed_access_type_node()
                    .ok_or(Error::ResolutionFailed)?;
                let object = self.type_node(
                    data.object_type().ok_or(Error::ResolutionFailed)?,
                    env,
                    None,
                )?;
                let index =
                    self.type_node(data.index_type().ok_or(Error::ResolutionFailed)?, env, None)?;
                self.indexed_access(object, index)?
            }
            Some(K::TypeQuery) => self.type_query(node, env)?,
            Some(K::ThisType) => self.this_type(node)?,
            Some(K::TemplateLiteralType) => {
                let data = read.data_source();
                let data = data
                    .as_template_literal_type_node()
                    .ok_or(Error::ResolutionFailed)?;
                let mut texts = vec![self.text(data.head().ok_or(Error::ResolutionFailed)?)?];
                let mut types = Vec::new();
                for span in self.list(node, data.template_spans())? {
                    let read = self.input.node(span)?;
                    let data = read.data_source();
                    let data = data
                        .as_template_literal_type_span()
                        .ok_or(Error::ResolutionFailed)?;
                    types.push(self.type_node(
                        data.r#type().ok_or(Error::ResolutionFailed)?,
                        env,
                        None,
                    )?);
                    texts.push(self.text(data.literal().ok_or(Error::ResolutionFailed)?)?);
                }
                self.checker.graph.template_literal(&texts, &types)?
            }
            _ => return missing(&format!("type syntax {}", read.kind_string())),
        };
        // A use site can return an existing literal, reference or reduced
        // compound. It must not attach its alias/provenance to that shared type.
        let (owns_result, carries_alias) = match read.kind().known() {
            Some(
                K::TypeLiteral
                | K::FunctionType
                | K::ConstructorType
                | K::MethodSignature
                | K::MethodDeclaration,
            ) => (
                ty.flags & tf::OBJECT != 0 && ty.object_flags & of::ANONYMOUS != 0,
                true,
            ),
            Some(K::MappedType) => (
                ty.flags & tf::OBJECT != 0 && ty.object_flags & of::MAPPED != 0,
                true,
            ),
            Some(K::UnionType) => (ty.flags & tf::UNION != 0, true),
            Some(K::IntersectionType) => (ty.flags & tf::INTERSECTION != 0, true),
            Some(K::ConditionalType) => (ty.flags & tf::CONDITIONAL != 0, true),
            Some(K::IndexedAccessType) => (ty.flags & tf::INDEXED_ACCESS != 0, true),
            Some(K::TypeOperator) => {
                let deferred = ty
                    .reference_shape()
                    .is_some_and(|reference| reference.deferred_node().is_some());
                (ty.flags & tf::INDEX != 0 || deferred, deferred)
            }
            Some(K::TemplateLiteralType) => (ty.flags & tf::TEMPLATE_LITERAL != 0, false),
            Some(K::TypeReference | K::TupleType) => (
                ty.reference_shape()
                    .is_some_and(|reference| reference.deferred_node().is_some()),
                true,
            ),
            _ => (false, false),
        };
        if owns_result && ty.id() as usize > before {
            let environment = if env.1.is_none() {
                self.outer_environment(node)?
            } else {
                env.clone()
            };
            self.record_type_source(
                &ty,
                instantiate::TypeSource {
                    node,
                    environment,
                    alias: if carries_alias { source_alias } else { None },
                },
            );
        }
        self.node_types.borrow_mut().insert(key, ty.clone());
        Ok(ty)
    }

    fn literal(&self, node: NodeId) -> Result<Rc<TypeCell>, Error> {
        let read = self.input.node(node)?;
        let (flag, literal, name) = match read.kind().known() {
            Some(K::TrueKeyword | K::FalseKeyword) => {
                let value = read.kind() == K::TrueKeyword;
                (
                    tf::BOOLEAN_LITERAL,
                    LiteralValue::Boolean(value),
                    value.to_string(),
                )
            }
            Some(K::StringLiteral | K::NoSubstitutionTemplateLiteral) => {
                let bytes = self.text(node)?;
                let regular = self.checker.graph.string_literal(&bytes);
                // Literal type syntax checks the expression (creating its
                // fresh form) before returning the regular type. Retain both
                // actual cells even though this caller returns only regular.
                self.checker.graph.fresh_literal(&regular)?;
                return Ok(regular);
            }
            Some(K::NumericLiteral) => {
                let number = ts_jsnum::from_string(&self.text(node)?);
                (
                    tf::NUMBER_LITERAL,
                    LiteralValue::Number(number.value().to_bits()),
                    number.to_string(),
                )
            }
            Some(K::BigIntLiteral) => {
                let value = ts_jsnum::PseudoBigInt::new(
                    &ts_jsnum::parse_pseudo_big_int(&self.text(node)?),
                    false,
                );
                let name =
                    String::from_utf8(value.to_text()).map_err(|_| Error::ResolutionFailed)? + "n";
                (
                    tf::BIG_INT_LITERAL,
                    LiteralValue::BigInt {
                        negative: value.negative,
                        digits: value.base10_value,
                    },
                    name,
                )
            }
            Some(K::PrefixUnaryExpression) => {
                let data = read.data_source();
                let data = data
                    .as_prefix_unary_expression()
                    .ok_or(Error::ResolutionFailed)?;
                let operand = data.operand().ok_or(Error::ResolutionFailed)?;
                let negative = data.operator() == K::MinusToken;
                if data.operator() != K::MinusToken && data.operator() != K::PlusToken {
                    return missing("non-numeric prefix literal");
                }
                let inner = self.literal(operand)?;
                match inner.literal.as_ref() {
                    Some(LiteralValue::Number(bits)) => {
                        let number = ts_jsnum::Number::new(if negative {
                            -f64::from_bits(*bits)
                        } else {
                            f64::from_bits(*bits)
                        });
                        (
                            tf::NUMBER_LITERAL,
                            LiteralValue::Number(number.value().to_bits()),
                            number.to_string(),
                        )
                    }
                    Some(LiteralValue::BigInt {
                        negative: sign,
                        digits,
                    }) => {
                        let value = ts_jsnum::PseudoBigInt::new(digits, negative != *sign);
                        let name = String::from_utf8(value.to_text())
                            .map_err(|_| Error::ResolutionFailed)?
                            + "n";
                        (
                            tf::BIG_INT_LITERAL,
                            LiteralValue::BigInt {
                                negative: value.negative,
                                digits: value.base10_value,
                            },
                            name,
                        )
                    }
                    _ => return missing("prefix literal operand"),
                }
            }
            Some(K::NullKeyword) => return self.builtin(tf::NULL),
            _ => return missing(&format!("literal expression {}", read.kind_string())),
        };
        let regular = self.checker.graph.intern_literal(flag, literal, &name);
        self.checker.graph.fresh_literal(&regular)?;
        Ok(regular)
    }

    fn object(
        self: &Rc<Self>,
        declarations: Vec<NodeId>,
        env: Environment,
        name: Rc<str>,
        flags: u32,
    ) -> Result<Rc<TypeCell>, Error> {
        self.object_with_alias(declarations, env, name, flags, None)
    }

    fn object_with_alias(
        self: &Rc<Self>,
        declarations: Vec<NodeId>,
        env: Environment,
        name: Rc<str>,
        flags: u32,
        alias: Option<u64>,
    ) -> Result<Rc<TypeCell>, Error> {
        let identity = declarations.first().map(|&node| self.identity(node));
        let weak = Rc::downgrade(self);
        let resolver = Box::new(move |_: &crate::Graph, cell: &TypeCell| {
            weak.upgrade()
                .ok_or(Error::Released)?
                .members(cell, &declarations, &env)
        });
        Ok(self.checker.graph.allocate_full(
            tf::OBJECT,
            flags,
            name,
            identity,
            alias,
            None,
            false,
            Vec::new(),
            flags & of::ANONYMOUS != 0,
            Some(resolver),
        ))
    }

    // port: tsc/internal/checker/checker.go:Checker.resolveAnonymousTypeMembers
    fn members(
        self: &Rc<Self>,
        cell: &TypeCell,
        declarations: &[NodeId],
        env: &Environment,
    ) -> Result<Structure, Error> {
        let member_environment = self.reference_member_environment(cell, env)?;
        let env = &member_environment;
        let mut structure = Structure::default();
        let mut by_name: HashMap<Rc<str>, Vec<NodeId>> = HashMap::new();
        let mut name_types: HashMap<Rc<str>, TypeLink> = HashMap::new();
        let mut order = Vec::new();
        for &declaration in declarations {
            let read = self.input.node(declaration)?;
            if matches!(
                read.kind().known(),
                Some(
                    K::FunctionType
                        | K::ConstructorType
                        | K::MethodSignature
                        | K::FunctionDeclaration
                )
            ) {
                let signature = self.signature(declaration, env)?;
                if signature.is_construct {
                    structure.construct_signatures.push(signature)
                } else {
                    structure.call_signatures.push(signature)
                }
                continue;
            }
            if !matches!(
                read.kind().known(),
                Some(K::InterfaceDeclaration | K::TypeLiteral)
            ) {
                return missing(&format!("member container {}", read.kind_string()));
            }
            for node in self.list(declaration, read.member_list())? {
                let read = self.input.node(node)?;
                match read.kind().known() {
                    Some(K::CallSignature | K::ConstructSignature) => {
                        let signature = self.signature(node, env)?;
                        if signature.is_construct {
                            structure.construct_signatures.push(signature)
                        } else {
                            structure.call_signatures.push(signature)
                        }
                    }
                    Some(K::IndexSignature) => {
                        let parameters = self.list(node, read.parameter_list())?;
                        let parameter = *parameters
                            .first()
                            .ok_or_else(|| Error::Unsupported("index parameter".into()))?;
                        let key = self.type_node(
                            self.input
                                .node(parameter)?
                                .type_node()
                                .ok_or_else(|| Error::Unsupported("index key type".into()))?,
                            env,
                            None,
                        )?;
                        let value = self.type_node(
                            read.type_node()
                                .ok_or_else(|| Error::Unsupported("index value type".into()))?,
                            env,
                            None,
                        )?;
                        structure.index_infos.push(IndexInfo {
                            key: Rc::downgrade(&key),
                            value: Rc::downgrade(&value),
                            readonly: read.modifier_flags(self.input.ast(node)?)? & mf::READONLY
                                != 0,
                        });
                    }
                    Some(K::PropertySignature | K::MethodSignature) => {
                        let (name, name_type) = self.member_name(
                            read.name()
                                .ok_or_else(|| Error::Unsupported("member name".into()))?,
                            env,
                        )?;
                        if let Some(name_type) = name_type {
                            name_types.insert(name.clone(), name_type);
                        }
                        if !by_name.contains_key(&name) {
                            order.push(name.clone())
                        }
                        by_name.entry(name).or_default().push(node);
                    }
                    _ => return missing(&format!("object member {}", read.kind_string())),
                }
            }
        }
        for name in order {
            let declarations = by_name.remove(&name).ok_or(Error::ResolutionFailed)?;
            let first = declarations[0];
            let read = self.input.node(first)?;
            let optional = read.question_token(self.input.ast(first)?)?.is_some();
            let readonly = read.modifier_flags(self.input.ast(first)?)? & mf::READONLY != 0;
            self.checker
                .register_member_declaration(cell, name.clone(), self.location(first)?);
            let weak = Rc::downgrade(self);
            let environment = env.clone();
            let r#type = TypeLink::lazy(move || {
                let state = weak.upgrade().ok_or(Error::Released)?;
                let read = state.input.node(first)?;
                let mut ty = if read.kind() == K::MethodSignature {
                    state.type_node(first, &environment, None)?
                } else {
                    state.type_node(
                        read.type_node()
                            .ok_or_else(|| Error::Unsupported("property annotation".into()))?,
                        &environment,
                        None,
                    )?
                };
                if optional && state.input.options().strict_null_checks {
                    ty = state
                        .checker
                        .graph
                        .union(&[ty, state.builtin(tf::UNDEFINED)?])?;
                }
                Ok(ty)
            });
            let name_type = name_types.remove(&name);
            structure.members.push(Member {
                name_type,
                name,
                optional,
                readonly,
                class_member: true,
                r#type,
            });
        }
        for inherited in self.inherited_members(declarations, env)? {
            for member in inherited.members {
                if !structure.members.iter().any(|own| own.name == member.name) {
                    structure.members.push(member)
                }
            }
            structure.call_signatures.extend(inherited.call_signatures);
            structure
                .construct_signatures
                .extend(inherited.construct_signatures);
            for index in inherited.index_infos {
                let key = index.key()?;
                let mut found = false;
                for own in &structure.index_infos {
                    if Rc::ptr_eq(&key, &own.key()?) {
                        found = true;
                        break;
                    }
                }
                if !found {
                    structure.index_infos.push(index)
                }
            }
        }
        Ok(structure)
    }

    // port: tsc/internal/checker/checker.go:Checker.getSignatureFromDeclaration
    fn signature(self: &Rc<Self>, node: NodeId, env: &Environment) -> Result<Signature, Error> {
        let key = (node, env.key()?);
        if let Some(signature) = self.source_signatures.borrow().get(&key).cloned() {
            return Ok(signature);
        }
        let read = self.input.node(node)?;
        let parameter_nodes = self.list(node, read.type_parameter_list())?;
        let is_generic = parameter_nodes
            .iter()
            .any(|&parameter| env.get(parameter).is_ok_and(|ty| ty.is_none()));
        let (environment, type_parameters) = if is_generic {
            self.parameters(node, env)?
        } else {
            (env.clone(), Vec::new())
        };
        let nodes = self.list(node, read.parameter_list())?;
        let mut parameters = Vec::new();
        let mut parameter_names = Vec::new();
        let mut minimum = 0;
        let mut rest = false;
        let mut this_type = None;
        for parameter in nodes {
            let read = self.input.node(parameter)?;
            let name = self.name(
                read.name()
                    .ok_or_else(|| Error::Unsupported("parameter name".into()))?,
            )?;
            let annotation = read.type_node();
            let optional = read.question_token(self.input.ast(parameter)?)?.is_some();
            rest = read
                .data_source()
                .as_parameter_declaration()
                .and_then(|data| data.dot_dot_dot_token())
                .is_some();
            let weak = Rc::downgrade(self);
            let environment = environment.clone();
            let ty = TypeLink::lazy(move || {
                let state = weak.upgrade().ok_or(Error::Released)?;
                let mut ty = if let Some(annotation) = annotation {
                    state.type_node(annotation, &environment, None)?
                } else {
                    state.parameter_type_of_declaration(parameter, &environment)?
                };
                if optional && state.input.options().strict_null_checks {
                    ty = state
                        .checker
                        .graph
                        .union(&[ty, state.builtin(tf::UNDEFINED)?])?
                }
                Ok(ty)
            });
            if &*name == "this" {
                this_type = Some(ty);
                continue;
            }
            parameters.push(ty);
            parameter_names.push(name);
            if !optional && !rest && read.initializer().is_none() {
                minimum = parameters.len()
            }
        }
        let weak = Rc::downgrade(self);
        let annotation = read.type_node();
        let return_type = TypeLink::lazy(move || {
            let state = weak.upgrade().ok_or(Error::Released)?;
            if let Some(annotation) = annotation {
                state.type_node(annotation, &environment, None)
            } else {
                state.return_type_of_declaration(node, &environment)
            }
        });
        self.signatures.set(self.signatures.get() + 1);
        let mut signature = Signature {
            parameters,
            parameter_names,
            min_argument_count: minimum,
            has_rest_parameter: rest,
            type_parameters: type_parameters.len(),
            generic: None,
            this_type,
            return_type,
            bivariant_parameters: read.kind() == K::MethodSignature,
            is_abstract: read.modifier_flags(self.input.ast(node)?)? & mf::ABSTRACT != 0,
            is_construct: matches!(
                read.kind().known(),
                Some(K::ConstructorType | K::ConstructSignature)
            ),
        };
        self.attach_signature_factory(&mut signature, &type_parameters);
        self.source_signatures
            .borrow_mut()
            .insert(key, signature.clone());
        Ok(signature)
    }
}
