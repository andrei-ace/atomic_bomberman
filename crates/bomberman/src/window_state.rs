//! Remember settled windowed dimensions without recording fullscreen animations.
use crate::settings::valid_window_size;

#[derive(Default)]
pub struct WindowSizeTracker {
    candidate: Option<([u32; 2], u64)>,
    fullscreen_at: Option<u64>,
}

impl WindowSizeTracker {
    pub fn fullscreen_transition(&mut self, now_ms: u64) {
        self.fullscreen_at = Some(now_ms);
        self.candidate = None;
    }

    pub fn observe(
        &mut self,
        size: [u32; 2],
        fullscreen: bool,
        now_ms: u64,
        force: bool,
    ) -> Option<[u32; 2]> {
        if fullscreen {
            self.fullscreen_transition(now_ms);
            return None;
        }
        if !valid_window_size(size)
            || self
                .fullscreen_at
                .is_some_and(|time| now_ms.saturating_sub(time) < 500)
        {
            return None;
        }
        if self.candidate.map(|(value, _)| value) != Some(size) {
            self.candidate = Some((size, now_ms));
        }
        let (_, changed_at) = self.candidate?;
        (force || now_ms.saturating_sub(changed_at) >= 300).then_some(size)
    }
}

/// The macOS green window button can change fullscreen outside game controls.
/// Inspect only this application's own windows, on the main render thread.
#[cfg(target_os = "macos")]
pub fn native_fullscreen() -> Option<bool> {
    use objc::{class, msg_send, rc::autoreleasepool, runtime::Object, sel, sel_impl};
    autoreleasepool(|| unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let windows: *mut Object = msg_send![app, windows];
        if windows.is_null() {
            return None;
        }
        let count: usize = msg_send![windows, count];
        if count == 0 {
            return None;
        }
        for index in 0..count {
            let window: *mut Object = msg_send![windows, objectAtIndex: index];
            let style: usize = msg_send![window, styleMask];
            if style & (1 << 14) != 0 {
                return Some(true);
            }
        }
        Some(false)
    })
}

#[cfg(not(target_os = "macos"))]
pub fn native_fullscreen() -> Option<bool> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_saves_only_after_settling_and_close_keeps_the_latest_size() {
        let mut tracker = WindowSizeTracker::default();
        assert_eq!(tracker.observe([1200, 700], false, 0, false), None);
        assert_eq!(tracker.observe([1280, 720], false, 200, false), None);
        assert_eq!(tracker.observe([1280, 720], false, 499, false), None);
        assert_eq!(
            tracker.observe([1280, 720], false, 500, false),
            Some([1280, 720])
        );
        assert_eq!(
            tracker.observe([1100, 750], false, 510, true),
            Some([1100, 750])
        );
    }

    #[test]
    fn fullscreen_and_its_exit_animation_do_not_replace_windowed_dimensions() {
        let mut tracker = WindowSizeTracker::default();
        assert_eq!(
            tracker.observe([1280, 720], false, 0, true),
            Some([1280, 720])
        );
        tracker.fullscreen_transition(100);
        assert_eq!(tracker.observe([1440, 900], false, 150, true), None);
        assert_eq!(tracker.observe([1440, 900], true, 700, true), None);
        assert_eq!(tracker.observe([1440, 900], false, 800, true), None);
        assert_eq!(tracker.observe([1280, 720], false, 1200, false), None);
        assert_eq!(
            tracker.observe([1280, 720], false, 1500, false),
            Some([1280, 720])
        );
        assert_eq!(tracker.observe([0, 0], false, 1600, true), None);
    }
}
