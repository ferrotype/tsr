// Phase 1 mutation witnesses: per-row, per-segment Go reach.
//
// scripts/phase1_mutation_go.py copies this file into the main package of an
// instrumented native oracle (built with -cover -covermode=atomic and a
// -coverpkg list that includes the main package) inside a git-archive export
// of the pin, and splices phase1CoverBegin/phase1CoverEnd calls around the
// pinned stages. It is never compiled into the plain oracles whose output is
// frozen, and it never touches upstream/.
//
// A segment is a named interval of one request: counters are cleared when it
// begins and snapshotted when it ends, so work that runs between segments
// (framing, graph observers outside a bracket) never reaches a row. Each
// snapshot is appended to the file named by PHASE1_COVER_STREAM as one record:
//
//	u32 little-endian id length, id bytes,
//	u16 little-endian segment length, segment bytes,
//	u32 little-endian payload length, payload
//
// where payload is exactly what runtime/coverage.WriteCounters emits (a
// counter-data file with one segment). The meta-data file is written once to
// PHASE1_COVER_META. With PHASE1_COVER_STREAM unset every hook is inert.
package main

import (
	"bytes"
	"encoding/binary"
	"fmt"
	"os"
	"runtime/coverage"
)

type phase1CoverState struct {
	file    *os.File
	segment string
	open    bool
	payload bytes.Buffer
	record  bytes.Buffer
}

var phase1Cover *phase1CoverState
var phase1CoverChecked bool

func phase1CoverMust(err error) {
	if err != nil {
		panic(fmt.Sprintf("phase1 cover: %v", err))
	}
}

// phase1CoverActive opens the stream lazily: the coverage runtime registers its
// meta-data in main.init, after package-level variables are initialized.
func phase1CoverActive() *phase1CoverState {
	if phase1CoverChecked {
		return phase1Cover
	}
	phase1CoverChecked = true
	path := os.Getenv("PHASE1_COVER_STREAM")
	if path == "" {
		return nil
	}
	meta := os.Getenv("PHASE1_COVER_META")
	if meta == "" {
		panic("phase1 cover: PHASE1_COVER_META is required with PHASE1_COVER_STREAM")
	}
	phase1CoverMust(os.MkdirAll(meta, 0o755))
	phase1CoverMust(coverage.WriteMetaDir(meta))
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o644)
	phase1CoverMust(err)
	phase1Cover = &phase1CoverState{file: file}
	return phase1Cover
}

// phase1CoverBegin clears every counter and opens the named segment. A
// segment still open is an instrumentation bug, never silently merged.
func phase1CoverBegin(segment string) {
	state := phase1CoverActive()
	if state == nil {
		return
	}
	if state.open {
		panic("phase1 cover: segment " + state.segment + " still open at " + segment)
	}
	phase1CoverMust(coverage.ClearCounters())
	state.segment = segment
	state.open = true
}

// phase1CoverEnd snapshots the open segment for request id. It is a no-op when
// no segment is open, so a deferred end after an explicit one is harmless.
func phase1CoverEnd(id string) {
	state := phase1CoverActive()
	if state == nil || !state.open {
		return
	}
	state.open = false
	state.payload.Reset()
	phase1CoverMust(coverage.WriteCounters(&state.payload))
	state.record.Reset()
	var word [4]byte
	binary.LittleEndian.PutUint32(word[:], uint32(len(id)))
	state.record.Write(word[:])
	state.record.WriteString(id)
	var half [2]byte
	binary.LittleEndian.PutUint16(half[:], uint16(len(state.segment)))
	state.record.Write(half[:])
	state.record.WriteString(state.segment)
	binary.LittleEndian.PutUint32(word[:], uint32(state.payload.Len()))
	state.record.Write(word[:])
	state.record.Write(state.payload.Bytes())
	// One write per record: the process may end without a final flush.
	_, err := state.file.Write(state.record.Bytes())
	phase1CoverMust(err)
}
