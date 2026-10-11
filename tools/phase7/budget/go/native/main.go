package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"os"

	budget "github.com/microsoft/TypeScript/tsc/cmd/phase7-budget/body"
)

func main() {
	if len(os.Args) > 1 {
		if len(os.Args) != 8 || os.Args[1] != "--discover" || os.Args[2] != "--metadata" || os.Args[4] != "--input" || os.Args[6] != "--output" {
			fmt.Fprintln(os.Stderr, "usage: budget-go --discover --metadata FILE --input INPUT.json --output EVENTS.ndjson")
			os.Exit(1)
		}
		if err := budget.Discover(os.Args[3], os.Args[5], os.Args[7]); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
		return
	}
	reader := bufio.NewReader(os.Stdin)
	engine := &budget.Engine{}
	for {
		frame, readErr := readFrame(reader)
		if readErr != nil && readErr != io.EOF {
			fmt.Fprintln(os.Stderr, readErr)
			os.Exit(1)
		}
		if len(frame) == 0 && readErr == io.EOF {
			return
		}
		response, err := engine.ExecuteCommand(frame)
		if err != nil {
			response.Kind = "failure"
			response.Value = err.Error()
		}
		if err == nil && response.Kind == "checkpoint" {
			peak, peakErr := processPeakRSS()
			if peakErr != nil {
				err = peakErr
				response.Kind = "failure"
				response.Value = err.Error()
			} else {
				response.Value.(map[string]string)["process_peak_rss_before_observation_bytes"] = peak
				response.Value.(map[string]string)["process_peak_rss_authority"] = "getrusage(RUSAGE_SELF).ru_maxrss"
			}
		}
		raw, encodeErr := json.Marshal(response)
		if encodeErr != nil {
			fmt.Fprintln(os.Stderr, encodeErr)
			os.Exit(1)
		}
		if _, writeErr := os.Stdout.Write(append(raw, '\n')); writeErr != nil {
			fmt.Fprintln(os.Stderr, writeErr)
			os.Exit(1)
		}
		if err != nil {
			os.Exit(1)
		}
		if readErr == io.EOF {
			return
		}
	}
}

// A fixed reader buffer does not retain the largest prepare frame through the
// later checkpoint. Frame ownership is per command, matching the Rust child.
func readFrame(reader *bufio.Reader) ([]byte, error) {
	var frame []byte
	for {
		part, err := reader.ReadSlice('\n')
		if len(frame)+len(part) > 256*1024*1024 {
			return nil, fmt.Errorf("budget frame exceeds 256 MiB")
		}
		frame = append(frame, part...)
		if err != bufio.ErrBufferFull {
			return frame, err
		}
	}
}
