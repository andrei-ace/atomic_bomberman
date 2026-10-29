//! Clipboard access belongs to the application, rather than the render loop.
//! Miniquad 0.4.11's public macOS Clipboard implementation is a no-op; its X11
//! backend implements real selection ownership and remains the Linux path.

#[cfg(not(target_os = "macos"))]
pub fn get_text() -> Option<String> {
    macroquad::miniquad::window::clipboard_get().filter(|text| !text.is_empty())
}

#[cfg(not(target_os = "macos"))]
pub fn set_text(text: &str) -> bool {
    macroquad::miniquad::window::clipboard_set(text);
    true
}

#[cfg(target_os = "macos")]
pub use macos::{get_text, set_text};

#[cfg(target_os = "macos")]
mod macos {
    use objc::{
        class, msg_send,
        rc::autoreleasepool,
        runtime::{Object, BOOL, YES},
        sel, sel_impl,
    };
    use std::ffi::{c_char, CStr, CString};

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {
        static NSPasteboardTypeString: *mut Object;
    }

    pub fn get_text() -> Option<String> {
        autoreleasepool(|| unsafe {
            let board: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
            if board.is_null() {
                return None;
            }
            let text: *mut Object = msg_send![board, stringForType: NSPasteboardTypeString];
            if text.is_null() {
                return None;
            }
            let bytes: *const c_char = msg_send![text, UTF8String];
            if bytes.is_null() {
                return None;
            }
            // Copy the UTF-8 bytes before draining the autorelease pool.
            CStr::from_ptr(bytes)
                .to_str()
                .ok()
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        })
    }

    pub fn set_text(text: &str) -> bool {
        let Ok(bytes) = CString::new(text) else {
            return false;
        };
        autoreleasepool(|| unsafe {
            let board: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
            if board.is_null() {
                return false;
            }
            let value: *mut Object =
                msg_send![class!(NSString), stringWithUTF8String: bytes.as_ptr()];
            if value.is_null() {
                return false;
            }
            let _: isize = msg_send![board, clearContents];
            let copied: BOOL = msg_send![board, setString: value forType: NSPasteboardTypeString];
            copied == YES
        })
    }
}
