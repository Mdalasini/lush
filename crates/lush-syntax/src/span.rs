//! Source spans for diagnostics (§11.6).

use std::fmt;
use std::ops::Range;

/// Byte offset into a UTF-8 source string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct BytePos(pub u32);

impl BytePos {
    pub fn new(offset: usize) -> Self {
        Self(offset as u32)
    }

    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// Inclusive-start, exclusive-end span in the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: BytePos,
    pub end: BytePos,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self {
            start: BytePos::new(start),
            end: BytePos::new(end),
        }
    }

    pub fn empty(at: usize) -> Self {
        Self::new(at, at)
    }

    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn range(self) -> Range<usize> {
        self.start.as_usize()..self.end.as_usize()
    }

    pub fn len(self) -> usize {
        self.end.as_usize().saturating_sub(self.start.as_usize())
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start.as_usize(), self.end.as_usize())
    }
}

impl From<Range<usize>> for Span {
    fn from(range: Range<usize>) -> Self {
        Span::new(range.start, range.end)
    }
}
