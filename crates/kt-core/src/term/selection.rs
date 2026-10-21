//! 缓冲区选区与 UI 坐标转换；不依赖渲染节点的生命周期。

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::viewport_to_point;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::TermEngine;

/// 坐标属于产生鼠标事件时的快照，不能用处理请求时的视口偏移替代。
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct SelectionPoint {
    pub row: usize,
    pub column: usize,
    pub right: bool,
    pub display_offset: usize,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    #[default]
    Simple,
    Semantic,
    Lines,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SelectionAction {
    Begin {
        point: SelectionPoint,
        #[serde(default)]
        mode: SelectionMode,
    },
    Extend {
        point: SelectionPoint,
        #[serde(default)]
        scroll: i32,
    },
    Range {
        anchor: SelectionPoint,
        focus: SelectionPoint,
    },
    All {
        point: SelectionPoint,
    },
    Clear,
    Copy,
}

/// accepted=false 表示网格已改变，调用方须等待 revision 对应的新帧重采样。
#[derive(Debug, Serialize)]
pub struct SelectionReply {
    pub accepted: bool,
    pub revision: u64,
    pub text: Option<String>,
}

pub struct SelectionCommand {
    pub owner: String,
    pub action: SelectionAction,
    pub reply: oneshot::Sender<Result<SelectionReply, String>>,
}

/// 仅投影可见行，列区间采用左闭右开，不传输历史文本。
#[derive(Debug, Clone)]
pub struct SnapshotSelection {
    pub owner: String,
    pub rows: Vec<Option<(usize, usize)>>,
}

impl TermEngine {
    pub fn select(
        &mut self,
        owner: &str,
        action: SelectionAction,
    ) -> Result<SelectionReply, String> {
        let points: &[SelectionPoint] = match &action {
            SelectionAction::Begin { point, .. }
            | SelectionAction::Extend { point, .. }
            | SelectionAction::All { point } => std::slice::from_ref(point),
            SelectionAction::Range { anchor, focus } => &[*anchor, *focus],
            SelectionAction::Clear | SelectionAction::Copy => &[],
        };
        for point in points {
            if point.revision < self.coordinate_revision || point.revision > self.revision {
                return Ok(SelectionReply {
                    accepted: false,
                    revision: self.revision,
                    text: None,
                });
            }
            if point.row >= self.size.screen_lines
                || point.column >= self.size.columns
                || point.display_offset > self.term.grid().history_size()
            {
                return Err("终端选区坐标超出缓冲区范围".into());
            }
        }

        let position = |point: SelectionPoint| {
            viewport_to_point(
                point.display_offset,
                Point::new(point.row, Column(point.column)),
            )
        };
        let side = |point: SelectionPoint| if point.right { Side::Right } else { Side::Left };
        match action {
            SelectionAction::Begin { point, mode } => {
                let ty = match mode {
                    SelectionMode::Simple => SelectionType::Simple,
                    SelectionMode::Semantic => SelectionType::Semantic,
                    SelectionMode::Lines => SelectionType::Lines,
                };
                self.term.selection = Some(Selection::new(ty, position(point), side(point)));
                self.selection_owner = Some(owner.to_owned());
            }
            SelectionAction::Extend { mut point, scroll } => {
                if self.selection_owner.as_deref() != Some(owner) || self.term.selection.is_none() {
                    return Err("终端选区已失效，请重新选择".into());
                }
                if scroll != 0 {
                    self.scroll(scroll);
                    point.display_offset = self.term.grid().display_offset();
                }
                if let Some(selection) = self.term.selection.as_mut() {
                    selection.update(position(point), side(point));
                }
            }
            SelectionAction::Range { anchor, focus } => {
                let mut selection =
                    Selection::new(SelectionType::Simple, position(anchor), side(anchor));
                selection.update(position(focus), side(focus));
                self.term.selection = Some(selection);
                self.selection_owner = Some(owner.to_owned());
            }
            SelectionAction::All { point } => {
                let start = viewport_to_point(point.display_offset, Point::new(0, Column(0)));
                let end = viewport_to_point(
                    point.display_offset,
                    Point::new(self.size.screen_lines - 1, Column(self.size.columns - 1)),
                );
                let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
                selection.update(end, Side::Right);
                self.term.selection = Some(selection);
                self.selection_owner = Some(owner.to_owned());
            }
            SelectionAction::Clear => {
                if self.selection_owner.as_deref() == Some(owner) {
                    self.term.selection = None;
                    self.selection_owner = None;
                }
            }
            SelectionAction::Copy => {
                return Ok(SelectionReply {
                    accepted: true,
                    revision: self.revision,
                    text: (self.selection_owner.as_deref() == Some(owner))
                        .then(|| self.term.selection_to_string())
                        .flatten(),
                });
            }
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(SelectionReply {
            accepted: true,
            revision: self.revision,
            text: None,
        })
    }

    pub(super) fn snapshot_selection(&self) -> Option<SnapshotSelection> {
        let range = self.term.selection.as_ref()?.to_range(&self.term)?;
        let owner = self.selection_owner.as_ref()?.clone();
        let offset = self.term.grid().display_offset() as i32;
        let rows = (0..self.size.screen_lines)
            .map(|row| {
                let line = row as i32 - offset;
                if line < range.start.line.0 || line > range.end.line.0 {
                    return None;
                }
                let start = if line == range.start.line.0 {
                    range.start.column.0
                } else {
                    0
                };
                let end = if line == range.end.line.0 {
                    range.end.column.0 + 1
                } else {
                    self.size.columns
                };
                Some((start, end))
            })
            .collect();
        Some(SnapshotSelection { owner, rows })
    }
}

#[cfg(test)]
#[path = "selection_tests.rs"]
mod tests;
