//! Immutable, `Send`-able snapshot of the terminal's visible grid.
//!
//! The terminal engine lives on the core thread; the UI thread only ever sees
//! [`GridSnapshot`]s. All colors are pre-resolved to RGB and all alacritty
//! types are dropped, so the snapshot can cross the channel freely and the
//! renderer needs no dependency on `alacritty_terminal`.

use super::color::Rgb;

/// Per-cell rendering attributes (a compact subset of alacritty's `Flags`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellAttrs {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub inverse: bool,
    pub dim: bool,
    /// True for the left cell of a double-width (CJK) glyph.
    pub wide: bool,
    /// True for the spacer cell that follows a wide glyph (skip when drawing).
    pub wide_spacer: bool,
}

/// One rendered cell: a character plus resolved colors and attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotCell {
    pub c: char,
    pub fg: Rgb,
    pub bg: Rgb,
    pub attrs: CellAttrs,
}

impl Default for SnapshotCell {
    fn default() -> Self {
        Self {
            c: ' ',
            fg: super::color::DEFAULT_FG,
            bg: super::color::DEFAULT_BG,
            attrs: CellAttrs::default(),
        }
    }
}

/// Cursor shape for rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Bar,
    Underline,
    Hidden,
}

/// Cursor position + shape within the visible grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    /// Row within the visible area (0 = top).
    pub line: usize,
    /// Column (0 = left).
    pub column: usize,
    pub shape: CursorShape,
}

/// A full snapshot of the visible terminal area.
///
/// `cells` is row-major, `rows * cols` entries.
#[derive(Debug, Clone)]
pub struct GridSnapshot {
    pub rows: usize,
    pub cols: usize,
    pub cells: Vec<SnapshotCell>,
    pub cursor: Cursor,
    /// Monotonic counter bumped on every rebuild — lets the UI cheaply detect
    /// "nothing changed" and skip re-uploading to the GPU.
    pub revision: u64,
    /// Current scrollback offset (0 = viewing the live bottom).
    pub display_offset: usize,
    /// Number of scrollback lines above the live viewport. Together with
    /// `display_offset` this yields absolute line numbers for the gutter.
    pub history_size: usize,
    /// Per-row wrap flag: `wrapped[r]` is true when row `r`'s content overflowed
    /// into row `r+1` (alacritty sets `WRAPLINE` on the row's last cell). The UI
    /// uses this to mark continuation rows in the line-number gutter. Length =
    /// `rows`; entries default to `false`.
    pub wrapped: Vec<bool>,
}

impl GridSnapshot {
    /// 可见视口首行的绝对行号（从 1 开始，含 scrollback 历史）。
    /// 滚动回看历史时（`display_offset > 0`）行号相应减小。
    pub fn first_visible_line_number(&self) -> usize {
        self.history_size.saturating_sub(self.display_offset) + 1
    }

    /// Borrow the cell at (row, col), if in bounds.
    pub fn cell(&self, row: usize, col: usize) -> Option<&SnapshotCell> {
        if row < self.rows && col < self.cols {
            self.cells.get(row * self.cols + col)
        } else {
            None
        }
    }

    /// Render row `row` to a plain `String` (used by the headless printer/tests).
    pub fn row_text(&self, row: usize) -> String {
        let mut s = String::with_capacity(self.cols);
        for col in 0..self.cols {
            if let Some(cell) = self.cell(row, col) {
                if cell.attrs.wide_spacer {
                    continue;
                }
                s.push(cell.c);
            }
        }
        // Trim trailing spaces for readability.
        s.trim_end().to_string()
    }

    /// Whole-screen plain text (newline-separated rows).
    pub fn to_plain_text(&self) -> String {
        (0..self.rows)
            .map(|r| self.row_text(r))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
