//! Front app, window title and window document (macOS). Never prompts for a permission.
//!
//! - Front app: NSWorkspace `frontmostApplication` (no permission).
//! - Window title: Accessibility (`AXFocusedWindow` -> `AXTitle`), needs the Accessibility
//!   permission. Fallback: `CGWindowListCopyWindowInfo` (`kCGWindowName`), which only has other
//!   apps' titles with the Screen Recording permission. Then `None`.
//! - Window document (`AXDocument`): the file or folder the window shows. Terminal.app and
//!   Ghostty set it to the shell's folder, editors to the open file. Needs Accessibility.

use crate::AppInfo;

#[cfg(target_os = "macos")]
mod mac {
    use super::AppInfo;
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFType, CFTypeRef, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;

    type AXUIElementRef = *const c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(element: AXUIElementRef, attribute: CFStringRef, value: *mut CFTypeRef) -> i32;
        fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout_seconds: f32) -> i32;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
        fn CGPreflightScreenCaptureAccess() -> bool;
    }

    const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
    const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

    pub fn front_app() -> Option<AppInfo> {
        use objc2_app_kit::NSWorkspace;
        objc2::rc::autoreleasepool(|_| {
            let ws = NSWorkspace::sharedWorkspace();
            let app = ws.frontmostApplication()?;
            let name = app.localizedName().map(|s| s.to_string()).unwrap_or_default();
            let bundle_id = app.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default();
            let pid = app.processIdentifier();
            if pid <= 0 {
                return None;
            }
            Some(AppInfo { name, bundle_id, pid })
        })
    }

    pub fn accessibility_trusted() -> bool {
        unsafe { AXIsProcessTrusted() != 0 }
    }

    pub fn screen_recording_allowed() -> bool {
        unsafe { CGPreflightScreenCaptureAccess() }
    }

    fn ax_copy(element: AXUIElementRef, attr: &str) -> Option<CFType> {
        let key = CFString::new(attr);
        let mut value: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(element, key.as_concrete_TypeRef(), &mut value) };
        if err != 0 || value.is_null() {
            return None;
        }
        Some(unsafe { CFType::wrap_under_create_rule(value) })
    }

    /// The front (focused, else main) window of `pid` as an AX element. Needs Accessibility.
    fn ax_front_window(pid: i32) -> Option<CFType> {
        if !accessibility_trusted() {
            return None;
        }
        let app_ref = unsafe { AXUIElementCreateApplication(pid) };
        if app_ref.is_null() {
            return None;
        }
        let app_el = unsafe { CFType::wrap_under_create_rule(app_ref as CFTypeRef) };
        unsafe { AXUIElementSetMessagingTimeout(app_el.as_CFTypeRef() as AXUIElementRef, 0.5) };
        let el = app_el.as_CFTypeRef() as AXUIElementRef;
        ax_copy(el, "AXFocusedWindow").or_else(|| ax_copy(el, "AXMainWindow"))
    }

    fn ax_window_string(pid: i32, attr: &str) -> Option<String> {
        let win = ax_front_window(pid)?;
        let v = ax_copy(win.as_CFTypeRef() as AXUIElementRef, attr)?;
        let s = v.downcast::<CFString>()?.to_string();
        if s.trim().is_empty() {
            None
        } else {
            Some(s)
        }
    }

    pub fn ax_title(pid: i32) -> Option<String> {
        ax_window_string(pid, "AXTitle")
    }

    pub fn ax_document(pid: i32) -> Option<String> {
        ax_window_string(pid, "AXDocument")
    }

    /// Title of the front-most normal window of `pid` from the window server.
    pub fn cg_title(pid: i32) -> Option<String> {
        let ptr = unsafe { CGWindowListCopyWindowInfo(K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS, 0) };
        if ptr.is_null() {
            return None;
        }
        let arr: CFArray<CFDictionary<CFString, CFType>> = unsafe { CFArray::wrap_under_create_rule(ptr) };
        let k_pid = CFString::from_static_string("kCGWindowOwnerPID");
        let k_layer = CFString::from_static_string("kCGWindowLayer");
        let k_name = CFString::from_static_string("kCGWindowName");
        // The list is ordered front to back.
        for d in arr.iter() {
            let owner = d.find(&k_pid).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i32());
            if owner != Some(pid) {
                continue;
            }
            let layer = d.find(&k_layer).and_then(|v| v.downcast::<CFNumber>()).and_then(|n| n.to_i32()).unwrap_or(0);
            if layer != 0 {
                continue;
            }
            if let Some(name) = d.find(&k_name).and_then(|v| v.downcast::<CFString>()).map(|s| s.to_string()) {
                if !name.trim().is_empty() {
                    return Some(name);
                }
            }
        }
        None
    }
}

#[cfg(not(target_os = "macos"))]
mod mac {
    use super::AppInfo;
    pub fn front_app() -> Option<AppInfo> {
        None
    }
    pub fn accessibility_trusted() -> bool {
        false
    }
    pub fn screen_recording_allowed() -> bool {
        false
    }
    pub fn ax_title(_pid: i32) -> Option<String> {
        None
    }
    pub fn ax_document(_pid: i32) -> Option<String> {
        None
    }
    pub fn cg_title(_pid: i32) -> Option<String> {
        None
    }
}

/// The app in front, from NSWorkspace. No permission needed.
pub fn front_app() -> Option<AppInfo> {
    mac::front_app()
}

/// The front window's title: Accessibility first, then the window server list, then None.
pub fn window_title(app: &AppInfo) -> Option<String> {
    window_title_with_source(app).0
}

/// Like [`window_title`], plus which method found it: "accessibility", "window-list" or "none".
pub fn window_title_with_source(app: &AppInfo) -> (Option<String>, &'static str) {
    if let Some(t) = mac::ax_title(app.pid) {
        return (Some(t), "accessibility");
    }
    if let Some(t) = mac::cg_title(app.pid) {
        return (Some(t), "window-list");
    }
    (None, "none")
}

/// True when this process may use the Accessibility API. Never prompts.
pub fn accessibility_trusted() -> bool {
    mac::accessibility_trusted()
}

/// True when this process has the Screen Recording permission (other apps' window titles). Never prompts.
pub fn screen_recording_allowed() -> bool {
    mac::screen_recording_allowed()
}

/// The front window's document (`AXDocument`) as a local path, when it is a `file://` URL.
pub(crate) fn window_document(app: &AppInfo) -> Option<std::path::PathBuf> {
    file_url_to_path(&mac::ax_document(app.pid)?)
}

pub(crate) fn file_url_to_path(url: &str) -> Option<std::path::PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let decoded = percent_encoding::percent_decode_str(rest).decode_utf8().ok()?;
    let s = decoded.trim_end_matches('/');
    Some(std::path::PathBuf::from(if s.is_empty() { "/" } else { s }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls() {
        assert_eq!(file_url_to_path("file:///Users/x/My%20Folder/").unwrap(), std::path::PathBuf::from("/Users/x/My Folder"));
        assert_eq!(file_url_to_path("file://localhost/tmp/a.rs").unwrap(), std::path::PathBuf::from("/tmp/a.rs"));
        assert!(file_url_to_path("https://x.com/a").is_none());
        assert_eq!(file_url_to_path("file:///").unwrap(), std::path::PathBuf::from("/"));
    }
}
