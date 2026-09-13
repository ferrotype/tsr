package printer

// Read-only overlay: builds synthetic nodes from a small JSON tree schema through
// the pinned factory and prints them with the pinned printer, exactly as the
// checker's type display does. It also records the printer constants the Rust
// port transcribes. No upstream file is modified.

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"runtime"
	"testing"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
)

var s08Kinds = map[string]ast.Kind{
	"AnyKeyword": ast.KindAnyKeyword, "UnknownKeyword": ast.KindUnknownKeyword, "NumberKeyword": ast.KindNumberKeyword,
	"BigIntKeyword": ast.KindBigIntKeyword, "ObjectKeyword": ast.KindObjectKeyword, "BooleanKeyword": ast.KindBooleanKeyword,
	"StringKeyword": ast.KindStringKeyword, "SymbolKeyword": ast.KindSymbolKeyword, "VoidKeyword": ast.KindVoidKeyword,
	"UndefinedKeyword": ast.KindUndefinedKeyword, "NeverKeyword": ast.KindNeverKeyword, "IntrinsicKeyword": ast.KindIntrinsicKeyword,
	"TrueKeyword": ast.KindTrueKeyword, "FalseKeyword": ast.KindFalseKeyword, "NullKeyword": ast.KindNullKeyword, "ThisKeyword": ast.KindThisKeyword,
	"ReadonlyKeyword": ast.KindReadonlyKeyword, "KeyOfKeyword": ast.KindKeyOfKeyword, "UniqueKeyword": ast.KindUniqueKeyword,
	"AssertsKeyword": ast.KindAssertsKeyword, "AbstractKeyword": ast.KindAbstractKeyword, "PublicKeyword": ast.KindPublicKeyword,
	"PrivateKeyword": ast.KindPrivateKeyword, "ProtectedKeyword": ast.KindProtectedKeyword, "StaticKeyword": ast.KindStaticKeyword,
	"ExportKeyword": ast.KindExportKeyword, "DeclareKeyword": ast.KindDeclareKeyword, "ConstKeyword": ast.KindConstKeyword,
	"InKeyword": ast.KindInKeyword, "OutKeyword": ast.KindOutKeyword, "AccessorKeyword": ast.KindAccessorKeyword,
	"QuestionToken": ast.KindQuestionToken, "DotDotDotToken": ast.KindDotDotDotToken, "MinusToken": ast.KindMinusToken,
	"PlusToken": ast.KindPlusToken, "ExclamationToken": ast.KindExclamationToken, "TildeToken": ast.KindTildeToken,
	"PlusPlusToken": ast.KindPlusPlusToken, "MinusMinusToken": ast.KindMinusMinusToken,
}

type s08Builder struct {
	ec *EmitContext
	f  *NodeFactory
	t  *testing.T
}

func init() {
	s08Kinds["EqualsToken"] = ast.KindEqualsToken
	s08Kinds["CommaToken"] = ast.KindCommaToken
	s08Kinds["QuestionQuestionToken"] = ast.KindQuestionQuestionToken
	s08Kinds["BarBarToken"] = ast.KindBarBarToken
	s08Kinds["AmpersandAmpersandToken"] = ast.KindAmpersandAmpersandToken
	s08Kinds["BarToken"] = ast.KindBarToken
	s08Kinds["CaretToken"] = ast.KindCaretToken
	s08Kinds["AmpersandToken"] = ast.KindAmpersandToken
	s08Kinds["EqualsEqualsToken"] = ast.KindEqualsEqualsToken
	s08Kinds["LessThanToken"] = ast.KindLessThanToken
	s08Kinds["LessThanLessThanToken"] = ast.KindLessThanLessThanToken
	s08Kinds["AsteriskToken"] = ast.KindAsteriskToken
	s08Kinds["SlashToken"] = ast.KindSlashToken
	s08Kinds["AsteriskAsteriskToken"] = ast.KindAsteriskAsteriskToken
	s08Kinds["PlusEqualsToken"] = ast.KindPlusEqualsToken
}

func (b *s08Builder) kind(v any) ast.Kind {
	kind, ok := s08Kinds[v.(string)]
	if !ok {
		panic(fmt.Sprintf("unknown kind %v", v))
	}
	return kind
}

func (b *s08Builder) str(m map[string]any, key string) string {
	value, ok := m[key].(string)
	if !ok {
		panic(fmt.Sprintf("missing string %q", key))
	}
	return value
}

