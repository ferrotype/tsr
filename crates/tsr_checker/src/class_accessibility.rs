//! Constructor accessibility is checked against the lexical class and its
//! non-mixin bases, separately from structural signature compatibility.
use crate::{object_flags as of, type_flags as tf, CheckerState, Error, SignatureId, TypeId};
use ts_arena::{NodeId, SymbolId};
use ts_ast::{modifier_flags as mf, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/checker.go:Checker.getConstructorAccessibilityError
    pub(crate) fn constructor_accessibility_error(
        &mut self,
        node: NodeId,
        signatures: &[SignatureId],
        mask: u32,
    ) -> Result<Option<(u32, TypeId)>, Error> {
        for &signature in signatures {
            let Some(declaration) = self.signatures.get(signature)?.declaration else {
                continue;
            };
            let read = self.node(declaration)?;
            let modifiers = read.modifier_flags(self.ast(declaration)?)? & mask;
            if modifiers == 0 || read.kind() != K::Constructor {
                continue;
            }
            let parent = read
                .parent()
                .ok_or(Error::MissingLink("constructor declaration class"))?;
            let symbol = self
                .get_symbol_of_declaration(parent)?
                .ok_or(Error::MissingLink("constructor class symbol"))?;
            let declaring_class = self
                .class_declaration(symbol)?
                .ok_or(Error::MissingLink("declaring class declaration"))?;
            if self.node_within_class(node, declaring_class)? {
                continue;
            }
            if modifiers & mf::PROTECTED != 0 {
                if let Some(containing) =
                    ts_ast::utilities::get_containing_class(self.ast(node)?, node)?
                {
                    let containing_symbol = self
                        .get_symbol_of_declaration(containing)?
                        .ok_or(Error::MissingLink("containing class symbol"))?;
                    let containing_type = if self.node(containing)?.kind() == K::ClassExpression {
                        let ty = self.check_class_expression(containing)?;
                        self.get_regular_type_of_literal_type(ty)?
                    } else {
                        self.get_declared_type_of_symbol(containing_symbol)?
                    };
                    if self.type_has_protected_accessible_base(symbol, containing_type)? {
                        continue;
                    }
                }
            }
            return self
                .get_declared_type_of_symbol(symbol)
                .map(|ty| Some((modifiers, ty)));
        }
        Ok(None)
    }

    // port: tsc/internal/checker/checker.go:Checker.isNodeWithinClass
    // port: tsc/internal/checker/checker.go:Checker.forEachEnclosingClass
    pub(crate) fn node_within_class(&self, node: NodeId, class: NodeId) -> Result<bool, Error> {
        let mut containing = ts_ast::utilities::get_containing_class(self.ast(node)?, node)?;
        while let Some(current) = containing {
            if current == class {
                return Ok(true);
            }
            containing = ts_ast::utilities::get_containing_class(self.ast(current)?, current)?;
        }
        Ok(false)
    }

    // port: tsc/internal/checker/checker.go:Checker.typeHasProtectedAccessibleBase
    fn type_has_protected_accessible_base(
        &mut self,
        target: SymbolId,
        ty: TypeId,
    ) -> Result<bool, Error> {
        let ty = if self.types.object_flags(ty)? & of::REFERENCE != 0 {
            self.types.target(ty)?
        } else {
            ty
        };
        let bases = self.interface_base_types(ty)?;
        let Some(&base) = bases.first() else {
            return Ok(false);
        };
        if self.types.flags(base)? & tf::INTERSECTION != 0 {
            let types = self.types.compound_types(base)?.clone();
            let mixins = self.constructor_mixin_flags(&types)?;
            for (&part, mixin) in types.iter().zip(mixins) {
                if !mixin
                    && self.types.object_flags(part)? & of::CLASS_OR_INTERFACE != 0
                    && (self.types.get(part)?.symbol == Some(target)
                        || self.type_has_protected_accessible_base(target, part)?)
                {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if self.types.get(base)?.symbol == Some(target) {
            return Ok(true);
        }
        self.type_has_protected_accessible_base(target, base)
    }

    // port: tsc/internal/checker/checker.go:Checker.findMixins
    fn constructor_mixin_flags(&mut self, types: &[TypeId]) -> Result<Vec<bool>, Error> {
        let mut flags = Vec::with_capacity(types.len());
        for &ty in types {
            flags.push(self.is_mixin_constructor_type(ty)?);
        }
        let mut constructor_count = 0;
        let mut mixin_count = 0;
        let mut first = None;
        for (index, &ty) in types.iter().enumerate() {
            if !self.signatures_of_type(ty, true)?.is_empty() {
                constructor_count += 1;
            }
            if flags[index] {
                first.get_or_insert(index);
                mixin_count += 1;
            }
        }
        if constructor_count > 0 && constructor_count == mixin_count {
            flags[first.ok_or(Error::MissingLink("first mixin constructor"))?] = false;
        }
        Ok(flags)
    }
}
