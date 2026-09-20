// Export the oracle toolchain's public strconv predicate without transcribing it.
package main

import (
	"encoding/json"
	"os"
	"runtime"
	"strconv"
	"unicode"
)

func main() {
	ranges := [][2]int{}
	for value := 0; value <= unicode.MaxRune; value++ {
		if !strconv.IsPrint(rune(value)) {
			continue
		}
		end := value
		for end < unicode.MaxRune && strconv.IsPrint(rune(end+1)) {
			end++
		}
		ranges = append(ranges, [2]int{value, end})
		value = end
	}
	if err := json.NewEncoder(os.Stdout).Encode(struct {
		Go      string   `json:"go"`
		Unicode string   `json:"unicode"`
		Ranges  [][2]int `json:"ranges"`
	}{runtime.Version(), unicode.Version, ranges}); err != nil {
		panic(err)
	}
}
