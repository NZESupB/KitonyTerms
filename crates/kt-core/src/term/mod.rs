//! Terminal engine: a thin, UI-agnostic wrapper around `alacritty_terminal`.
//!
//! Responsibilities:
//! * own a [`Term`] grid + a `vte` ANSI [`Processor`]
//! * feed it raw bytes from the SSH channel ([`TermEngine::advance`])
//! * expose resize ([`TermEngine::resize`])
//! * build a `Send`-able [`GridSnapshot`] for the renderer
//! * surface terminal events (bell, title changes, PTY write-backs)
//!
//! The alacritty public API is explicitly *not* stability-guaranteed, so all of
//! it is contained here behind these methods. Swapping the backend later only
//! touches this module.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Point;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{CursorShape as VteCursorShape, Processor};

pub mod color;
pub mod snapshot;

pub use color::Rgb;
pub use snapshot::{CellAttrs, Cursor, CursorShape, GridSnapshot, SnapshotCell};

/// Terminal grid dimensions. Implements alacritty's [`Dimensions`] trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub columns: usize,
    pub screen_lines: usize,
    pub total_lines: usize,
}

impl TermSize {
    pub fn new(columns: usize, screen_lines: usize, scrollback: usize) -> Self {
        Self {
            columns: columns.max(1),
            screen_lines: screen_lines.max(1),
            total_lines: screen_lines.max(1) + scrollback,
        }
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.total_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

/// Terminal-originated events the session loop needs to react to.
#[derive(Debug, Clone)]
pub enum TermEvent {
    /// Bell (BEL / `\a`).
    Bell,
    /// Window title change (OSC 0/2).
    Title(String),
    /// The terminal wants bytes written back to the PTY (e.g. responses to
    /// device-status queries, bracketed-paste, clipboard formatters).
    PtyWrite(Vec<u8>),
    /// New content is available (alacritty `Wakeup`).
    Wakeup,
}

/// `EventListener` impl that funnels alacritty events into a shared queue.
///
/// Cloneable and `Send`/`Sync` so the `Term` can hold one while the session
/// loop drains the queue.
#[derive(Clone)]
pub struct EventProxy {
    queue: Arc<Mutex<VecDeque<TermEvent>>>,
}

impl EventProxy {
    fn new() -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn drain(&self) -> Vec<TermEvent> {
        let mut q = self.queue.lock().expect("term event queue poisoned");
        q.drain(..).collect()
    }

    fn push(&self, ev: TermEvent) {
        self.queue
            .lock()
            .expect("term event queue poisoned")
            .push_back(ev);
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::Bell => self.push(TermEvent::Bell),
            Event::Title(t) => self.push(TermEvent::Title(t)),
            Event::ResetTitle => self.push(TermEvent::Title(String::new())),
            Event::PtyWrite(s) => self.push(TermEvent::PtyWrite(s.into_bytes())),
            Event::Wakeup => self.push(TermEvent::Wakeup),
            // ClipboardStore/Load, ColorRequest, etc. are handled in later phases.
            _ => {}
        }
    }
}

/// The terminal engine.
pub struct TermEngine {
    term: Term<EventProxy>,
    parser: Processor,
    proxy: EventProxy,
    size: TermSize,
    revision: u64,
}

impl TermEngine {
    /// Create an engine with the given visible size and scrollback depth.
    pub fn new(columns: usize, rows: usize, scrollback: usize) -> Self {
        let size = TermSize::new(columns, rows, scrollback);
        let proxy = EventProxy::new();
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let term = Term::new(config, &size, proxy.clone());
        Self {
            term,
            parser: Processor::new(),
            proxy,
            size,
            revision: 0,
        }
    }

    /// Current visible dimensions (columns, rows).
    pub fn dimensions(&self) -> (usize, usize) {
        (self.size.columns, self.size.screen_lines)
    }

