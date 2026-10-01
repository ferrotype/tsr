//! `transformers/estransforms/classthis.go`.
use super::utilities::{is_assignment_expression, NIL};
use tsr_ast::{NodeId, RuntimeFactory, SyntaxKind as K};
use tsr_printer::EmitContext;

/// Gets whether a node is a `static {}` block containing only a single
/// assignment of the static `this` to the `_classThis` (or similar) variable
/// stored in the `classthis` property of the block's `EmitNode`.
// port: tsc/internal/transformers/estransforms/classthis.go:isClassThisAssignmentBlock
pub fn is_class_this_assignment_block(
    emit_context: &EmitContext,
    factory: &dyn RuntimeFactory,
    node: NodeId,
) -> bool {
    let read = factory.node(node);
    if read.kind() == K::ClassStaticBlockDeclaration {
        // The payload's `Body`: `Node.Body()` answers nil for a static block.
        let body = read
            .as_class_static_block_declaration()
            .expect("ClassStaticBlockDeclaration payload")
            .body()
            .expect(NIL);
        // `body.Statements.Nodes`: a nil list is a nil dereference.
        let statements = factory.node(body).statement_list().expect(NIL);
        let statements = factory.read_list(statements).nodes();
        let statements: Vec<_> = factory.read_nodes(statements).iter().collect();
        if statements.len() == 1 {
            let statement = factory.node(statements[0].expect(NIL));
            if statement.kind() == K::ExpressionStatement {
                let expression = statement.expression().expect(NIL);
                if is_assignment_expression(
                    factory, expression, true, /*excludeCompoundAssignment*/
                ) {
                    let binary = factory.node(expression);
                    let binary = binary
                        .as_binary_expression()
                        .expect("BinaryExpression payload");
                    let left = binary.left().expect(NIL);
                    let right = binary.right().expect(NIL);
                    return factory.node(left).kind() == K::Identifier
                        && emit_context.class_this(node) == Some(left)
                        && factory.node(right).kind() == K::ThisKeyword;
                }
            }
        }
    }
    false
}
