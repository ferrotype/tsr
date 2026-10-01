//! Text for generated identifiers and private identifiers
//! (`namegenerator.go`), with the name helpers of `printer/utilities.go` it
//! uses.
//!
//! The pinned generator keeps `generatedNames` on the generator rather than on
//! its scopes ("to match Strada"): a unique name reserved in one nested scope
//! stays reserved after that scope is popped and in every sibling scope. That
//! is ported as written.
//!
//! The generator reads syntax and binder locals through a
//! [`NameGeneratorHost`]; the pinned `*ast.Node` carries both. The two pinned
//! callback fields stay injectable fields here.

use crate::generated_identifier_flags::{self as g, Flags as GeneratedIdentifierFlags};
use crate::generatedidentifierflags::GeneratedIdentifierFlagsExt;
use crate::{AutoGenerateId, EmitContext, Error};
use hashbrown::{HashMap, HashSet};
use std::rc::Rc;
use tsr_arena::hash::FastState;
use tsr_ast::{
    is_identifier, is_locals_container, is_private_identifier, is_string_literal, symbol_flags,
    utilities::is_node_descendant_of, utilities_modules::get_external_module_name, AstView,
    Factory, JsString, NodeAccess, NodeBinding, NodeId, SymbolFlags, SymbolId, SymbolTableId,
    SymbolTableRead, SyntaxKind as K,
};

/// Go's nil-pointer panic text, for the places the pinned code dereferences
/// an absent node or symbol.
const NIL: &str = "runtime error: invalid memory address or nil pointer dereference";

/// Flags enum to track count of temp variables and a few dedicated names.
/// Go `type tempFlags int`.
type TempFlags = i64;

/// No preferred name.
const TEMP_FLAGS_AUTO: TempFlags = 0x0000_0000;
/// Temp variable counter.
const TEMP_FLAGS_COUNT_MASK: TempFlags = 0x0FFF_FFFF;
/// Use/preference flag for '_i'.
const TEMP_FLAGS_I: TempFlags = 0x1000_0000;

/// `NameGenerator.IsFileLevelUniqueNameInCurrentFile`: the printer's
/// `isFileLevelUniqueNameInCurrentFile`.
pub type IsFileLevelUniqueNameFn<'a> = Rc<dyn Fn(&[u8], bool) -> Result<bool, Error> + 'a>;
/// `NameGenerator.GetTextOfNode`: the printer's `getTextOfNode`.
pub type GetTextOfNodeFn<'a> = Rc<dyn Fn(NodeId) -> Result<JsString, Error> + 'a>;

/// The reads a pinned `*ast.Node` provides to the generator: syntax, and the
/// binder's `Locals` and `NextContainer` with the flags of the local symbols.
pub trait NameGeneratorHost {
    /// The factory the generated names were made with, for
    /// [`EmitContext::node_for_generated_name`].
    fn factory(&self) -> &dyn Factory;
    /// Syntax reads of generated names and of the nodes they were made for.
    fn view(&self) -> AstView<'_>;
    /// The bound fields of `node`, or `None` for an unbound node.
    fn binding(&self, node: NodeId) -> Result<Option<NodeBinding>, tsr_arena::Error>;
    fn table(&self, table: SymbolTableId) -> Result<SymbolTableRead<'_>, tsr_arena::Error>;
    fn symbol_flags(&self, symbol: SymbolId) -> Result<SymbolFlags, tsr_arena::Error>;
}

/// Names a generated identifier or private identifier. One generator serves
/// one printer; the caches are keyed by node identity and generated-name id.
pub struct NameGenerator<'a> {
    pub context: Option<EmitContext>,
    /// Callback for `Printer.isFileLevelUniqueNameInCurrentFile`; `None` is
    /// the pinned nil field.
    pub is_file_level_unique_name_in_current_file: Option<IsFileLevelUniqueNameFn<'a>>,
    /// Callback for `Printer.getTextOfNode`; `None` is the pinned nil field,
    /// which panics if it is called.
    pub get_text_of_node: Option<GetTextOfNodeFn<'a>>,
    /// Map of generated names for specific nodes, by `ast.GetNodeId`.
    node_id_to_generated_name: HashMap<u64, JsString, FastState>,
    /// Map of generated private names for specific nodes, by `ast.GetNodeId`.
    node_id_to_generated_private_name: HashMap<u64, JsString, FastState>,
    /// Map of generated names for temp and loop variables.
    auto_generated_id_to_generated_name: HashMap<AutoGenerateId, JsString, FastState>,
    name_generation_scope: Option<Box<NameGenerationScope>>,
    private_name_generation_scope: Option<Box<NameGenerationScope>>,
    /// NOTE: Used to match Strada, but should be moved to nameGenerationScope
    /// after port is complete.
    generated_names: HashSet<JsString, FastState>,
}

