//! A two-level squarified treemap (Bruls, Huizing and van Wijk): groups tile the box, and each
//! group's items tile the group.
//!
//! Layout runs in `f64` user units; every rect is then written as its EDGES rounded to tenths, so
//! two rects that share an edge round it identically and the tiles cover the box with no gap or
//! overlap beyond a tenth. Areas are proportional to value, so a tile's size is the claim the chart
//! makes.

use log::{debug, trace};

/// The treemap box, in user units: `viewBox="0 0 WIDTH HEIGHT"`.
pub const WIDTH: i64 = 1000;
pub const HEIGHT: i64 = 480;

/// Smallest tile, in user units, that carries an inline label. A smaller tile keeps its tooltip;
/// a label squeezed into it would overflow or be clipped to nonsense.
pub const MIN_LABEL_W: i64 = 130;
pub const MIN_LABEL_H: i64 = 60;

/// Narrowest labeled tile, in user units, whose label stays on a phone-width page, where the box
/// renders at roughly a third of its desktop width.
pub const MIN_NARROW_LABEL_W: i64 = 240;

/// One rect, every field in TENTHS of a user unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

impl Rect {
    /// `true` when the rect is large enough to carry an inline label.
    pub fn labeled(&self) -> bool {
        self.w >= MIN_LABEL_W * 10 && self.h >= MIN_LABEL_H * 10
    }

    /// `true` when the rect is wide enough to keep its label at phone width.
    pub fn labeled_narrow(&self) -> bool {
        self.labeled() && self.w >= MIN_NARROW_LABEL_W * 10
    }

    /// Area in tenths squared.
    pub fn area(&self) -> i64 {
        self.w * self.h
    }
}

/// One group's laid-out geometry: its own rect and one rect per item, aligned with the input.
/// A non-positive value has no rect (`None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupTiles {
    pub rect: Rect,
    pub items: Vec<Option<Rect>>,
}

/// Lay out `groups`, each `(group value, item values)`. A group's items are scaled to fill the
/// group's rect whatever they sum to, so a group whose items do not cover its whole value (an org
/// whose sessions carry no repo) still tiles; a group with no positive item is left as one tile.
///
/// Returns one entry per input group, `None` for a non-positive group. `None` overall when no group
/// is positive.
pub fn treemap(groups: &[(f64, Vec<f64>)]) -> Option<Vec<Option<GroupTiles>>> {
    let values: Vec<f64> = groups.iter().map(|(v, _)| *v).collect();
    debug!("chart::treemap::treemap: groups={}", groups.len());
    let outer = Box2 {
        x: 0.0,
        y: 0.0,
        w: WIDTH as f64,
        h: HEIGHT as f64,
    };
    let group_boxes = squarify(&values, outer)?;
    let out = groups
        .iter()
        .zip(group_boxes)
        .map(|((_, items), boxed)| {
            boxed.map(|b| GroupTiles {
                rect: b.round(),
                items: squarify(items, b)
                    .map(|inner| inner.into_iter().map(|r| r.map(Box2::round)).collect())
                    .unwrap_or_else(|| vec![None; items.len()]),
            })
        })
        .collect();
    Some(out)
}

/// A rect in `f64` user units, during layout.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Box2 {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Box2 {
    /// Round EDGES to tenths, then derive the size, so shared edges round identically.
    fn round(self) -> Rect {
        let x0 = to_tenths(self.x);
        let y0 = to_tenths(self.y);
        let x1 = to_tenths(self.x + self.w);
        let y1 = to_tenths(self.y + self.h);
        Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }
}

fn to_tenths(v: f64) -> i64 {
    (v * 10.0).round() as i64
}

/// Squarify `values` into `area`, returning one rect per input value in input order (`None` for a
/// non-positive value). `None` when no value is positive.
fn squarify(values: &[f64], area: Box2) -> Option<Vec<Option<Box2>>> {
    // Largest first, input index breaking ties, so the layout is deterministic.
    let mut order: Vec<usize> = (0..values.len()).filter(|i| values[*i] > 0.0).collect();
    order.sort_by(|a, b| values[*b].total_cmp(&values[*a]).then(a.cmp(b)));
    let total: f64 = order.iter().map(|i| values[*i]).sum();
    if order.is_empty() {
        return None;
    }
    let unit = area.w * area.h / total;
    let sizes: Vec<f64> = order.iter().map(|i| values[*i] * unit).collect();

    let mut out: Vec<Option<Box2>> = vec![None; values.len()];
    let mut free = area;
    // A bounded walk, never a `while` on a moving index: the row `start..i` grows while adding the
    // next size does not worsen its aspect ratio, and is laid out the moment it would.
    let mut start = 0;
    for i in 1..sizes.len() {
        let side = free.w.min(free.h);
        if worst(&sizes[start..=i], side) > worst(&sizes[start..i], side) {
            place(&sizes[start..i], &order[start..i], &mut free, &mut out);
            start = i;
        }
    }
    place(&sizes[start..], &order[start..], &mut free, &mut out);
    trace!("chart::treemap::squarify: placed={}", sizes.len());
    Some(out)
}

/// Lay one row and record each rect at its input index.
fn place(sizes: &[f64], indices: &[usize], free: &mut Box2, out: &mut [Option<Box2>]) {
    for (index, rect) in indices.iter().zip(lay_row(sizes, free)) {
        out[*index] = Some(rect);
    }
}

/// The worst aspect ratio in a row of `sizes` laid along a side of length `side`.
fn worst(sizes: &[f64], side: f64) -> f64 {
    let sum: f64 = sizes.iter().sum();
    let max = sizes.iter().copied().fold(f64::MIN, f64::max);
    let min = sizes.iter().copied().fold(f64::MAX, f64::min);
    let side2 = side * side;
    let sum2 = sum * sum;
    (side2 * max / sum2).max(sum2 / (side2 * min))
}

/// Lay one row along the shorter side of `free`, then shrink `free` past it.
fn lay_row(sizes: &[f64], free: &mut Box2) -> Vec<Box2> {
    let sum: f64 = sizes.iter().sum();
    let mut out = Vec::new();
    if free.w >= free.h {
        // A column on the left, items stacked top to bottom.
        let width = sum / free.h;
        let mut y = free.y;
        for size in sizes {
            let h = size / width;
            out.push(Box2 {
                x: free.x,
                y,
                w: width,
                h,
            });
            y += h;
        }
        free.x += width;
        free.w -= width;
    } else {
        // A row along the top, items left to right.
        let height = sum / free.w;
        let mut x = free.x;
        for size in sizes {
            let w = size / height;
            out.push(Box2 {
                x,
                y: free.y,
                w,
                h: height,
            });
            x += w;
        }
        free.y += height;
        free.h -= height;
    }
    out
}

#[cfg(test)]
mod tests;
