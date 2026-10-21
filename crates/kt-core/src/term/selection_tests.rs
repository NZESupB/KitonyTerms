//! 模型选区的真实网格回归，不依赖 DOM 或模拟文本提取。
use super::*;

fn point(engine: &TermEngine, row: usize, column: usize, right: bool) -> SelectionPoint {
    let snapshot = engine.snapshot();
    SelectionPoint {
        row,
        column,
        right,
        display_offset: snapshot.display_offset,
        revision: snapshot.revision,
    }
}

fn range(engine: &mut TermEngine, start: (usize, usize), end: (usize, usize)) {
    let anchor = point(engine, start.0, start.1, false);
    let focus = point(engine, end.0, end.1, true);
    assert!(
        engine
            .select("pane", SelectionAction::Range { anchor, focus })
            .unwrap()
            .accepted
    );
}

fn copy(engine: &mut TermEngine) -> Option<String> {
    engine.select("pane", SelectionAction::Copy).unwrap().text
}

#[test]
fn strips_grid_padding_but_keeps_indentation_tabs_nbsp_and_blank_lines() {
    let mut engine = TermEngine::new(40, 4, 100);
    engine.advance("  alpha  beta\r\n\r\n\tz\u{a0}".as_bytes());
    range(&mut engine, (0, 0), (2, 39));
    assert_eq!(
        copy(&mut engine).as_deref(),
        Some("  alpha  beta\n\n\tz\u{a0}")
    );
}

#[test]
fn soft_wrap_joins_without_removing_a_real_space_at_the_wrap_boundary() {
    let mut engine = TermEngine::new(5, 3, 100);
    engine.advance(b"abcd ef\r\nz");
    range(&mut engine, (0, 0), (2, 4));
    assert_eq!(copy(&mut engine).as_deref(), Some("abcd ef\nz"));
}

#[test]
fn wide_characters_and_combining_marks_are_copied_once() {
    let mut engine = TermEngine::new(20, 2, 100);
    engine.advance("中e\u{301}文🙂!".as_bytes());
    range(&mut engine, (0, 0), (0, 19));
    assert_eq!(copy(&mut engine).as_deref(), Some("中e\u{301}文🙂!"));
}

#[test]
fn selecting_a_wide_spacer_selects_and_highlights_the_whole_glyph() {
    let mut engine = TermEngine::new(10, 2, 100);
    engine.advance("中文".as_bytes());
    range(&mut engine, (0, 1), (0, 1));
    assert_eq!(copy(&mut engine).as_deref(), Some("中"));
    let snapshot = engine.snapshot();
    assert!(snapshot.cell_selected("pane", 0, 0));
    assert!(snapshot.cell_selected("pane", 0, 1));
    assert!(!snapshot.cell_selected("other", 0, 0));
}

fn history() -> (TermEngine, String) {
    let lines: Vec<_> = (0..30).map(|n| format!("line {n:02}")).collect();
    let mut engine = TermEngine::new(20, 4, 100);
    engine.advance(lines.join("\r\n").as_bytes());
    (engine, lines.join("\n"))
}

#[test]
fn forward_drag_copies_more_than_seven_viewports_without_gaps_or_duplicates() {
    let (mut engine, expected) = history();
    engine.scroll(1000);
    let start = point(&engine, 0, 0, false);
    engine
        .select(
            "pane",
            SelectionAction::Begin {
                point: start,
                mode: SelectionMode::Simple,
            },
        )
        .unwrap();
    for delta in [-5, -5, -5, -5, -1000] {
        let end = point(&engine, 3, 19, true);
        engine
            .select(
                "pane",
                SelectionAction::Extend {
                    point: end,
                    scroll: delta,
                },
            )
            .unwrap();
    }
    assert_eq!(copy(&mut engine).as_deref(), Some(expected.as_str()));
    engine.scroll(7);
    assert_eq!(copy(&mut engine).as_deref(), Some(expected.as_str()));
}

