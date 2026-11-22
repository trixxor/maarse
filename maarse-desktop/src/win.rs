//! The Windows side of the tray app: splash, tray icon, update window. It runs as the signed-in user and
//! needs no rights of its own: the Maarse service (SYSTEM) runs the host and installs updates, and this app
//! talks to it over a named pipe.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{COLOR_BTNFACE, CreateBitmap, DeleteObject, GetStockObject};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, ShellExecuteW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use maarse_service::ipc::{self, Status, UpdateInfo};

const ERROR_ALREADY_EXISTS: u32 = 183;
const VERSION: &str = match option_env!("MAARSE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

const WM_TRAY: u32 = WM_APP + 1;
const WM_STATE: u32 = WM_APP + 2;
const WM_CHECKED: u32 = WM_APP + 3;
const WM_RESTART_SELF: u32 = WM_APP + 4;

const ID_STATUS: usize = 1;
const ID_WEB: usize = 2;
const ID_UPDATE: usize = 3;
const ID_LOGS: usize = 4;
const ID_QUIT: usize = 5;
const ID_START: usize = 6;
const ID_BTN_UPDATE: usize = 101;
const ID_BTN_GITHUB: usize = 102;
const ID_BTN_CLOSE: usize = 103;

static HWND_MAIN: AtomicIsize = AtomicIsize::new(0);
static HWND_SPLASH: AtomicIsize = AtomicIsize::new(0);
static HWND_CHANGES: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();
/// The last answer of the service, or why there is none.
static STATUS: Mutex<Result<Status, String>> = Mutex::new(Err(String::new()));
static OPEN_BROWSER: AtomicBool = AtomicBool::new(true);
/// Last version a balloon was shown for, so one update gives one notice.
static NOTIFIED: Mutex<String> = Mutex::new(String::new());
/// Last update error shown, so it is shown once.
static SHOWN_ERROR: Mutex<String> = Mutex::new(String::new());

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn hwnd(slot: &AtomicIsize) -> HWND {
    slot.load(Ordering::SeqCst) as HWND
}

fn post(slot: &AtomicIsize, msg: u32) {
    unsafe { PostMessageW(hwnd(slot), msg, 0, 0) };
}

pub fn error_box(text: &str) {
    let (t, c) = (wide(text), wide("Må:rse"));
    unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONERROR) };
}

fn open(target: &str) {
    let (verb, file) = (wide("open"), wide(target));
    unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
    }
}

fn log_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into())).join("Maarse").join("logs")
}

// ---- talking to the service ----------------------------------------------

fn set_status(r: Result<Status, String>) {
    *STATUS.lock().unwrap() = r;
}

fn current() -> Result<Status, String> {
    STATUS.lock().unwrap().clone()
}

fn update_info() -> Option<UpdateInfo> {
    current().ok().and_then(|s| s.update)
}

/// Polls the service every 3 seconds. Tells the window when something changed, opens the web client once
/// when the host is first up, and notices when an update has replaced the files (the service then reports
/// a new version): then this app starts itself again from the new files.
fn spawn_poller() {
    std::thread::spawn(|| {
        let mut last = String::new();
        loop {
            let r = ipc::request("status");
            if let Ok(s) = &r {
                if s.version != VERSION && !s.version.is_empty() {
                    set_status(r);
                    post(&HWND_MAIN, WM_RESTART_SELF);
                    return;
                }
                if s.state == "running" && OPEN_BROWSER.swap(false, Ordering::SeqCst) {
                    open(&s.url);
                }
            }
            let key = format!("{r:?}");
            set_status(r);
            if key != last {
                last = key;
                post(&HWND_MAIN, WM_STATE);
            }
            std::thread::sleep(Duration::from_secs(3));
        }
    });
}

/// Sends a command on a background thread and shows the answer when it arrives.
fn command_async(cmd: &'static str) {
    std::thread::spawn(move || {
        set_status(ipc::request(cmd));
        post(&HWND_MAIN, WM_STATE);
    });
}

// ---- splash and changes window ---------------------------------------------

fn centred(w: i32, h: i32) -> (i32, i32) {
    unsafe { ((GetSystemMetrics(SM_CXSCREEN) - w) / 2, (GetSystemMetrics(SM_CYSCREEN) - h) / 2) }
}

