//! Replays `data/s08/printer-cases.json` through the Rust printer and compares
//! every byte with what the pinned Go printer produced for the same trees
//! (`data/s08/printer-observations.json`, made by `scripts/s08_printer.py`).

use crate::{
    emit_flags, list_format, EmitContext, EmitTextWriter, Printer, PrinterOptions,
    SingleLineStringWriter, TextWriter, TypePrecedence,
};
use serde_json::Value;
use ts_arena::Counters;
use ts_ast::{
    token_flags, AstBuilder, Factory, FactoryMethods, JsString, NodeId, NodeListId, SyntaxKind as K,
};
use ts_core::{NewLineKind, TextRange};
use ts_jsstring::{LiteralEscapeFlags, SourceText};

const CASES: &str = include_str!("../../../data/s08/printer-cases.json");
const OBSERVATIONS: &str = include_str!("../../../data/s08/printer-observations.json");

fn kind(name: &str) -> K {
    match name {
        "AnyKeyword" => K::AnyKeyword,
        "UnknownKeyword" => K::UnknownKeyword,
        "NumberKeyword" => K::NumberKeyword,
        "BigIntKeyword" => K::BigIntKeyword,
        "ObjectKeyword" => K::ObjectKeyword,
        "BooleanKeyword" => K::BooleanKeyword,
        "StringKeyword" => K::StringKeyword,
        "SymbolKeyword" => K::SymbolKeyword,
        "VoidKeyword" => K::VoidKeyword,
        "UndefinedKeyword" => K::UndefinedKeyword,
        "NeverKeyword" => K::NeverKeyword,
        "IntrinsicKeyword" => K::IntrinsicKeyword,
        "TrueKeyword" => K::TrueKeyword,
        "FalseKeyword" => K::FalseKeyword,
        "NullKeyword" => K::NullKeyword,
        "ThisKeyword" => K::ThisKeyword,
        "ReadonlyKeyword" => K::ReadonlyKeyword,
        "KeyOfKeyword" => K::KeyOfKeyword,
        "UniqueKeyword" => K::UniqueKeyword,
        "AssertsKeyword" => K::AssertsKeyword,
        "AbstractKeyword" => K::AbstractKeyword,
        "PublicKeyword" => K::PublicKeyword,
        "PrivateKeyword" => K::PrivateKeyword,
        "ProtectedKeyword" => K::ProtectedKeyword,
        "StaticKeyword" => K::StaticKeyword,
        "ExportKeyword" => K::ExportKeyword,
        "DeclareKeyword" => K::DeclareKeyword,
        "ConstKeyword" => K::ConstKeyword,
        "InKeyword" => K::InKeyword,
        "OutKeyword" => K::OutKeyword,
        "AccessorKeyword" => K::AccessorKeyword,
        "QuestionToken" => K::QuestionToken,
        "DotDotDotToken" => K::DotDotDotToken,
        "MinusToken" => K::MinusToken,
        "PlusToken" => K::PlusToken,
        "ExclamationToken" => K::ExclamationToken,
        "TildeToken" => K::TildeToken,
        "PlusPlusToken" => K::PlusPlusToken,
        "MinusMinusToken" => K::MinusMinusToken,
        other => panic!("unknown kind {other}"),
    }
}

struct Builder<'a> {
    ast: &'a mut AstBuilder,
    context: &'a mut EmitContext,
}

fn text(value: &Value, key: &str) -> JsString {
    let string = value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string {key}"));
    JsString::from_bytes(string.as_bytes().to_vec())
}

fn flag(value: &Value, key: &str) -> bool {
    value[key].as_bool().unwrap_or(false)
}