#[derive(Default)]
struct NameGenerationScope {
    /// The next nameGenerationScope in the stack.
    next: Option<Box<NameGenerationScope>>,
    /// TempFlags for the current name generation scope.
    temp_flags: TempFlags,
    /// TempFlags for the current name generation scope, by formatted name.
    formatted_name_temp_flags: HashMap<JsString, TempFlags, FastState>,
    /// Names reserved in nested name generation scopes.
    reserved_names: HashSet<JsString, FastState>,
}

impl<'a> NameGenerator<'a> {
    /// `&NameGenerator{Context: context}`; the callbacks start nil.
    pub fn new(context: Option<EmitContext>) -> Self {
        Self {
            context,
            is_file_level_unique_name_in_current_file: None,
            get_text_of_node: None,
            node_id_to_generated_name: HashMap::default(),
            node_id_to_generated_private_name: HashMap::default(),
            auto_generated_id_to_generated_name: HashMap::default(),
            name_generation_scope: None,
            private_name_generation_scope: None,
            generated_names: HashSet::default(),
        }
    }

    /// Calls the `GetTextOfNode` field.
    fn text_of_node(&self, node: NodeId) -> Result<JsString, Error> {
        (self.get_text_of_node.as_ref().expect(NIL))(node)
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.PushScope
    pub fn push_scope(&mut self, reuse_temp_variable_scope: bool) {
        let next = self.private_name_generation_scope.take();
        self.private_name_generation_scope = Some(Box::new(NameGenerationScope {
            next,
            ..NameGenerationScope::default()
        }));
        if !reuse_temp_variable_scope {
            let next = self.name_generation_scope.take();
            self.name_generation_scope = Some(Box::new(NameGenerationScope {
                next,
                ..NameGenerationScope::default()
            }));
        }
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.PopScope
    pub fn pop_scope(&mut self, reuse_temp_variable_scope: bool) {
        if let Some(scope) = self.private_name_generation_scope.take() {
            self.private_name_generation_scope = scope.next;
        }
        if !reuse_temp_variable_scope {
            if let Some(scope) = self.name_generation_scope.take() {
                self.name_generation_scope = scope.next;
            }
        }
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.getScope
    fn get_scope(&mut self, private_name: bool) -> &mut Option<Box<NameGenerationScope>> {
        if private_name {
            &mut self.private_name_generation_scope
        } else {
            &mut self.name_generation_scope
        }
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.getTempFlags
    fn get_temp_flags(&mut self, private_name: bool) -> TempFlags {
        match self.get_scope(private_name) {
            Some(scope) => scope.temp_flags,
            None => TEMP_FLAGS_AUTO,
        }
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.setTempFlags
    fn set_temp_flags(&mut self, private_name: bool, flags: TempFlags) {
        let scope = self
            .get_scope(private_name)
            .get_or_insert_with(Box::default);
        scope.temp_flags = flags;
    }

    /// Gets the TempFlags to use in the current nameGenerationScope for the
    /// given key.
    // port: tsc/internal/printer/namegenerator.go:NameGenerator.getTempFlagsForFormattedName
    fn get_temp_flags_for_formatted_name(
        &mut self,
        private_name: bool,
        formatted_name_key: &[u8],
    ) -> TempFlags {
        if let Some(scope) = self.get_scope(private_name) {
            if let Some(&flags) = scope.formatted_name_temp_flags.get(formatted_name_key) {
                return flags;
            }
        }
        TEMP_FLAGS_AUTO
    }

    /// Sets the TempFlags to use in the current nameGenerationScope for the
    /// given key.
    // port: tsc/internal/printer/namegenerator.go:NameGenerator.setTempFlagsForFormattedName
    fn set_temp_flags_for_formatted_name(
        &mut self,
        private_name: bool,
        formatted_name_key: JsString,
        flags: TempFlags,
    ) {
        let scope = self
            .get_scope(private_name)
            .get_or_insert_with(Box::default);
        scope
            .formatted_name_temp_flags
            .insert(formatted_name_key, flags);
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.reserveName
    fn reserve_name(&mut self, name: &JsString, private_name: bool, scoped: bool, temp: bool) {
        let scope = self
            .get_scope(private_name)
            .get_or_insert_with(Box::default);
        if private_name || scoped {
            scope.reserved_names.insert(name.clone());
        } else if !temp {
            // NOTE: Matches Strada, but is incorrect.
            self.generated_names.insert(name.clone());
        }
    }

    /// Generate the text for a generated identifier or private identifier.
    // port: tsc/internal/printer/namegenerator.go:NameGenerator.GenerateName
    pub fn generate_name(
        &mut self,
        host: &dyn NameGeneratorHost,
        name: NodeId,
    ) -> Result<JsString, Error> {
        if let Some(context) = &self.context {
            if let Some(auto_generate) = context.auto_generate_info(name) {
                if auto_generate.flags.is_node() {
                    // Node names generate unique names based on their original node
                    // and are cached based on that node's id.
                    let node = context.node_for_generated_name(host.factory(), name);
                    let private_name = is_private_identifier(&host.view().node(name)?);
                    return self.generate_name_for_node_cached(
                        host,
                        node,
                        private_name,
                        auto_generate.flags,
                        auto_generate.prefix.as_bytes(),
                        auto_generate.suffix.as_bytes(),
                    );
                }
                // Auto, Loop, and Unique names are cached based on their unique autoGenerateId.
                if let Some(auto_generated_name) = self
                    .auto_generated_id_to_generated_name
                    .get(&auto_generate.id)
                {
                    return Ok(auto_generated_name.clone());
                }
                let auto_generated_name = self.make_name(host, name)?;
                self.auto_generated_id_to_generated_name
                    .insert(auto_generate.id, auto_generated_name.clone());
                return Ok(auto_generated_name);
            }
        }
        self.text_of_node(name)
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.generateNameForNodeCached
    fn generate_name_for_node_cached(
        &mut self,
        host: &dyn NameGeneratorHost,
        node: NodeId,
        private_name: bool,
        flags: GeneratedIdentifierFlags,
        prefix: &[u8],
        suffix: &[u8],
    ) -> Result<JsString, Error> {
        let node_id = host.view().node(node)?.runtime_id();
        let cache = if private_name {
            &self.node_id_to_generated_private_name
        } else {
            &self.node_id_to_generated_name
        };

        if let Some(name) = cache.get(&node_id) {
            return Ok(name.clone());
        }

        let name = self.generate_name_for_node(host, node, private_name, flags, prefix, suffix)?;
        let cache = if private_name {
            &mut self.node_id_to_generated_private_name
        } else {
            &mut self.node_id_to_generated_name
        };
        cache.insert(node_id, name.clone());
        Ok(name)
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.generateNameForNode
    fn generate_name_for_node(
        &mut self,
        host: &dyn NameGeneratorHost,
        node: NodeId,
        private_name: bool,
        flags: GeneratedIdentifierFlags,
        prefix: &[u8],
        suffix: &[u8],
    ) -> Result<JsString, Error> {
        let read = host.view().node(node)?;
        let no_affixes = !private_name && prefix.is_empty() && suffix.is_empty();
        match read.kind().known() {
            Some(K::Identifier | K::PrivateIdentifier) => {
                let text = self.text_of_node(node)?;
                self.make_unique_name(
                    text.as_bytes(),
                    None, /*checkFn*/
                    flags.is_optimistic(),
                    flags.is_reserved_in_nested_scopes(),
                    private_name,
                    prefix,
                    suffix,
                )
            }
            Some(K::ModuleDeclaration | K::EnumDeclaration) => {
                assert!(
                    no_affixes,
                    "Generated name for a module or enum cannot be private and may have neither a prefix nor suffix"
                );
                self.generate_name_for_module_or_enum(host, node)
            }
            Some(K::ImportDeclaration | K::JSImportDeclaration | K::ExportDeclaration) => {
                assert!(
                    no_affixes,
                    "Generated name for an import or export cannot be private and may have neither a prefix nor suffix"
                );
                self.generate_name_for_import_or_export_declaration(host, node)
            }
            Some(K::FunctionDeclaration | K::ClassDeclaration) => {
                assert!(
                    no_affixes,
                    "Generated name for a class or function declaration cannot be private and may have neither a prefix nor suffix"
                );
                if let Some(name) = read.name() {
                    // Pinned: `!(g.Context == nil && g.Context.HasAutoGenerateInfo(name))`.
                    // With a context the condition holds without the call; without
                    // one, HasAutoGenerateInfo reads through the nil context.
                    assert!(self.context.is_some(), "{NIL}");
                    return self.generate_name_for_node(
                        host, name, false, /*privateName*/
                        flags, b"", /*prefix*/
                        b"", /*suffix*/
                    );
                }
                self.generate_name_for_export_default()
            }
            Some(K::ExportAssignment) => {
                assert!(
                    no_affixes,
                    "Generated name for an export assignment cannot be private and may have neither a prefix nor suffix"
                );
                self.generate_name_for_export_default()
            }
            Some(K::ClassExpression) => {
                assert!(
                    no_affixes,
                    "Generated name for a class expression cannot be private and may have neither a prefix nor suffix"
                );
                self.generate_name_for_class_expression()
            }
            Some(K::MethodDeclaration | K::GetAccessor | K::SetAccessor) => {
                self.generate_name_for_method_or_accessor(host, node, private_name, prefix, suffix)
            }
            Some(K::ComputedPropertyName) => self.make_temp_variable_name(
                TEMP_FLAGS_AUTO,
                true, /*reservedInNestedScopes*/
                private_name,
                prefix,
                suffix,
            ),
            _ => self.make_temp_variable_name(
                TEMP_FLAGS_AUTO,
                false, /*reservedInNestedScopes*/
                private_name,
                prefix,
                suffix,
            ),
        }
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.generateNameForModuleOrEnum
    fn generate_name_for_module_or_enum(
        &mut self,
        host: &dyn NameGeneratorHost,
        node: NodeId, /* ModuleDeclaration | EnumDeclaration */
    ) -> Result<JsString, Error> {
        let name = self.text_of_node(host.view().node(node)?.name().expect(NIL))?;
        // Use module/enum name itself if it is unique, otherwise make a unique variation
        if is_unique_local_name(host, name.as_bytes(), node)? {
            Ok(name)
        } else {
            self.make_unique_name(
                name.as_bytes(),
                None,  /*checkFn*/
                false, /*optimistic*/
                false, /*scoped*/
                false, /*privateName*/
                b"",   /*prefix*/
                b"",   /*suffix*/
            )
        }
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.generateNameForImportOrExportDeclaration
    fn generate_name_for_import_or_export_declaration(
        &mut self,
        host: &dyn NameGeneratorHost,
        node: NodeId, /* ImportDeclaration | ExportDeclaration */
    ) -> Result<JsString, Error> {
        let view = host.view();
        let expr = get_external_module_name(view, node)?;
        let mut base_name = JsString::from_bytes(b"module".as_slice());
        if is_string_literal(&view.node(expr.expect(NIL))?) {
            let expr = expr.expect(NIL);
            base_name = make_identifier_from_module_name(&view.node_text(expr)?);
        }
        self.make_unique_name(
            base_name.as_bytes(),
            None,  /*checkFn*/
            false, /*optimistic*/
            false, /*scoped*/
            false, /*privateName*/
            b"",   /*prefix*/
            b"",   /*suffix*/
        )
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.generateNameForExportDefault
    fn generate_name_for_export_default(&mut self) -> Result<JsString, Error> {
        self.make_unique_name(
            b"default", None,  /*checkFn*/
            false, /*optimistic*/
            false, /*scoped*/
            false, /*privateName*/
            b"",   /*prefix*/
            b"",   /*suffix*/
        )
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.generateNameForClassExpression
    fn generate_name_for_class_expression(&mut self) -> Result<JsString, Error> {
        self.make_unique_name(
            b"class", None,  /*checkFn*/
            false, /*optimistic*/
            false, /*scoped*/
            false, /*privateName*/
            b"",   /*prefix*/
            b"",   /*suffix*/
        )
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.generateNameForMethodOrAccessor
    fn generate_name_for_method_or_accessor(
        &mut self,
        host: &dyn NameGeneratorHost,
        node: NodeId, /* MethodDeclaration | AccessorDeclaration */
        private_name: bool,
        prefix: &[u8],
        suffix: &[u8],
    ) -> Result<JsString, Error> {
        let name = host.view().node(node)?.name().expect(NIL);
        if is_identifier(&host.view().node(name)?) {
            return self.generate_name_for_node_cached(
                host,
                name,
                private_name,
                g::NONE,
                prefix,
                suffix,
            );
        }
        self.make_temp_variable_name(
            TEMP_FLAGS_AUTO,
            false, /*reservedInNestedScopes*/
            private_name,
            prefix,
            suffix,
        )
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.makeName
    fn make_name(&mut self, host: &dyn NameGeneratorHost, name: NodeId) -> Result<JsString, Error> {
        let auto_generate = self
            .context
            .as_ref()
            .and_then(|context| context.auto_generate_info(name));
        if let Some(auto_generate) = auto_generate {
            match auto_generate.flags.kind() {
                g::AUTO => {
                    let private_name = is_private_identifier(&host.view().node(name)?);
                    return self.make_temp_variable_name(
                        TEMP_FLAGS_AUTO,
                        auto_generate.flags.is_reserved_in_nested_scopes(),
                        private_name,
                        auto_generate.prefix.as_bytes(),
                        auto_generate.suffix.as_bytes(),
                    );
                }
                g::LOOP => {
                    assert!(
                        is_identifier(&host.view().node(name)?),
                        "Debug failure. False expression."
                    );
                    return self.make_temp_variable_name(
                        TEMP_FLAGS_I,
                        auto_generate.flags.is_reserved_in_nested_scopes(),
                        false, /*privateName*/
                        auto_generate.prefix.as_bytes(),
                        auto_generate.suffix.as_bytes(),
                    );
                }
                g::UNIQUE => {
                    let view = host.view();
                    let text = view.node_text(name)?;
                    let check_fn = if auto_generate.flags.is_file_level() {
                        self.is_file_level_unique_name_in_current_file.clone()
                    } else {
                        None
                    };
                    let private_name = is_private_identifier(&view.node(name)?);
                    return self.make_unique_name(
                        &text,
                        check_fn.as_ref(),
                        auto_generate.flags.is_optimistic(),
                        auto_generate.flags.is_reserved_in_nested_scopes(),
                        private_name,
                        auto_generate.prefix.as_bytes(),
                        auto_generate.suffix.as_bytes(),
                    );
                }
                _ => {}
            }
        }
        self.text_of_node(name)
    }

    /// Return the next available name in the pattern _a ... _z, _0, _1, ...
    /// TempFlags._i may be used to express a preference for that dedicated
    /// name. Note that names generated by makeTempVariableName and
    /// makeUniqueName will never conflict.
    // port: tsc/internal/printer/namegenerator.go:NameGenerator.makeTempVariableName
    fn make_temp_variable_name(
        &mut self,
        flags: TempFlags,
        reserved_in_nested_scopes: bool,
        private_name: bool,
        prefix: &[u8],
        suffix: &[u8],
    ) -> Result<JsString, Error> {
        let mut temp_flags;
        let mut key = JsString::default();
        let simple = prefix.is_empty() && suffix.is_empty();
        if simple {
            temp_flags = self.get_temp_flags(private_name);
        } else {
            // Generate a key to use to acquire a TempFlags counter based on the fixed portions of the generated name.
            key = format_generated_name(private_name, prefix, b"" /*base*/, suffix);
            if private_name {
                key = ensure_leading_hash(key.as_bytes());
            }
            temp_flags = self.get_temp_flags_for_formatted_name(private_name, key.as_bytes());
        }

        if flags != 0 && temp_flags & flags == 0 {
            let full_name = format_generated_name(private_name, prefix, b"_i", suffix);
            if self.is_unique_name(full_name.as_bytes(), private_name)? {
                temp_flags |= flags;
                self.reserve_name(
                    &full_name,
                    private_name,
                    reserved_in_nested_scopes,
                    true, /*temp*/
                );
                if simple {
                    self.set_temp_flags(private_name, temp_flags);
                } else {
                    self.set_temp_flags_for_formatted_name(private_name, key, temp_flags);
                }
                return Ok(full_name);
            }
        }

        loop {
            let count = temp_flags & TEMP_FLAGS_COUNT_MASK;
            temp_flags += 1;
            // Skip over 'i' and 'n'
            if count != 8 && count != 13 {
                let name = if count < 26 {
                    let letter = u8::try_from(count).expect("count is below 26");
                    vec![b'_', b'a' + letter]
                } else {
                    format!("_{}", count - 26).into_bytes()
                };
                let full_name = format_generated_name(private_name, prefix, &name, suffix);
                if self.is_unique_name(full_name.as_bytes(), private_name)? {
                    self.reserve_name(
                        &full_name,
                        private_name,
                        reserved_in_nested_scopes,
                        true, /*temp*/
                    );
                    if simple {
                        self.set_temp_flags(private_name, temp_flags);
                    } else {
                        self.set_temp_flags_for_formatted_name(private_name, key, temp_flags);
                    }
                    return Ok(full_name);
                }
            }
        }
    }

    /// Generate a name that is unique within the current file and doesn't
    /// conflict with any names in global scope. The name is formed by adding
    /// an '_n' suffix to the specified base name, where n is a positive
    /// integer. Note that names generated by makeTempVariableName and
    /// makeUniqueName are guaranteed to never conflict. If `optimistic` is
    /// set, the first instance will use 'baseName' verbatim instead of
    /// 'baseName_1'.
    // port: tsc/internal/printer/namegenerator.go:NameGenerator.makeUniqueName
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    fn make_unique_name(
        &mut self,
        base_name: &[u8],
        check_fn: Option<&IsFileLevelUniqueNameFn<'a>>,
        optimistic: bool,
        scoped: bool,
        private_name: bool,
        prefix: &[u8],
        suffix: &[u8],
    ) -> Result<JsString, Error> {
        let mut base_name = remove_leading_hash(base_name).to_vec();
        if optimistic {
            let full_name = format_generated_name(private_name, prefix, &base_name, suffix);
            if self.check_unique_name(full_name.as_bytes(), private_name, check_fn)? {
                self.reserve_name(&full_name, private_name, scoped, false /*temp*/);
                return Ok(full_name);
            }
        }

        // Find the first unique 'name_n', where n is a positive integer
        if base_name.last().is_some_and(|&last| last != b'_') {
            base_name.push(b'_');
        }

        let mut i: i64 = 1;
        loop {
            let mut numbered = base_name.clone();
            numbered.extend_from_slice(i.to_string().as_bytes());
            let full_name = format_generated_name(private_name, prefix, &numbered, suffix);
            if self.check_unique_name(full_name.as_bytes(), private_name, check_fn)? {
                self.reserve_name(&full_name, private_name, scoped, false /*temp*/);
                return Ok(full_name);
            }
            i += 1;
        }
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.MakeFileLevelOptimisticUniqueName
    pub fn make_file_level_optimistic_unique_name(
        &mut self,
        name: &[u8],
    ) -> Result<JsString, Error> {
        let check_fn = self.is_file_level_unique_name_in_current_file.clone();
        self.make_unique_name(
            name,
            check_fn.as_ref(),
            true,  /*optimistic*/
            false, /*scoped*/
            false, /*privateName*/
            b"",   /*prefix*/
            b"",   /*suffix*/
        )
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.checkUniqueName
    fn check_unique_name(
        &mut self,
        name: &[u8],
        private_name: bool,
        check_fn: Option<&IsFileLevelUniqueNameFn<'a>>,
    ) -> Result<bool, Error> {
        if let Some(check_fn) = check_fn {
            check_fn(name, private_name)
        } else {
            self.is_unique_name(name, private_name)
        }
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.isUniqueName
    fn is_unique_name(&mut self, name: &[u8], private_name: bool) -> Result<bool, Error> {
        let file_level_unique = match &self.is_file_level_unique_name_in_current_file {
            None => true,
            Some(is_file_level_unique_name) => is_file_level_unique_name(name, private_name)?,
        };
        Ok(file_level_unique && !self.is_reserved_name(name, private_name))
    }

    // port: tsc/internal/printer/namegenerator.go:NameGenerator.isReservedName
    fn is_reserved_name(&mut self, name: &[u8], private_name: bool) -> bool {
        // NOTE: The following matches Strada, but is incorrect.
        if self.generated_names.contains(name) {
            return true;
        }

        // TODO: generated names should be scoped after Strada port is complete.
        let mut scope = self.get_scope(private_name).as_deref();
        while let Some(current) = scope {
            if current.reserved_names.contains(name) {
                return true;
            }
            scope = current.next.as_deref();
        }
        false
    }
}

// port: tsc/internal/printer/namegenerator.go:nextContainer
fn next_container(
    host: &dyn NameGeneratorHost,
    node: NodeId,
) -> Result<Option<NodeId>, tsr_arena::Error> {
    if is_locals_container(&host.view().node(node)?) {
        return Ok(host
            .binding(node)?
            .and_then(|binding| binding.next_container));
    }
    Ok(None)
}

// port: tsc/internal/printer/namegenerator.go:isUniqueLocalName
fn is_unique_local_name(
    host: &dyn NameGeneratorHost,
    name: &[u8],
    container: NodeId,
) -> Result<bool, tsr_arena::Error> {
    let view = host.view();
    let mut node = Some(container);
    while let Some(current) = node {
        if !(is_node_descendant_of(view, Some(current), Some(container))?
            && is_locals_container(&view.node(current)?))
        {
            break;
        }
        let locals = host.binding(current)?.and_then(|binding| binding.locals);
        if let Some(locals) = locals {
            // We conservatively include alias symbols to cover cases where they're emitted as locals
            if let Some(local) = host.table(locals)?.get(name) {
                let flags = host.symbol_flags(local.expect(NIL))?;
                if flags & (symbol_flags::VALUE | symbol_flags::EXPORT_VALUE | symbol_flags::ALIAS)
                    != 0
                {
                    return Ok(false);
                }
            }
        }
        node = next_container(host, current)?;
    }
    Ok(true)
}

// port: tsc/internal/printer/utilities.go:hasLeadingHash
fn has_leading_hash(text: &[u8]) -> bool {
    text.first() == Some(&b'#')
}

// port: tsc/internal/printer/utilities.go:removeLeadingHash
fn remove_leading_hash(text: &[u8]) -> &[u8] {
    if has_leading_hash(text) {
        &text[1..]
    } else {
        text
    }
}

// port: tsc/internal/printer/utilities.go:ensureLeadingHash
fn ensure_leading_hash(text: &[u8]) -> JsString {
    if has_leading_hash(text) {
        JsString::from_bytes(text.to_vec())
    } else {
        let mut hashed = Vec::with_capacity(text.len() + 1);
        hashed.push(b'#');
        hashed.extend_from_slice(text);
        JsString::from_bytes(hashed)
    }
}

// port: tsc/internal/printer/utilities.go:FormatGeneratedName
pub fn format_generated_name(
    private_name: bool,
    prefix: &[u8],
    base: &[u8],
    suffix: &[u8],
) -> JsString {
    let mut name = remove_leading_hash(prefix).to_vec();
    name.extend_from_slice(remove_leading_hash(base));
    name.extend_from_slice(remove_leading_hash(suffix));
    if private_name {
        return ensure_leading_hash(&name);
    }
    JsString::from_bytes(name)
}

// port: tsc/internal/printer/utilities.go:isASCIIWordCharacter
fn is_ascii_word_character(ch: u8) -> bool {
    ch.is_ascii_alphabetic() || ch.is_ascii_digit() || ch == b'_'
}

// port: tsc/internal/printer/utilities.go:makeIdentifierFromModuleName
fn make_identifier_from_module_name(module_name: &[u8]) -> JsString {
    let module_name = tsr_tspath::base_name(module_name);
    let mut builder = Vec::with_capacity(module_name.len());
    let mut start = 0;
    let mut pos = 0;
    while pos < module_name.len() {
        let ch = module_name[pos];
        if pos == 0 && ch.is_ascii_digit() {
            builder.push(b'_');
        } else if !is_ascii_word_character(ch) {
            if start < pos {
                builder.extend_from_slice(&module_name[start..pos]);
            }
            builder.push(b'_');
            start = pos + 1;
        }
        pos += 1;
    }
    if start < pos {
        builder.extend_from_slice(&module_name[start..pos]);
    }
    JsString::from_bytes(builder)
}

#[cfg(test)]
#[path = "namegenerator_tests.rs"]
mod tests;
