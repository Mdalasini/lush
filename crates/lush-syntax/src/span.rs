//! Source locations.

/// Byte span into the original source (UTF-8 offsets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

impl Span {
    /// Create a span from `start` (inclusive) to `end` (exclusive).
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Zero-width span at `offset`.
    pub fn point(offset: usize) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }

    /// Merge two spans into the smallest covering both.
    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// Slice `source` by this span.
    pub fn slice<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }
}
