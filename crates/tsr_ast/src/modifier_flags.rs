//! Syntactic and JSDoc modifier bits from the pin.
// port: tsc/internal/ast/modifierflags.go

pub type ModifierFlags = u32;
pub const NONE: ModifierFlags = 0;
pub const PUBLIC: ModifierFlags = 1 << 0;
pub const PRIVATE: ModifierFlags = 1 << 1;
pub const PROTECTED: ModifierFlags = 1 << 2;
pub const READONLY: ModifierFlags = 1 << 3;
pub const OVERRIDE: ModifierFlags = 1 << 4;
pub const EXPORT: ModifierFlags = 1 << 5;
pub const ABSTRACT: ModifierFlags = 1 << 6;
pub const AMBIENT: ModifierFlags = 1 << 7;
pub const STATIC: ModifierFlags = 1 << 8;
pub const ACCESSOR: ModifierFlags = 1 << 9;
pub const ASYNC: ModifierFlags = 1 << 10;
pub const DEFAULT: ModifierFlags = 1 << 11;
pub const CONST: ModifierFlags = 1 << 12;
pub const IN: ModifierFlags = 1 << 13;
pub const OUT: ModifierFlags = 1 << 14;
pub const DECORATOR: ModifierFlags = 1 << 15;
pub const DEPRECATED: ModifierFlags = 1 << 16;
pub const JS_DOC_PUBLIC: ModifierFlags = 1 << 23;
pub const JS_DOC_PRIVATE: ModifierFlags = 1 << 24;
pub const JS_DOC_PROTECTED: ModifierFlags = 1 << 25;
pub const JS_DOC_READONLY: ModifierFlags = 1 << 26;
pub const JS_DOC_OVERRIDE: ModifierFlags = 1 << 27;
pub const HAS_COMPUTED_JS_DOC_MODIFIERS: ModifierFlags = 1 << 28;
pub const HAS_COMPUTED_FLAGS: ModifierFlags = 1 << 29;

pub const SYNTACTIC_OR_JS_DOC_MODIFIERS: ModifierFlags =
    PUBLIC | PRIVATE | PROTECTED | READONLY | OVERRIDE;
pub const SYNTACTIC_ONLY_MODIFIERS: ModifierFlags = EXPORT
    | AMBIENT
    | ABSTRACT
    | STATIC
    | ACCESSOR
    | ASYNC
    | DEFAULT
    | CONST
    | IN
    | OUT
    | DECORATOR;
pub const SYNTACTIC_MODIFIERS: ModifierFlags =
    SYNTACTIC_OR_JS_DOC_MODIFIERS | SYNTACTIC_ONLY_MODIFIERS;
pub const JS_DOC_CACHE_ONLY_MODIFIERS: ModifierFlags =
    JS_DOC_PUBLIC | JS_DOC_PRIVATE | JS_DOC_PROTECTED | JS_DOC_READONLY | JS_DOC_OVERRIDE;
pub const JS_DOC_ONLY_MODIFIERS: ModifierFlags = DEPRECATED;
pub const NON_CACHE_ONLY_MODIFIERS: ModifierFlags = SYNTACTIC_MODIFIERS | JS_DOC_ONLY_MODIFIERS;
pub const ACCESSIBILITY_MODIFIER: ModifierFlags = PUBLIC | PRIVATE | PROTECTED;
pub const PARAMETER_PROPERTY_MODIFIER: ModifierFlags = ACCESSIBILITY_MODIFIER | READONLY | OVERRIDE;
pub const NON_PUBLIC_ACCESSIBILITY_MODIFIER: ModifierFlags = PRIVATE | PROTECTED;
pub const TYPE_SCRIPT_MODIFIER: ModifierFlags =
    AMBIENT | PUBLIC | PRIVATE | PROTECTED | READONLY | ABSTRACT | CONST | OVERRIDE | IN | OUT;
pub const EXPORT_DEFAULT: ModifierFlags = EXPORT | DEFAULT;
pub const ALL: ModifierFlags = NON_CACHE_ONLY_MODIFIERS;
pub const MODIFIER: ModifierFlags = ALL & !DECORATOR;
pub const JAVA_SCRIPT: ModifierFlags = EXPORT | STATIC | ACCESSOR | ASYNC | DEFAULT;
