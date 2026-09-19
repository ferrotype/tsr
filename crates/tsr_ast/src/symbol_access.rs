//! Semantic symbol queries shared by owned transient and stored binding symbols.

use crate::{
    modifier_flags, symbol_flags, AstView, DeclarationSlice, JsString, NodeId, Symbol, SymbolId,
    SymbolRead, SymbolTableId,
};
use ts_arena::Error;

pub(crate) mod sealed {
    pub trait Sealed {
        fn observe_runtime_identity(&self) -> u64;
        fn assign_runtime_identity(&self) -> u64;
    }
}

/// Symbol queries expose controlled runtime identity operations, not its atomic.
///
/// ```compile_fail
/// fn overwrite(symbol: &dyn ts_ast::SymbolAccess) {
///     symbol.runtime_cell().store(7, std::sync::atomic::Ordering::SeqCst);
/// }
/// ```
pub trait SymbolAccess: sealed::Sealed {
    fn flags(&self) -> u32;
    fn check_flags(&self) -> u32;
    fn name_bytes(&self) -> &[u8];
    fn name_to_owned(&self) -> JsString;
    fn declarations(&self) -> DeclarationSlice;
    fn value_declaration(&self) -> Option<NodeId>;
    fn members(&self) -> Option<SymbolTableId>;
    fn exports(&self) -> Option<SymbolTableId>;
    fn parent(&self) -> Option<SymbolId>;
    fn export_symbol(&self) -> Option<SymbolId>;
    fn is_external_module(&self) -> bool {
        self.flags() & symbol_flags::MODULE != 0
            && crate::is_ambient_module_symbol_name(self.name_bytes())
    }
    fn is_static(&self, view: AstView<'_>) -> Result<bool, Error> {
        self.value_declaration().map_or(Ok(false), |node| {
            Ok(view.node(node)?.modifier_flags(view)? & modifier_flags::STATIC != 0)
        })
    }
    fn combined_local_and_export_symbol_flags(
        &self,
        resolve_flags: impl FnOnce(SymbolId) -> Result<u32, Error>,
    ) -> Result<u32, Error>
    where
        Self: Sized,
    {
        self.export_symbol()
            .map_or(Ok(self.flags()), |id| Ok(self.flags() | resolve_flags(id)?))
    }
}

impl sealed::Sealed for Symbol {
    fn observe_runtime_identity(&self) -> u64 {
        crate::symbols::observe_runtime_cell(&self.runtime_id)
    }
    fn assign_runtime_identity(&self) -> u64 {
        crate::symbols::assign_runtime_cell(&self.runtime_id)
    }
}
impl SymbolAccess for Symbol {
    fn flags(&self) -> u32 {
        self.flags
    }
    fn check_flags(&self) -> u32 {
        self.check_flags
    }
    fn name_bytes(&self) -> &[u8] {
        self.name.as_bytes()
    }
    fn name_to_owned(&self) -> JsString {
        self.name.clone()
    }
    fn declarations(&self) -> DeclarationSlice {
        self.declarations
    }
    fn value_declaration(&self) -> Option<NodeId> {
        self.value_declaration
    }
    fn members(&self) -> Option<SymbolTableId> {
        self.members
    }
    fn exports(&self) -> Option<SymbolTableId> {
        self.exports
    }
    fn parent(&self) -> Option<SymbolId> {
        self.parent
    }
    fn export_symbol(&self) -> Option<SymbolId> {
        self.export_symbol
    }
}
impl sealed::Sealed for SymbolRead<'_> {
    fn observe_runtime_identity(&self) -> u64 {
        crate::symbols::observe_runtime_cell((*self).runtime_cell())
    }
    fn assign_runtime_identity(&self) -> u64 {
        crate::symbols::assign_runtime_cell((*self).runtime_cell())
    }
}
impl SymbolAccess for SymbolRead<'_> {
    fn flags(&self) -> u32 {
        (*self).flags()
    }
    fn check_flags(&self) -> u32 {
        (*self).check_flags()
    }
    fn name_bytes(&self) -> &[u8] {
        (*self).name_bytes()
    }
    fn name_to_owned(&self) -> JsString {
        (*self).name_to_owned()
    }
    fn declarations(&self) -> DeclarationSlice {
        (*self).declarations()
    }
    fn value_declaration(&self) -> Option<NodeId> {
        (*self).value_declaration()
    }
    fn members(&self) -> Option<SymbolTableId> {
        (*self).members()
    }
    fn exports(&self) -> Option<SymbolTableId> {
        (*self).exports()
    }
    fn parent(&self) -> Option<SymbolId> {
        (*self).parent()
    }
    fn export_symbol(&self) -> Option<SymbolId> {
        (*self).export_symbol()
    }
}

