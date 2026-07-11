use std::time::{Duration, Instant};

pub const BLINK_INTERVAL: Duration = Duration::from_millis(530);

#[derive(Debug, Default)]
pub struct CursorBlink {
    focused: bool,
    visible: bool,
    next_toggle: Option<Instant>,
}

impl CursorBlink {
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.next_toggle
    }

    pub fn set_focused(&mut self, focused: bool, now: Instant) -> bool {
        let was_visible = self.visible;
        self.focused = focused;
        self.visible = focused;
        self.next_toggle = focused.then_some(now + BLINK_INTERVAL);
        was_visible != self.visible
    }

    pub fn reset(&mut self, now: Instant) -> bool {
        if !self.focused {
            return false;
        }

        let was_visible = self.visible;
        self.visible = true;
        self.next_toggle = Some(now + BLINK_INTERVAL);
        was_visible != self.visible
    }

    pub fn tick(&mut self, now: Instant) -> bool {
        let Some(deadline) = self.next_toggle else {
            return false;
        };
        if now < deadline {
            return false;
        }

        self.visible = !self.visible;
        self.next_toggle = Some(now + BLINK_INTERVAL);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{BLINK_INTERVAL, CursorBlink};

    #[test]
    fn focus_starts_a_visible_blink_cycle() {
        let now = Instant::now();
        let mut cursor = CursorBlink::default();

        assert!(cursor.set_focused(true, now));
        assert!(cursor.is_visible());
        assert_eq!(cursor.next_deadline(), Some(now + BLINK_INTERVAL));
    }

    #[test]
    fn deadline_toggles_visibility_without_polling() {
        let now = Instant::now();
        let mut cursor = CursorBlink::default();
        cursor.set_focused(true, now);

        assert!(!cursor.tick(now + BLINK_INTERVAL / 2));
        assert!(cursor.is_visible());
        assert!(cursor.tick(now + BLINK_INTERVAL));
        assert!(!cursor.is_visible());
    }

    #[test]
    fn interaction_resets_hidden_cursor_to_visible() {
        let now = Instant::now();
        let mut cursor = CursorBlink::default();
        cursor.set_focused(true, now);
        cursor.tick(now + BLINK_INTERVAL);

        let reset_at = now + BLINK_INTERVAL + Duration::from_millis(10);
        assert!(cursor.reset(reset_at));
        assert!(cursor.is_visible());
        assert_eq!(cursor.next_deadline(), Some(reset_at + BLINK_INTERVAL));
    }

    #[test]
    fn losing_focus_hides_and_stops_cursor() {
        let now = Instant::now();
        let mut cursor = CursorBlink::default();
        cursor.set_focused(true, now);

        assert!(cursor.set_focused(false, now));
        assert!(!cursor.is_visible());
        assert_eq!(cursor.next_deadline(), None);
        assert!(!cursor.tick(now + BLINK_INTERVAL));
    }
}
