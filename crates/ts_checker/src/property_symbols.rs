//! Shared symbol modifiers, identity and write types for structural properties.
use crate::{ternary as tr, type_flags as tf, CheckerState, Error, Ternary, TypeId};
use ts_arena::SymbolId;
use ts_ast::{check_flags as cf, modifier_flags as mf, symbol_flags as sf, SyntaxKind as K};

impl CheckerState {
    // port: tsc/internal/checker/utilities.go:getDeclarationModifierFlagsFromSymbol
    pub(crate) fn property_modifiers(&self, property: SymbolId) -> Result<u32, Error> {
        self.property_modifiers_ex(property, false)
    }

    // port: tsc/internal/checker/utilities.go:getDeclarationModifierFlagsFromSymbolEx
    pub(crate) fn property_modifiers_ex(
        &self,
        property: SymbolId,
        write: bool,
    ) -> Result<u32, Error> {
        let read = self.symbol(property)?;
        let flags = read.check_flags();
        if flags & cf::SYNTHETIC != 0 {
            let (public, protected, private) = if write {
                (
                    cf::CONTAINS_WRITE_PUBLIC,
                    cf::CONTAINS_WRITE_PROTECTED,
                    cf::CONTAINS_WRITE_PRIVATE,
                )
            } else {
                (
                    cf::CONTAINS_PUBLIC,
                    cf::CONTAINS_PROTECTED,
                    cf::CONTAINS_PRIVATE,
                )
            };
            let access = if flags & public != 0 {
                mf::PUBLIC
            } else if flags & protected != 0 {
                mf::PROTECTED
            } else if flags & private != 0 {
                mf::PRIVATE
            } else {
                0
            };
            return Ok(access
                | if flags & cf::CONTAINS_STATIC != 0 {
                    mf::STATIC
                } else {
                    0
                });
        }
        if let Some(value) = read.value_declaration() {
            let mut node = None;
            if write {
                for declaration in self.symbol_declarations(property)?.iter().flatten() {
                    if self.ast(declaration)?.node(declaration)?.kind() == K::SetAccessor {
                        node = Some(declaration);
                        break;
                    }
                }
            }
            if node.is_none() && read.flags() & sf::GET_ACCESSOR != 0 {
                for declaration in self.symbol_declarations(property)?.iter().flatten() {
                    if self.ast(declaration)?.node(declaration)?.kind() == K::GetAccessor {
                        node = Some(declaration);
                        break;
                    }
                }
            }
            let node = node.unwrap_or(value);
            let flags = self
                .ast(node)?
                .node(node)?
                .modifier_flags(self.ast(node)?)?;
            let class_parent = read
                .parent()
                .map(|parent| {
                    self.symbol(parent)
                        .map(|parent| parent.flags() & sf::CLASS != 0)
                })
                .transpose()?
                .unwrap_or(false);
            return Ok(if class_parent {
                flags
            } else {
                flags & !mf::ACCESSIBILITY_MODIFIER
            });
        }
        Ok(if read.flags() & sf::PROTOTYPE != 0 {
            mf::PUBLIC | mf::STATIC
        } else {
            0
        })
    }

    // port: tsc/internal/checker/checker.go:Checker.getTargetSymbol
    pub(crate) fn target_symbol(&self, symbol: SymbolId) -> Result<SymbolId, Error> {
        if self.symbol(symbol)?.check_flags() & cf::INSTANTIATED != 0 {
            self.value_symbol_links
                .try_get(symbol)
                .and_then(|l| l.target)
                .ok_or(Error::MissingLink("instantiated target symbol"))
        } else {
            Ok(symbol)
        }
    }