    /// Feed raw bytes from the PTY into the parser/grid.
    pub fn advance(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Resize the grid. Caller is responsible for sending `window-change` to
    /// the SSH channel separately.
    pub fn resize(&mut self, columns: usize, rows: usize, scrollback: usize) {
        let size = TermSize::new(columns, rows, scrollback);
        self.term.resize(size);
        self.size = size;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Drain any pending terminal events (bell, title, pty write-backs).
    pub fn take_events(&self) -> Vec<TermEvent> {
        self.proxy.drain()
    }

    /// Scroll the viewport through scrollback. Positive = scroll up (into
    /// history), negative = scroll down (toward the live view).
    pub fn scroll(&mut self, delta: i32) {
        use alacritty_terminal::grid::Scroll;
        self.term.scroll_display(Scroll::Delta(delta));
        self.revision = self.revision.wrapping_add(1);
    }

    /// Jump the viewport back to the live bottom.
    pub fn scroll_to_bottom(&mut self) {
        use alacritty_terminal::grid::Scroll;
        self.term.scroll_display(Scroll::Bottom);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Build an immutable, fully-resolved snapshot of the visible grid.
    pub fn snapshot(&self) -> GridSnapshot {
        let cols = self.size.columns;
        let rows = self.size.screen_lines;
        let palette = self.term.colors();

        let mut cells = vec![SnapshotCell::default(); rows * cols];

        let content = self.term.renderable_content();
        let display_offset = content.display_offset;

        for indexed in content.display_iter {
            let point: Point = indexed.point;
            // `display_iter` yields points relative to the visible area where
            // line 0 is the top visible row.
            let line = point.line.0;
            if line < 0 {
                continue;
            }
            let row = line as usize;
            let col = point.column.0;
            if row >= rows || col >= cols {
                continue;
            }

            let cell = indexed.cell;
            let flags = cell.flags;

            let mut attrs = CellAttrs {
                bold: flags.contains(Flags::BOLD),
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES),
                strikeout: flags.contains(Flags::STRIKEOUT),
                inverse: flags.contains(Flags::INVERSE),
                dim: flags.contains(Flags::DIM),
                wide: flags.contains(Flags::WIDE_CHAR),
                wide_spacer: flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            };

            let mut fg = color::resolve(cell.fg, palette);
            let mut bg = color::resolve(cell.bg, palette);
            if attrs.inverse {
                std::mem::swap(&mut fg, &mut bg);
            }
            // Render hidden text as blanks (keep bg).
            let c = if flags.contains(Flags::HIDDEN) {
                ' '
            } else {
                cell.c
            };
            // Approximate DIM by blending fg toward bg.
            if attrs.dim {
                fg = blend(fg, bg, 0.4);
            }
            let _ = &mut attrs;

            cells[row * cols + col] = SnapshotCell { c, fg, bg, attrs };
        }

        let cursor = {
            let rc = content.cursor;
            let line = rc.point.line.0;
            let shape = match rc.shape {
                VteCursorShape::Block => CursorShape::Block,
                VteCursorShape::Underline => CursorShape::Underline,
                VteCursorShape::Beam => CursorShape::Bar,
                VteCursorShape::HollowBlock => CursorShape::Block,
                VteCursorShape::Hidden => CursorShape::Hidden,
            };
            Cursor {
                line: if line < 0 { 0 } else { line as usize },
                column: rc.point.column.0.min(cols.saturating_sub(1)),
                shape,
            }
        };

        GridSnapshot {
            rows,
            cols,
            cells,
            cursor,
            revision: self.revision,
            display_offset,
        }
    }
}

/// Linear blend of two colors: `a*(1-t) + b*t`.
fn blend(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let lerp = |x: u8, y: u8| -> u8 {
        (x as f32 * (1.0 - t) + y as f32 * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgb::new(lerp(a.r, b.r), lerp(a.g, b.g), lerp(a.b, b.b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_plain_text_into_grid() {
        let mut eng = TermEngine::new(20, 5, 100);
        eng.advance(b"hello");
        let snap = eng.snapshot();
        assert_eq!(snap.rows, 5);
        assert_eq!(snap.cols, 20);
        assert_eq!(snap.row_text(0), "hello");
    }

    #[test]
    fn newline_moves_to_next_row() {
        let mut eng = TermEngine::new(20, 5, 100);
        eng.advance(b"line1\r\nline2");
        let snap = eng.snapshot();
        assert_eq!(snap.row_text(0), "line1");
        assert_eq!(snap.row_text(1), "line2");
    }

    #[test]
    fn sgr_bold_and_color_applied() {
        let mut eng = TermEngine::new(20, 3, 50);
        // bold + red "X", then reset.
        eng.advance(b"\x1b[1;31mX\x1b[0m");
        let snap = eng.snapshot();
        let cell = snap.cell(0, 0).unwrap();
        assert_eq!(cell.c, 'X');
        assert!(cell.attrs.bold);
        // red maps onto ANSI_16[1].
        assert_eq!(cell.fg, color::Rgb::new(0xf7, 0x76, 0x8e));
    }

    #[test]
    fn cursor_advances_with_text() {
        let mut eng = TermEngine::new(20, 3, 50);
        eng.advance(b"abc");
        let snap = eng.snapshot();
        assert_eq!(snap.cursor.line, 0);
        assert_eq!(snap.cursor.column, 3);
    }

    #[test]
    fn clear_screen_and_home() {
        let mut eng = TermEngine::new(10, 3, 50);
        eng.advance(b"junk\r\nmore");
        eng.advance(b"\x1b[2J\x1b[H"); // clear + home
        let snap = eng.snapshot();
        assert_eq!(snap.to_plain_text().trim(), "");
        assert_eq!(snap.cursor.line, 0);
        assert_eq!(snap.cursor.column, 0);
    }

    #[test]
    fn bell_event_surfaced() {
        let mut eng = TermEngine::new(10, 3, 50);
        eng.advance(b"\x07");
        let events = eng.take_events();
        assert!(events.iter().any(|e| matches!(e, TermEvent::Bell)));
    }

    #[test]
    fn resize_changes_dimensions() {
        let mut eng = TermEngine::new(80, 24, 100);
        assert_eq!(eng.dimensions(), (80, 24));
        eng.resize(100, 30, 100);
        assert_eq!(eng.dimensions(), (100, 30));
        let snap = eng.snapshot();
        assert_eq!((snap.cols, snap.rows), (100, 30));
    }

    #[test]
    fn revision_increments_on_advance() {
        let mut eng = TermEngine::new(10, 3, 50);
        let r0 = eng.snapshot().revision;
        eng.advance(b"x");
        let r1 = eng.snapshot().revision;
        assert!(r1 > r0);
    }
}