func (b *s08Builder) flag(m map[string]any, key string) bool {
	value, _ := m[key].(bool)
	return value
}

func (b *s08Builder) opt(v any) *ast.Node {
	if v == nil {
		return nil
	}
	return b.build(v)
}

func (b *s08Builder) optToken(m map[string]any, key string, kind ast.Kind) *ast.Node {
	if b.flag(m, key) {
		return b.f.NewToken(kind)
	}
	return nil
}

func (b *s08Builder) optKindToken(v any) *ast.Node {
	if v == nil {
		return nil
	}
	return b.f.NewToken(b.kind(v))
}

func (b *s08Builder) list(v any) *ast.NodeList {
	if v == nil {
		return nil
	}
	items := v.([]any)
	nodes := make([]*ast.Node, 0, len(items))
	for _, item := range items {
		nodes = append(nodes, b.build(item))
	}
	return b.f.NewNodeList(nodes)
}

func (b *s08Builder) modifiers(v any) *ast.ModifierList {
	if v == nil {
		return nil
	}
	items := v.([]any)
	nodes := make([]*ast.Node, 0, len(items))
	for _, item := range items {
		nodes = append(nodes, b.f.NewModifier(b.kind(item)))
	}
	return b.f.NewModifierList(nodes)
}

func (b *s08Builder) singleLine(m map[string]any, node *ast.Node) *ast.Node {
	if b.flag(m, "singleLine") {
		b.ec.SetEmitFlags(node, EFSingleLine)
	}
	return node
}

