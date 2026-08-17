use std::time::{Duration, Instant};

const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_DISTANCE: i32 = 4;

#[derive(Default)]
pub(super) struct PinInteraction {
    drag: Option<((i32, i32), (i32, i32))>,
    last_click: Option<(Instant, (i32, i32))>,
}

impl PinInteraction {
    pub(super) fn begin_drag(&mut self, cursor: (i32, i32), window: (i32, i32)) {
        self.drag = Some((cursor, window));
    }

    pub(super) fn drag_to(&self, cursor: (i32, i32)) -> Option<(i32, i32)> {
        let (cursor_start, window_start) = self.drag?;
        Some(dragged_window_position(cursor_start, window_start, cursor))
    }

    pub(super) fn finish_drag(&mut self, cursor: Option<(i32, i32)>, now: Instant) -> bool {
        let Some((cursor_start, _)) = self.drag.take() else {
            return false;
        };
        let Some(cursor) = cursor else {
            self.last_click = None;
            return false;
        };
        if exceeds_click_distance(cursor_start, cursor) {
            self.last_click = None;
            return false;
        }
        let current = (now, cursor);
        if self
            .last_click
            .take()
            .is_some_and(|previous| is_double_click(previous, current))
        {
            true
        } else {
            self.last_click = Some(current);
            false
        }
    }

    pub(super) fn cancel(&mut self) {
        self.drag = None;
        self.last_click = None;
    }
}

fn exceeds_click_distance(first: (i32, i32), second: (i32, i32)) -> bool {
    (second.0 - first.0).abs() > DOUBLE_CLICK_DISTANCE
        || (second.1 - first.1).abs() > DOUBLE_CLICK_DISTANCE
}

fn is_double_click(first: (Instant, (i32, i32)), second: (Instant, (i32, i32))) -> bool {
    second.0.saturating_duration_since(first.0) <= DOUBLE_CLICK_INTERVAL
        && !exceeds_click_distance(first.1, second.1)
}

pub(super) fn dragged_window_position(
    cursor_start: (i32, i32),
    window_start: (i32, i32),
    cursor_now: (i32, i32),
) -> (i32, i32) {
    (
        window_start.0 + cursor_now.0 - cursor_start.0,
        window_start.1 + cursor_now.1 - cursor_start.1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_preserves_the_initial_pointer_offset() {
        assert_eq!(
            dragged_window_position((100, 80), (400, 300), (125, 110)),
            (425, 330)
        );
    }

    #[test]
    fn double_click_requires_two_nearby_short_clicks() {
        let first = Instant::now();
        assert!(is_double_click(
            (first, (100, 80)),
            (first + Duration::from_millis(499), (104, 76))
        ));
        assert!(!is_double_click(
            (first, (100, 80)),
            (first + Duration::from_millis(501), (100, 80))
        ));
        assert!(!is_double_click(
            (first, (100, 80)),
            (first + Duration::from_millis(200), (105, 80))
        ));
    }
}
