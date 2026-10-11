//go:build !darwin && !linux

package main

import "fmt"

func processPeakRSS() (string, error) {
	return "", fmt.Errorf("native peak RSS accounting is unsupported on this platform")
}
