//! The pinned `Affects*` option tables and `optionsHaveChanges`
//! (`tsoptions/declscompiler.go`).
//!
//! Ports of `tsc/internal/tsoptions/declscompiler.go`, witnessed by the `tsoptions` group of the Phase 1
//! operation tables (`docs/PHASE1-mutation-witnesses.md`, section 9).
use crate::options_value::compiler_options_value;
use crate::ConfigValue;
use tsr_core::{CompilerOptions, Tristate};

#[path = "option_fields_generated.rs"]
mod option_fields_generated;
pub use option_fields_generated::COMPILER_OPTION_FIELDS;

/// One field of `core.CompilerOptions` in declaration order: its reflect
/// index, and the name and flags of the option declaration
/// `CommandLineCompilerOptionsMap` finds for it (an empty name for none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionField {
    pub index: usize,
    pub field: &'static str,
    pub declaration: &'static str,
    pub affects_emit: bool,
    pub affects_declaration_path: bool,
    pub affects_semantic_diagnostics: bool,
    pub strict_flag: bool,
    pub allow_js_flag: bool,
    /// The key of the declaration's category message.
    pub category: &'static str,
}

/// Go's reflect values, as the observable value of each set field (an unset,
/// zero field has none).
fn field_value<'a>(values: &'a ConfigValue, field: &OptionField) -> Option<&'a ConfigValue> {
    values.get(field.declaration.as_bytes())
}

/// Go calls `fn` with each declared field's `reflect.Value`; here it gets the
/// field's observable value, `None` for a zero field.
/// port: tsc/internal/tsoptions/declscompiler.go:ForEachCompilerOptionValue
pub fn for_each_compiler_option_value(
    options: &CompilerOptions,
    decl_filter: fn(&OptionField) -> bool,
    f: &mut dyn FnMut(&OptionField, Option<&ConfigValue>, usize) -> bool,
) -> bool {
    let values = compiler_options_value(options);
    for field in COMPILER_OPTION_FIELDS {
        if !field.declaration.is_empty() && decl_filter(field) && f(field, field_value(&values, field), field.index) {
            return true;
        }
    }
    false
}

fn tristate_of(value: Option<&ConfigValue>) -> Tristate {
    match value {
        Some(ConfigValue::Boolean(true)) => Tristate::TRUE,
        Some(ConfigValue::Boolean(false)) => Tristate::FALSE,
        _ => Tristate::UNKNOWN,
    }
}

/// Go compares the options pointers first: the same options (or two nil
/// ones) have no changes, one nil side always has.
/// port: tsc/internal/tsoptions/declscompiler.go:optionsHaveChanges
fn options_have_changes(
    old_options: Option<&CompilerOptions>,
    new_options: Option<&CompilerOptions>,
    decl_filter: fn(&OptionField) -> bool,
) -> bool {
    let (old_options, new_options) = match (old_options, new_options) {
        (None, None) => return false,
        (Some(old), Some(new)) if std::ptr::eq(old, new) => return false,
        (Some(old), Some(new)) => (old, new),
        _ => return true,
    };
    let old_values = compiler_options_value(old_options);
    for_each_compiler_option_value(new_options, decl_filter, &mut |field, new_value, _| {
        let old_value = field_value(&old_values, field);
        if field.strict_flag {
            return old_options.strict_option_value(tristate_of(old_value))
                != new_options.strict_option_value(tristate_of(new_value));
        }
        if field.allow_js_flag {
            return old_options.allow_js() != new_options.allow_js();
        }
        new_value != old_value
    })
}

/// port: tsc/internal/tsoptions/declscompiler.go:CompilerOptionsAffectDeclarationPath
pub fn compiler_options_affect_declaration_path(
    old_options: Option<&CompilerOptions>,
    new_options: Option<&CompilerOptions>,
) -> bool {
    options_have_changes(old_options, new_options, |field| field.affects_declaration_path)
}

/// port: tsc/internal/tsoptions/declscompiler.go:CompilerOptionsAffectEmit
pub fn compiler_options_affect_emit(old_options: Option<&CompilerOptions>, new_options: Option<&CompilerOptions>) -> bool {
    options_have_changes(old_options, new_options, |field| field.affects_emit)
}

/// port: tsc/internal/tsoptions/declscompiler.go:CompilerOptionsAffectSemanticDiagnostics
pub fn compiler_options_affect_semantic_diagnostics(
    old_options: Option<&CompilerOptions>,
    new_options: Option<&CompilerOptions>,
) -> bool {
    options_have_changes(old_options, new_options, |field| field.affects_semantic_diagnostics)
}
