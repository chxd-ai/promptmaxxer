//! macOS calls: front app, the whole pasteboard (every item and type), synthetic Cmd+C / Cmd+V, modifier state,
//! the Accessibility check, and notifications.
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString, NSPasteboardWriting, NSWorkspace};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSNumber, NSString};

/// Same shape as `pmx_context::AppInfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    pub name: String,
    pub bundle_id: String,
    pub pid: i32,
}

pub fn front_app() -> Option<AppInfo> {
    autoreleasepool(|_| {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        Some(AppInfo {
            name: app.localizedName().map(|s| s.to_string()).unwrap_or_else(|| "unknown".into()),
            bundle_id: app.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default(),
            pid: app.processIdentifier(),
        })
    })
}

/// Terminals: the selection usually cannot be replaced, so the result goes to the clipboard only (SPEC section 1).
pub fn is_terminal(app: &AppInfo) -> bool {
    const IDS: &[&str] = &["com.apple.Terminal", "com.googlecode.iterm2", "com.mitchellh.ghostty", "dev.warp.Warp-Stable", "dev.warp.Warp"];
    const NAMES: &[&str] = &["Terminal", "iTerm2", "iTerm", "Ghostty", "Warp"];
    IDS.contains(&app.bundle_id.as_str()) || NAMES.contains(&app.name.as_str())
}

/// Apps that mix chat boxes and terminal panes (Orca). Their chat box takes a normal paste; a terminal pane
/// (xterm.js, labelled "Terminal input") does not replace the selection.
pub fn is_mixed_terminal_app(app: &AppInfo) -> bool {
    app.bundle_id == "com.stablyai.orca" || app.name == "Orca"
}

/// The focused element of app `pid` is a terminal pane: its Accessibility label or description says "terminal".
pub fn focused_is_terminal(pid: i32) -> bool {
    focused_labels(pid).iter().any(|l| l.to_lowercase().contains("terminal"))
}

fn focused_labels(pid: i32) -> Vec<String> {
    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;
    type AXUIElementRef = *const c_void;
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(e: AXUIElementRef, attr: CFStringRef, value: *mut CFTypeRef) -> i32;
        fn AXUIElementSetMessagingTimeout(e: AXUIElementRef, secs: f32) -> i32;
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFStringGetTypeID() -> usize;
    }
    let mut out = vec![];
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return out;
        }
        AXUIElementSetMessagingTimeout(app, 0.15);
        let mut focused: CFTypeRef = std::ptr::null();
        let attr = CFString::from_static_string("AXFocusedUIElement");
        let ok = AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut focused) == 0 && !focused.is_null();
        CFRelease(app as CFTypeRef);
        if !ok {
            return out;
        }
        for name in ["AXDescription", "AXTitle", "AXRoleDescription"] {
            let attr = CFString::new(name);
            let mut v: CFTypeRef = std::ptr::null();
            if AXUIElementCopyAttributeValue(focused as AXUIElementRef, attr.as_concrete_TypeRef(), &mut v) == 0 && !v.is_null() {
                if CFGetTypeID(v) == CFStringGetTypeID() {
                    out.push(CFString::wrap_under_create_rule(v as CFStringRef).to_string());
                } else {
                    CFRelease(v);
                }
            }
        }
        CFRelease(focused);
    }
    out
}

/// Everything on the general pasteboard: each item with each of its types and bytes.
pub struct SavedClipboard(Vec<Vec<(String, Vec<u8>)>>);

impl SavedClipboard {
    pub fn item_count(&self) -> usize {
        self.0.len()
    }
}

pub fn change_count() -> isize {
    NSPasteboard::generalPasteboard().changeCount()
}

// Clipboard watch. Some apps copy on select (Claude Code's fullscreen view runs `pbcopy` on mouse release and
// has no terminal selection, so our Cmd+C copies nothing). To use that copy, we must know when the clipboard
// last changed and whether the change was ours.
struct ClipWatch {
    count: isize,
    at: std::time::Instant,
    own: bool,
}
static CLIP_WATCH: std::sync::Mutex<Option<ClipWatch>> = std::sync::Mutex::new(None);

