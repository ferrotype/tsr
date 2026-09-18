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
