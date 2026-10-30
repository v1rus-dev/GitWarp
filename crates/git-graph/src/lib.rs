//! Incremental lane layout for a commit graph.
//!
//! Feed commits in topological order (children before parents, as
//! `git log --topo-order` prints them) one at a time with
//! [`GraphBuilder::push`]. Each call returns a [`Row`] describing everything
//! a renderer needs to draw that one row: where the commit dot sits and which
//! line segments cross the row. Rows never change after they are produced, so
//! the UI can lay out history lazily, page by page, while it scrolls.
//!
//! A row is drawn in two halves around the commit dot:
//!
//! ```text
//!   top edge    ── lines arriving from the row above
//!   center      ── the commit dot at `column`
//!   bottom edge ── lines leaving towards the row below
//! ```
//!
//! The crate is generic over the id type, so it does not depend on any git
//! library: use a 20-byte hash, an interned `u32`, or a `String` in tests.
//!
//! There is at most one lane per expected commit. When a commit's first parent
//! is already expected by another lane, the commit's line bends into that lane
//! on its own row instead of running a second line down to the parent; a
//! merged-in parent that is not expected yet gets a new lane in the leftmost
//! free column, which may be left of the merge commit.

use std::hash::Hash;

pub mod ascii;
pub mod collapse;

pub use collapse::{ColumnGeometry, IdleLanes, FOLDED_WIDTH, LANE_WIDTH};

/// Stable color slot of a lane. The renderer maps it onto a palette
/// (`color % palette.len()`).
pub type ColorId = u32;

/// A line segment inside one row.
///
/// `from` is the column at the segment's start and `to` the column at its end.
/// For [`Row::above`] segments go from the top edge to the center line, for
/// [`Row::below`] from the center line to the bottom edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub from: usize,
    pub to: usize,
    pub color: ColorId,
}

/// One laid-out row of the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row<Id> {
    pub id: Id,
    /// Column of the commit dot.
    pub column: usize,
    /// Color of the commit dot (the color of the lane it sits on).
    pub color: ColorId,
    /// Segments in the upper half: from the top edge down to the center.
    /// A segment ending at `column` is a child connecting into this commit;
    /// any other segment is a lane passing by (`from == to`).
    pub above: Vec<Segment>,
    /// Segments in the lower half: from the center down to the bottom edge.
    /// A segment starting at `column` leads to one of this commit's parents;
    /// any other segment is a lane passing by (`from == to`).
    pub below: Vec<Segment>,
    /// Number of columns this row occupies (max column used + 1).
    pub width: usize,
    /// True when no child points at this commit (a branch tip or the only
    /// commit of an orphan line).
    pub is_tip: bool,
    /// True for merge commits (two or more parents).
    pub is_merge: bool,
}

#[derive(Debug, Clone)]
struct Lane<Id> {
    /// The commit this lane is waiting for.
    expects: Id,
    color: ColorId,
}

/// Incremental graph layout. Cheap to keep around: memory is proportional to
/// the number of currently open lanes, not to the history size.
#[derive(Debug, Clone)]
pub struct GraphBuilder<Id> {
    lanes: Vec<Option<Lane<Id>>>,
    next_color: ColorId,
    rows_emitted: usize,
}

impl<Id> Default for GraphBuilder<Id> {
    fn default() -> Self {
        Self {
            lanes: Vec::new(),
            next_color: 0,
            rows_emitted: 0,
        }
    }
}

