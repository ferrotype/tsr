package main

import (
	"encoding/json"
	"fmt"
	"os"

	"github.com/peter-evans/patience"
)

// Copied from tsc/internal/stringutil/util.go:SplitLines at the pin.
func SplitLines(text string) []string {
	lines := make([]string, 0)
	start := 0
	pos := 0
	for pos < len(text) {
		switch text[pos] {
		case '\r':
			if pos+1 < len(text) && text[pos+1] == '\n' {
				lines = append(lines, text[start:pos])
				pos += 2
				start = pos
				continue
			}
			fallthrough
		case '\n':
			lines = append(lines, text[start:pos])
			pos++
			start = pos
			continue
		}
		pos++
	}
	if start < len(text) {
		lines = append(lines, text[start:])
	}
	return lines
}

// tsc/internal/testutil/baseline/baseline.go:DiffText at the pin.
func DiffText(oldName string, newName string, expected string, actual string) string {
	lines := patience.Diff(SplitLines(expected), SplitLines(actual))
	return patience.UnifiedDiffTextWithOptions(lines, patience.UnifiedDiffOptions{
		Precontext:  3,
		Postcontext: 3,
		SrcHeader:   oldName,
		DstHeader:   newName,
	})
}

func main() {
	var cases [][2]string
	if err := json.NewDecoder(os.Stdin).Decode(&cases); err != nil {
		panic(err)
	}
	out := make([]string, len(cases))
	for i, c := range cases {
		out[i] = DiffText("Expected\tThe full check baseline", "Actual\twith noCheck set", c[0], c[1])
	}
	b, _ := json.Marshal(out)
	fmt.Println(string(b))
}