fn create_splash() {
    let (w, h) = (420, 150);
    let (x, y) = centred(w, h);
    let text = wide(&format!("Må:rse\r\n\r\nChecking for updates …\r\nversion {VERSION}"));
    unsafe {
        let s = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            wide("STATIC").as_ptr(),
            text.as_ptr(),
            WS_POPUP | WS_VISIBLE | WS_BORDER | 0x1 /* SS_CENTER */ | 0x200, /* SS_CENTERIMAGE */
            x,
            y,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        );
        SendMessageW(s, WM_SETFONT, GetStockObject(17) as usize, 1);
        HWND_SPLASH.store(s as isize, Ordering::SeqCst);
    }
}

fn splash_text(text: &str) {
    let t = wide(text);
    unsafe { SetWindowTextW(hwnd(&HWND_SPLASH), t.as_ptr()) };
}

fn close_splash() {
    let s = HWND_SPLASH.swap(0, Ordering::SeqCst) as HWND;
    if !s.is_null() {
        unsafe { DestroyWindow(s) };
    }
}

/// The window with the changes and fixes of the new version (the release notes on GitHub).
fn show_changes(owner: HWND) {
    let Some(r) = update_info() else { return };
    let existing = hwnd(&HWND_CHANGES);
    if !existing.is_null() {
        unsafe { SetForegroundWindow(existing) };
        return;
    }
    let (w, h) = (620, 520);
    let (x, y) = centred(w, h);
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let title = wide(&format!("Må:rse {} is available (you have {VERSION})", r.version));
        let win = CreateWindowExW(
            0,
            wide("MaarseChanges").as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            x,
            y,
            w,
            h,
            owner,
            std::ptr::null_mut(),
            inst,
            std::ptr::null(),
        );
        if win.is_null() {
            return;
        }
        let notes = if r.notes.is_empty() { "No release notes were written for this version.".to_string() } else { r.notes.clone() };
        let notes = wide(&notes.replace("\r\n", "\n").replace('\n', "\r\n"));
        let child = |class: &str, text: &[u16], style: u32, x: i32, y: i32, w: i32, h: i32, id: usize| {
            let c = CreateWindowExW(
                if class == "EDIT" { WS_EX_CLIENTEDGE } else { 0 },
                wide(class).as_ptr(),
                text.as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                x,
                y,
                w,
                h,
                win,
                id as _,
                inst,
                std::ptr::null(),
            );
            SendMessageW(c, WM_SETFONT, GetStockObject(17) as usize, 1);
        };
        // ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL
        child("EDIT", &notes, WS_VSCROLL | 0x4 | 0x800 | 0x40, 10, 10, w - 36, h - 100, 0);
        child("BUTTON", &wide("Update and restart"), WS_TABSTOP, 10, h - 78, 160, 30, ID_BTN_UPDATE);
        child("BUTTON", &wide("View on GitHub"), WS_TABSTOP, 180, h - 78, 140, 30, ID_BTN_GITHUB);
        child("BUTTON", &wide("Close"), WS_TABSTOP, w - 130, h - 78, 100, 30, ID_BTN_CLOSE);
        HWND_CHANGES.store(win as isize, Ordering::SeqCst);
        SetForegroundWindow(win);
    }
}

unsafe extern "system" fn changes_proc(win: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            match wp & 0xffff {
                ID_BTN_UPDATE => {
                    unsafe { DestroyWindow(win) };
                    command_async("update");
                }
                ID_BTN_GITHUB => {
                    if let Some(r) = update_info() {
                        open(&r.page);
                    }
                }
                ID_BTN_CLOSE => unsafe {
                    DestroyWindow(win);
                },
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(win) };
            0
        }
        WM_DESTROY => {
            HWND_CHANGES.store(0, Ordering::SeqCst);
            0
        }
        _ => unsafe { DefWindowProcW(win, msg, wp, lp) },
    }
}

// ---- tray icon and menu ------------------------------------------------------

fn state_label(st: &Result<Status, String>) -> &'static str {
    match st {
        Err(_) => "Service not running",
        Ok(s) => match s.state.as_str() {
            "running" => "Running",
            "updating" => "Updating",
            "stopped" => "Host stopped",
            _ => "Starting",
        },
    }
}

fn state_colour(st: &Result<Status, String>) -> u32 {
    match st {
        Err(_) => 0xc83232,
        Ok(s) => match s.state.as_str() {
            "running" => 0x2e9e44,
            "stopped" => 0xc83232,
            _ => 0xe0a020,
        },
    }
}

