// Export the oracle toolchain's unicode.ToLower table for LowerFirstChar.
package main

import (
	"encoding/json"
	"os"
	"runtime"
	"unicode"
)

func main() {
	rows := make([][2]int32, 0)
	for r := rune(0); r <= unicode.MaxRune; r++ {
		if lower := unicode.ToLower(r); lower != r {
			rows = append(rows, [2]int32{r, lower})
		}
	}
	if err := json.NewEncoder(os.Stdout).Encode(struct {
		Go      string     `json:"go"`
		Unicode string     `json:"unicode"`
		Lower   [][2]int32 `json:"lower"`
	}{runtime.Version(), unicode.Version, rows}); err != nil {
		panic(err)
	}
}
