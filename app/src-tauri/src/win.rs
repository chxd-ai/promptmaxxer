//! Windows calls, with the same names as `mac.rs` (lib.rs makes this module `crate::mac` on Windows): front app,
//! the whole clipboard (every format), synthetic Ctrl+C / Ctrl+V, modifier state, and the done sound.
//! No Accessibility permission exists on Windows: those calls say "allowed" and do nothing.
//! Apps running as administrator cannot receive keys from a normal app (Windows UIPI): a press there copies nothing.
use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND, LPARAM, POINT};
use windows_sys::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VK_CONTROL, VK_INSERT, VK_LWIN,
    VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, GW_OWNER,
    SW_RESTORE,
};

/// Same shape as `pmx_context::AppInfo`. `bundle_id` is the program file name in lower case (`code.exe`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppInfo {
    pub name: String,
    pub bundle_id: String,
    pub pid: i32,
}

pub fn front_app() -> Option<AppInfo> {
    pmx_context::front_app().map(|a| AppInfo { name: a.name, bundle_id: a.bundle_id, pid: a.pid })
}

/// Terminals: the selection usually cannot be replaced, so the result goes to the clipboard only (SPEC section 1).
pub fn is_terminal(app: &AppInfo) -> bool {
    const EXES: &[&str] = &[
        "windowsterminal.exe",
        "powershell.exe",
        "pwsh.exe",
        "cmd.exe",
        "conhost.exe",
        "openconsole.exe",
        "mintty.exe",
        "wezterm-gui.exe",
        "alacritty.exe",
        "tabby.exe",
        "hyper.exe",
        "warp.exe",
    ];
    EXES.contains(&app.bundle_id.as_str())
}

/// VS Code style editors: their terminal panel sends Ctrl+C to the shell as an interrupt when nothing is selected.
fn is_code_editor(app: &AppInfo) -> bool {
    ["code.exe", "code - insiders.exe", "vscodium.exe", "cursor.exe", "windsurf.exe"].contains(&app.bundle_id.as_str())
}

/// Orca (chat boxes plus terminal panes) is a macOS app; nothing mixes the two on Windows.
pub fn is_mixed_terminal_app(_app: &AppInfo) -> bool {
    false
}

pub fn focused_is_terminal(_pid: i32) -> bool {
    false
}

/// Everything on the clipboard: each format with its bytes. Formats that hold a GDI handle instead of memory
/// (bitmap, metafile, palette) are skipped: Windows makes the bitmap again from the saved DIB.
pub struct SavedClipboard(Vec<(u32, Vec<u8>)>);

impl SavedClipboard {
    pub fn item_count(&self) -> usize {
        usize::from(!self.0.is_empty())
    }
}

pub fn change_count() -> isize {
    // SAFETY: no arguments, no state.
    unsafe { GetClipboardSequenceNumber() as isize }
}

// Clipboard watch: as in mac.rs. Some apps copy on select; to use that copy we must know when the clipboard last
// changed and whether the change was ours.
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

/// Poll the clipboard sequence number 4 times a second, from app start.
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

fn clip_mark_own() {
    let count = change_count();
    *CLIP_WATCH.lock().unwrap() = Some(ClipWatch { count, at: std::time::Instant::now(), own: true });
}