func (b *s08Builder) build(v any) *ast.Node {
	m := v.(map[string]any)
	f := b.f
	switch b.str(m, "kind") {
	case "KeywordType":
		return f.NewKeywordTypeNode(b.kind(m["keyword"]))
	case "Identifier":
		return f.NewIdentifier(b.str(m, "text"))
	case "PrivateIdentifier":
		return f.NewPrivateIdentifier(b.str(m, "text"))
	case "QualifiedName":
		return f.NewQualifiedName(b.build(m["left"]), b.build(m["right"]))
	case "ComputedPropertyName":
		return f.NewComputedPropertyName(b.build(m["expression"]))
	case "StringLiteral":
		var flags ast.TokenFlags
		if b.flag(m, "singleQuote") {
			flags = ast.TokenFlagsSingleQuote
		}
		return f.NewStringLiteral(b.str(m, "text"), flags)
	case "NumericLiteral":
		return f.NewNumericLiteral(b.str(m, "text"), ast.TokenFlagsNone)
	case "BigIntLiteral":
		return f.NewBigIntLiteral(b.str(m, "text"), ast.TokenFlagsNone)
	case "NoSubstitutionTemplateLiteral":
		return f.NewNoSubstitutionTemplateLiteral(b.str(m, "text"), ast.TokenFlagsNone)
	case "TemplateHead":
		return f.NewTemplateHead(b.str(m, "text"), b.str(m, "rawText"), ast.TokenFlagsNone)
	case "TemplateMiddle":
		return f.NewTemplateMiddle(b.str(m, "text"), b.str(m, "rawText"), ast.TokenFlagsNone)
	case "TemplateTail":
		return f.NewTemplateTail(b.str(m, "text"), b.str(m, "rawText"), ast.TokenFlagsNone)
	case "KeywordExpression":
		return f.NewKeywordExpression(b.kind(m["keyword"]))
	case "PrefixUnary":
		return f.NewPrefixUnaryExpression(b.kind(m["operator"]), b.build(m["operand"]))
	case "PropertyAccess":
		return f.NewPropertyAccessExpression(b.build(m["expression"]), nil, b.build(m["name"]), ast.NodeFlagsNone)
	case "Call":
        flags := ast.NodeFlagsNone
        if b.flag(m, "optional") { flags = ast.NodeFlagsOptionalChain }
        return f.NewCallExpression(b.build(m["expression"]), b.optToken(m, "optional", ast.KindQuestionDotToken), b.list(m["typeArguments"]), b.list(m["arguments"]), flags)
    case "ElementAccess":
        flags := ast.NodeFlagsNone
        if b.flag(m, "optional") { flags = ast.NodeFlagsOptionalChain }
        return f.NewElementAccessExpression(b.build(m["expression"]), b.optToken(m, "optional", ast.KindQuestionDotToken), b.build(m["argument"]), flags)
    case "ParenthesizedExpression":
        return f.NewParenthesizedExpression(b.build(m["expression"]))
    case "Binary":
        return f.NewBinaryExpression(nil, b.build(m["left"]), nil, f.NewToken(b.kind(m["operator"])), b.build(m["right"]))
	case "ExpressionWithTypeArguments":
		return f.NewExpressionWithTypeArguments(b.build(m["expression"]), b.list(m["typeArguments"]))
	case "UnionType":
		return f.NewUnionTypeNode(b.list(m["types"]))
	case "IntersectionType":
		return f.NewIntersectionTypeNode(b.list(m["types"]))
	case "LiteralType":
		return f.NewLiteralTypeNode(b.build(m["literal"]))
	case "TypeReference":
		return f.NewTypeReferenceNode(b.build(m["typeName"]), b.list(m["typeArguments"]))
	case "ArrayType":
		return f.NewArrayTypeNode(b.build(m["elementType"]))
	case "TupleType":
		return b.singleLine(m, f.NewTupleTypeNode(b.list(m["elements"])))
	case "NamedTupleMember":
		return f.NewNamedTupleMember(b.optToken(m, "dotDotDot", ast.KindDotDotDotToken), b.build(m["name"]), b.optToken(m, "question", ast.KindQuestionToken), b.build(m["type"]))
	case "OptionalType":
		return f.NewOptionalTypeNode(b.build(m["type"]))
	case "RestType":
		return f.NewRestTypeNode(b.build(m["type"]))
	case "TypeLiteral":
		return b.singleLine(m, f.NewTypeLiteralNode(b.list(m["members"])))
	case "PropertySignature":
		return f.NewPropertySignatureDeclaration(b.modifiers(m["modifiers"]), b.build(m["name"]), b.optToken(m, "question", ast.KindQuestionToken), b.opt(m["type"]), nil)
	case "MethodSignature":
		return f.NewMethodSignatureDeclaration(b.modifiers(m["modifiers"]), b.build(m["name"]), b.optToken(m, "question", ast.KindQuestionToken), b.list(m["typeParameters"]), b.list(m["parameters"]), b.opt(m["type"]))
	case "IndexSignature":
		return f.NewIndexSignatureDeclaration(b.modifiers(m["modifiers"]), b.list(m["parameters"]), b.opt(m["type"]))
	case "CallSignature":
		return f.NewCallSignatureDeclaration(b.list(m["typeParameters"]), b.list(m["parameters"]), b.opt(m["type"]))
	case "ConstructSignature":
		return f.NewConstructSignatureDeclaration(b.list(m["typeParameters"]), b.list(m["parameters"]), b.opt(m["type"]))
	case "FunctionType":
		return f.NewFunctionTypeNode(b.list(m["typeParameters"]), b.list(m["parameters"]), b.opt(m["type"]))
	case "ConstructorType":
		return f.NewConstructorTypeNode(b.modifiers(m["modifiers"]), b.list(m["typeParameters"]), b.list(m["parameters"]), b.opt(m["type"]))
	case "Parameter":
		return f.NewParameterDeclaration(b.modifiers(m["modifiers"]), b.optToken(m, "dotDotDot", ast.KindDotDotDotToken), b.build(m["name"]), b.optToken(m, "question", ast.KindQuestionToken), b.opt(m["type"]), b.opt(m["initializer"]))
	case "TypeParameter":
		return f.NewTypeParameterDeclaration(b.modifiers(m["modifiers"]), b.build(m["name"]), b.opt(m["constraint"]), nil, b.opt(m["default"]))
	case "ParenthesizedType":
		return f.NewParenthesizedTypeNode(b.build(m["type"]))
	case "TypeOperator":
		return f.NewTypeOperatorNode(b.kind(m["operator"]), b.build(m["type"]))
	case "IndexedAccessType":
		return f.NewIndexedAccessTypeNode(b.build(m["objectType"]), b.build(m["indexType"]))
	case "TypeQuery":
		return f.NewTypeQueryNode(b.build(m["exprName"]), b.list(m["typeArguments"]))
	case "ThisType":
		return f.NewThisTypeNode()
	case "TypePredicate":
		return f.NewTypePredicateNode(b.optToken(m, "asserts", ast.KindAssertsKeyword), b.build(m["parameterName"]), b.opt(m["type"]))
	case "ConditionalType":
		return f.NewConditionalTypeNode(b.build(m["checkType"]), b.build(m["extendsType"]), b.build(m["trueType"]), b.build(m["falseType"]))
	case "InferType":
		return f.NewInferTypeNode(b.build(m["typeParameter"]))
	case "MappedType":
		return b.singleLine(m, f.NewMappedTypeNode(b.optKindToken(m["readonlyToken"]), b.build(m["typeParameter"]), b.opt(m["nameType"]), b.optKindToken(m["questionToken"]), b.opt(m["type"]), b.list(m["members"])))
	case "TemplateLiteralType":
		return f.NewTemplateLiteralTypeNode(b.build(m["head"]), b.list(m["spans"]))
	case "TemplateLiteralTypeSpan":
		return f.NewTemplateLiteralTypeSpan(b.build(m["type"]), b.build(m["literal"]))
	case "ImportType":
		return f.NewImportTypeNode(b.flag(m, "isTypeOf"), b.build(m["argument"]), nil, b.opt(m["qualifier"]), b.list(m["typeArguments"]))
	case "JSDocNullableType":
		return f.NewJSDocNullableType(b.build(m["type"]))
	case "JSDocNonNullableType":
		return f.NewJSDocNonNullableType(b.build(m["type"]))
	case "JSDocOptionalType":
		return f.NewJSDocOptionalType(b.build(m["type"]))
	case "JSDocVariadicType":
		return f.NewJSDocVariadicType(b.build(m["type"]))
	case "JSDocAllType":
		return f.NewJSDocAllType()
	}
	panic(fmt.Sprintf("unknown tree kind %q", b.str(m, "kind")))
}

