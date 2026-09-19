package jsnum

import (
    "crypto/sha256"
    "encoding/hex"
    "encoding/json"
    "math"
    "os"
    "runtime"
    "testing"
)

// Access only: exercise the pinned implementation, including its native float
// to int64 endpoint inside Exponentiate. The cast is deliberately not modeled.
func TestS08P4NumberArithmetic(t *testing.T) {
    input, err := os.ReadFile(os.Getenv("S08_REQUESTS"))
    if err != nil { t.Fatal(err) }
    var request struct { Pairs [][2]uint64 `json:"pairs"` }
    if err := json.Unmarshal(input, &request); err != nil { t.Fatal(err) }
    var values []uint64
    var conversions []int64
    for _, pair := range request.Pairs {
        base := math.Float64frombits(pair[0])
        exponent := math.Float64frombits(pair[1])
        values = append(values, math.Float64bits(float64(Number(base).Exponentiate(Number(exponent)))))
        conversions = append(conversions, int64(base))
    }
    hash := sha256.Sum256(input)
    output := map[string]any {
        "request_sha256": hex.EncodeToString(hash[:]),
        "go": runtime.Version(), "goos": runtime.GOOS, "goarch": runtime.GOARCH,
        "power_bits": values, "int64_conversions": conversions,
    }
    data, err := json.Marshal(output)
    if err != nil { t.Fatal(err) }
    if err := os.WriteFile(os.Getenv("S08_OUTPUT"), append(data, '\n'), 0644); err != nil { t.Fatal(err) }
}
