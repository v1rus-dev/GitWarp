//! Idle-lane collapse geometry.
//!
//! Lanes whose history is a single short branch are invisible noise in a
//! commit graph: they carry one commit dot and then join another lane. The UI
//! can fold a run of such lanes into one narrow gap lane. [`IdleLanes`]
//! accumulates the per-column statistics needed for that decision row by row
//! and turns them into x positions with [`IdleLanes::geometry`].
//!
//! A column is *occupied* in a row when the commit dot or any segment of the
//! row touches it. A *lane instance* is one occupancy of the column: a lane
//! that leaves the column (a lane change, or the commit's own dot with nothing
//! carrying down) ends the instance even when another lane takes the column on
//! the next row. An instance is *idle* when it holds at most one commit dot and
//! that dot is not a merge. Passing lanes and lines joining another column do
//! not count as a dot. A column is *foldable* when it has at least one instance
//! and every instance so far is idle.

use crate::Row;

/// Width of one unfolded lane, px.
pub const LANE_WIDTH: f32 = 14.0;

/// Width of the gap lane that replaces a run of folded lanes, px.
pub const FOLDED_WIDTH: f32 = 12.0;

/// Distance from a lane's left edge to its centre, px.
const LANE_CENTRE: f32 = LANE_WIDTH / 2.0;

/// Distance from a folded gap's left edge to its centre, px.
const FOLDED_CENTRE: f32 = FOLDED_WIDTH / 2.0;

/// A run of at least this many consecutive foldable columns is folded.
const MIN_FOLDED_RUN: usize = 3;

/// Accumulates per-column idle statistics row by row.
///
/// Feed every emitted row in order; memory is O(lanes), independent of the
/// number of rows.
#[derive(Debug, Clone, Default)]
pub struct IdleLanes {
    /// Whether the column has had an instance at all.
    has_instance: Vec<bool>,
    /// Whether every completed instance of the column was idle.
    all_idle: Vec<bool>,
    /// Whether the last row carried the column down to the next row (the
    /// instance is open).
    open: Vec<bool>,
    /// Commit dots in the open instance.
    dots: Vec<u32>,
    /// Scratch marks: which columns the current row touches.
    occupied: Vec<bool>,
    /// Scratch marks: which columns carry a lane down to the next row.
    continuing: Vec<bool>,
}

impl IdleLanes {
    /// A tracker with no rows pushed yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed every emitted row, in order.
    pub fn push<Id>(&mut self, row: &Row<Id>) {
        if row.width > self.has_instance.len() {
            let lanes = row.width;
            self.has_instance.resize(lanes, false);
            self.all_idle.resize(lanes, true);
            self.open.resize(lanes, false);
            self.dots.resize(lanes, 0);
            self.occupied.resize(lanes, false);
            self.continuing.resize(lanes, false);
        }
        // Mark the columns this row touches once, then walk the columns once:
        // rescanning every segment for every column would be quadratic in the
        // lane count.
        self.occupied.fill(false);
        self.occupied[row.column] = true;
        for s in row.above.iter().chain(&row.below) {
            self.occupied[s.from] = true;
            self.occupied[s.to] = true;
        }
        // A lane is only present at the bottom edge when it carries straight
        // down in its column. A lane change (`c -> d`) leaves the column, and a
        // line entering a column is a new lane there, not a continuation.
        self.continuing.fill(false);
        for s in &row.below {
            if s.from == s.to {
                self.continuing[s.from] = true;
            }
        }
        for col in 0..self.has_instance.len() {
            if !self.occupied[col] {
                self.open[col] = false;
                continue;
            }
            if !self.open[col] {
                self.open[col] = true;
                self.has_instance[col] = true;
                self.dots[col] = 0;
            }
            if row.column == col {
                // A second dot or a merge dot makes the instance non-idle.
                if self.dots[col] > 0 || row.is_merge {
                    self.all_idle[col] = false;
                }
                self.dots[col] += 1;
            }
        }
        // A lane that does not carry down its column closes its instance here,
        // even when another lane occupies the same column on the next row: the
        // two occupancies are separate instances with their own dots.
        for col in 0..self.open.len() {
            if !self.continuing[col] {
                self.open[col] = false;
            }
        }
    }

    /// Column geometry for the rows pushed so far.
    ///
    /// With `collapse` false every column is `LANE_WIDTH` wide. With it true,
    /// each run of at least three consecutive foldable columns becomes one
    /// `FOLDED_WIDTH` gap lane shared by all its columns.
    pub fn geometry(&self, collapse: bool) -> ColumnGeometry {
        let lanes = self.has_instance.len();
        let mut xs = Vec::with_capacity(lanes);
        let mut folded = vec![false; lanes];
        let mut folded_count = 0;
        let mut x = 0.0;
        let mut col = 0;
        while col < lanes {
            let run = if collapse && self.foldable(col) {
                (col..lanes).take_while(|&c| self.foldable(c)).count()
            } else {
                0
            };
            if run >= MIN_FOLDED_RUN {
                let centre = x + FOLDED_CENTRE;
                for _ in col..col + run {
                    xs.push(centre);
                }
                folded[col..col + run].fill(true);
                folded_count += run;
                x += FOLDED_WIDTH;
                col += run;
            } else {
                xs.push(x + LANE_CENTRE);
                x += LANE_WIDTH;
                col += 1;
            }
        }
        ColumnGeometry {
            xs,
            folded,
            width: x,
            folded_count,
        }
    }

    /// Whether the column has an instance and all of them are idle.
    fn foldable(&self, col: usize) -> bool {
        self.has_instance[col] && self.all_idle[col]
    }
}

/// Per-column x positions and the total width, in px.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnGeometry {
    /// x of each column's centre, px, for columns 0..lanes.
    pub xs: Vec<f32>,
    /// true for columns folded into a gap lane.
    pub folded: Vec<bool>,
    /// Total width, px.
    pub width: f32,
    /// Number of folded columns.
    pub folded_count: usize,
}
