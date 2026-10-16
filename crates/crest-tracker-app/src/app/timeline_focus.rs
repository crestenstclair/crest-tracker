//! Arrangement focus uses exact source rows and occurrence identities.
use super::*;

impl App {
    pub(super) fn timeline_source_beat(
        &self,
        location: &SourceLocation,
        view: View,
    ) -> Result<Option<Beat>, DocumentError> {
        let Some(at) = location.row_at else {
            return Ok(None);
        };
        if self.view != View::Expanded || !matches!(view, View::Expanded | View::Phrase) {
            return Err(DocumentError::Invalid(
                "displayed row navigation requires Expanded",
            ));
        }
        let timeline = self.timeline()?;
        let span = timeline
            .lanes
            .iter()
            .flat_map(|lane| &lane.spans)
            .find(|span| span.occurrence == location.occurrence && span.start == location.start);
        if span.is_some_and(|span| {
            span.rows
                .iter()
                .any(|row| row.source_row == location.row && row.at == at)
        }) {
            Ok(Some(at.checked_sub(location.start)?))
        } else {
            Err(DocumentError::Invalid(
                "displayed timeline row is stale; reopen Expanded",
            ))
        }
    }

    pub(super) fn open_timeline_source(&mut self) -> Result<(), DocumentError> {
        let Some(occurrence) = self.occurrence() else {
            return Ok(());
        };
        self.locate(
            SourceLocation {
                occurrence,
                start: self.timeline_focus.unwrap_or(Beat::ZERO),
                row: self.timeline_row,
                row_at: (self.view == View::Expanded)
                    .then_some(self.timeline_row_at)
                    .flatten(),
            },
            View::Phrase,
        )
    }

    /// True means handled, including clamped no-ops. False permits grid fallback.
    pub(super) fn navigate_expanded_rows(
        &mut self,
        timeline: &Timeline,
        lane: usize,
        vertical: i32,
        changed_lane: bool,
    ) -> bool {
        let rows = lane_rows(&timeline.lanes[lane]);
        if rows.is_empty() {
            return false;
        }
        let current = self.focused_timeline_span(timeline);
        let at = current
            .and_then(|span| {
                span.rows
                    .iter()
                    .find(|row| self.matches_timeline_row_selector(row))
            })
            .map(|row| row.at)
            .unwrap_or(self.timeline_focus.unwrap_or(timeline.start));
        let exact = rows.iter().position(|(span, row)| {
            Some(span.occurrence) == self.occurrence()
                && self.timeline_focus.is_none_or(|start| span.start == start)
                && self.matches_timeline_row_selector(row)
        });
        let index = if changed_lane {
            nearest_displayed_row_index(&rows, at)
        } else {
            exact.unwrap_or_else(|| nearest_displayed_row_index(&rows, at))
        };
        let index = index
            .saturating_add_signed(vertical as isize)
            .min(rows.len() - 1);
        let (span, row) = rows[index];
        self.locate(
            SourceLocation {
                occurrence: span.occurrence,
                start: span.start,
                row: row.source_row,
                row_at: Some(row.at),
            },
            View::Expanded,
        )
        .is_ok()
    }

    /// The caller restricts the occurrence/span first. An absent displayed beat
    /// matches any time for the source row; selecting one uses chronological first.
    fn matches_timeline_row_selector(&self, row: &TimelineRow) -> bool {
        row.source_row == self.timeline_row && self.timeline_row_at.is_none_or(|at| row.at == at)
    }

    fn focused_timeline_span<'a>(&self, timeline: &'a Timeline) -> Option<&'a PhraseSpan> {
        timeline
            .lanes
            .iter()
            .flat_map(|lane| &lane.spans)
            .find(|span| {
                Some(span.occurrence) == self.occurrence()
                    && self.timeline_focus.is_none_or(|at| span.start == at)
            })
    }

    pub(super) fn timeline_feedback_focus(&self) -> Option<crate::feedback::Focus> {
        let timeline = self.timeline().ok()?;
        let span = self.focused_timeline_span(&timeline)?;
        let row = if self.view == View::Expanded {
            Some(
                span.rows
                    .iter()
                    .find(|row| self.matches_timeline_row_selector(row))?,
            )
        } else {
            None
        };
        Some(crate::feedback::Focus::Timeline {
            occurrence: span.occurrence,
            phrase: span.phrase,
            start: span.start,
            row: row.map(|row| row.source_row),
            at: row.map(|row| row.at),
            cells: row.map_or_else(Vec::new, |row| row.cells.clone()),
        })
    }
}

fn span_rows(span: &PhraseSpan) -> impl Iterator<Item = (&PhraseSpan, &TimelineRow)> {
    span.rows.iter().map(move |row| (span, row))
}

fn lane_rows(lane: &TimelineLane) -> Vec<(&PhraseSpan, &TimelineRow)> {
    lane.spans.iter().flat_map(span_rows).collect()
}

/// Return a slice index into nonempty rows ordered by displayed musical time.
/// Horizontal movement selects the nearest displayed musical row. Ties choose
/// the earlier row; unrepresentable derived distances keep the later candidate.
fn nearest_displayed_row_index(rows: &[(&PhraseSpan, &TimelineRow)], at: Beat) -> usize {
    let next = rows.partition_point(|(_, row)| row.at < at);
    if next == 0 || next == rows.len() {
        return next.min(rows.len() - 1);
    }
    match (
        at.checked_sub(rows[next - 1].1.at),
        rows[next].1.at.checked_sub(at),
    ) {
        (Ok(before), Ok(after)) if before <= after => next - 1,
        _ => next,
    }
}
