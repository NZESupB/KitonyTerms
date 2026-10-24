//! 输出合帧只推迟快照，不推迟解析；持续输出最多等待一帧。

use std::time::{Duration, Instant};

use super::TermEngine;

const OUTPUT_QUIET: Duration = Duration::from_millis(8);
const FRAME_BUDGET: Duration = Duration::from_millis(16);

#[derive(Default)]
pub(super) struct RenderSchedule {
    first: Option<Instant>,
    deadline: Option<Instant>,
}

impl RenderSchedule {
    pub(super) fn queue(&mut self, now: Instant) {
        let first = *self.first.get_or_insert(now);
        self.deadline = Some((now + OUTPUT_QUIET).min(first + FRAME_BUDGET));
    }

    fn retry(&mut self, now: Instant) {
        self.first = Some(now);
        self.deadline = Some(now + FRAME_BUDGET);
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

impl TermEngine {
    /// 同步更新有自己的协议截止时间，不能被普通合帧提前拆开。
    pub fn render_deadline(&self) -> Option<Instant> {
        self.render_schedule.deadline?;
        self.parser
            .sync_timeout()
            .sync_timeout()
            .or(self.render_schedule.deadline)
    }

    /// 到期后先释放同步缓冲。调用方须照常处理 take_events 中的 PTY 回写。
    pub fn prepare_render(&mut self, now: Instant) -> bool {
        if !self
            .render_deadline()
            .is_some_and(|deadline| deadline <= now)
        {
            return false;
        }
        self.finish_synchronized_update();
        true
    }

    /// 交互可以立即显示当前网格，但不能消费尚未释放的同步输出。
    pub fn mark_rendered(&mut self) {
        if self.parser.sync_timeout().sync_timeout().is_none() {
            self.render_schedule.clear();
        }
    }

    /// 发送队列暂满时保留最新网格，下一帧重试，避免末帧永久丢失或忙等。
    pub fn retry_render(&mut self, now: Instant) {
        self.render_schedule.retry(now);
    }

    /// 退出前提交尾部；也用于尺寸改变前结束旧坐标系的同步更新。
    pub fn flush_pending_output(&mut self) -> bool {
        self.finish_synchronized_update();
        self.render_schedule.deadline.is_some()
    }

    pub(super) fn finish_synchronized_update(&mut self) {
        if self.parser.sync_timeout().sync_timeout().is_some() {
            self.parser.stop_sync(&mut self.term);
            self.revision = self.revision.wrapping_add(1);
            self.coordinate_revision = self.revision;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::selection::{SelectionAction, SelectionMode, SelectionPoint};

    #[test]
    fn burst_waits_for_quiet_but_continuous_output_has_a_deadline() {
        let now = Instant::now();
        let mut schedule = RenderSchedule::default();
        schedule.queue(now);
        assert_eq!(schedule.deadline, Some(now + OUTPUT_QUIET));
        for ms in [4, 8, 12, 15, 20] {
            schedule.queue(now + Duration::from_millis(ms));
            assert!(schedule.deadline.unwrap() <= now + FRAME_BUDGET);
        }
        schedule.retry(now + FRAME_BUDGET);
        assert_eq!(schedule.deadline, Some(now + FRAME_BUDGET * 2));
        schedule.clear();
        assert!(schedule.deadline.is_none());
    }

    #[test]
    fn split_clear_and_body_are_presented_together_and_idle_has_no_timer() {
        let mut term = TermEngine::new(20, 3, 100);
        term.advance(b"old");
        term.mark_rendered();
        assert!(term.render_deadline().is_none());
        term.advance(b"\x1b[H\x1b[2J");
        let before = term.render_deadline().unwrap() - OUTPUT_QUIET;
        assert!(!term.prepare_render(before));
        term.advance(b"new");
        assert!(term.prepare_render(term.render_deadline().unwrap()));
        assert_eq!(term.snapshot().row_text(0), "new");
        term.mark_rendered();
        assert!(term.render_deadline().is_none());
    }

    #[test]
    fn synchronized_update_end_commits_a_split_frame_once() {
        let mut term = TermEngine::new(20, 3, 100);
        term.advance(b"old");
        term.mark_rendered();
        term.advance(b"\x1b[?2026h\x1b[H\x1b[2J");
        term.advance(b"new");
        assert_eq!(term.snapshot().row_text(0), "old");
        let timeout = term.render_deadline().unwrap();
        assert!(!term.prepare_render(timeout - Duration::from_millis(1)));
        term.mark_rendered();
        assert_eq!(term.render_deadline(), Some(timeout));
        term.advance(b"\x1b[?2026l");
        assert_eq!(term.snapshot().row_text(0), "new");
        assert!(term.prepare_render(term.render_deadline().unwrap()));
        term.mark_rendered();
        assert!(term.render_deadline().is_none());
    }

    #[test]
    fn timeout_releases_output_queries_and_invalidates_old_coordinates() {
        let mut term = TermEngine::new(20, 3, 100);
        term.advance(b"old");
        term.advance(b"\x1b[?2026h\x1b[Hnew\x1b[6n");
        let point = SelectionPoint {
            row: 0,
            column: 0,
            right: false,
            display_offset: 0,
            revision: term.snapshot().revision,
        };
        assert!(term.take_events().is_empty());
        assert!(term.prepare_render(term.render_deadline().unwrap()));
        assert_eq!(term.snapshot().row_text(0), "new");
        assert!(term
            .take_events()
            .iter()
            .any(|event| matches!(event, super::super::TermEvent::PtyWrite(_))));
        assert!(
            !term
                .select(
                    "pane",
                    SelectionAction::Begin {
                        point,
                        mode: SelectionMode::Simple
                    }
                )
                .unwrap()
                .accepted
        );
        term.mark_rendered();
        assert!(term.render_deadline().is_none());
        term.advance(b"!");
        assert_eq!(term.snapshot().row_text(0), "new!");
    }

    #[test]
    fn close_flushes_incomplete_sync_and_resize_uses_old_geometry_first() {
        let mut term = TermEngine::new(5, 3, 100);
        term.advance(b"\x1b[?2026habcdef\x1b[6n");
        term.resize(10, 3, 100);
        assert_eq!(term.snapshot().row_text(0), "abcdef");
        assert!(term.take_events().iter().any(|event| {
            matches!(event, super::super::TermEvent::PtyWrite(reply) if reply == b"\x1b[2;2R")
        }));
        assert!(term.parser.sync_timeout().sync_timeout().is_none());
        term.advance(b"\x1b[?2026h!");
        assert!(term.flush_pending_output());
        assert_eq!(term.snapshot().row_text(0), "abcdef!");
        term.mark_rendered();
        assert!(!term.flush_pending_output());
    }

    #[test]
    fn repeated_size_does_not_invalidate_selection_or_revision() {
        let mut term = TermEngine::new(20, 3, 100);
        term.advance(b"hello");
        let point = SelectionPoint {
            row: 0,
            column: 0,
            right: false,
            display_offset: 0,
            revision: term.snapshot().revision,
        };
        term.select(
            "pane",
            SelectionAction::Range {
                anchor: point,
                focus: SelectionPoint {
                    column: 4,
                    right: true,
                    ..point
                },
            },
        )
        .unwrap();
        let before = term.snapshot();
        term.resize(20, 3, 100);
        let after = term.snapshot();
        assert_eq!(after.revision, before.revision);
        let selection = after.selection.unwrap();
        assert_eq!(selection.owner, "pane");
        assert_eq!(selection.rows[0], Some((0, 5)));
        assert_eq!(
            term.select("pane", SelectionAction::Copy).unwrap().text,
            Some("hello".into())
        );
        assert!(
            term.select("pane", SelectionAction::Extend { point, scroll: 0 })
                .unwrap()
                .accepted
        );
    }
}
