//go:build darwin || linux

package main

import (
	"runtime"
	"strconv"
	"syscall"
)

// Unlike whole-process wait4, this peak is sampled before observation JSON.
func processPeakRSS() (string, error) {
	var usage syscall.Rusage
	if err := syscall.Getrusage(syscall.RUSAGE_SELF, &usage); err != nil {
		return "", err
	}
	peak := uint64(usage.Maxrss)
	if runtime.GOOS == "linux" {
		peak *= 1024
	}
	return strconv.FormatUint(peak, 10), nil
}