fn clip_note(own: bool) {
    let count = change_count();
    let mut g = CLIP_WATCH.lock().unwrap();
    match g.as_mut() {
        Some(w) if w.count == count => {}
        _ => *g = Some(ClipWatch { count, at: std::time::Instant::now(), own }),
    }
}

/// Poll the clipboard change count 4 times a second, from app start.
pub fn start_clipboard_watch() {
    clip_note(true); // whatever is on the clipboard at start is old
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        clip_note(false);
    });
}

/// The clipboard was changed by another app (not by promptmaxxer) less than `max` ago.
pub fn copied_elsewhere_within(max: std::time::Duration) -> bool {
    clip_note(false);
    CLIP_WATCH.lock().unwrap().as_ref().is_some_and(|w| !w.own && w.at.elapsed() <= max)
}

/// Remember the current clipboard as our own write.
fn clip_mark_own() {
    let count = change_count();
    *CLIP_WATCH.lock().unwrap() = Some(ClipWatch { count, at: std::time::Instant::now(), own: true });
}

pub fn save_clipboard() -> SavedClipboard {
    autoreleasepool(|_| {
        let pb = NSPasteboard::generalPasteboard();
        let mut items = Vec::new();
        if let Some(list) = pb.pasteboardItems() {
            for item in list.iter() {
                let mut types = Vec::new();
                for t in item.types().iter() {
                    if let Some(data) = item.dataForType(&t) {
                        types.push((t.to_string(), data.to_vec()));
                    }
                }
                items.push(types);
            }
        }
        SavedClipboard(items)
    })
}

pub fn restore_clipboard(saved: &SavedClipboard) {
    autoreleasepool(|_| {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        if saved.0.is_empty() {
            clip_mark_own();
            return;
        }
        let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = saved
            .0
            .iter()
            .map(|types| {
                let item = NSPasteboardItem::new();
                for (t, bytes) in types {
                    item.setData_forType(&NSData::with_bytes(bytes), &NSString::from_str(t));
                }
                ProtocolObject::from_retained(item)
            })
            .collect();
        pb.writeObjects(&NSArray::from_retained_slice(&items));
    });
    clip_mark_own();
}

pub fn clipboard_text() -> Option<String> {
    autoreleasepool(|_| {
        let pb = NSPasteboard::generalPasteboard();
        // SAFETY: an AppKit constant string, valid for the life of the process.
        let t = unsafe { NSPasteboardTypeString };
        pb.stringForType(t).map(|s| s.to_string())
    })
}

pub fn set_clipboard_text(text: &str) -> bool {
    let ok = autoreleasepool(|_| {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let t = unsafe { NSPasteboardTypeString };
        pb.setString_forType(&NSString::from_str(text), t)
    });
    clip_mark_own();
    ok
}

pub const KEY_C: CGKeyCode = 8; // kVK_ANSI_C
pub const KEY_V: CGKeyCode = 9; // kVK_ANSI_V
pub const KEY_Z: CGKeyCode = 6; // kVK_ANSI_Z

/// Press and release Cmd+<key>. Needs Accessibility; without it macOS drops the events silently.
pub fn cmd_key(key: CGKeyCode) -> Result<(), String> {
    let src = CGEventSource::new(CGEventSourceStateID::HIDSystemState).map_err(|_| "no event source")?;
    for down in [true, false] {
        let ev = CGEvent::new_keyboard_event(src.clone(), key, down).map_err(|_| "cannot make key event")?;
        ev.set_flags(CGEventFlags::CGEventFlagCommand);
        ev.post(CGEventTapLocation::HID);
    }
    Ok(())
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGEventSourceFlagsState(state: i32) -> u64;
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
}

/// Shift, Control, Option or Command is held down right now.
pub fn modifiers_held() -> bool {
    const MASK: u64 = 0x0002_0000 | 0x0004_0000 | 0x0008_0000 | 0x0010_0000;
    // 1 = kCGEventSourceStateHIDSystemState
    unsafe { CGEventSourceFlagsState(1) & MASK != 0 }
}

pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Shows the system "allow Accessibility" prompt (once per app identity). The click is the person's.
pub fn ask_accessibility() -> bool {
    autoreleasepool(|_| {
        let key = NSString::from_str("AXTrustedCheckOptionPrompt");
        let yes = NSNumber::new_bool(true);
        let dict = NSDictionary::from_slices(&[&*key], &[&*yes]);
        unsafe { AXIsProcessTrustedWithOptions(Retained::as_ptr(&dict).cast()) }
    })
}

/// Bring an app to the front (used before a paste when our own window took the focus).
pub fn activate(pid: i32) -> bool {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    autoreleasepool(|_| match NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
        Some(app) => {
            #[allow(deprecated)]
            app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps)
        }
        None => false,
    })
}

/// Opens System Settings > Privacy & Security > Accessibility.
pub fn open_accessibility_pane() {
    let _ = std::process::Command::new("/usr/bin/open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .spawn();
}

/// The mouse position in global points, origin top-left of the main screen.
pub fn mouse_location() -> Option<(f64, f64)> {
    let src = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
    let ev = CGEvent::new(src).ok()?;
    let p = ev.location();
    Some((p.x, p.y))
}

/// Screen rectangle (x, y, w, h; global points, origin top-left) of the selected text in the focused element
/// of app `pid`, through Accessibility (`AXSelectedTextRange` + `AXBoundsForRange`). `None` when the app
/// does not expose it (many web views and terminals).
pub fn selection_bounds(pid: i32) -> Option<(f64, f64, f64, f64)> {
    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;
    type AXUIElementRef = *const c_void;
    type AXValueRef = *const c_void;
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct CGPointC {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct CGRectC {
        origin: CGPointC,
        w: f64,
        h: f64,
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(e: AXUIElementRef, attr: CFStringRef, value: *mut CFTypeRef) -> i32;
        fn AXUIElementCopyParameterizedAttributeValue(e: AXUIElementRef, attr: CFStringRef, param: CFTypeRef, value: *mut CFTypeRef) -> i32;
        fn AXUIElementSetMessagingTimeout(e: AXUIElementRef, secs: f32) -> i32;
        fn AXValueGetValue(v: AXValueRef, kind: u32, out: *mut c_void) -> bool;
    }
    const K_AX_VALUE_CGRECT: u32 = 3;
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        AXUIElementSetMessagingTimeout(app, 0.15);
        let mut focused: CFTypeRef = std::ptr::null();
        let attr = CFString::from_static_string("AXFocusedUIElement");
        let ok = AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut focused) == 0 && !focused.is_null();
        CFRelease(app as CFTypeRef);
        if !ok {
            return None;
        }
        AXUIElementSetMessagingTimeout(focused as AXUIElementRef, 0.15);
        let mut range: CFTypeRef = std::ptr::null();
        let attr = CFString::from_static_string("AXSelectedTextRange");
        if AXUIElementCopyAttributeValue(focused as AXUIElementRef, attr.as_concrete_TypeRef(), &mut range) != 0 || range.is_null() {
            CFRelease(focused);
            return None;
        }
        let mut bounds: CFTypeRef = std::ptr::null();
        let attr = CFString::from_static_string("AXBoundsForRange");
        let r = AXUIElementCopyParameterizedAttributeValue(focused as AXUIElementRef, attr.as_concrete_TypeRef(), range, &mut bounds);
        CFRelease(range);
        CFRelease(focused);
        if r != 0 || bounds.is_null() {
            return None;
        }
        let mut rect = CGRectC::default();
        let got = AXValueGetValue(bounds as AXValueRef, K_AX_VALUE_CGRECT, &mut rect as *mut CGRectC as *mut c_void);
        CFRelease(bounds);
        (got && rect.w >= 0.0 && rect.h > 0.0 && (rect.origin.x != 0.0 || rect.origin.y != 0.0))
            .then_some((rect.origin.x, rect.origin.y, rect.w, rect.h))
    }
}

/// Plays a short system sound (settings: sound on).
pub fn play_done_sound() {
    let _ = std::process::Command::new("/usr/bin/afplay").args(["-v", "0.5", "/System/Library/Sounds/Tink.aiff"]).spawn();
}