/// Holds the clipboard open; closes it when dropped. Another app may hold it for a moment: retry for 300 ms.
struct Open;
impl Open {
    fn new() -> Option<Open> {
        for _ in 0..30 {
            // SAFETY: plain call; no owner window.
            if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                return Some(Open);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: we opened it.
        unsafe {
            CloseClipboard();
        }
    }
}

const CF_UNICODETEXT: u32 = 13;

/// Formats whose data is a GDI handle, not global memory.
fn is_handle_format(f: u32) -> bool {
    // CF_BITMAP 2, CF_METAFILEPICT 3, CF_PALETTE 9, CF_ENHMETAFILE 14, CF_OWNERDISPLAY 0x80, CF_DSPBITMAP 0x82,
    // CF_DSPMETAFILEPICT 0x83, CF_DSPENHMETAFILE 0x8E, private GDI objects 0x300-0x3FF.
    matches!(f, 2 | 3 | 9 | 14 | 0x80 | 0x82 | 0x83 | 0x8E) || (0x300..=0x3FF).contains(&f)
}

/// Copy of the global memory behind a clipboard handle.
unsafe fn read_global(h: HANDLE) -> Option<Vec<u8>> {
    if h.is_null() {
        return None;
    }
    let size = GlobalSize(h);
    let p = GlobalLock(h) as *const u8;
    if p.is_null() {
        return None;
    }
    let v = std::slice::from_raw_parts(p, size).to_vec();
    GlobalUnlock(h);
    Some(v)
}

/// Put `bytes` on the open clipboard as `format`. The clipboard owns the memory once this succeeds.
unsafe fn write_global(format: u32, bytes: &[u8]) -> bool {
    let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
    if h.is_null() {
        return false;
    }
    let p = GlobalLock(h) as *mut u8;
    if p.is_null() {
        GlobalFree(h);
        return false;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
    GlobalUnlock(h);
    if SetClipboardData(format, h).is_null() {
        GlobalFree(h);
        return false;
    }
    true
}

pub fn save_clipboard() -> SavedClipboard {
    let Some(_open) = Open::new() else { return SavedClipboard(vec![]) };
    let mut out = vec![];
    // SAFETY: the clipboard is open for the whole loop.
    unsafe {
        let mut f = EnumClipboardFormats(0);
        while f != 0 {
            if !is_handle_format(f) {
                if let Some(bytes) = read_global(GetClipboardData(f)) {
                    out.push((f, bytes));
                }
            }
            f = EnumClipboardFormats(f);
        }
    }
    SavedClipboard(out)
}

pub fn restore_clipboard(saved: &SavedClipboard) {
    if let Some(_open) = Open::new() {
        // SAFETY: the clipboard is open.
        unsafe {
            EmptyClipboard();
            for (f, bytes) in &saved.0 {
                write_global(*f, bytes);
            }
        }
    }
    clip_mark_own();
}

pub fn clipboard_text() -> Option<String> {
    let _open = Open::new()?;
    // SAFETY: the clipboard is open.
    let bytes = unsafe { read_global(GetClipboardData(CF_UNICODETEXT))? };
    let wide: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    Some(String::from_utf16_lossy(&wide))
}

pub fn set_clipboard_text(text: &str) -> bool {
    let ok = match Open::new() {
        Some(_open) => {
            let wide: Vec<u8> = text.encode_utf16().chain(std::iter::once(0)).flat_map(|c| c.to_le_bytes()).collect();
            // SAFETY: the clipboard is open.
            unsafe { EmptyClipboard() != 0 && write_global(CF_UNICODETEXT, &wide) }
        }
        None => false,
    };
    clip_mark_own();
    ok
}

pub const KEY_C: u16 = 0x43;
pub const KEY_V: u16 = 0x56;
pub const KEY_Z: u16 = 0x5A;

fn key(vk: u16, up: bool, extended: bool) -> INPUT {
    let mut flags = if up { KEYEVENTF_KEYUP } else { 0 };
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn send(inputs: &[INPUT]) -> Result<(), String> {
    // SAFETY: a slice of plain structs and its exact length.
    let n = unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    if n as usize == inputs.len() {
        Ok(())
    } else {
        Err("Windows did not take the key presses (the app may run as administrator)".into())
    }
}

/// Press and release Ctrl+<key> (the Windows form of Cmd+<key>).
pub fn cmd_key(vk: u16) -> Result<(), String> {
    send(&[key(VK_CONTROL, false, false), key(vk, false, false), key(vk, true, false), key(VK_CONTROL, true, false)])
}

/// Copy the selection in `app`. Terminals and VS Code style editors get Ctrl+Insert: there Ctrl+C with nothing
/// selected interrupts the running program (an agent session), and Ctrl+Shift+C can reach the shell as Ctrl+C.
/// Ctrl+Insert copies in Windows Terminal, the console, Git Bash and VS Code, and never interrupts.
pub fn copy_selection(app: &AppInfo) -> Result<(), String> {
    if is_terminal(app) || is_code_editor(app) {
        send(&[key(VK_CONTROL, false, false), key(VK_INSERT, false, true), key(VK_INSERT, true, true), key(VK_CONTROL, true, false)])
    } else {
        cmd_key(KEY_C)
    }
}

/// Shift, Ctrl, Alt or a Windows key is held down right now.
pub fn modifiers_held() -> bool {
    // SAFETY: plain calls.
    [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN].iter().any(|&vk| unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000 != 0)
}

/// No Accessibility permission on Windows.
pub fn accessibility_trusted() -> bool {
    true
}

pub fn ask_accessibility() -> bool {
    true
}

pub fn open_accessibility_pane() {}

/// The first visible top-level window of `pid`.
fn main_window(pid: i32) -> Option<HWND> {
    struct Find {
        pid: u32,
        found: HWND,
    }
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> i32 {
        let f = &mut *(lp as *mut Find);
        let mut owner = 0u32;
        GetWindowThreadProcessId(hwnd, &mut owner);
        if owner == f.pid && IsWindowVisible(hwnd) != 0 && GetWindow(hwnd, GW_OWNER).is_null() {
            f.found = hwnd;
            return 0; // stop
        }
        1
    }
    let mut f = Find { pid: pid as u32, found: std::ptr::null_mut() };
    // SAFETY: `f` outlives the call; the callback only reads window facts.
    unsafe {
        EnumWindows(Some(each), &mut f as *mut Find as LPARAM);
    }
    (!f.found.is_null()).then_some(f.found)
}

/// Bring an app to the front (used before a paste when our own bar took the focus).
pub fn activate(pid: i32) -> bool {
    let Some(hwnd) = main_window(pid) else { return false };
    // SAFETY: a live window handle.
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd) != 0
    }
}

/// The mouse position in logical pixels of the screen it is on (the space `bar::screens` uses).
pub fn mouse_location() -> Option<(f64, f64)> {
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: plain calls with out-pointers to locals.
    unsafe {
        if GetCursorPos(&mut p) == 0 {
            return None;
        }
        let mon = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        if mon.is_null() || GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) != 0 {
            dx = 96;
        }
        let s = dx.max(1) as f64 / 96.0;
        Some((p.x as f64 / s, p.y as f64 / s))
    }
}

/// No selection rectangle on Windows: the bar goes near the mouse.
pub fn selection_bounds(_pid: i32) -> Option<(f64, f64, f64, f64)> {
    None
}

/// Plays the default system sound (settings: sound on).
pub fn play_done_sound() {
    // SAFETY: plain call. 0 = MB_OK, the default sound.
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_terminals_are_clipboard_only() {
        let app = |exe: &str| AppInfo { name: exe.trim_end_matches(".exe").into(), bundle_id: exe.into(), pid: 1 };
        assert!(is_terminal(&app("windowsterminal.exe")));
        assert!(is_terminal(&app("pwsh.exe")));
        assert!(is_terminal(&app("cmd.exe")));
        assert!(!is_terminal(&app("code.exe")));
        assert!(!is_terminal(&app("notepad.exe")));
    }

    #[test]
    fn handle_formats_are_skipped() {
        assert!(is_handle_format(2) && is_handle_format(14) && is_handle_format(0x301));
        assert!(!is_handle_format(CF_UNICODETEXT) && !is_handle_format(8) && !is_handle_format(0xC0A1));
    }
}
