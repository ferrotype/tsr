package checker

import (
	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/nodebuilder"
	"github.com/microsoft/TypeScript/tsc/internal/printer"
)

// Access-only observation of the native branch with its ordinary thresholds.
// Full source fixtures separately prove these branches at naturally reached lengths.
func C2PropertyElision(c *Checker, node *ast.Node, length int, noTruncation bool) any {
	typ := c.GetTypeAtLocation(node)
	emit := printer.NewEmitContext()
	b := newNodeBuilderImpl(c, emit, nil)
	var flags nodebuilder.Flags
	if noTruncation {
		flags = nodebuilder.FlagsNoTruncation
	}
	b.ctx = &NodeBuilderContext{flags: flags, approximateLength: length, maxExpansionDepth: -1}
	result := b.createTypeNodeFromObjectType(typ)
	members := []any{}
	if result.Kind == ast.KindTypeLiteral {
		for _, n := range result.AsTypeLiteralNode().Members.Nodes {
			comments := []any{}
			for _, comment := range emit.GetSyntheticTrailingComments(n) {
				comments = append(comments, map[string]any{"kind": comment.Kind, "text": comment.Text, "pos": comment.Loc.Pos(), "end": comment.Loc.End(), "leading_newline": comment.HasLeadingNewLine, "trailing_newline": comment.HasTrailingNewLine})
			}
			members = append(members, map[string]any{"kind": n.Kind, "comments": comments})
		}
	}
	writer := printer.NewTextWriter("", 0)
	printer.NewPrinter(printer.PrinterOptions{}, printer.PrintHandlers{}, emit).Write(result, nil, writer, nil)
	return map[string]any{"text": writer.String(), "kind": result.Kind, "members": members, "added_length": b.ctx.approximateLength - length, "restored_flags": b.ctx.flags == flags}
}
