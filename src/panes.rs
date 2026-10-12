//! Split-pane layouts: a grid of columns × rows whose sizes are fractions of
//! the editor area. Groups (panes) are numbered row by row.

use egui::{Color32, CursorIcon, Id, Rect, Sense, Stroke, Ui, pos2};
use serde::{Deserialize, Serialize};

/// Smallest pane, as a fraction of the editor area.
const MIN_FRAC: f32 = 0.08;
/// Grab width of a divider, in points.
const GRIP: f32 = 6.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    /// Column widths and row heights, each summing to 1.
    pub cols: Vec<f32>,
    pub rows: Vec<f32>,
}

impl Default for Layout {
    fn default() -> Self {
        Self::grid(1, 1)
    }
}

impl Layout {
    pub fn grid(cols: usize, rows: usize) -> Self {
        let even = |n: usize| vec![1.0 / n.max(1) as f32; n.max(1)];
        Self { cols: even(cols), rows: even(rows) }
    }

    pub fn groups(&self) -> usize {
        self.cols.len() * self.rows.len()
    }

    /// Repair a layout read from disk (wrong sums, zero sizes, too many panes).
    pub fn sanitized(mut self) -> Self {
        for v in [&mut self.cols, &mut self.rows] {
            v.truncate(4);
            if v.is_empty() || v.iter().any(|f| !f.is_finite() || *f <= 0.0) {
                *v = vec![1.0 / v.len().max(1) as f32; v.len().max(1)];
            }
            let sum: f32 = v.iter().sum();
            v.iter_mut().for_each(|f| *f /= sum);
        }
        self
    }

    fn edges(fracs: &[f32], from: f32, len: f32) -> Vec<f32> {
        let mut out = vec![from];
        let mut acc = from;
        for f in fracs {
            acc += f * len;
            out.push(acc);
        }
        out
    }

    /// Screen rect of every group, row by row.
    pub fn cells(&self, area: Rect) -> Vec<Rect> {
        let xs = Self::edges(&self.cols, area.left(), area.width());
        let ys = Self::edges(&self.rows, area.top(), area.height());
        let mut out = Vec::with_capacity(self.groups());
        for r in 0..self.rows.len() {
            for c in 0..self.cols.len() {
                out.push(Rect::from_min_max(pos2(xs[c].round(), ys[r].round()), pos2(xs[c + 1].round(), ys[r + 1].round())));
            }
        }
        out
    }

    /// Draw the dividers and let them be dragged.
    pub fn dividers(&mut self, ui: &Ui, area: Rect, color: Color32) {
        for vertical in [true, false] {
            let n = if vertical { self.cols.len() } else { self.rows.len() };
            for k in 1..n {
                let (from, len) = if vertical { (area.left(), area.width()) } else { (area.top(), area.height()) };
                let fracs = if vertical { &mut self.cols } else { &mut self.rows };
                let at = from + fracs[..k].iter().sum::<f32>() * len;
                let grip = if vertical {
                    Rect::from_min_max(pos2(at - GRIP / 2.0, area.top()), pos2(at + GRIP / 2.0, area.bottom()))
                } else {
                    Rect::from_min_max(pos2(area.left(), at - GRIP / 2.0), pos2(area.right(), at + GRIP / 2.0))
                };
                let resp = ui.interact(grip, Id::new(("pane-divider", vertical, k)), Sense::drag());
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(if vertical { CursorIcon::ResizeHorizontal } else { CursorIcon::ResizeVertical });
                }
                if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.dragged()) {
                    let pos = if vertical { p.x } else { p.y };
                    // Move the boundary between panes k-1 and k, keeping both usable.
                    let before: f32 = fracs[..k - 1].iter().sum();
                    let pair = fracs[k - 1] + fracs[k];
                    let left = ((pos - from) / len - before).clamp(MIN_FRAC, pair - MIN_FRAC);
                    fracs[k - 1] = left;
                    fracs[k] = pair - left;
                }
                let stroke = Stroke::new(if resp.dragged() { 2.0 } else { 1.0 }, color);
                if vertical {
                    ui.painter().vline(at.round() + 0.5, area.y_range(), stroke);
                } else {
                    ui.painter().hline(area.x_range(), at.round() + 0.5, stroke);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn grid_cells_are_row_major_and_cover_the_area() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let cells = Layout::grid(2, 2).cells(area);
        assert_eq!(cells.len(), 4);
        assert_eq!(cells[1], Rect::from_min_max(pos2(400.0, 0.0), pos2(800.0, 300.0)));
        assert_eq!(cells[2], Rect::from_min_max(pos2(0.0, 300.0), pos2(400.0, 600.0)));
    }

    #[test]
    fn sanitize_repairs_bad_sizes() {
        let l = Layout { cols: vec![0.0, 3.0], rows: vec![] }.sanitized();
        assert_eq!(l, Layout::grid(2, 1));
        let l = Layout { cols: vec![1.0, 3.0], rows: vec![1.0] }.sanitized();
        assert_eq!(l.cols, vec![0.25, 0.75]);
    }
}
