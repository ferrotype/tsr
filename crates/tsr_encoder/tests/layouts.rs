use ts_ast::SyntaxKind;
use ts_encoder::{layout, ChildType, DataType, NODE_LAYOUTS};

#[test]
fn preserves_protocol_eight_offsets_and_masks() {
    assert_eq!(ts_encoder::PROTOCOL_VERSION, 8);
    assert_eq!(ts_encoder::HEADER_SIZE, 44);
    assert_eq!(ts_encoder::HEADER_OFFSET_PARSE_OPTIONS, 20);
    assert_eq!(ts_encoder::HEADER_OFFSET_STRUCTURED_DATA, 36);
    assert_eq!(ts_encoder::HEADER_OFFSET_NODES, 40);
    assert_eq!(ts_encoder::NODE_SIZE, 28);
    assert_eq!(ts_encoder::NODE_OFFSET_FLAGS, 24);
    assert_eq!(ts_encoder::NODE_DATA_TYPE_MASK, 0xc000_0000);
    assert_eq!(ts_encoder::NODE_DATA_CHILD_MASK, 0xff);
    assert_eq!(ts_encoder::NODE_DATA_STRING_INDEX_MASK, 0x00ff_ffff);
    assert_eq!(ts_encoder::SYNTAX_KIND_NODE_LIST, u32::MAX);
    assert_eq!(DataType::String as u32, 1 << 30);
    assert_eq!(DataType::Extended as u32, 2 << 30);
}

#[test]
fn specialized_kinds_override_generic_token_metadata() {
    assert_eq!(NODE_LAYOUTS.len(), 192);
    let identifier = layout(SyntaxKind::Identifier);
    assert_eq!(identifier.name, "Identifier");
    assert_eq!(identifier.data_type, DataType::String);
    assert_eq!(identifier.text_member, Some("text"));
    assert!(!identifier.requires_custom_codec());

    assert_eq!(
        layout(SyntaxKind::StringLiteral).data_type,
        DataType::Extended
    );
    assert_eq!(layout(SyntaxKind::TrueKeyword).name, "KeywordExpression");
    assert_eq!(layout(SyntaxKind::AnyKeyword).name, "KeywordTypeNode");
    assert_eq!(layout(SyntaxKind::PlusToken).name, "Token");

    for node in NODE_LAYOUTS {
        for &kind in node.kinds {
            assert!(layout(kind).kinds.contains(&kind));
        }
    }
}

#[test]
fn preserves_child_order_list_shapes_and_shared_kind_layouts() {
    let parameter = layout(SyntaxKind::Parameter);
    assert_eq!(
        parameter
            .children
            .iter()
            .map(|child| child.name)
            .collect::<Vec<_>>(),
        [
            "modifiers",
            "dotDotDotToken",
            "name",
            "questionToken",
            "type",
            "initializer"
        ]
    );
    assert_eq!(parameter.children[0].child_type, ChildType::ModifierList);
    assert!(parameter.children[0].optional);
    assert!(!parameter.children[2].optional);
    assert!(std::ptr::eq(
        layout(SyntaxKind::ForInStatement),
        layout(SyntaxKind::ForOfStatement)
    ));
    assert_eq!(
        layout(SyntaxKind::NonNullExpression)
            .single_child()
            .unwrap()
            .name,
        "expression"
    );
    let source_file = layout(SyntaxKind::SourceFile);
    assert_eq!(source_file.children[0].child_type, ChildType::NodeList);
    assert_eq!(source_file.children[0].name, "statements");
    assert!(source_file.single_child().is_none());
}

#[test]
fn preserves_boolean_and_kind_union_bit_order_and_absent_value() {
    let attributes = layout(SyntaxKind::ImportAttributes);
    assert_eq!(attributes.common_data[0].name, "multiLine");
    assert_eq!(attributes.common_data[0].bit_position, 0);
    assert!(attributes.common_data[0].kind_values.is_empty());
    assert_eq!(attributes.common_data[1].name, "token");
    assert_eq!(attributes.common_data[1].bit_position, 1);
    assert_eq!(
        attributes.common_data[1].kind_values,
        &[SyntaxKind::WithKeyword, SyntaxKind::AssertKeyword]
    );

    let import = &layout(SyntaxKind::ImportClause).common_data[0];
    assert!(import.optional);
    assert_eq!(import.bit_width, 2);
    assert_eq!(
        import.kind_values,
        &[SyntaxKind::TypeKeyword, SyntaxKind::DeferKeyword]
    );
    let unary = &layout(SyntaxKind::PrefixUnaryExpression).common_data[0];
    assert_eq!(unary.bit_width, 3);
    assert_eq!(unary.kind_values.len(), 6);
    assert_eq!(unary.kind_values[0], SyntaxKind::PlusToken);
}

#[test]
fn keeps_custom_codec_obligations_explicit() {
    let custom: Vec<_> = NODE_LAYOUTS
        .iter()
        .filter(|node| node.requires_custom_codec())
        .map(|node| node.name)
        .collect();
    assert_eq!(
        custom,
        [
            "StringLiteral",
            "NumericLiteral",
            "BigIntLiteral",
            "RegularExpressionLiteral",
            "NoSubstitutionTemplateLiteral",
            "TemplateHead",
            "TemplateMiddle",
            "TemplateTail",
            "SyntheticExpression",
            "SourceFile"
        ]
    );
    let synthetic = layout(SyntaxKind::SyntheticExpression);
    assert!(synthetic.hand_written_common_data);
    assert!(synthetic.common_data.is_empty());
    assert_eq!(synthetic.data_type, DataType::Children);
}
