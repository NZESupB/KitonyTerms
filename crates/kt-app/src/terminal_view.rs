//! Terminal grid rendering using egui's `Painter`.
//!
//! Rather than a bespoke wgpu pass, we draw the [`GridSnapshot`] directly with
//! egui: a background rectangle per contiguous color run, then one glyph per
//! cell, then the cursor. egui rasterizes glyphs into its own atlas and submits
//! through wgpu, so this is still GPU-accelerated while being dramatically
//! simpler than hand-rolling a glyph atlas. We can swap in a custom wgpu pass
//! later without touching the rest of the app — the input is just a snapshot.

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Stroke, Vec2};
use kt_core::term::{CursorShape, GridSnapshot};

/// Renders terminal grids and reports the pixel geometry back so the caller can
/// translate a widget size into terminal (cols, rows).
pub struct TerminalView {
    /// Monospace font size in points.
    pub font_size: f32,
    /// Cached single-cell size in points (w, h), recomputed when font changes.
    cell_size: Vec2,
    font_size_cached_for: f32,
}

impl Default for TerminalView {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            cell_size: Vec2::new(8.0, 16.0),
            font_size_cached_for: 0.0,
        }
    }
}

impl TerminalView {
    pub fn new(font_size: f32) -> Self {
        Self {
            font_size,
            ..Default::default()
        }
    }

    /// The monospace [`FontId`] used for the grid.
    fn font_id(&self) -> FontId {
        FontId::monospace(self.font_size)
    }

    /// Measure and cache the cell size for the current font size.
    fn ensure_cell_size(&mut self, ctx: &egui::Context) {
        if (self.font_size - self.font_size_cached_for).abs() < f32::EPSILON {
            return;
        }
        let font_id = self.font_id();
        ctx.fonts(|f| {
            // Width of a representative monospace glyph.
            let row_height = f.row_height(&font_id);
            let glyph_width = f.glyph_width(&font_id, 'M');
            self.cell_size = Vec2::new(glyph_width.max(1.0), row_height.max(1.0));
        });
        self.font_size_cached_for = self.font_size;
    }

    /// Current cell size in points.
    #[allow(dead_code)]
    pub fn cell_size(&self) -> Vec2 {
        self.cell_size
    }

    /// Given an available pixel area, compute how many (cols, rows) fit.
    pub fn grid_size_for(&mut self, ctx: &egui::Context, available: Vec2) -> (u16, u16) {
        self.ensure_cell_size(ctx);
        let cols = (available.x / self.cell_size.x).floor().max(1.0) as u16;
        let rows = (available.y / self.cell_size.y).floor().max(1.0) as u16;
        (cols, rows)
    }

    /// Paint a snapshot into `rect`. Returns the painted area.
    pub fn paint(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        snapshot: &GridSnapshot,
        has_focus: bool,
    ) {
        self.ensure_cell_size(ui.ctx());
        let painter = ui.painter_at(rect);
        let cw = self.cell_size.x;
        let ch = self.cell_size.y;
        let origin = rect.min;
        let font_id = self.font_id();

        // Default background fill for the whole grid (covers gaps).
        if let Some(bg0) = snapshot.cell(0, 0) {
            painter.rect_filled(rect, 0.0, to_color32(bg0.bg));
        }

        for row in 0..snapshot.rows {
            let y = origin.y + row as f32 * ch;

            // --- Pass 1: background rectangles, merged into runs. ---
            let mut col = 0usize;
            while col < snapshot.cols {
                let Some(cell) = snapshot.cell(row, col) else {
                    col += 1;
                    continue;
                };
                if cell.attrs.wide_spacer {
                    col += 1;
                    continue;
                }
                let bg = cell.bg;
                let mut run_end = col + 1;
                while run_end < snapshot.cols {
                    match snapshot.cell(row, run_end) {
                        Some(c) if c.bg == bg && !c.attrs.wide_spacer => run_end += 1,
                        Some(c) if c.attrs.wide_spacer => run_end += 1,
                        _ => break,
                    }
                }
                let x0 = origin.x + col as f32 * cw;
                let x1 = origin.x + run_end as f32 * cw;
                let run_rect = Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + ch));
                painter.rect_filled(run_rect, 0.0, to_color32(bg));
                col = run_end;
            }

            // --- Pass 2: glyphs. ---
            for col in 0..snapshot.cols {
                let Some(cell) = snapshot.cell(row, col) else {
                    continue;
                };
                if cell.attrs.wide_spacer || cell.c == ' ' || cell.c == '\0' {
                    continue;
                }
                let x = origin.x + col as f32 * cw;
                let pos = Pos2::new(x, y);
                let color = to_color32(cell.fg);
                painter.text(
                    pos,
                    egui::Align2::LEFT_TOP,
                    cell.c,
                    font_id.clone(),
                    color,
                );
                // Underline / strikeout overlays.
                if cell.attrs.underline {
                    let uy = y + ch - 1.5;
                    painter.line_segment(
                        [Pos2::new(x, uy), Pos2::new(x + cw, uy)],
                        Stroke::new(1.0, color),
                    );
                }
                if cell.attrs.strikeout {
                    let sy = y + ch * 0.5;
                    painter.line_segment(
                        [Pos2::new(x, sy), Pos2::new(x + cw, sy)],
                        Stroke::new(1.0, color),
                    );
                }
            }
        }

        // --- Cursor. ---
        if !matches!(snapshot.cursor.shape, CursorShape::Hidden) {
            let cx = origin.x + snapshot.cursor.column as f32 * cw;
            let cy = origin.y + snapshot.cursor.line as f32 * ch;
            let cursor_color = Color32::from_rgb(0xc0, 0xca, 0xf5);
            match snapshot.cursor.shape {
                CursorShape::Block => {
                    let r = Rect::from_min_size(Pos2::new(cx, cy), Vec2::new(cw, ch));
                    if has_focus {
                        painter.rect_filled(r, 0.0, cursor_color);
                        // Redraw the glyph under the cursor in inverted color.
                        if let Some(cell) = snapshot.cell(snapshot.cursor.line, snapshot.cursor.column)
                        {
                            if cell.c != ' ' && cell.c != '\0' {
                                painter.text(
                                    Pos2::new(cx, cy),
                                    egui::Align2::LEFT_TOP,
                                    cell.c,
                                    font_id.clone(),
                                    to_color32(cell.bg),
                                );
                            }
                        }
                    } else {
                        painter.rect_stroke(
                            r,
                            0.0,
                            Stroke::new(1.0, cursor_color),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
                CursorShape::Bar => {
                    let r = Rect::from_min_size(Pos2::new(cx, cy), Vec2::new(2.0, ch));
                    painter.rect_filled(r, 0.0, cursor_color);
                }
                CursorShape::Underline => {
                    let r =
                        Rect::from_min_size(Pos2::new(cx, cy + ch - 2.0), Vec2::new(cw, 2.0));
                    painter.rect_filled(r, 0.0, cursor_color);
                }
                CursorShape::Hidden => {}
            }
        }
    }
}

fn to_color32(c: kt_core::term::Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}
