use crate::TextRange;
impl TextRange {
    /// port: tsc/internal/core/text.go:UndefinedTextRange
    pub const fn undefined() -> Self {
        Self::new(-1, -1)
    }
    /// One nonnegative endpoint is sufficient, including inverted ranges.
    /// port: tsc/internal/core/text.go:TextRange.IsValid
    pub fn is_valid(self) -> bool {
        self.pos >= 0 || self.end >= 0
    }
    /// port: tsc/internal/core/text.go:TextRange.Contains
    pub fn contains(self, pos: i64) -> bool {
        self.pos() <= pos && pos < self.end()
    }
    /// port: tsc/internal/core/text.go:TextRange.ContainsInclusive
    pub fn contains_inclusive(self, pos: i64) -> bool {
        self.pos() <= pos && pos <= self.end()
    }
    /// port: tsc/internal/core/text.go:TextRange.ContainsExclusive
    pub fn contains_exclusive(self, pos: i64) -> bool {
        self.pos() < pos && pos < self.end()
    }
    /// port: tsc/internal/core/text.go:TextRange.WithPos
    #[must_use]
    pub fn with_pos(self, pos: i64) -> Self {
        Self::new(pos, self.end())
    }
    /// port: tsc/internal/core/text.go:TextRange.WithEnd
    #[must_use]
    pub fn with_end(self, end: i64) -> Self {
        Self::new(self.pos(), end)
    }
    /// port: tsc/internal/core/text.go:TextRange.ContainedBy
    pub fn contained_by(self, other: Self) -> bool {
        other.pos <= self.pos && self.end <= other.end
    }
    /// port: tsc/internal/core/text.go:TextRange.Overlaps
    pub fn overlaps(self, other: Self) -> bool {
        self.pos.max(other.pos) < self.end.min(other.end)
    }
    /// port: tsc/internal/core/text.go:TextRange.Intersects
    pub fn intersects(self, other: Self) -> bool {
        self.pos.max(other.pos) <= self.end.min(other.end)
    }
    /// The signed difference is observable, not only its ordering sign.
    /// port: tsc/internal/core/text.go:CompareTextRanges
    pub fn compare(self, other: Self) -> i64 {
        let pos = self.pos() - other.pos();
        if pos != 0 {
            pos
        } else {
            self.end() - other.end()
        }
    }
}