#[test]
fn reverse_drag_and_direction_changes_keep_one_normalized_buffer_range() {
    let (mut engine, expected) = history();
    let start = point(&engine, 3, 19, true);
    engine
        .select(
            "pane",
            SelectionAction::Begin {
                point: start,
                mode: SelectionMode::Simple,
            },
        )
        .unwrap();
    for delta in [8, -3, 8, 1000] {
        let end = point(&engine, 0, 0, false);
        engine
            .select(
                "pane",
                SelectionAction::Extend {
                    point: end,
                    scroll: delta,
                },
            )
            .unwrap();
    }
    assert_eq!(copy(&mut engine).as_deref(), Some(expected.as_str()));
}

#[test]
fn old_viewport_coordinates_use_their_original_offset_after_a_scroll() {
    let (mut engine, _) = history();
    engine.scroll(1000);
    let anchor = point(&engine, 0, 0, false);
    let focus = point(&engine, 1, 19, true);
    engine.scroll(-10);
    assert!(
        engine
            .select("pane", SelectionAction::Range { anchor, focus })
            .unwrap()
            .accepted
    );
    assert_eq!(copy(&mut engine).as_deref(), Some("line 00\nline 01"));
    assert!(engine
        .snapshot()
        .selection
        .unwrap()
        .rows
        .iter()
        .all(Option::is_none));
}

#[test]
fn output_invalidates_old_hit_coordinates_without_replacing_the_selection() {
    let mut engine = TermEngine::new(20, 3, 100);
    engine.advance(b"first\r\nsecond");
    range(&mut engine, (0, 0), (0, 19));
    let stale = point(&engine, 1, 0, false);
    engine.advance(b"!");
    let result = engine
        .select(
            "pane",
            SelectionAction::Begin {
                point: stale,
                mode: SelectionMode::Simple,
            },
        )
        .unwrap();
    assert!(!result.accepted);
    assert_eq!(copy(&mut engine).as_deref(), Some("first"));
}

#[test]
fn another_instance_cannot_copy_extend_or_clear_the_selection() {
    let mut engine = TermEngine::new(20, 2, 100);
    engine.advance(b"selected");
    range(&mut engine, (0, 0), (0, 19));
    assert!(engine
        .select("other", SelectionAction::Copy)
        .unwrap()
        .text
        .is_none());
    let end = point(&engine, 1, 0, false);
    assert!(engine
        .select(
            "other",
            SelectionAction::Extend {
                point: end,
                scroll: 0
            }
        )
        .is_err());
    engine.select("other", SelectionAction::Clear).unwrap();
    assert_eq!(copy(&mut engine).as_deref(), Some("selected"));
    engine.select("pane", SelectionAction::Clear).unwrap();
    assert!(copy(&mut engine).is_none());
}

#[test]
fn column_resize_and_screen_switch_clear_selection_and_highlight() {
    let mut engine = TermEngine::new(20, 3, 100);
    engine.advance(b"selected");
    range(&mut engine, (0, 0), (0, 19));
    engine.resize(10, 3, 100);
    assert!(copy(&mut engine).is_none());
    assert!(engine.snapshot().selection.is_none());
    range(&mut engine, (0, 0), (0, 9));
    engine.advance(b"\x1b[?1049h");
    assert!(copy(&mut engine).is_none());
}

#[test]
fn eviction_does_not_rebind_a_deleted_selection_to_new_history() {
    let mut engine = TermEngine::new(10, 2, 3);
    engine.advance(b"old0\r\nold1\r\nold2\r\nold3\r\nold4");
    engine.scroll(1000);
    range(&mut engine, (0, 0), (0, 9));
    for _ in 0..20 {
        engine.advance(b"\r\nnew");
    }
    assert!(copy(&mut engine).is_none());
    assert!(engine.snapshot().selection.is_none());
}

#[test]
fn word_and_line_selection_preserve_standard_mouse_selection_modes() {
    let mut engine = TermEngine::new(30, 2, 100);
    engine.advance(b"hello world");
    let start = point(&engine, 0, 8, false);
    engine
        .select(
            "pane",
            SelectionAction::Begin {
                point: start,
                mode: SelectionMode::Semantic,
            },
        )
        .unwrap();
    assert_eq!(copy(&mut engine).as_deref(), Some("world"));
    let start = point(&engine, 0, 8, false);
    engine
        .select(
            "pane",
            SelectionAction::Begin {
                point: start,
                mode: SelectionMode::Lines,
            },
        )
        .unwrap();
    assert_eq!(copy(&mut engine).as_deref(), Some("hello world\n"));
}