impl Builder<'_> {
    fn opt(&mut self, value: &Value) -> Option<NodeId> {
        if value.is_null() {
            None
        } else {
            Some(self.build(value))
        }
    }

    fn opt_token(&mut self, value: &Value, key: &str, token: K) -> Option<NodeId> {
        flag(value, key).then(|| self.ast.new_token(token.into()))
    }

    fn opt_kind_token(&mut self, value: &Value) -> Option<NodeId> {
        value
            .as_str()
            .map(|name| self.ast.new_token(kind(name).into()))
    }

    fn list_of(&mut self, nodes: Vec<NodeId>) -> NodeListId {
        let slice = self
            .ast
            .node_slice(nodes.into_iter().map(Some).collect())
            .expect("list slice");
        self.ast
            .new_list(TextRange::new(-1, -1), slice)
            .expect("list")
    }

    fn list(&mut self, value: &Value) -> Option<NodeListId> {
        let items = value.as_array()?;
        let nodes = items.iter().map(|item| self.build(item)).collect();
        Some(self.list_of(nodes))
    }

    fn modifiers(&mut self, value: &Value) -> Option<NodeListId> {
        let items = value.as_array()?;
        let nodes = items
            .iter()
            .map(|item| {
                self.ast
                    .new_modifier(kind(item.as_str().expect("modifier name")).into())
            })
            .collect();
        Some(self.list_of(nodes))
    }

    fn single_line(&mut self, value: &Value, node: NodeId) -> NodeId {
        if flag(value, "singleLine") {
            self.context.set_emit_flags(node, emit_flags::SINGLE_LINE);
        }
        node
    }

    fn build(&mut self, v: &Value) -> NodeId {
        let kind_name = v["kind"].as_str().expect("tree kind");
        match kind_name {
            "KeywordType" => self
                .ast
                .new_keyword_type_node(kind(v["keyword"].as_str().unwrap()).into()),
            "Identifier" => self.ast.new_identifier(text(v, "text")),
            "PrivateIdentifier" => self.ast.new_private_identifier(text(v, "text")),
            "QualifiedName" => {
                let left = self.build(&v["left"]);
                let right = self.build(&v["right"]);
                self.ast.new_qualified_name(Some(left), Some(right))
            }
            "ComputedPropertyName" => {
                let expression = self.build(&v["expression"]);
                self.ast.new_computed_property_name(Some(expression))
            }
            "StringLiteral" => {
                let flags = if flag(v, "singleQuote") {
                    token_flags::SINGLE_QUOTE
                } else {
                    0
                };
                self.ast.new_string_literal(text(v, "text"), flags)
            }
            "NumericLiteral" => self.ast.new_numeric_literal(text(v, "text"), 0),
            "BigIntLiteral" => self.ast.new_big_int_literal(text(v, "text"), 0),
            "NoSubstitutionTemplateLiteral" => self
                .ast
                .new_no_substitution_template_literal(text(v, "text"), 0),
            "TemplateHead" => self
                .ast
                .new_template_head(text(v, "text"), text(v, "rawText"), 0),
            "TemplateMiddle" => {
                self.ast
                    .new_template_middle(text(v, "text"), text(v, "rawText"), 0)
            }
            "TemplateTail" => self
                .ast
                .new_template_tail(text(v, "text"), text(v, "rawText"), 0),
            "KeywordExpression" => self
                .ast
                .new_keyword_expression(kind(v["keyword"].as_str().unwrap()).into()),
            "PrefixUnary" => {
                let operand = self.build(&v["operand"]);
                self.ast.new_prefix_unary_expression(
                    kind(v["operator"].as_str().unwrap()).into(),
                    Some(operand),
                )
            }
            "PropertyAccess" => {
                let expression = self.build(&v["expression"]);
                let name = self.build(&v["name"]);
                self.ast
                    .new_property_access_expression(Some(expression), None, Some(name), 0)
            }
            "ExpressionWithTypeArguments" => {
                let expression = self.build(&v["expression"]);
                let arguments = self.list(&v["typeArguments"]);
                self.ast
                    .new_expression_with_type_arguments(Some(expression), arguments)
            }
            "UnionType" => {
                let types = self.list(&v["types"]);
                self.ast.new_union_type_node(types)
            }
            "IntersectionType" => {
                let types = self.list(&v["types"]);
                self.ast.new_intersection_type_node(types)
            }
            "LiteralType" => {
                let literal = self.build(&v["literal"]);
                self.ast.new_literal_type_node(Some(literal))
            }
            "TypeReference" => {
                let name = self.build(&v["typeName"]);
                let arguments = self.list(&v["typeArguments"]);
                self.ast.new_type_reference_node(Some(name), arguments)
            }
            "ArrayType" => {
                let element = self.build(&v["elementType"]);
                self.ast.new_array_type_node(Some(element))
            }
            "TupleType" => {
                let elements = self.list(&v["elements"]);
                let node = self.ast.new_tuple_type_node(elements);
                self.single_line(v, node)
            }
            "NamedTupleMember" => {
                let dots = self.opt_token(v, "dotDotDot", K::DotDotDotToken);
                let name = self.build(&v["name"]);
                let question = self.opt_token(v, "question", K::QuestionToken);
                let type_node = self.build(&v["type"]);
                self.ast
                    .new_named_tuple_member(dots, Some(name), question, Some(type_node))
            }
            "OptionalType" => {
                let inner = self.build(&v["type"]);
                self.ast.new_optional_type_node(Some(inner))
            }
            "RestType" => {
                let inner = self.build(&v["type"]);
                self.ast.new_rest_type_node(Some(inner))
            }
            "TypeLiteral" => {
                let members = self.list(&v["members"]);
                let node = self.ast.new_type_literal_node(members);
                self.single_line(v, node)
            }
            "PropertySignature" => {
                let modifiers = self.modifiers(&v["modifiers"]);
                let name = self.build(&v["name"]);
                let question = self.opt_token(v, "question", K::QuestionToken);
                let type_node = self.opt(&v["type"]);
                self.ast.new_property_signature_declaration(
                    modifiers,
                    Some(name),
                    question,
                    type_node,
                    None,
                )
            }
            "MethodSignature" => {
                let modifiers = self.modifiers(&v["modifiers"]);
                let name = self.build(&v["name"]);
                let question = self.opt_token(v, "question", K::QuestionToken);
                let type_parameters = self.list(&v["typeParameters"]);
                let parameters = self.list(&v["parameters"]);
                let type_node = self.opt(&v["type"]);
                self.ast.new_method_signature_declaration(
                    modifiers,
                    Some(name),
                    question,
                    type_parameters,
                    parameters,
                    type_node,
                )
            }
            "IndexSignature" => {
                let modifiers = self.modifiers(&v["modifiers"]);
                let parameters = self.list(&v["parameters"]);
                let type_node = self.opt(&v["type"]);
                self.ast
                    .new_index_signature_declaration(modifiers, parameters, type_node)
            }
            "CallSignature" => {
                let type_parameters = self.list(&v["typeParameters"]);
                let parameters = self.list(&v["parameters"]);
                let type_node = self.opt(&v["type"]);
                self.ast
                    .new_call_signature_declaration(type_parameters, parameters, type_node)
            }
            "ConstructSignature" => {
                let type_parameters = self.list(&v["typeParameters"]);
                let parameters = self.list(&v["parameters"]);
                let type_node = self.opt(&v["type"]);
                self.ast
                    .new_construct_signature_declaration(type_parameters, parameters, type_node)
            }
            "FunctionType" => {
                let type_parameters = self.list(&v["typeParameters"]);
                let parameters = self.list(&v["parameters"]);
                let type_node = self.opt(&v["type"]);
                self.ast
                    .new_function_type_node(type_parameters, parameters, type_node)
            }
            "ConstructorType" => {
                let modifiers = self.modifiers(&v["modifiers"]);
                let type_parameters = self.list(&v["typeParameters"]);
                let parameters = self.list(&v["parameters"]);
                let type_node = self.opt(&v["type"]);
                self.ast.new_constructor_type_node(
                    modifiers,
                    type_parameters,
                    parameters,
                    type_node,
                )
            }
            "Parameter" => {
                let modifiers = self.modifiers(&v["modifiers"]);
                let dots = self.opt_token(v, "dotDotDot", K::DotDotDotToken);
                let name = self.build(&v["name"]);
                let question = self.opt_token(v, "question", K::QuestionToken);
                let type_node = self.opt(&v["type"]);
                let initializer = self.opt(&v["initializer"]);
                self.ast.new_parameter_declaration(
                    modifiers,
                    dots,
                    Some(name),
                    question,
                    type_node,
                    initializer,
                )
            }
            "TypeParameter" => {
                let modifiers = self.modifiers(&v["modifiers"]);
                let name = self.build(&v["name"]);
                let constraint = self.opt(&v["constraint"]);
                let default = self.opt(&v["default"]);
                self.ast.new_type_parameter_declaration(
                    modifiers,
                    Some(name),
                    constraint,
                    None,
                    default,
                )
            }
            "ParenthesizedType" => {
                let inner = self.build(&v["type"]);
                self.ast.new_parenthesized_type_node(Some(inner))
            }
            "TypeOperator" => {
                let inner = self.build(&v["type"]);
                self.ast.new_type_operator_node(
                    kind(v["operator"].as_str().unwrap()).into(),
                    Some(inner),
                )
            }
            "IndexedAccessType" => {
                let object = self.build(&v["objectType"]);
                let index = self.build(&v["indexType"]);
                self.ast
                    .new_indexed_access_type_node(Some(object), Some(index))
            }
            "TypeQuery" => {
                let name = self.build(&v["exprName"]);
                let arguments = self.list(&v["typeArguments"]);
                self.ast.new_type_query_node(Some(name), arguments)
            }
            "ThisType" => self.ast.new_this_type_node(),
            "TypePredicate" => {
                let asserts = self.opt_token(v, "asserts", K::AssertsKeyword);
                let parameter = self.build(&v["parameterName"]);
                let type_node = self.opt(&v["type"]);
                self.ast
                    .new_type_predicate_node(asserts, Some(parameter), type_node)
            }
            "ConditionalType" => {
                let check = self.build(&v["checkType"]);
                let extends = self.build(&v["extendsType"]);
                let true_type = self.build(&v["trueType"]);
                let false_type = self.build(&v["falseType"]);
                self.ast.new_conditional_type_node(
                    Some(check),
                    Some(extends),
                    Some(true_type),
                    Some(false_type),
                )
            }
            "InferType" => {
                let parameter = self.build(&v["typeParameter"]);
                self.ast.new_infer_type_node(Some(parameter))
            }
            "MappedType" => {
                let readonly = self.opt_kind_token(&v["readonlyToken"]);
                let parameter = self.build(&v["typeParameter"]);
                let name_type = self.opt(&v["nameType"]);
                let question = self.opt_kind_token(&v["questionToken"]);
                let type_node = self.opt(&v["type"]);
                let members = self.list(&v["members"]);
                let node = self.ast.new_mapped_type_node(
                    readonly,
                    Some(parameter),
                    name_type,
                    question,
                    type_node,
                    members,
                );
                self.single_line(v, node)
            }
            "TemplateLiteralType" => {
                let head = self.build(&v["head"]);
                let spans = self.list(&v["spans"]);
                self.ast.new_template_literal_type_node(Some(head), spans)
            }
            "TemplateLiteralTypeSpan" => {
                let type_node = self.build(&v["type"]);
                let literal = self.build(&v["literal"]);
                self.ast
                    .new_template_literal_type_span(Some(type_node), Some(literal))
            }
            "ImportType" => {
                let argument = self.build(&v["argument"]);
                let qualifier = self.opt(&v["qualifier"]);
                let arguments = self.list(&v["typeArguments"]);
                self.ast.new_import_type_node(
                    flag(v, "isTypeOf"),
                    Some(argument),
                    None,
                    qualifier,
                    arguments,
                )
            }
            "JSDocNullableType" => {
                let inner = self.build(&v["type"]);
                self.ast.new_js_doc_nullable_type(Some(inner))
            }
            "JSDocNonNullableType" => {
                let inner = self.build(&v["type"]);
                self.ast.new_js_doc_non_nullable_type(Some(inner))
            }
            "JSDocOptionalType" => {
                let inner = self.build(&v["type"]);
                self.ast.new_js_doc_optional_type(Some(inner))
            }
            "JSDocVariadicType" => {
                let inner = self.build(&v["type"]);
                self.ast.new_js_doc_variadic_type(Some(inner))
            }
            "JSDocAllType" => self.ast.new_js_doc_all_type(),
            other => panic!("unknown tree kind {other}"),
        }
    }
}

