//! Symbol kind/modifiers shared by hover and later completion labels.
use crate::{LanguageService, Result};
use tsr_ast::{
    check_flags as cf, modifier_flags as mf, node_flags as nf, symbol_flags as sf,
    utilities as ast, utilities_positions as pos, NodeId, SyntaxKind as K,
};
use tsr_checker::{Operation, SignatureKind, SymbolRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptElementKind {
    Unknown,
    Warning,
    Keyword,
    ScriptElement,
    ModuleElement,
    ClassElement,
    LocalClassElement,
    InterfaceElement,
    TypeElement,
    EnumElement,
    EnumMemberElement,
    VariableElement,
    LocalVariableElement,
    VariableUsingElement,
    VariableAwaitUsingElement,
    FunctionElement,
    LocalFunctionElement,
    MemberFunctionElement,
    MemberGetAccessorElement,
    MemberSetAccessorElement,
    MemberVariableElement,
    MemberAccessorVariableElement,
    ConstructorImplementationElement,
    CallSignatureElement,
    IndexSignatureElement,
    ConstructSignatureElement,
    ParameterElement,
    TypeParameterElement,
    PrimitiveType,
    Label,
    Alias,
    ConstElement,
    LetElement,
    Directory,
    ExternalModuleName,
    String,
    Link,
    LinkName,
    LinkText,
}
pub mod modifiers {
    pub const PUBLIC: u32 = 1 << 1;
    pub const PRIVATE: u32 = 1 << 2;
    pub const PROTECTED: u32 = 1 << 3;
    pub const EXPORTED: u32 = 1 << 4;
    pub const AMBIENT: u32 = 1 << 5;
    pub const STATIC: u32 = 1 << 6;
    pub const ABSTRACT: u32 = 1 << 7;
    pub const OPTIONAL: u32 = 1 << 8;
    pub const DEPRECATED: u32 = 1 << 9;
}
impl LanguageService<'_> {
    // port: tsc/internal/ls/lsutil/symbol_display.go:GetSymbolKind
    pub fn symbol_kind(
        &self,
        checker: &mut Operation<'_>,
        symbol: SymbolRef,
        location: NodeId,
    ) -> Result<ScriptElementKind> {
        use ScriptElementKind as E;
        let roots = checker.get_root_symbols(symbol)?;
        if roots.len() == 1 && checker.symbol(roots[0])?.flags() & sf::METHOD != 0 {
            let ty = checker.get_type_of_symbol_at_location(symbol, Some(location))?;
            let ty = checker.get_non_nullable_type(ty)?;
            if !checker
                .get_signatures_of_type(ty, SignatureKind::Call)?
                .is_empty()
            {
                return Ok(E::MemberFunctionElement);
            }
        }
        if checker.is_undefined_symbol(symbol)? {
            return Ok(E::VariableElement);
        }
        if checker.is_arguments_symbol(symbol)? {
            return Ok(E::LocalVariableElement);
        }
        let view = self.view(location)?;
        if view.node(location)?.kind() == K::ThisKeyword && checker.is_expression_node(location)?
            || pos::is_this_in_type_query(view, location)?
        {
            return Ok(E::ParameterElement);
        }
        let mut flags = checker.symbol(symbol)?.flags();
        if let Some(export) = checker.symbol(symbol)?.export_symbol() {
            flags |= checker.symbol(checker.symbol_ref(export)?)?.flags();
        }
        let decls: Vec<_> = checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect();
        if flags & sf::VARIABLE != 0 {
            if let Some(&decl) = decls.first() {
                let mut node = Some(decl);
                while let Some(id) = node {
                    let read = checker.node(id)?;
                    if read.kind() == K::Parameter {
                        return Ok(E::ParameterElement);
                    }
                    if !matches!(
                        read.kind().known(),
                        Some(K::BindingElement | K::ObjectBindingPattern | K::ArrayBindingPattern)
                    ) {
                        break;
                    }
                    node = read.parent();
                }
            }
            if let Some(decl) = checker.symbol(symbol)?.value_declaration() {
                let combined =
                    ast::get_combined_node_flags(self.view(decl)?, decl)? & nf::BLOCK_SCOPED;
                match combined {
                    nf::CONST => return Ok(E::ConstElement),
                    nf::USING => return Ok(E::VariableUsingElement),
                    nf::AWAIT_USING => return Ok(E::VariableAwaitUsingElement),
                    _ => {}
                }
            }
            for &decl in &decls {
                if ast::get_combined_node_flags(self.view(decl)?, decl)? & nf::BLOCK_SCOPED
                    == nf::LET
                {
                    return Ok(E::LetElement);
                }
            }
            return Ok(if self.local_symbol(checker, symbol)? {
                E::LocalVariableElement
            } else {
                E::VariableElement
            });
        }
        if flags & sf::FUNCTION != 0 {
            return Ok(if self.local_symbol(checker, symbol)? {
                E::LocalFunctionElement
            } else {
                E::FunctionElement
            });
        }
        for (mask, kind) in [
            (sf::GET_ACCESSOR, E::MemberGetAccessorElement),
            (sf::SET_ACCESSOR, E::MemberSetAccessorElement),
            (sf::METHOD, E::MemberFunctionElement),
            (sf::CONSTRUCTOR, E::ConstructorImplementationElement),
            (sf::SIGNATURE, E::IndexSignatureElement),
        ] {
            if flags & mask != 0 {
                return Ok(kind);
            }
        }
        if flags & sf::PROPERTY != 0 {
            if flags & sf::TRANSIENT != 0
                && checker.symbol(symbol)?.check_flags() & cf::SYNTHETIC != 0
            {
                let any_non_method = roots.iter().any(|root| {
                    checker
                        .symbol(*root)
                        .is_ok_and(|s| s.flags() & (sf::PROPERTY_OR_ACCESSOR | sf::VARIABLE) != 0)
                });
                if !any_non_method {
                    let ty = checker.get_type_of_symbol_at_location(symbol, Some(location))?;
                    if !checker
                        .get_signatures_of_type(ty, SignatureKind::Call)?
                        .is_empty()
                    {
                        return Ok(E::MemberFunctionElement);
                    }
                }
            }
            return Ok(E::MemberVariableElement);
        }
        if flags & sf::CLASS != 0 {
            return Ok(
                if decls.iter().any(|d| {
                    checker
                        .node(*d)
                        .is_ok_and(|n| n.kind() == K::ClassExpression)
                }) {
                    E::LocalClassElement
                } else {
                    E::ClassElement
                },
            );
        }
        for (mask, kind) in [
            (sf::ENUM, E::EnumElement),
            (sf::TYPE_ALIAS, E::TypeElement),
            (sf::INTERFACE, E::InterfaceElement),
            (sf::TYPE_PARAMETER, E::TypeParameterElement),
            (sf::ENUM_MEMBER, E::EnumMemberElement),
            (sf::ALIAS, E::Alias),
            (sf::MODULE, E::ModuleElement),
        ] {
            if flags & mask != 0 {
                return Ok(kind);
            }
        }
        Ok(E::Unknown)
    }
    // port: tsc/internal/ls/lsutil/symbol_display.go:isLocalVariableOrFunction
    fn local_symbol(&self, checker: &Operation<'_>, symbol: SymbolRef) -> Result<bool> {
        if checker.symbol(symbol)?.parent().is_some() {
            return Ok(false);
        }
        for decl in checker.symbol_declarations(symbol)?.iter().flatten() {
            let read = checker.node(decl)?;
            if read.kind() == K::FunctionExpression {
                return Ok(true);
            }
            if !matches!(
                read.kind().known(),
                Some(K::VariableDeclaration | K::FunctionDeclaration)
            ) {
                continue;
            }
            let view = self.view(decl)?;
            let mut parent = read.parent();
            while let Some(node) = parent {
                if ast::is_function_block(view, Some(node))? {
                    return Ok(true);
                }
                let read = view.node(node)?;
                if matches!(read.kind().known(), Some(K::SourceFile | K::ModuleBlock)) {
                    break;
                }
                parent = read.parent();
            }
        }
        Ok(false)
    }
    // port: tsc/internal/ls/lsutil/symbol_display.go:GetSymbolModifiers
    pub fn symbol_modifiers(&self, checker: &mut Operation<'_>, symbol: SymbolRef) -> Result<u32> {
        let mut flags = self.normalized_modifiers(checker, symbol)?;
        if checker.symbol(symbol)?.flags() & sf::ALIAS != 0 {
            let target = checker.get_aliased_symbol(symbol)?;
            if target != symbol {
                flags |= self.normalized_modifiers(checker, target)?;
            }
        }
        if checker.symbol(symbol)?.flags() & sf::OPTIONAL != 0 {
            flags |= modifiers::OPTIONAL;
        }
        Ok(flags)
    }
    // port: tsc/internal/ls/lsutil/symbol_display.go:getNormalizedSymbolModifiers
    fn normalized_modifiers(&self, checker: &mut Operation<'_>, symbol: SymbolRef) -> Result<u32> {
        let decls: Vec<_> = checker
            .symbol_declarations(symbol)?
            .iter()
            .flatten()
            .collect();
        let Some(&first) = decls.first() else {
            return Ok(0);
        };
        let mut deprecated = checker.is_deprecated_declaration(first)?;
        for &d in &decls[1..] {
            if deprecated && !checker.is_deprecated_declaration(d)? {
                deprecated = false;
            }
        }
        let view = self.view(first)?;
        let read = view.node(first)?;
        let mut flags = if tsr_ast::is_declaration(&read) {
            ast::get_combined_modifier_flags(view, first)?
        } else {
            0
        };
        if deprecated {
            flags |= mf::DEPRECATED;
        } else {
            flags &= !mf::DEPRECATED;
        }
        let mut result = 0;
        for (node, symbol) in [
            (mf::PRIVATE, modifiers::PRIVATE),
            (mf::PROTECTED, modifiers::PROTECTED),
            (mf::PUBLIC, modifiers::PUBLIC),
            (mf::STATIC, modifiers::STATIC),
            (mf::ABSTRACT, modifiers::ABSTRACT),
            (mf::EXPORT, modifiers::EXPORTED),
            (mf::DEPRECATED, modifiers::DEPRECATED),
            (mf::AMBIENT, modifiers::AMBIENT),
        ] {
            if flags & node != 0 {
                result |= symbol;
            }
        }
        if read.flags() & nf::AMBIENT != 0 {
            result |= modifiers::AMBIENT;
        }
        if read.kind() == K::ExportAssignment {
            result |= modifiers::EXPORTED;
        }
        Ok(result)
    }
}