    // port: tsc/internal/checker/checker.go:Checker.compareProperties
    pub(crate) fn compare_properties(
        &mut self,
        source: SymbolId,
        target: SymbolId,
        compare: &mut impl FnMut(&mut Self, TypeId, TypeId) -> Result<Ternary, Error>,
    ) -> Result<Ternary, Error> {
        if source == target {
            return Ok(tr::TRUE);
        }
        let s = self.property_modifiers(source)? & mf::NON_PUBLIC_ACCESSIBILITY_MODIFIER;
        let t = self.property_modifiers(target)? & mf::NON_PUBLIC_ACCESSIBILITY_MODIFIER;
        if s != t {
            return Ok(tr::FALSE);
        }
        if s != 0 {
            if self.target_symbol(source)? != self.target_symbol(target)? {
                return Ok(tr::FALSE);
            }
        } else if (self.symbol(source)?.flags() ^ self.symbol(target)?.flags()) & sf::OPTIONAL != 0
        {
            return Ok(tr::FALSE);
        }
        if self.is_readonly_symbol(source)? != self.is_readonly_symbol(target)? {
            return Ok(tr::FALSE);
        }
        let s = self.non_missing_symbol_type(source)?;
        let t = self.non_missing_symbol_type(target)?;
        compare(self, s, t)
    }

    // port: tsc/internal/checker/checker.go:Checker.getWriteTypeOfSymbol
    pub(crate) fn write_type_of_symbol(&mut self, symbol: SymbolId) -> Result<TypeId, Error> {
        let read = self.symbol(symbol)?;
        let check = read.check_flags();
        let flags = read.flags();
        if check & cf::SYNTHETIC_PROPERTY != 0 {
            if let Some(ty) = self
                .value_symbol_links
                .try_get(symbol)
                .and_then(|l| l.write_type)
            {
                return Ok(ty);
            }
            if check & cf::DEFERRED_TYPE != 0 {
                let parts = self
                    .query
                    .deferred_property_write_types
                    .try_get(symbol)
                    .and_then(Option::as_ref)
                    .cloned();
                let ty = if let Some(parts) = parts.filter(|parts| !parts.is_empty()) {
                    let containing = self
                        .value_symbol_links
                        .try_get(symbol)
                        .and_then(|l| l.containing_type)
                        .ok_or(Error::MissingLink("deferred write parent"))?;
                    if self.types.flags(containing)? & tf::UNION != 0 {
                        self.get_union_type(&parts)?
                    } else {
                        self.get_intersection_type(&parts)?
                    }
                } else {
                    self.get_type_of_symbol_with_deferred_type(symbol)?
                };
                self.value_symbol_links.get_or_default(symbol).write_type = Some(ty);
                return Ok(ty);
            }
            return self
                .value_symbol_links
                .try_get(symbol)
                .and_then(|l| l.resolved_type)
                .ok_or(Error::MissingLink("synthetic write type"));
        }
        if flags & sf::PROPERTY != 0 {
            let ty = self.get_type_of_symbol(symbol)?;
            return self.remove_missing_type(ty, flags & sf::OPTIONAL != 0);
        }
        if flags & sf::ACCESSOR != 0 {
            if check & cf::INSTANTIATED != 0 {
                let links = self
                    .value_symbol_links
                    .try_get(symbol)
                    .copied()
                    .ok_or(Error::MissingLink("accessor write links"))?;
                if let Some(ty) = links.write_type {
                    return Ok(ty);
                }
                let ty = self.write_type_of_symbol(
                    links
                        .target
                        .ok_or(Error::MissingLink("accessor write target"))?,
                )?;
                let ty = self.instantiate_type(ty, links.mapper)?;
                self.value_symbol_links.get_or_default(symbol).write_type = Some(ty);
                return Ok(ty);
            }
            return self.write_type_of_accessors(symbol);
        }
        self.get_type_of_symbol(symbol)
    }

    // port: tsc/internal/checker/checker.go:Checker.hasCommonDeclaration
    pub(crate) fn common_property_declaration(&self, symbols: &[SymbolId]) -> Result<bool, Error> {
        let mut common = Vec::new();
        for &symbol in symbols {
            let declarations = self.symbol_declarations(symbol)?.to_vec();
            if declarations.is_empty() {
                return Ok(false);
            }
            if common.is_empty() {
                common = declarations;
            } else {
                common.retain(|d| declarations.contains(d));
            }
            if common.is_empty() {
                return Ok(false);
            }
        }
        Ok(!common.is_empty())
    }
}
