//! Pinned open 32-bit symbol flags and declaration exclusion masks.
// port: tsc/internal/ast/symbolflags.go

pub type SymbolFlags = u32;

pub const NONE: SymbolFlags = 0;
pub const FUNCTION_SCOPED_VARIABLE: SymbolFlags = 1 << 0;
pub const BLOCK_SCOPED_VARIABLE: SymbolFlags = 1 << 1;
pub const PROPERTY: SymbolFlags = 1 << 2;
pub const ENUM_MEMBER: SymbolFlags = 1 << 3;
pub const FUNCTION: SymbolFlags = 1 << 4;
pub const CLASS: SymbolFlags = 1 << 5;
pub const INTERFACE: SymbolFlags = 1 << 6;
pub const CONST_ENUM: SymbolFlags = 1 << 7;
pub const REGULAR_ENUM: SymbolFlags = 1 << 8;
pub const VALUE_MODULE: SymbolFlags = 1 << 9;
pub const NAMESPACE_MODULE: SymbolFlags = 1 << 10;
pub const TYPE_LITERAL: SymbolFlags = 1 << 11;
pub const OBJECT_LITERAL: SymbolFlags = 1 << 12;
pub const METHOD: SymbolFlags = 1 << 13;
pub const CONSTRUCTOR: SymbolFlags = 1 << 14;
pub const GET_ACCESSOR: SymbolFlags = 1 << 15;
pub const SET_ACCESSOR: SymbolFlags = 1 << 16;
pub const SIGNATURE: SymbolFlags = 1 << 17;
pub const TYPE_PARAMETER: SymbolFlags = 1 << 18;
pub const TYPE_ALIAS: SymbolFlags = 1 << 19;
pub const EXPORT_VALUE: SymbolFlags = 1 << 20;
pub const ALIAS: SymbolFlags = 1 << 21;
pub const PROTOTYPE: SymbolFlags = 1 << 22;
pub const EXPORT_STAR: SymbolFlags = 1 << 23;
pub const OPTIONAL: SymbolFlags = 1 << 24;
pub const TRANSIENT: SymbolFlags = 1 << 25;
pub const ASSIGNMENT: SymbolFlags = 1 << 26;
pub const MODULE_EXPORTS: SymbolFlags = 1 << 27;
pub const CONST_ENUM_ONLY_MODULE: SymbolFlags = 1 << 28;
pub const REPLACEABLE_BY_METHOD: SymbolFlags = 1 << 29;
pub const GLOBAL_LOOKUP: SymbolFlags = 1 << 30;
pub const ALL: SymbolFlags = (1 << 30) - 1;
pub const ENUM: SymbolFlags = REGULAR_ENUM | CONST_ENUM;
pub const VARIABLE: SymbolFlags = FUNCTION_SCOPED_VARIABLE | BLOCK_SCOPED_VARIABLE;
pub const VALUE: SymbolFlags = VARIABLE
    | PROPERTY
    | ENUM_MEMBER
    | OBJECT_LITERAL
    | FUNCTION
    | CLASS
    | ENUM
    | VALUE_MODULE
    | METHOD
    | GET_ACCESSOR
    | SET_ACCESSOR;
pub const TYPE: SymbolFlags =
    CLASS | INTERFACE | ENUM | ENUM_MEMBER | TYPE_LITERAL | TYPE_PARAMETER | TYPE_ALIAS;
pub const NAMESPACE: SymbolFlags = VALUE_MODULE | NAMESPACE_MODULE | ENUM;
pub const MODULE: SymbolFlags = VALUE_MODULE | NAMESPACE_MODULE;
pub const ACCESSOR: SymbolFlags = GET_ACCESSOR | SET_ACCESSOR;
pub const FUNCTION_SCOPED_VARIABLE_EXCLUDES: SymbolFlags = VALUE & !FUNCTION_SCOPED_VARIABLE;
pub const BLOCK_SCOPED_VARIABLE_EXCLUDES: SymbolFlags = VALUE;
pub const PARAMETER_EXCLUDES: SymbolFlags = VALUE;
pub const PROPERTY_EXCLUDES: SymbolFlags = VALUE & !(PROPERTY | ACCESSOR);
pub const ENUM_MEMBER_EXCLUDES: SymbolFlags = VALUE | TYPE;
pub const FUNCTION_EXCLUDES: SymbolFlags = VALUE & !(FUNCTION | VALUE_MODULE | CLASS);
pub const CLASS_EXCLUDES: SymbolFlags = (VALUE | TYPE) & !(VALUE_MODULE | INTERFACE | FUNCTION);
pub const INTERFACE_EXCLUDES: SymbolFlags = TYPE & !(INTERFACE | CLASS);
pub const REGULAR_ENUM_EXCLUDES: SymbolFlags = (VALUE | TYPE) & !(REGULAR_ENUM | VALUE_MODULE);
pub const CONST_ENUM_EXCLUDES: SymbolFlags = (VALUE | TYPE) & !CONST_ENUM;
pub const VALUE_MODULE_EXCLUDES: SymbolFlags =
    VALUE & !(FUNCTION | CLASS | REGULAR_ENUM | VALUE_MODULE);
pub const NAMESPACE_MODULE_EXCLUDES: SymbolFlags = NONE;
pub const METHOD_EXCLUDES: SymbolFlags = VALUE & !METHOD;
pub const GET_ACCESSOR_EXCLUDES: SymbolFlags = VALUE & !(SET_ACCESSOR | PROPERTY);
pub const SET_ACCESSOR_EXCLUDES: SymbolFlags = VALUE & !(GET_ACCESSOR | PROPERTY);
pub const ACCESSOR_EXCLUDES: SymbolFlags = VALUE & !PROPERTY;
pub const TYPE_PARAMETER_EXCLUDES: SymbolFlags = TYPE & !TYPE_PARAMETER;
pub const TYPE_ALIAS_EXCLUDES: SymbolFlags = TYPE;
pub const ALIAS_EXCLUDES: SymbolFlags = ALIAS;
pub const MODULE_MEMBER: SymbolFlags =
    VARIABLE | FUNCTION | CLASS | INTERFACE | ENUM | MODULE | TYPE_ALIAS | ALIAS;
pub const EXPORT_HAS_LOCAL: SymbolFlags = FUNCTION | CLASS | ENUM | VALUE_MODULE;
pub const BLOCK_SCOPED: SymbolFlags = BLOCK_SCOPED_VARIABLE | CLASS | ENUM;
pub const PROPERTY_OR_ACCESSOR: SymbolFlags = PROPERTY | ACCESSOR;
pub const CLASS_MEMBER: SymbolFlags = METHOD | ACCESSOR | PROPERTY;
pub const EXPORT_SUPPORTS_DEFAULT_MODIFIER: SymbolFlags = CLASS | FUNCTION | INTERFACE;
pub const EXPORT_DOES_NOT_SUPPORT_DEFAULT_MODIFIER: SymbolFlags = !EXPORT_SUPPORTS_DEFAULT_MODIFIER;
pub const LATE_BINDING_CONTAINER: SymbolFlags =
    CLASS | INTERFACE | TYPE_LITERAL | OBJECT_LITERAL | FUNCTION;
