// Access to unexported pieces of the pinned format package, for the oracle in
// main.go. scripts/s09_format.py copies this file into the fresh export as
// internal/format/s09_bridge.go; the submodule is never touched. It adds
// exported entry points and changes nothing that exists.
package format

import (
	"fmt"
	"strconv"
	"strings"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/astnav"
	"github.com/microsoft/TypeScript/tsc/internal/ls/lsutil"
	"github.com/microsoft/TypeScript/tsc/internal/scanner"
)

func s09Range(r TextRangeWithKind) string {
	return strconv.Itoa(int(r.Kind)) + "," + strconv.Itoa(r.Loc.Pos()) + "," + strconv.Itoa(r.Loc.End())
}

func s09Ranges(ranges []TextRangeWithKind) string {
	parts := make([]string, len(ranges))
	for i, r := range ranges {
		parts[i] = s09Range(r)
	}
	return strings.Join(parts, ";")
}

// S09ScanProbe drives the formatting scanner over a whole file. The span worker
// normally supplies the container for each token; here it is the token-level
// node navigation finds at the token's start, which is what gives the rescan
// predicates (greater-than, slash, template, JSX identifier, text and attribute
// value) realistic input. A native panic inside one step ends the probe with a
// final '!' row, since the scanner's state is not meaningful after it.
func S09ScanProbe(file *ast.SourceFile, row func(string)) {
	scan := scanner.NewScanner()
	scan.SetSkipTrivia(false)
	scan.SetLanguageVariant(file.LanguageVariant)
	scan.SetText(file.Text())
	scan.ResetTokenState(0)
	s := &formattingScanner{s: scan, startPos: 0, endPos: len(file.Text()), wasNewLine: true}
	defer func() {
		if recovered := recover(); recovered != nil {
			row("K|!" + fmt.Sprint(recovered))
		}
	}()
	s.advance()
	for s.isOnToken() {
		container := astnav.GetTokenAtPosition(file, s.s.TokenStart())
		info := s.readTokenInfo(container)
		newLine := "0"
		if s.lastTrailingTriviaWasNewLine() {
			newLine = "1"
		}
		row("K|" + s09Range(info.token) + "|" + strconv.Itoa(int(container.Kind)) + "|" + newLine +
			"|" + s09Ranges(info.leadingTrivia) + "|" + s09Ranges(info.trailingTrivia))
		s.advance()
	}
	if s.isOnEOF() {
		row("E|" + s09Range(s.readEOFTokenRange()) + "|" + s09Ranges(s.getCurrentLeadingTrivia()))
	} else {
		row("E|-")
	}
}

type s09Item struct {
	span   TextRangeWithKind
	parent *ast.Node
}

func s09IsComment(kind ast.Kind) bool {
	return kind == ast.KindSingleLineCommentTrivia || kind == ast.KindMultiLineCommentTrivia
}

// s09Items is the token sequence the rule probe pairs up: every token of the
// file, with the comments of its leading and trailing trivia before and after
// it. An item's parent is the parent of the token-level node navigation finds
// at the token's start, or that node itself when it has none.
func s09Items(file *ast.SourceFile) []s09Item {
	scan := scanner.NewScanner()
	scan.SetSkipTrivia(false)
	scan.SetLanguageVariant(file.LanguageVariant)
	scan.SetText(file.Text())
	scan.ResetTokenState(0)
	s := &formattingScanner{s: scan, startPos: 0, endPos: len(file.Text()), wasNewLine: true}
	var items []s09Item
	s.advance()
	for s.isOnToken() {
		container := astnav.GetTokenAtPosition(file, s.s.TokenStart())
		parent := container
		if container.Parent != nil {
			parent = container.Parent
		}
		info := s.readTokenInfo(container)
		for _, trivia := range info.leadingTrivia {
			if s09IsComment(trivia.Kind) {
				items = append(items, s09Item{trivia, parent})
			}
		}
		items = append(items, s09Item{info.token, parent})
		for _, trivia := range info.trailingTrivia {
			if s09IsComment(trivia.Kind) {
				items = append(items, s09Item{trivia, parent})
			}
		}
		s.advance()
	}
	return items
}

func s09CommonAncestor(a *ast.Node, b *ast.Node, file *ast.SourceFile) *ast.Node {
	seen := map[*ast.Node]bool{}
	for n := a; n != nil; n = n.Parent {
		seen[n] = true
	}
	for n := b; n != nil; n = n.Parent {
		if seen[n] {
			return n
		}
	}
	return file.AsNode()
}

// S09RulesProbe asks the rules map which rules apply between every pair of
// adjacent items, in the context of their lowest common ancestor. The span
// worker sets contexts up differently; what matters here is that both
// implementations answer the same realistic questions. A native panic while
// answering one pair becomes that row's value.
func S09RulesProbe(file *ast.SourceFile, options lsutil.FormatCodeSettings, row func(string)) {
	var items []s09Item
	func() {
		defer func() {
			if recovered := recover(); recovered != nil {
				row("R|!" + fmt.Sprint(recovered))
			}
		}()
		items = s09Items(file)
	}()
	context := NewFormattingContext(file, FormatRequestKindFormatDocument, options)
	for i := 0; i+1 < len(items); i++ {
		current, next := items[i], items[i+1]
		head := "R|" + s09Range(current.span) + "|" + s09Range(next.span) + "|"
		func() {
			defer func() {
				if recovered := recover(); recovered != nil {
					row(head + "!" + fmt.Sprint(recovered))
				}
			}()
			common := s09CommonAncestor(current.parent, next.parent, file)
			context.UpdateContext(current.span, current.parent, next.span, next.parent, common)
			names := []string{}
			for _, rule := range getRules(context, nil) {
				names = append(names, rule.String())
			}
			row(head + strconv.Itoa(int(common.Kind)) + "|" + strings.Join(names, ","))
		}()
	}
}

// S09RulesMapProbe lists every non-empty bucket of the rules map in order, so
// the construction of the map is compared directly and not only through the
// questions a corpus happens to ask.
func S09RulesMapProbe(row func(string)) {
	for index, bucket := range getRulesMap() {
		if len(bucket) == 0 {
			continue
		}
		names := make([]string, len(bucket))
		for i, rule := range bucket {
			names[i] = rule.String()
		}
		row("B|" + strconv.Itoa(index/mapRowLength) + "|" + strconv.Itoa(index%mapRowLength) + "|" + strings.Join(names, ","))
	}
}
