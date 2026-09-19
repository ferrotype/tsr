use std::ops::ControlFlow;

use ts_arena::Counters;

use crate::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Node(NodeId),
    List(NodeListId),
    Raw(NodeSlice),
}

#[derive(Default)]
struct Record {
    edges: Vec<Edge>,
    stop_after: Option<usize>,
}
impl Record {
    fn push(&mut self, edge: Edge) -> ControlFlow<()> {
        self.edges.push(edge);
        if self.stop_after == Some(self.edges.len()) {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }
}
impl ChildVisitor for Record {
    fn visit_node(&mut self, node: NodeId) -> ControlFlow<()> {
        self.push(Edge::Node(node))
    }
    fn visit_list(&mut self, nodes: NodeListId) -> ControlFlow<()> {
        self.push(Edge::List(nodes))
    }
    fn visit_node_slice(&mut self, nodes: NodeSlice) -> ControlFlow<()> {
        self.push(Edge::Raw(nodes))
    }
}

fn ids() -> (AstBuilder, [NodeId; 4]) {
    let mut builder = AstBuilder::new(
        ts_jsstring::SourceText::from_loaded_bytes(&b""[..]),
        &Counters::default(),
    );
    let ids = std::array::from_fn(|_| builder.new_token(SyntaxKind::Unknown.into()));
    (builder, ids)
}

#[test]
fn syntax_kind_values_round_trip_and_markers_keep_upstream_values() {
    assert_eq!(SyntaxKind::Unknown as u16, 0);
    assert_eq!(SyntaxKind::EndOfFile as u16, 1);
    assert_eq!(SyntaxKind::NumericLiteral as u16, 8);
    assert_eq!(SyntaxKind::FirstToken, SyntaxKind::Unknown);
    assert_eq!(SyntaxKind::LastToken, SyntaxKind::DeferKeyword);
    for kind in SyntaxKind::ALL {
        assert_eq!(SyntaxKind::from_u16(*kind as u16), Some(*kind));
        assert!(!kind.as_str().is_empty());
    }
    assert_eq!(SyntaxKind::from_u16(SyntaxKind::COUNT as u16), None);
    assert_eq!(SyntaxKind::from_u16(u16::MAX), None);
    assert!(SyntaxKind::PlusToken.is_binary_operator());
    assert!(!SyntaxKind::SourceFile.is_binary_operator());
}

#[test]
fn validates_kind_data_pair_without_losing_text_bytes() {
    let data = IdentifierData {
        text: JsString::from_bytes(&b"\xed\xa0\x80\xff"[..]),
    };
    let node = Node::new(SyntaxKind::Identifier, -1, -1, data.clone().into()).unwrap();
    assert_eq!(
        node.data_source().as_identifier().unwrap().text(),
        b"\xed\xa0\x80\xff"
    );
    assert!(node.data_source().as_binary_expression().is_none());
    assert!(Node::new(SyntaxKind::StringLiteral, 0, 0, data.into()).is_err());
    assert_eq!((node.pos(), node.end()), (-1, -1));
}

#[test]
fn optional_schema_kind_is_a_go_scalar_and_flags_keep_all_bits() {
    for phase_modifier in [
        SyntaxKind::Unknown,
        SyntaxKind::TypeKeyword,
        SyntaxKind::DeferKeyword,
    ] {
        let mut node = Node::new(
            SyntaxKind::ImportClause,
            0,
            0,
            ImportClauseData {
                phase_modifier: phase_modifier.into(),
                name: None,
                named_bindings: None,
            }
            .into(),
        )
        .unwrap();
        assert_eq!(node.flags(), 0);
        node.set_flags(u32::MAX);
        assert_eq!(node.flags(), u32::MAX);
        assert_eq!(
            node.data_source()
                .as_import_clause()
                .unwrap()
                .phase_modifier(),
            phase_modifier
        );
    }
}

#[test]
fn visitor_preserves_list_boundary_order_and_short_circuits() {
    let (mut builder, [left, operator, right, type_node]) = ids();
    let list = builder
        .new_list(ts_core::TextRange::new(-1, -1), NodeSlice::empty())
        .unwrap();
    let binary: NodeData = BinaryExpressionData {
        modifiers: Some(list),
        left: Some(left),
        r#type: Some(type_node),
        operator_token: Some(operator),
        right: Some(right),
    }
    .into();
    let mut record = Record::default();
    assert_eq!(
        binary.for_each_child(&mut record),
        ControlFlow::Continue(())
    );
    assert_eq!(
        record.edges,
        [
            Edge::List(list),
            Edge::Node(left),
            Edge::Node(type_node),
            Edge::Node(operator),
            Edge::Node(right)
        ]
    );
    let mut record = Record {
        stop_after: Some(2),
        ..Record::default()
    };
    assert_eq!(binary.for_each_child(&mut record), ControlFlow::Break(()));
    assert_eq!(record.edges, [Edge::List(list), Edge::Node(left)]);
}

#[test]
fn default_clause_can_have_nil_expression_despite_required_schema_property() {
    let (mut builder, _) = ids();
    let statements = builder
        .new_list(ts_core::TextRange::new(-1, -1), NodeSlice::empty())
        .unwrap();
    let node = Node::new(
        SyntaxKind::DefaultClause,
        0,
        0,
        CaseOrDefaultClauseData {
            expression: None,
            statements: Some(statements),
        }
        .into(),
    )
    .unwrap();
    let mut record = Record::default();
    assert_eq!(node.for_each_child(&mut record), ControlFlow::Continue(()));
    assert_eq!(record.edges, [Edge::List(statements)]);
}

#[test]
fn jsdoc_visit_order_is_runtime_dependent_and_mapping_keeps_factory_order() {
    let (mut builder, [tag, name, typ, replacement]) = ids();
    let comments = builder.node_slice(vec![Some(tag)]).unwrap();
    let comment = builder
        .new_list(ts_core::TextRange::new(-1, -1), comments)
        .unwrap();
    for is_name_first in [false, true] {
        let data: NodeData = JSDocParameterOrPropertyTagData {
            tag_name: Some(tag),
            comment: Some(comment),
            name: Some(name),
            is_bracketed: false,
            type_expression: Some(typ),
            is_name_first,
        }
        .into();
        let mut record = Record::default();
        assert_eq!(data.for_each_child(&mut record), ControlFlow::Continue(()));
        let (first, second) = if is_name_first {
            (name, typ)
        } else {
            (typ, name)
        };
        assert_eq!(
            record.edges,
            [
                Edge::Node(tag),
                Edge::Node(first),
                Edge::Node(second),
                Edge::List(comment)
            ]
        );
        struct Mapper {
            seen: Vec<NodeId>,
            replacement: NodeId,
        }
        impl ChildMapper for Mapper {
            fn map_node(&mut self, id: NodeId, role: ChildRole) -> NodeId {
                assert_eq!(role, ChildRole::Node);
                self.seen.push(id);
                self.replacement
            }
            fn map_list(&mut self, list: NodeListId, role: ChildRole) -> NodeListId {
                assert_eq!(role, ChildRole::Nodes);
                list
            }
            fn map_node_slice(&mut self, nodes: NodeSlice, _: ChildRole) -> NodeSlice {
                nodes
            }
        }
        let mut mapper = Mapper {
            seen: Vec::new(),
            replacement,
        };
        let mapped = data.map_children(&mut mapper);
        assert_eq!(mapper.seen, [tag, name, typ]);
        assert_eq!(
            mapped.as_js_doc_parameter_or_property_tag().unwrap().name,
            Some(replacement)
        );
        assert_eq!(
            data.as_js_doc_parameter_or_property_tag().unwrap().name,
            Some(name)
        );
    }
}

#[test]
fn range_checks_arithmetic_without_claiming_owner_validation() {
    let (builder, _) = ids();
    let owner = builder.id().arena();
    assert!(NodeListRange::new(owner, u32::MAX, 1).is_none());
    assert!(NodeListRange::new(owner, u32::MAX, 0).unwrap().is_empty());
    let range = NodeListRange::new(owner, 10, 20).unwrap();
    assert_eq!((range.owner(), range.start(), range.len()), (owner, 10, 20));
}

#[test]
fn mapping_preserves_special_source_file_and_statement_roles() {
    let (mut builder, [eof, condition, then_statement, else_statement]) = ids();
    let edges = builder
        .node_slice(vec![Some(then_statement), Some(else_statement)])
        .unwrap();
    let statements = builder
        .new_list(ts_core::TextRange::new(-1, -1), edges)
        .unwrap();
    struct Roles(Vec<ChildRole>);
    impl ChildMapper for Roles {
        fn map_node(&mut self, node: NodeId, role: ChildRole) -> NodeId {
            self.0.push(role);
            node
        }
        fn map_list(&mut self, list: NodeListId, role: ChildRole) -> NodeListId {
            self.0.push(role);
            list
        }
        fn map_node_slice(&mut self, nodes: NodeSlice, role: ChildRole) -> NodeSlice {
            self.0.push(role);
            nodes
        }
    }
    let source: NodeData = SourceFileData {
        statements: Some(statements),
        end_of_file_token: Some(eof),
    }
    .into();
    let mut roles = Roles(Vec::new());
    assert_eq!(source.map_children(&mut roles), source);
    assert_eq!(roles.0, [ChildRole::TopLevelStatements, ChildRole::Token]);

    let statement: NodeData = IfStatementData {
        expression: Some(condition),
        then_statement: Some(then_statement),
        else_statement: Some(else_statement),
    }
    .into();
    let mut roles = Roles(Vec::new());
    assert_eq!(statement.map_children(&mut roles), statement);
    assert_eq!(
        roles.0,
        [
            ChildRole::Node,
            ChildRole::EmbeddedStatement,
            ChildRole::EmbeddedStatement
        ]
    );
    let raw: NodeData = SyntaxListData { children: edges }.into();
    let mut roles = Roles(Vec::new());
    assert_eq!(raw.map_children(&mut roles), raw);
    assert_eq!(roles.0, [ChildRole::RawNodes]);
}

#[test]
fn large_payloads_do_not_inflate_every_enum_slot() {
    assert!(std::mem::size_of::<NodeData>() <= 72);
    assert!(std::mem::size_of::<Node>() <= 96);
    assert!(DEFERRED_FIELDS
        .iter()
        .any(|field| field.node == "SyntheticExpression"
            && field.field == "Type"
            && field.owner == "checker type"));
    assert!(!DEFERRED_FIELDS.iter().any(|field| field.field == "Text"));
}