fn make_icon(rgb: u32, badge: bool) -> HICON {
    const N: i32 = 16;
    let mut px = vec![0u32; (N * N) as usize];
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            if d <= 7.2 {
                px[(y * N + x) as usize] = 0xff00_0000 | if d > 6.0 { 0x202020 } else { rgb };
            }
            // A blue dot in the corner: an update is ready.
            if badge && x >= 10 && y <= 5 && ((x as f32 - 12.5).powi(2) + (y as f32 - 2.5).powi(2)).sqrt() <= 3.0 {
                px[(y * N + x) as usize] = 0xff00_0000 | 0x2060e0;
            }
        }
    }
    let mask = vec![0u8; (N * N / 8) as usize];
    unsafe {
        let color = CreateBitmap(N, N, 1, 32, px.as_ptr().cast());
        let maskbm = CreateBitmap(N, N, 1, 1, mask.as_ptr().cast());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: maskbm, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(maskbm);
        icon
    }
}

fn put_text<const N: usize>(dst: &mut [u16; N], text: &str) {
    let t: Vec<u16> = text.encode_utf16().take(N - 1).collect();
    dst[..t.len()].copy_from_slice(&t);
}

fn notify(win: HWND, msg: u32, balloon: Option<(&str, &str)>) {
    let st = current();
    let update = update_info();
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = win;
    nid.uID = 1;
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    let icon = if msg == NIM_DELETE { std::ptr::null_mut() } else { make_icon(state_colour(&st), update.is_some()) };
    nid.hIcon = icon;
    let mut tip = format!("Må:rse {VERSION}: {}", state_label(&st));
    if let Some(r) = &update {
        tip.push_str(&format!(" - update {} ready", r.version));
    }
    put_text(&mut nid.szTip, &tip);
    if let Some((title, text)) = balloon {
        nid.uFlags |= NIF_INFO;
        put_text(&mut nid.szInfoTitle, title);
        put_text(&mut nid.szInfo, text);
        nid.dwInfoFlags = 1; // NIIF_INFO
    }
    unsafe {
        Shell_NotifyIconW(msg, &nid);
        if !icon.is_null() {
            DestroyIcon(icon);
        }
    }
}

fn show_menu(win: HWND) {
    let st = current();
    unsafe {
        let m = CreatePopupMenu();
        let add = |flags: u32, id: usize, text: &str| {
            let w = wide(text);
            AppendMenuW(m, flags, id, w.as_ptr());
        };
        add(MF_STRING | MF_GRAYED, ID_STATUS, &format!("Må:rse {VERSION}: {}", state_label(&st)));
        if let Some(r) = update_info() {
            add(MF_STRING, ID_UPDATE, &format!("Update {} is ready - see what changed ...", r.version));
        }
        if matches!(&st, Ok(s) if s.state == "stopped") {
            add(MF_STRING, ID_START, "Start the host");
        }
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        add(if st.is_ok() { MF_STRING } else { MF_STRING | MF_GRAYED }, ID_WEB, "Open the PC client");
        add(MF_STRING, ID_LOGS, "Open log folder");
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        add(MF_STRING, ID_QUIT, "Close this icon (Må:rse keeps running)");
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(win);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, win, std::ptr::null());
        DestroyMenu(m);
        if cmd > 0 {
            on_command(win, cmd as usize);
        }
    }
}

fn on_command(win: HWND, id: usize) {
    match id {
        ID_WEB => {
            if let Ok(s) = current() {
                open(&s.url);
            }
        }
        ID_UPDATE => show_changes(win),
        ID_START => command_async("start"),
        ID_LOGS => open(&log_dir().to_string_lossy()),
        ID_QUIT => unsafe {
            DestroyWindow(win);
        },
        _ => {}
    }
}

/// A new update was found: a balloon once per version, and the icon gets its badge.
fn announce_update(win: HWND) {
    let Some(r) = update_info() else { return };
    let mut seen = NOTIFIED.lock().unwrap();
    let first = *seen != r.version;
    *seen = r.version.clone();
    drop(seen);
    let text = format!("Version {} is ready. Click to see what changed.", r.version);
    notify(win, NIM_MODIFY, if first { Some(("Må:rse update", text.as_str())) } else { None });
}

/// The startup check is done: ask whether to update, or just go on.
fn startup_decision(win: HWND) {
    match (current(), update_info()) {
        (Err(e), _) => {
            close_splash();
            error_box(&format!(
                "The Må:rse service is not running, so the PC client cannot start.\n\n{e}\n\nStart \"Maarse\" in Windows Services, or reinstall Må:rse."
            ));
        }
        (Ok(_), Some(r)) => {
            let text = wide(&format!(
                "Version {} is available (you have {VERSION}).\n\nUpdate now? The app restarts when it is done.",
                r.version
            ));
            let caption = wide("Må:rse update");
            let answer = unsafe {
                MessageBoxW(hwnd(&HWND_SPLASH), text.as_ptr(), caption.as_ptr(), MB_YESNO | MB_ICONQUESTION | MB_TOPMOST)
            };
            if answer == IDYES {
                splash_text(&format!("Må:rse\r\n\r\nInstalling version {} …\r\nThe app restarts by itself.", r.version));
                command_async("update");
                return;
            }
            // "No": go on with the version we have; the tray shows that an update is ready.
            close_splash();
            notify(win, NIM_MODIFY, None);
        }
        (Ok(_), None) => close_splash(),
    }
}