func TestS08PrintTypeNodes(t *testing.T) {
	raw, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
	if err != nil {
		t.Fatal(err)
	}
	var request struct {
		Cases []struct {
			Name    string          `json:"name"`
			Tree    json.RawMessage `json:"tree"`
			Options struct {
				RemoveComments        bool   `json:"remove_comments"`
				OmitTrailingSemicolon bool   `json:"omit_trailing_semicolon"`
				NeverAsciiEscape      bool   `json:"never_ascii_escape"`
				NewLine               string `json:"new_line"`
				Writer                string `json:"writer"`
			} `json:"options"`
		} `json:"cases"`
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&request); err != nil {
		t.Fatal(err)
	}
	var extra any
	if err := decoder.Decode(&extra); err != io.EOF {
		t.Fatalf("trailing input: %v", err)
	}
	rows := make([]map[string]any, 0, len(request.Cases))
	for _, c := range request.Cases {
		row := map[string]any{"name": c.Name}
		func() {
			defer func() {
				if value := recover(); value != nil {
					row["panic"] = fmt.Sprint(value)
				}
			}()
			var tree any
			if err := json.Unmarshal(c.Tree, &tree); err != nil {
				panic(err)
			}
			ec := NewEmitContext()
			b := &s08Builder{ec: ec, f: ec.Factory, t: t}
			node := b.build(tree)
			var writer EmitTextWriter
			if c.Options.Writer == "single_line" {
				w, put := GetSingleLineStringWriter()
				defer put()
				writer = w
			} else {
				writer = NewTextWriter(c.Options.NewLine, 0)
			}
			p := NewPrinter(PrinterOptions{RemoveComments: c.Options.RemoveComments, OmitTrailingSemicolon: c.Options.OmitTrailingSemicolon, NeverAsciiEscape: c.Options.NeverAsciiEscape}, PrintHandlers{}, ec)
			p.Write(node, nil, writer, nil)
			row["text_hex"] = hex.EncodeToString([]byte(writer.String()))
		}()
		rows = append(rows, row)
	}
	constants := map[string]map[string]int64{
		"printer.EmitFlags": {
			"EFSingleLine": int64(EFSingleLine), "EFMultiLine": int64(EFMultiLine), "EFNoLeadingSourceMap": int64(EFNoLeadingSourceMap),
			"EFNoTrailingSourceMap": int64(EFNoTrailingSourceMap), "EFNoNestedSourceMaps": int64(EFNoNestedSourceMaps),
			"EFNoTokenLeadingSourceMaps": int64(EFNoTokenLeadingSourceMaps), "EFNoTokenTrailingSourceMaps": int64(EFNoTokenTrailingSourceMaps),
			"EFNoLeadingComments": int64(EFNoLeadingComments), "EFNoTrailingComments": int64(EFNoTrailingComments), "EFNoNestedComments": int64(EFNoNestedComments),
			"EFHelperName": int64(EFHelperName), "EFExportName": int64(EFExportName), "EFLocalName": int64(EFLocalName), "EFIndented": int64(EFIndented),
			"EFNoIndentation": int64(EFNoIndentation), "EFReuseTempVariableScope": int64(EFReuseTempVariableScope), "EFCustomPrologue": int64(EFCustomPrologue),
			"EFNoAsciiEscaping": int64(EFNoAsciiEscaping), "EFExternalHelpers": int64(EFExternalHelpers), "EFStartOnNewLine": int64(EFStartOnNewLine),
			"EFIndirectCall": int64(EFIndirectCall), "EFAsyncFunctionBody": int64(EFAsyncFunctionBody), "EFNoLexicalArguments": int64(EFNoLexicalArguments),
			"EFTransformPrivateStaticElements": int64(EFTransformPrivateStaticElements), "EFNoLexicalThis": int64(EFNoLexicalThis),
			"EFNone": int64(EFNone), "EFNoSourceMap": int64(EFNoSourceMap), "EFNoTokenSourceMaps": int64(EFNoTokenSourceMaps), "EFNoComments": int64(EFNoComments),
		},
		"printer.ListFormat": {
			"LFNone": int64(LFNone), "LFSingleLine": int64(LFSingleLine), "LFMultiLine": int64(LFMultiLine), "LFPreserveLines": int64(LFPreserveLines),
			"LFLinesMask": int64(LFLinesMask), "LFNotDelimited": int64(LFNotDelimited), "LFBarDelimited": int64(LFBarDelimited),
			"LFAmpersandDelimited": int64(LFAmpersandDelimited), "LFCommaDelimited": int64(LFCommaDelimited), "LFAsteriskDelimited": int64(LFAsteriskDelimited),
			"LFDelimitersMask": int64(LFDelimitersMask), "LFAllowTrailingComma": int64(LFAllowTrailingComma), "LFIndented": int64(LFIndented),
			"LFSpaceBetweenBraces": int64(LFSpaceBetweenBraces), "LFSpaceBetweenSiblings": int64(LFSpaceBetweenSiblings), "LFBraces": int64(LFBraces),
			"LFParenthesis": int64(LFParenthesis), "LFAngleBrackets": int64(LFAngleBrackets), "LFSquareBrackets": int64(LFSquareBrackets),
			"LFBracketsMask": int64(LFBracketsMask), "LFOptionalIfNil": int64(LFOptionalIfNil), "LFOptionalIfEmpty": int64(LFOptionalIfEmpty),
			"LFOptional": int64(LFOptional), "LFPreferNewLine": int64(LFPreferNewLine), "LFNoTrailingNewLine": int64(LFNoTrailingNewLine),
			"LFNoInterveningComments": int64(LFNoInterveningComments), "LFNoSpaceIfEmpty": int64(LFNoSpaceIfEmpty), "LFSingleElement": int64(LFSingleElement),
			"LFSpaceAfterList": int64(LFSpaceAfterList), "LFModifiers": int64(LFModifiers), "LFHeritageClauses": int64(LFHeritageClauses),
			"LFSingleLineTypeLiteralMembers": int64(LFSingleLineTypeLiteralMembers), "LFMultiLineTypeLiteralMembers": int64(LFMultiLineTypeLiteralMembers),
			"LFSingleLineTupleTypeElements": int64(LFSingleLineTupleTypeElements), "LFMultiLineTupleTypeElements": int64(LFMultiLineTupleTypeElements),
			"LFUnionTypeConstituents": int64(LFUnionTypeConstituents), "LFIntersectionTypeConstituents": int64(LFIntersectionTypeConstituents),
			"LFObjectBindingPatternElements": int64(LFObjectBindingPatternElements), "LFArrayBindingPatternElements": int64(LFArrayBindingPatternElements),
			"LFObjectLiteralExpressionProperties": int64(LFObjectLiteralExpressionProperties), "LFImportAttributes": int64(LFImportAttributes),
			"LFArrayLiteralExpressionElements": int64(LFArrayLiteralExpressionElements), "LFCommaListElements": int64(LFCommaListElements),
			"LFCallExpressionArguments": int64(LFCallExpressionArguments), "LFNewExpressionArguments": int64(LFNewExpressionArguments),
			"LFTemplateExpressionSpans": int64(LFTemplateExpressionSpans), "LFSingleLineBlockStatements": int64(LFSingleLineBlockStatements),
			"LFMultiLineBlockStatements": int64(LFMultiLineBlockStatements), "LFVariableDeclarationList": int64(LFVariableDeclarationList),
			"LFSingleLineFunctionBodyStatements": int64(LFSingleLineFunctionBodyStatements), "LFMultiLineFunctionBodyStatements": int64(LFMultiLineFunctionBodyStatements),
			"LFClassHeritageClauses": int64(LFClassHeritageClauses), "LFClassMembers": int64(LFClassMembers), "LFInterfaceMembers": int64(LFInterfaceMembers),
			"LFEnumMembers": int64(LFEnumMembers), "LFCaseBlockClauses": int64(LFCaseBlockClauses), "LFNamedImportsOrExportsElements": int64(LFNamedImportsOrExportsElements),
			"LFJsxElementOrFragmentChildren": int64(LFJsxElementOrFragmentChildren), "LFJsxElementAttributes": int64(LFJsxElementAttributes),
			"LFCaseOrDefaultClauseStatements": int64(LFCaseOrDefaultClauseStatements), "LFHeritageClauseTypes": int64(LFHeritageClauseTypes),
			"LFSourceFileStatements": int64(LFSourceFileStatements), "LFDecorators": int64(LFDecorators), "LFTypeArguments": int64(LFTypeArguments),
			"LFTypeParameters": int64(LFTypeParameters), "LFParameters": int64(LFParameters), "LFSingleArrowParameter": int64(LFSingleArrowParameter),
			"LFIndexSignatureParameters": int64(LFIndexSignatureParameters), "LFJSDocComment": int64(LFJSDocComment), "LFImportClauseEntries": int64(LFImportClauseEntries),
		},
		"printer.getLiteralTextFlags": {
			"getLiteralTextFlagsNone": int64(getLiteralTextFlagsNone), "getLiteralTextFlagsNeverAsciiEscape": int64(getLiteralTextFlagsNeverAsciiEscape),
			"getLiteralTextFlagsJsxAttributeEscape": int64(getLiteralTextFlagsJsxAttributeEscape),
			"getLiteralTextFlagsTerminateUnterminatedLiterals": int64(getLiteralTextFlagsTerminateUnterminatedLiterals),
			"getLiteralTextFlagsAllowNumericSeparator": int64(getLiteralTextFlagsAllowNumericSeparator),
		},
		"ast.TypePrecedence": {
			"TypePrecedenceConditional": int64(ast.TypePrecedenceConditional), "TypePrecedenceJSDoc": int64(ast.TypePrecedenceJSDoc),
			"TypePrecedenceFunction": int64(ast.TypePrecedenceFunction), "TypePrecedenceUnion": int64(ast.TypePrecedenceUnion),
			"TypePrecedenceIntersection": int64(ast.TypePrecedenceIntersection), "TypePrecedenceTypeOperator": int64(ast.TypePrecedenceTypeOperator),
			"TypePrecedencePostfix": int64(ast.TypePrecedencePostfix), "TypePrecedenceNonArray": int64(ast.TypePrecedenceNonArray),
			"TypePrecedenceLowest": int64(ast.TypePrecedenceLowest), "TypePrecedenceHighest": int64(ast.TypePrecedenceHighest),
		},
		"core.NewLineKind": {"NewLineKindNone": int64(core.NewLineKindNone), "NewLineKindCRLF": int64(core.NewLineKindCRLF), "NewLineKindLF": int64(core.NewLineKindLF)},
	}
	sum := sha256.Sum256(raw)
	output, err := json.Marshal(map[string]any{
		"request_sha256": hex.EncodeToString(sum[:]), "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH,
		"scope": "pinned printer output for synthetic type-display trees and printer constants; not a Rust result",
		"rows": rows, "constants": constants,
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(os.Getenv("S08_OUTPUT"), output, 0o600); err != nil {
		t.Fatal(err)
	}
}