fn decode_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex"))
        .collect()
}

#[test]
fn printer_matches_the_pinned_go_printer_on_every_case() {
    let cases: Value = serde_json::from_str(CASES).expect("cases");
    let observed: Value = serde_json::from_str(OBSERVATIONS).expect("observations");
    let cases = cases["cases"].as_array().expect("case list");
    let rows = observed["rows"].as_array().expect("observed rows");
    assert_eq!(cases.len(), rows.len(), "every case has an observation");
    let mut failures = Vec::new();
    for (case, row) in cases.iter().zip(rows) {
        let name = case["name"].as_str().expect("case name");
        assert_eq!(Some(name), row["name"].as_str());
        let counters = Counters::new();
        let mut context = EmitContext::new();
        let mut ast = AstBuilder::with_hooks(
            SourceText::from_bytes(&b""[..]),
            &counters,
            context.factory_hooks(),
        );
        let node = Builder {
            ast: &mut ast,
            context: &mut context,
        }
        .build(&case["tree"]);
        let options = &case["options"];
        let printer = Printer::new(
            PrinterOptions {
                remove_comments: flag(options, "remove_comments"),
                omit_trailing_semicolon: flag(options, "omit_trailing_semicolon"),
                never_ascii_escape: flag(options, "never_ascii_escape"),
                ..PrinterOptions::default()
            },
            &context,
        );
        let result = if options["writer"].as_str() == Some("single_line") {
            let mut writer = SingleLineStringWriter::new();
            printer
                .write(ast.view(), node, None, &mut writer)
                .map(|()| writer.text().to_vec())
        } else {
            let new_line = options["new_line"].as_str().unwrap_or("");
            let mut writer = TextWriter::new(new_line.as_bytes(), 0);
            printer
                .write(ast.view(), node, None, &mut writer)
                .map(|()| writer.text().to_vec())
        };
        match (
            row.get("text_hex").and_then(Value::as_str),
            row.get("panic"),
        ) {
            (Some(hex), _) => {
                let expected = decode_hex(hex);
                match result {
                    Ok(actual) if actual == expected => {}
                    Ok(actual) => failures.push(format!(
                        "{name}: Go {:?}, Rust {:?}",
                        String::from_utf8_lossy(&expected),
                        String::from_utf8_lossy(&actual)
                    )),
                    Err(error) => failures.push(format!(
                        "{name}: Go {:?}, Rust error {error}",
                        String::from_utf8_lossy(&expected)
                    )),
                }
            }
            (None, Some(panic)) => {
                if let Ok(actual) = result {
                    failures.push(format!(
                        "{name}: Go panicked ({panic}), Rust printed {:?}",
                        String::from_utf8_lossy(&actual)
                    ));
                }
            }
            (None, None) => failures.push(format!("{name}: observation has no outcome")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn identifier_source_text_requires_the_same_source_file_not_just_owner() {
    let counters = Counters::new();
    let first_text = SourceText::from_bytes(&b"other___"[..]);
    let second_text = SourceText::from_bytes(&br"\u0062ar"[..]);
    let mut ast = AstBuilder::new(first_text.clone(), &counters);
    let first = ast.new_source_file(
        ts_ast::SourceFileParseOptions {
            file_name: JsString::from_bytes(&b"/first.ts"[..]),
            ..Default::default()
        },
        first_text,
        None,
        None,
    );
    let second = ast.new_source_file(
        ts_ast::SourceFileParseOptions {
            file_name: JsString::from_bytes(&b"/second.ts"[..]),
            ..Default::default()
        },
        second_text,
        None,
        None,
    );
    let identifier = ast.new_identifier(JsString::from_bytes(&b"bar"[..]));
    {
        let mut node = ast.node_mut(identifier).unwrap();
        node.set_range(TextRange::new(0, 8));
        node.set_parent(Some(second));
    }
    let context = EmitContext::new();
    let mut printer = Printer::new(
        PrinterOptions {
            remove_comments: true,
            ..Default::default()
        },
        &context,
    );
    // Go's getTextOfNode copies raw source spelling only from the node's own
    // SourceFile; sibling SourceFiles can share one storage owner.
    let sibling_text = printer.emit(ast.view(), identifier, Some(first)).unwrap();
    assert_eq!(sibling_text, b"bar");
    let own_text = printer.emit(ast.view(), identifier, Some(second)).unwrap();
    assert_eq!(own_text, br"\u0062ar");
    let without_source = printer.emit(ast.view(), identifier, None).unwrap();
    assert_eq!(without_source, b"bar");
}

fn constants(kind: &str) -> serde_json::Map<String, Value> {
    let observed: Value = serde_json::from_str(OBSERVATIONS).expect("observations");
    observed["constants"][kind]
        .as_object()
        .unwrap_or_else(|| panic!("constants {kind}"))
        .clone()
}

fn check(kind: &str, ours: &[(&str, i64)]) {
    let expected = constants(kind);
    assert_eq!(
        ours.len(),
        expected.len(),
        "{kind}: every observed constant has a port"
    );
    for (name, value) in ours {
        assert_eq!(expected[*name].as_i64(), Some(*value), "{kind}::{name}");
    }
}

#[test]
fn emit_flags_match_the_pinned_go_values() {
    use emit_flags as ef;
    check(
        "printer.EmitFlags",
        &[
            ("EFSingleLine", i64::from(ef::SINGLE_LINE)),
            ("EFMultiLine", i64::from(ef::MULTI_LINE)),
            ("EFNoLeadingSourceMap", i64::from(ef::NO_LEADING_SOURCE_MAP)),
            (
                "EFNoTrailingSourceMap",
                i64::from(ef::NO_TRAILING_SOURCE_MAP),
            ),
            ("EFNoNestedSourceMaps", i64::from(ef::NO_NESTED_SOURCE_MAPS)),
            (
                "EFNoTokenLeadingSourceMaps",
                i64::from(ef::NO_TOKEN_LEADING_SOURCE_MAPS),
            ),
            (
                "EFNoTokenTrailingSourceMaps",
                i64::from(ef::NO_TOKEN_TRAILING_SOURCE_MAPS),
            ),
            ("EFNoLeadingComments", i64::from(ef::NO_LEADING_COMMENTS)),
            ("EFNoTrailingComments", i64::from(ef::NO_TRAILING_COMMENTS)),
            ("EFNoNestedComments", i64::from(ef::NO_NESTED_COMMENTS)),
            ("EFHelperName", i64::from(ef::HELPER_NAME)),
            ("EFExportName", i64::from(ef::EXPORT_NAME)),
            ("EFLocalName", i64::from(ef::LOCAL_NAME)),
            ("EFIndented", i64::from(ef::INDENTED)),
            ("EFNoIndentation", i64::from(ef::NO_INDENTATION)),
            (
                "EFReuseTempVariableScope",
                i64::from(ef::REUSE_TEMP_VARIABLE_SCOPE),
            ),
            ("EFCustomPrologue", i64::from(ef::CUSTOM_PROLOGUE)),
            ("EFNoAsciiEscaping", i64::from(ef::NO_ASCII_ESCAPING)),
            ("EFExternalHelpers", i64::from(ef::EXTERNAL_HELPERS)),
            ("EFStartOnNewLine", i64::from(ef::START_ON_NEW_LINE)),
            ("EFIndirectCall", i64::from(ef::INDIRECT_CALL)),
            ("EFAsyncFunctionBody", i64::from(ef::ASYNC_FUNCTION_BODY)),
            ("EFNoLexicalArguments", i64::from(ef::NO_LEXICAL_ARGUMENTS)),
            (
                "EFTransformPrivateStaticElements",
                i64::from(ef::TRANSFORM_PRIVATE_STATIC_ELEMENTS),
            ),
            ("EFNoLexicalThis", i64::from(ef::NO_LEXICAL_THIS)),
            ("EFNone", i64::from(ef::NONE)),
            ("EFNoSourceMap", i64::from(ef::NO_SOURCE_MAP)),
            ("EFNoTokenSourceMaps", i64::from(ef::NO_TOKEN_SOURCE_MAPS)),
            ("EFNoComments", i64::from(ef::NO_COMMENTS)),
        ],
    );
}

#[test]
fn list_formats_match_the_pinned_go_values() {
    use list_format as lf;
    check(
        "printer.ListFormat",
        &[
            ("LFNone", i64::from(lf::NONE)),
            ("LFSingleLine", i64::from(lf::SINGLE_LINE)),
            ("LFMultiLine", i64::from(lf::MULTI_LINE)),
            ("LFPreserveLines", i64::from(lf::PRESERVE_LINES)),
            ("LFLinesMask", i64::from(lf::LINES_MASK)),
            ("LFNotDelimited", i64::from(lf::NOT_DELIMITED)),
            ("LFBarDelimited", i64::from(lf::BAR_DELIMITED)),
            ("LFAmpersandDelimited", i64::from(lf::AMPERSAND_DELIMITED)),
            ("LFCommaDelimited", i64::from(lf::COMMA_DELIMITED)),
            ("LFAsteriskDelimited", i64::from(lf::ASTERISK_DELIMITED)),
            ("LFDelimitersMask", i64::from(lf::DELIMITERS_MASK)),
            ("LFAllowTrailingComma", i64::from(lf::ALLOW_TRAILING_COMMA)),
            ("LFIndented", i64::from(lf::INDENTED)),
            ("LFSpaceBetweenBraces", i64::from(lf::SPACE_BETWEEN_BRACES)),
            (
                "LFSpaceBetweenSiblings",
                i64::from(lf::SPACE_BETWEEN_SIBLINGS),
            ),
            ("LFBraces", i64::from(lf::BRACES)),
            ("LFParenthesis", i64::from(lf::PARENTHESIS)),
            ("LFAngleBrackets", i64::from(lf::ANGLE_BRACKETS)),
            ("LFSquareBrackets", i64::from(lf::SQUARE_BRACKETS)),
            ("LFBracketsMask", i64::from(lf::BRACKETS_MASK)),
            ("LFOptionalIfNil", i64::from(lf::OPTIONAL_IF_NIL)),
            ("LFOptionalIfEmpty", i64::from(lf::OPTIONAL_IF_EMPTY)),
            ("LFOptional", i64::from(lf::OPTIONAL)),
            ("LFPreferNewLine", i64::from(lf::PREFER_NEW_LINE)),
            ("LFNoTrailingNewLine", i64::from(lf::NO_TRAILING_NEW_LINE)),
            (
                "LFNoInterveningComments",
                i64::from(lf::NO_INTERVENING_COMMENTS),
            ),
            ("LFNoSpaceIfEmpty", i64::from(lf::NO_SPACE_IF_EMPTY)),
            ("LFSingleElement", i64::from(lf::SINGLE_ELEMENT)),
            ("LFSpaceAfterList", i64::from(lf::SPACE_AFTER_LIST)),
            ("LFModifiers", i64::from(lf::MODIFIERS)),
            ("LFHeritageClauses", i64::from(lf::HERITAGE_CLAUSES)),
            (
                "LFSingleLineTypeLiteralMembers",
                i64::from(lf::SINGLE_LINE_TYPE_LITERAL_MEMBERS),
            ),
            (
                "LFMultiLineTypeLiteralMembers",
                i64::from(lf::MULTI_LINE_TYPE_LITERAL_MEMBERS),
            ),
            (
                "LFSingleLineTupleTypeElements",
                i64::from(lf::SINGLE_LINE_TUPLE_TYPE_ELEMENTS),
            ),
            (
                "LFMultiLineTupleTypeElements",
                i64::from(lf::MULTI_LINE_TUPLE_TYPE_ELEMENTS),
            ),
            (
                "LFUnionTypeConstituents",
                i64::from(lf::UNION_TYPE_CONSTITUENTS),
            ),
            (
                "LFIntersectionTypeConstituents",
                i64::from(lf::INTERSECTION_TYPE_CONSTITUENTS),
            ),
            (
                "LFObjectBindingPatternElements",
                i64::from(lf::OBJECT_BINDING_PATTERN_ELEMENTS),
            ),
            (
                "LFArrayBindingPatternElements",
                i64::from(lf::ARRAY_BINDING_PATTERN_ELEMENTS),
            ),
            (
                "LFObjectLiteralExpressionProperties",
                i64::from(lf::OBJECT_LITERAL_EXPRESSION_PROPERTIES),
            ),
            ("LFImportAttributes", i64::from(lf::IMPORT_ATTRIBUTES)),
            (
                "LFArrayLiteralExpressionElements",
                i64::from(lf::ARRAY_LITERAL_EXPRESSION_ELEMENTS),
            ),
            ("LFCommaListElements", i64::from(lf::COMMA_LIST_ELEMENTS)),
            (
                "LFCallExpressionArguments",
                i64::from(lf::CALL_EXPRESSION_ARGUMENTS),
            ),
            (
                "LFNewExpressionArguments",
                i64::from(lf::NEW_EXPRESSION_ARGUMENTS),
            ),
            (
                "LFTemplateExpressionSpans",
                i64::from(lf::TEMPLATE_EXPRESSION_SPANS),
            ),
            (
                "LFSingleLineBlockStatements",
                i64::from(lf::SINGLE_LINE_BLOCK_STATEMENTS),
            ),
            (
                "LFMultiLineBlockStatements",
                i64::from(lf::MULTI_LINE_BLOCK_STATEMENTS),
            ),
            (
                "LFVariableDeclarationList",
                i64::from(lf::VARIABLE_DECLARATION_LIST),
            ),
            (
                "LFSingleLineFunctionBodyStatements",
                i64::from(lf::SINGLE_LINE_FUNCTION_BODY_STATEMENTS),
            ),
            (
                "LFMultiLineFunctionBodyStatements",
                i64::from(lf::MULTI_LINE_FUNCTION_BODY_STATEMENTS),
            ),
            (
                "LFClassHeritageClauses",
                i64::from(lf::CLASS_HERITAGE_CLAUSES),
            ),
            ("LFClassMembers", i64::from(lf::CLASS_MEMBERS)),
            ("LFInterfaceMembers", i64::from(lf::INTERFACE_MEMBERS)),
            ("LFEnumMembers", i64::from(lf::ENUM_MEMBERS)),
            ("LFCaseBlockClauses", i64::from(lf::CASE_BLOCK_CLAUSES)),
            (
                "LFNamedImportsOrExportsElements",
                i64::from(lf::NAMED_IMPORTS_OR_EXPORTS_ELEMENTS),
            ),
            (
                "LFJsxElementOrFragmentChildren",
                i64::from(lf::JSX_ELEMENT_OR_FRAGMENT_CHILDREN),
            ),
            (
                "LFJsxElementAttributes",
                i64::from(lf::JSX_ELEMENT_ATTRIBUTES),
            ),
            (
                "LFCaseOrDefaultClauseStatements",
                i64::from(lf::CASE_OR_DEFAULT_CLAUSE_STATEMENTS),
            ),
            (
                "LFHeritageClauseTypes",
                i64::from(lf::HERITAGE_CLAUSE_TYPES),
            ),
            (
                "LFSourceFileStatements",
                i64::from(lf::SOURCE_FILE_STATEMENTS),
            ),
            ("LFDecorators", i64::from(lf::DECORATORS)),
            ("LFTypeArguments", i64::from(lf::TYPE_ARGUMENTS)),
            ("LFTypeParameters", i64::from(lf::TYPE_PARAMETERS)),
            ("LFParameters", i64::from(lf::PARAMETERS)),
            (
                "LFSingleArrowParameter",
                i64::from(lf::SINGLE_ARROW_PARAMETER),
            ),
            (
                "LFIndexSignatureParameters",
                i64::from(lf::INDEX_SIGNATURE_PARAMETERS),
            ),
            ("LFJSDocComment", i64::from(lf::JSDOC_COMMENT)),
            (
                "LFImportClauseEntries",
                i64::from(lf::IMPORT_CLAUSE_ENTRIES),
            ),
        ],
    );
}

#[test]
fn literal_text_flags_type_precedence_and_newlines_match_the_pinned_go_values() {
    check(
        "printer.getLiteralTextFlags",
        &[
            (
                "getLiteralTextFlagsNone",
                i64::from(LiteralEscapeFlags::NONE.bits()),
            ),
            (
                "getLiteralTextFlagsNeverAsciiEscape",
                i64::from(LiteralEscapeFlags::NEVER_ASCII_ESCAPE.bits()),
            ),
            (
                "getLiteralTextFlagsJsxAttributeEscape",
                i64::from(LiteralEscapeFlags::JSX_ATTRIBUTE_ESCAPE.bits()),
            ),
            (
                "getLiteralTextFlagsTerminateUnterminatedLiterals",
                i64::from(LiteralEscapeFlags::TERMINATE_UNTERMINATED_LITERALS.bits()),
            ),
            (
                "getLiteralTextFlagsAllowNumericSeparator",
                i64::from(LiteralEscapeFlags::ALLOW_NUMERIC_SEPARATOR.bits()),
            ),
        ],
    );
    check(
        "ast.TypePrecedence",
        &[
            (
                "TypePrecedenceConditional",
                TypePrecedence::Conditional as i64,
            ),
            ("TypePrecedenceJSDoc", TypePrecedence::JsDoc as i64),
            ("TypePrecedenceFunction", TypePrecedence::Function as i64),
            ("TypePrecedenceUnion", TypePrecedence::Union as i64),
            (
                "TypePrecedenceIntersection",
                TypePrecedence::Intersection as i64,
            ),
            (
                "TypePrecedenceTypeOperator",
                TypePrecedence::TypeOperator as i64,
            ),
            ("TypePrecedencePostfix", TypePrecedence::Postfix as i64),
            ("TypePrecedenceNonArray", TypePrecedence::NonArray as i64),
            ("TypePrecedenceLowest", TypePrecedence::LOWEST as i64),
            ("TypePrecedenceHighest", TypePrecedence::HIGHEST as i64),
        ],
    );
    check(
        "core.NewLineKind",
        &[
            ("NewLineKindNone", i64::from(NewLineKind::NONE.0)),
            ("NewLineKindCRLF", i64::from(NewLineKind::CRLF.0)),
            ("NewLineKindLF", i64::from(NewLineKind::LF.0)),
        ],
    );
}