/// A failed update is shown once.
fn show_update_error() {
    let Ok(s) = current() else { return };
    if s.error.is_empty() {
        return;
    }
    let mut shown = SHOWN_ERROR.lock().unwrap();
    if *shown == s.error {
        return;
    }
    *shown = s.error.clone();
    drop(shown);
    close_splash();
    error_box(&format!("The update failed and nothing was changed.\n\n{}", s.error));
}

unsafe extern "system" fn wnd_proc(win: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            let ev = (lp & 0xffff) as u32;
            if ev == WM_RBUTTONUP || ev == WM_LBUTTONUP || ev == WM_CONTEXTMENU {
                show_menu(win);
            } else if ev == WM_LBUTTONDBLCLK {
                on_command(win, ID_WEB);
            } else if ev == 0x405 {
                // NIN_BALLOONUSERCLICK
                show_changes(win);
            }
            0
        }
        WM_STATE => {
            notify(win, NIM_MODIFY, None);
            announce_update(win);
            show_update_error();
            0
        }
        WM_CHECKED => {
            startup_decision(win);
            notify(win, NIM_MODIFY, None);
            0
        }
        WM_RESTART_SELF => {
            // The service has installed a new version: start the new files and close this copy.
            if let Ok(exe) = std::env::current_exe() {
                let _ = Command::new(exe).args(["--updated", "--no-browser"]).spawn();
            }
            close_splash();
            unsafe { DestroyWindow(win) };
            0
        }
        WM_DESTROY => {
            notify(win, NIM_DELETE, None);
            unsafe { PostQuitMessage(0) };
            0
        }
        _ if Some(&msg) == TASKBAR_CREATED.get() => {
            notify(win, NIM_ADD, None);
            0
        }
        _ => unsafe { DefWindowProcW(win, msg, wp, lp) },
    }
}

pub fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return Err("Usage: maarse [--no-browser]".into());
    }
    if args.iter().any(|a| a == "--no-browser") {
        OPEN_BROWSER.store(false, Ordering::SeqCst);
    }
    unsafe {
        // One copy per user session. After an update the old copy may still be closing: wait for it.
        let name = wide("Local\\MaarseDesktop");
        let wait = if args.iter().any(|a| a == "--updated") { 15 } else { 0 };
        let deadline = Instant::now() + Duration::from_secs(wait);
        loop {
            let m = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
            if GetLastError() != ERROR_ALREADY_EXISTS && !m.is_null() {
                break; // the handle stays open for the life of the process
            }
            if Instant::now() >= deadline {
                if let Ok(s) = ipc::request("status") {
                    open(&s.url);
                }
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let classes: [(&str, WNDPROC); 2] = [("MaarseTrayWindow", Some(wnd_proc)), ("MaarseChanges", Some(changes_proc))];
        for (class, proc) in classes {
            let class = wide(class);
            let wc = WNDCLASSW {
                lpfnWndProc: proc,
                hInstance: inst,
                lpszClassName: class.as_ptr(),
                hbrBackground: (COLOR_BTNFACE + 1) as _,
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                ..std::mem::zeroed()
            };
            if RegisterClassW(&wc) == 0 {
                return Err("Could not register the window class.".into());
            }
        }
        let _ = TASKBAR_CREATED.set(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()));
        let win = CreateWindowExW(
            0,
            wide("MaarseTrayWindow").as_ptr(),
            wide("Må:rse").as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inst,
            std::ptr::null(),
        );
        if win.is_null() {
            return Err("Could not create the window.".into());
        }
        HWND_MAIN.store(win as isize, Ordering::SeqCst);
        notify(win, NIM_ADD, None);
        // After an update the splash is not needed again.
        if args.iter().any(|a| a == "--updated") {
            spawn_poller();
        } else {
            create_splash();
            std::thread::spawn(|| {
                set_status(ipc::request("check"));
                post(&HWND_MAIN, WM_CHECKED);
                spawn_poller();
            });
        }

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
