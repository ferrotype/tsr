package baseline

// Phase 3 overlay hook. When set, Run hands the composed baseline text to the
// observer instead of writing and comparing it; the pinned writers compose the
// text exactly as they do for the committed references.
var Phase3Observe func(fileName string, actual string, opts Options) bool