impl<Id: Clone + Eq + Hash> GraphBuilder<Id> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of rows produced so far.
    pub fn len(&self) -> usize {
        self.rows_emitted
    }

    pub fn is_empty(&self) -> bool {
        self.rows_emitted == 0
    }

    /// Number of lanes that are open after the last pushed row.
    pub fn open_lanes(&self) -> usize {
        self.lanes.iter().filter(|l| l.is_some()).count()
    }

    /// Lay out the next commit. `parents` must be in git order: the first
    /// parent is the one the branch continues on.
    ///
    /// At most one lane waits for any commit. A parent that is already waited
    /// for is joined on this row — the first parent bends into its waiting
    /// lane, so the commit's own column becomes free below it; the other
    /// parents join that lane too. A parent nobody waits for yet gets a new
    /// lane: the first parent's lane stays in the commit's column, the other
    /// parents take the leftmost free column anywhere except that column
    /// (left of the merge is allowed), appending when no column is free.
    pub fn push(&mut self, id: Id, parents: &[Id]) -> Row<Id> {
        // ---- upper half -------------------------------------------------
        // Every open lane either ends in this commit or passes by.
        let mut above = Vec::new();
        let mut passing = Vec::new();
        let mut incoming = Vec::new();
        for (col, lane) in self.lanes.iter().enumerate() {
            if let Some(lane) = lane {
                if lane.expects == id {
                    incoming.push((col, lane.color));
                } else {
                    passing.push((col, lane.color));
                }
            }
        }

        let is_tip = incoming.is_empty();
        // The dot goes on the leftmost lane that expects it, or into a new lane.
        let (column, color) = match incoming.first() {
            Some(&(col, color)) => (col, color),
            None => {
                let color = self.new_color();
                let col = self.alloc_lane(
                    Lane {
                        expects: id.clone(),
                        color,
                    },
                    0,
                );
                (col, color)
            }
        };

        // Children converge into the dot; all their lanes close here.
        for &(col, lane_color) in &incoming {
            above.push(Segment {
                from: col,
                to: column,
                color: lane_color,
            });
            self.lanes[col] = None;
        }
        self.lanes[column] = None;
        for &(col, lane_color) in &passing {
            above.push(Segment {
                from: col,
                to: col,
                color: lane_color,
            });
        }

        // ---- lower half -------------------------------------------------
        // Lanes passing by keep going.
        let mut below: Vec<Segment> = passing
            .iter()
            .map(|&(col, c)| Segment {
                from: col,
                to: col,
                color: c,
            })
            .collect();

        for (i, parent) in parents.iter().enumerate() {
            // Duplicate parents (git allows them) draw one line.
            if parents[..i].contains(parent) {
                continue;
            }
            if i == 0 {
                if let Some(col) = self.find_lane(parent) {
                    // A lane already waits for the first parent (the branch
                    // forks off somewhere further down): bend into it here
                    // and leave this commit's own column free.
                    let lane_color = self.lane_color(col, color);
                    below.push(Segment {
                        from: column,
                        to: col,
                        color: lane_color,
                    });
                } else {
                    // Nobody waits for the first parent yet: the branch
                    // continues straight down in its own lane.
                    self.lanes[column] = Some(Lane {
                        expects: parent.clone(),
                        color,
                    });
                    below.push(Segment {
                        from: column,
                        to: column,
                        color,
                    });
                }
            } else if let Some(col) = self.find_lane(parent) {
                // A merged-in parent that some lane already waits for: join it
                // instead of opening yet another parallel line.
                let lane_color = self.lane_color(col, color);
                below.push(Segment {
                    from: column,
                    to: col,
                    color: lane_color,
                });
            } else {
                // Merged-in parent seen for the first time: open a lane for it
                // in the leftmost free column, which may be left of the merge
                // commit but never on the commit's own column.
                let lane_color = self.new_color();
                let col = self.alloc_lane_beside(
                    Lane {
                        expects: parent.clone(),
                        color: lane_color,
                    },
                    column,
                );
                below.push(Segment {
                    from: column,
                    to: col,
                    color: lane_color,
                });
            }
        }

        self.trim();
        self.rows_emitted += 1;

        above.sort_by_key(|s| (s.from, s.to));
        below.sort_by_key(|s| (s.from, s.to));
        let width = above
            .iter()
            .chain(below.iter())
            .flat_map(|s| [s.from, s.to])
            .fold(column, usize::max)
            + 1;

        Row {
            id,
            column,
            color,
            above,
            below,
            width,
            is_tip,
            is_merge: parents.len() > 1,
        }
    }

    fn new_color(&mut self) -> ColorId {
        let c = self.next_color;
        self.next_color = self.next_color.wrapping_add(1);
        c
    }

    fn find_lane(&self, id: &Id) -> Option<usize> {
        self.lanes
            .iter()
            .position(|l| l.as_ref().is_some_and(|l| l.expects == *id))
    }

    /// Color of the lane in `col`, falling back to `default` if it is free.
    fn lane_color(&self, col: usize, default: ColorId) -> ColorId {
        self.lanes[col].as_ref().map_or(default, |l| l.color)
    }

    /// Put a lane into the leftmost free column at or right of `min`.
    fn alloc_lane(&mut self, lane: Lane<Id>, min: usize) -> usize {
        if let Some(col) = (min..self.lanes.len()).find(|&c| self.lanes[c].is_none()) {
            self.lanes[col] = Some(lane);
            return col;
        }
        while self.lanes.len() < min {
            self.lanes.push(None);
        }
        self.lanes.push(Some(lane));
        self.lanes.len() - 1
    }

    /// Put a lane into the leftmost free column other than `skip`, appending
    /// when every other column is taken.
    fn alloc_lane_beside(&mut self, lane: Lane<Id>, skip: usize) -> usize {
        if let Some(col) = (0..self.lanes.len()).find(|&c| c != skip && self.lanes[c].is_none()) {
            self.lanes[col] = Some(lane);
            return col;
        }
        self.lanes.push(Some(lane));
        self.lanes.len() - 1
    }

    fn trim(&mut self) {
        while matches!(self.lanes.last(), Some(None)) {
            self.lanes.pop();
        }
    }
}

/// Convenience: lay out a whole history at once.
pub fn layout<Id, I, P>(commits: I) -> Vec<Row<Id>>
where
    Id: Clone + Eq + Hash,
    I: IntoIterator<Item = (Id, P)>,
    P: AsRef<[Id]>,
{
    let mut builder = GraphBuilder::new();
    commits
        .into_iter()
        .map(|(id, parents)| builder.push(id, parents.as_ref()))
        .collect()
}

#[cfg(test)]
mod tests;
