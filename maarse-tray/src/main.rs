//! System-tray app for a maarse-host installed as a Scheduled Task by
//! `packaging/windows/install-service.ps1`.
//!
//! Shows whether the host runs, starts/stops/restarts the task (asking for
//! administrator rights only for those actions) and opens the web client,
//! the Service page and the log folder.
//!
//! Options (all optional, defaults match install-service.ps1):
//!   --task <name>          Scheduled Task name (default: maarse-host)
//!   --data <dir>           data folder (default: %ProgramData%\maarse-host)
//!   --install-dir <dir>    install folder (default: %ProgramFiles%\maarse-host)

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("maarse-tray is Windows only.");
}

#[cfg(windows)]
fn main() {
    if let Err(e) = win::run() {
        eprintln!("maarse-tray: {e}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod win {
    use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicIsize, AtomicU8, Ordering};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Shell::{
        NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, ShellExecuteW,
        Shell_NotifyIconW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const WM_TRAY: u32 = WM_APP + 1;
    const WM_STATE: u32 = WM_APP + 2;

    const ID_STATUS: usize = 1;
    const ID_START: usize = 2;
    const ID_STOP: usize = 3;
    const ID_RESTART: usize = 4;
    const ID_WEB: usize = 5;
    const ID_SERVICE: usize = 6;
    const ID_LOGS: usize = 7;
    const ID_QUIT: usize = 8;

    struct Config {
        task: String,
        data: PathBuf,
        install_dir: PathBuf,
    }

    static CONFIG: OnceLock<Config> = OnceLock::new();
    static STATE: AtomicU8 = AtomicU8::new(State::Unknown as u8);
    static HWND_MAIN: AtomicIsize = AtomicIsize::new(0);
    static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();

    fn cfg() -> &'static Config {
        CONFIG.get().expect("config set at startup")
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    enum State {
        Unknown = 0,
        /// Task and web client both up.
        Running = 1,
        /// Task runs but the web client does not answer (yet).
        Starting = 2,
        Stopped = 3,
        /// Task not installed (or not readable) and no web client.
        Missing = 4,
    }

    impl State {
        fn from_u8(v: u8) -> State {
            match v {
                1 => State::Running,
                2 => State::Starting,
                3 => State::Stopped,
                4 => State::Missing,
                _ => State::Unknown,
            }
        }
        fn label(self) -> &'static str {
            match self {
                State::Unknown => "Checking...",
                State::Running => "Running",
                State::Starting => "Starting (web client not answering)",
                State::Stopped => "Stopped",
                State::Missing => "Not installed (task not found)",
            }
        }
        /// Icon colour as 0x00RRGGBB.
        fn colour(self) -> u32 {
            match self {
                State::Running => 0x2e9e44,
                State::Starting => 0xe0a020,
                State::Stopped => 0xc83232,
                State::Missing | State::Unknown => 0x808080,
            }
        }
    }

    fn parse_args() -> Result<Config, String> {
        let env_dir = |var: &str, fallback: &str| {
            PathBuf::from(std::env::var_os(var).unwrap_or_else(|| fallback.into())).join("maarse-host")
        };
        let mut c = Config {
            task: "maarse-host".into(),
            data: env_dir("ProgramData", r"C:\ProgramData"),
            install_dir: env_dir("ProgramFiles", r"C:\Program Files"),
        };
        let mut args = std::env::args().skip(1);
        while let Some(a) = args.next() {
            let mut val = || args.next().ok_or_else(|| format!("{a} needs a value"));
            match a.as_str() {
                "--task" => c.task = val()?,
                "--data" => c.data = val()?.into(),
                "--install-dir" => c.install_dir = val()?.into(),
                "--help" | "-h" => {
                    return Err("usage: maarse-tray [--task NAME] [--data DIR] [--install-dir DIR]".into());
                }
                other => return Err(format!("unknown option: {other}")),
            }
        }
        Ok(c)
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    // ---- host settings -------------------------------------------------

    /// The `--http` address from `<data>/settings.txt`, default 127.0.0.1:8080.
    /// A wildcard bind address is shown as 127.0.0.1.
    fn http_addr() -> String {
        let text = std::fs::read_to_string(cfg().data.join("settings.txt")).unwrap_or_default();
        let mut addr = "127.0.0.1:8080".to_string();
        for line in text.lines() {
            if let Some(v) = line.trim().strip_prefix("--http") {
                let v = v.trim();
                if !v.is_empty() {
                    addr = v.to_string();
                }
            }
        }
        if let Some(port) = addr.strip_prefix("0.0.0.0:") {
            addr = format!("127.0.0.1:{port}");
        } else if let Some(port) = addr.strip_prefix("[::]:") {
            addr = format!("[::1]:{port}");
        }
        addr
    }

    fn client_url(fragment: &str) -> String {
        // The web client reads the key from the fragment (`#token=…`), the same
        // form the host prints at startup; extra flags follow as `&service`.
        let mut parts = Vec::new();
        if let Ok(t) = std::fs::read_to_string(cfg().data.join("access-key.txt")) {
            let t = t.trim();
            if !t.is_empty() {
                parts.push(format!("token={t}"));
            }
        }
        if !fragment.is_empty() {
            parts.push(fragment.to_string());
        }
        let mut url = format!("http://{}/", http_addr());
        if !parts.is_empty() {
            url.push('#');
            url.push_str(&parts.join("&"));
        }
        url
    }

    // ---- status --------------------------------------------------------

    /// Some(true) = task running, Some(false) = installed but not running,
    /// None = not found or not readable.
    fn task_running() -> Option<bool> {
        let out = Command::new("schtasks")
            .args(["/Query", "/TN", &cfg().task, "/FO", "CSV", "/NH"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        // "\TaskName","Next run","Status" – status is localized, but
        // "Running" is the English one; also check the running process.
        let text = String::from_utf8_lossy(&out.stdout);
        let status = text.lines().next()?.rsplit(',').next()?.trim().trim_matches('"').to_string();
        Some(status.eq_ignore_ascii_case("Running") || host_process_running())
    }

    fn host_process_running() -> bool {
        Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq maarse-host.exe", "/NH"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_ascii_lowercase().contains("maarse-host.exe"))
            .unwrap_or(false)
    }

    fn web_up() -> bool {
        let addrs: Vec<SocketAddr> = http_addr().to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
        addrs.iter().any(|a| TcpStream::connect_timeout(a, Duration::from_millis(500)).is_ok())
    }

    fn poll_state() -> State {
        let web = web_up();
        match (task_running(), web) {
            (_, true) => State::Running,
            (Some(true), false) => State::Starting,
            (Some(false), false) => State::Stopped,
            (None, false) => {
                if host_process_running() {
                    State::Starting
                } else {
                    State::Missing
                }
            }
        }
    }

    fn spawn_poller() {
        std::thread::spawn(|| {
            loop {
                let s = poll_state();
                let old = STATE.swap(s as u8, Ordering::SeqCst);
                if old != s as u8 {
                    let hwnd = HWND_MAIN.load(Ordering::SeqCst) as HWND;
                    unsafe { PostMessageW(hwnd, WM_STATE, 0, 0) };
                }
                std::thread::sleep(Duration::from_secs(3));
            }
        });
    }

    // ---- actions -------------------------------------------------------

    fn ps_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "''"))
    }

    /// PowerShell that ends the task and then the host. Ending the task
    /// kills the supervisor (run-host.ps1) first, so nothing respawns the
    /// host; the host itself is a child that survives, so it is stopped by
    /// path. Same steps as Stop-HostTask in install-service.ps1.
    fn stop_script() -> String {
        let exe = cfg().install_dir.join("maarse-host.exe");
        format!(
            "Stop-ScheduledTask -TaskName {task} -ErrorAction SilentlyContinue; \
             Get-Process maarse-host -ErrorAction SilentlyContinue | \
             Where-Object {{ $_.Path -eq {exe} }} | Stop-Process -Force -ErrorAction SilentlyContinue",
            task = ps_quote(&cfg().task),
            exe = ps_quote(&exe.to_string_lossy()),
        )
    }

    fn start_script() -> String {
        format!("Start-ScheduledTask -TaskName {}", ps_quote(&cfg().task))
    }

    /// Runs a PowerShell command elevated (UAC prompt), hidden.
    fn run_elevated(script: &str) {
        let params = format!(
            "-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -Command \"{}\"",
            script.replace('"', "\\\"")
        );
        shell_execute("runas", "powershell.exe", Some(&params));
    }

    fn shell_execute(verb: &str, file: &str, params: Option<&str>) {
        let verb = wide(verb);
        let file = wide(file);
        let params = params.map(wide);
        unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                file.as_ptr(),
                params.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                std::ptr::null(),
                SW_SHOWNORMAL,
            );
        }
    }

    fn on_command(hwnd: HWND, id: usize) {
        match id {
            ID_START => run_elevated(&start_script()),
            ID_STOP => run_elevated(&stop_script()),
            ID_RESTART => {
                run_elevated(&format!("{}; Start-Sleep -Seconds 2; {}", stop_script(), start_script()))
            }
            ID_WEB => shell_execute("open", &client_url(""), None),
            ID_SERVICE => shell_execute("open", &client_url("service"), None),
            ID_LOGS => {
                let dir = cfg().data.join("logs");
                shell_execute("open", &dir.to_string_lossy(), None);
            }
            ID_QUIT => unsafe {
                DestroyWindow(hwnd);
            },
            _ => {}
        }
    }

    // ---- icon and menu -------------------------------------------------

    /// A 16x16 filled circle in the given colour on a transparent background.
    fn make_icon(rgb: u32) -> HICON {
        const N: i32 = 16;
        let mut px = vec![0u32; (N * N) as usize];
        let c = (N as f32 - 1.0) / 2.0;
        for y in 0..N {
            for x in 0..N {
                let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
                let i = (y * N + x) as usize;
                if d <= 7.2 {
                    // Dark rim, coloured fill.
                    let col = if d > 6.0 { 0x202020 } else { rgb };
                    px[i] = 0xff00_0000 | col;
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

    fn notify(hwnd: HWND, msg: u32) {
        let state = State::from_u8(STATE.load(Ordering::SeqCst));
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        let icon = if msg == NIM_DELETE { std::ptr::null_mut() } else { make_icon(state.colour()) };
        nid.hIcon = icon;
        let tip: Vec<u16> = format!("Må:rse host: {}", state.label()).encode_utf16().take(127).collect();
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        unsafe {
            Shell_NotifyIconW(msg, &nid);
            if !icon.is_null() {
                DestroyIcon(icon);
            }
        }
    }

    fn show_menu(hwnd: HWND) {
        let state = State::from_u8(STATE.load(Ordering::SeqCst));
        let running = matches!(state, State::Running | State::Starting);
        let installed = state != State::Missing;
        unsafe {
            let m = CreatePopupMenu();
            let add = |flags: u32, id: usize, text: &str| {
                let w = wide(text);
                AppendMenuW(m, flags, id, w.as_ptr());
            };
            let en = |ok: bool| if ok { MF_STRING } else { MF_STRING | MF_GRAYED };
            add(MF_STRING | MF_GRAYED, ID_STATUS, &format!("Host: {}", state.label()));
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            add(en(installed && !running), ID_START, "Start");
            add(en(running), ID_STOP, "Stop");
            add(en(installed), ID_RESTART, "Restart");
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            add(MF_STRING, ID_WEB, "Open web client");
            add(MF_STRING, ID_SERVICE, "Open Service page");
            add(MF_STRING, ID_LOGS, "Open log folder");
            AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
            add(MF_STRING, ID_QUIT, "Quit");

            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            // Required so the menu closes when clicking elsewhere.
            SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, std::ptr::null());
            DestroyMenu(m);
            if cmd > 0 {
                on_command(hwnd, cmd as usize);
            }
        }
    }

    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_TRAY => {
                let ev = (lp & 0xffff) as u32;
                if ev == WM_RBUTTONUP || ev == WM_LBUTTONUP || ev == WM_CONTEXTMENU {
                    show_menu(hwnd);
                } else if ev == WM_LBUTTONDBLCLK {
                    on_command(hwnd, ID_WEB);
                }
                0
            }
            WM_STATE => {
                notify(hwnd, NIM_MODIFY);
                0
            }
            WM_DESTROY => {
                notify(hwnd, NIM_DELETE);
                unsafe { PostQuitMessage(0) };
                0
            }
            _ if Some(&msg) == TASKBAR_CREATED.get() => {
                // Explorer restarted: put the icon back.
                notify(hwnd, NIM_ADD);
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        }
    }

    pub fn run() -> Result<(), String> {
        let c = parse_args()?;
        CONFIG.set(c).map_err(|_| "config already set")?;
        unsafe {
            let hinst = GetModuleHandleW(std::ptr::null());
            let class = wide("MaarseTrayWindow");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinst,
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            if RegisterClassW(&wc) == 0 {
                return Err("RegisterClassW failed".into());
            }
            let _ = TASKBAR_CREATED.set(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()));
            // Hidden top-level window (not message-only, so TaskbarCreated arrives).
            let title = wide("maarse-tray");
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinst,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return Err("CreateWindowExW failed".into());
            }
            HWND_MAIN.store(hwnd as isize, Ordering::SeqCst);
            notify(hwnd, NIM_ADD);
            spawn_poller();

            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }
}