/// A resolver can also own transient symbols outside an immutable binding store.
#[derive(Clone, Copy, Debug)]
pub enum SymbolRef<'a> {
    Owned(&'a Symbol),
    Stored(SymbolRead<'a>),
}
macro_rules! query {
    ($name:ident, $result:ty) => {
        pub fn $name(self) -> $result {
            match self {
                Self::Owned(value) => SymbolAccess::$name(value),
                Self::Stored(value) => value.$name(),
            }
        }
    };
}
impl<'a> SymbolRef<'a> {
    query!(flags, u32);
    query!(check_flags, u32);
    query!(name_bytes, &'a [u8]);
    query!(name_to_owned, JsString);
    query!(declarations, DeclarationSlice);
    query!(value_declaration, Option<NodeId>);
    query!(members, Option<SymbolTableId>);
    query!(exports, Option<SymbolTableId>);
    query!(parent, Option<SymbolId>);
    query!(export_symbol, Option<SymbolId>);
    pub fn is_external_module(self) -> bool {
        SymbolAccess::is_external_module(&self)
    }
    pub fn is_static(self, view: AstView<'_>) -> Result<bool, Error> {
        SymbolAccess::is_static(&self, view)
    }
}
impl sealed::Sealed for SymbolRef<'_> {
    fn observe_runtime_identity(&self) -> u64 {
        match self {
            Self::Owned(value) => crate::existing_runtime_symbol_id(*value),
            Self::Stored(value) => crate::existing_runtime_symbol_id(value),
        }
    }
    fn assign_runtime_identity(&self) -> u64 {
        match self {
            Self::Owned(value) => crate::runtime_symbol_id(*value),
            Self::Stored(value) => crate::runtime_symbol_id(value),
        }
    }
}
impl SymbolAccess for SymbolRef<'_> {
    fn flags(&self) -> u32 {
        (*self).flags()
    }
    fn check_flags(&self) -> u32 {
        (*self).check_flags()
    }
    fn name_bytes(&self) -> &[u8] {
        (*self).name_bytes()
    }
    fn name_to_owned(&self) -> JsString {
        (*self).name_to_owned()
    }
    fn declarations(&self) -> DeclarationSlice {
        (*self).declarations()
    }
    fn value_declaration(&self) -> Option<NodeId> {
        (*self).value_declaration()
    }
    fn members(&self) -> Option<SymbolTableId> {
        (*self).members()
    }
    fn exports(&self) -> Option<SymbolTableId> {
        (*self).exports()
    }
    fn parent(&self) -> Option<SymbolId> {
        (*self).parent()
    }
    fn export_symbol(&self) -> Option<SymbolId> {
        (*self).export_symbol()
    }
}

macro_rules! retained_access {
    ($type:ty) => {
        impl sealed::Sealed for $type {
            fn observe_runtime_identity(&self) -> u64 {
                crate::existing_runtime_symbol_id(&self.symbol())
            }
            fn assign_runtime_identity(&self) -> u64 {
                crate::runtime_symbol_id(&self.symbol())
            }
        }
        impl SymbolAccess for $type {
            fn flags(&self) -> u32 {
                self.symbol().flags()
            }
            fn check_flags(&self) -> u32 {
                self.symbol().check_flags()
            }
            fn name_bytes(&self) -> &[u8] {
                self.symbol().name_bytes()
            }
            fn name_to_owned(&self) -> JsString {
                self.symbol().name_to_owned()
            }
            fn declarations(&self) -> DeclarationSlice {
                self.symbol().declarations()
            }
            fn value_declaration(&self) -> Option<NodeId> {
                self.symbol().value_declaration()
            }
            fn members(&self) -> Option<SymbolTableId> {
                self.symbol().members()
            }
            fn exports(&self) -> Option<SymbolTableId> {
                self.symbol().exports()
            }
            fn parent(&self) -> Option<SymbolId> {
                self.symbol().parent()
            }
            fn export_symbol(&self) -> Option<SymbolId> {
                self.symbol().export_symbol()
            }
        }
    };
}
retained_access!(crate::RetainedSymbol);
retained_access!(crate::CompletedSymbol);
