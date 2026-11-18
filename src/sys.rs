//! Utilidades del sistema: ventanas, procesos, teclado, cuenta de usuario, autoarranque.

use windows::core::{w, BOOL, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;

// ---------------------------------------------------------------- teclado
fn key(k: VIRTUAL_KEY, up: bool) -> INPUT {
    // Flechas, Inicio, etc. necesitan la marca de tecla extendida
    let ext = matches!(k, VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_HOME | VK_END | VK_PRIOR | VK_NEXT | VK_INSERT | VK_DELETE);
    let mut flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
    if ext { flags |= KEYEVENTF_EXTENDEDKEY; }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: k, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

/// Pulsa una combinación (p. ej. Ctrl+C) y suelta en orden inverso.
pub fn combo(keys: &[VIRTUAL_KEY]) {
    let mut v: Vec<INPUT> = keys.iter().map(|k| key(*k, false)).collect();
    v.extend(keys.iter().rev().map(|k| key(*k, true)));
    unsafe { SendInput(&v, size_of::<INPUT>() as i32); }
}

pub fn vk(c: u8) -> VIRTUAL_KEY { VIRTUAL_KEY(c as u16) }

// ---------------------------------------------------------------- shell
pub fn open(target: &str) { open_args(target, ""); }

pub fn open_args(target: &str, args: &str) {
    unsafe {
        let a = HSTRING::from(args);
        ShellExecuteW(None, w!("open"), &HSTRING::from(target), if args.is_empty() { PCWSTR::null() } else { PCWSTR(a.as_ptr()) }, PCWSTR::null(), SW_SHOWNORMAL);
    }
}

pub fn ask(owner: HWND, text: &str) -> bool {
    unsafe { MessageBoxW(Some(owner), &HSTRING::from(text), w!("macbar"), MB_OKCANCEL | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND) == IDOK }
}

// ---------------------------------------------------------------- procesos y ventanas
pub fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(p);
        ok.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

pub fn window_pid(h: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)); }
    pid
}


fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n as usize])
}

/// ¿Es el escritorio (fondo) o la barra de tareas?
pub fn is_shell_window(h: HWND) -> bool {
    matches!(class_name(h).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
}

/// Ventanas que saldrían en Alt+Tab: visibles, sin dueño, no herramientas, no ocultas por DWM.
pub fn app_windows() -> Vec<HWND> {
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        unsafe {
            let out = &mut *(lp.0 as *mut Vec<HWND>);
            if !IsWindowVisible(h).as_bool() { return TRUE; }
            if GetWindow(h, GW_OWNER).map(|o| !o.0.is_null()).unwrap_or(false) { return TRUE; }
            let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 { return TRUE; }
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as _, 4);
            if cloaked != 0 { return TRUE; }
            if GetWindowTextLengthW(h) == 0 || is_shell_window(h) { return TRUE; }
            out.push(h);
            TRUE
        }
    }
    let mut v: Vec<HWND> = Vec::new();
    unsafe { let _ = EnumWindows(Some(cb), LPARAM(&mut v as *mut _ as isize)); }
    v
}

/// Proceso "real" de una ventana. Las apps de la Store viven dentro de ApplicationFrameHost.
pub fn real_pid(h: HWND) -> u32 {
    let pid = window_pid(h);
    let host = process_path(pid).map(|p| p.to_lowercase().ends_with("applicationframehost.exe")).unwrap_or(false);
    if !host { return pid; }
    unsafe extern "system" fn cb(c: HWND, lp: LPARAM) -> BOOL {
        let st = unsafe { &mut *(lp.0 as *mut (u32, u32)) };
        let p = window_pid(c);
        if p != st.0 { st.1 = p; return FALSE; }
        TRUE
    }
    let mut st = (pid, pid);
    unsafe { let _ = EnumChildWindows(Some(h), Some(cb), LPARAM(&mut st as *mut _ as isize)); }
    st.1
}

pub fn windows_of(pid: u32) -> Vec<HWND> {
    app_windows().into_iter().filter(|h| real_pid(*h) == pid).collect()
}

pub fn activate(h: HWND) {
    unsafe {
        if IsIconic(h).as_bool() { let _ = ShowWindow(h, SW_RESTORE); }
        if !SetForegroundWindow(h).as_bool() {
            // Si Windows lo bloquea, pulsar Alt desbloquea SetForegroundWindow desde otro proceso
            combo(&[VK_MENU]);
            let _ = SetForegroundWindow(h);
        }
    }
}

pub fn minimize(h: HWND) { unsafe { let _ = ShowWindow(h, SW_MINIMIZE); } }
pub fn close(h: HWND) { unsafe { let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0)); } }

/// Coloca la ventana exactamente en una mitad del área de trabajo (respeta la barra y la de tareas).
/// Windows 11 dibuja un borde invisible alrededor: se compensa para que no quede hueco.

// ---------------------------------------------------------------- cuenta de usuario
fn user_sid() -> Option<String> {
    unsafe {
        let mut tok = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(tok, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let ok = GetTokenInformation(tok, TokenUser, Some(buf.as_mut_ptr() as _), len, &mut len);
        let _ = CloseHandle(tok);
        ok.ok()?;
        let tu = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(tu.User.Sid, &mut s).ok()?;
        let out = s.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(s.0 as _)));
        out
    }
}

fn reg_string(root: HKEY, key: &str, value: &str) -> Option<String> {
    unsafe {
        let mut buf = vec![0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        RegGetValueW(root, &HSTRING::from(key), &HSTRING::from(value), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut size)).ok().ok()?;
        let n = (size as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&buf[..n]))
    }
}

/// Ruta de la foto de la cuenta de Windows al tamaño más cercano a `px`.
/// Primero la que muestra Configuración (UserImage.jpg, la de la cuenta Microsoft): las copias del
/// registro, que usa la pantalla de bloqueo, a veces se quedan en la silueta por defecto sin actualizar.
pub fn account_picture(px: u32) -> Option<String> {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default() + "\\Microsoft\\Windows\\AccountPicture\\UserImage.jpg";
    if std::path::Path::new(&local).exists() { return Some(local); }
    let sid = user_sid()?;
    let key = format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\AccountPicture\\Users\\{sid}");
    for size in [96u32, 192, 64, 208, 240, 448, 48, 40, 32].iter().filter(|s| **s >= px).chain([96u32, 64, 48, 32].iter()) {
        if let Some(p) = reg_string(HKEY_LOCAL_MACHINE, &key, &format!("Image{size}")) {
            if std::path::Path::new(&p).exists() { return Some(p); }
        }
    }
    None
}

/// Nombre para mostrar de la cuenta (p. ej. "Pelayo Naredo"); si no hay, el usuario.
pub fn display_name() -> String {
    use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameDisplay};
    unsafe {
        let mut buf = [0u16; 256];
        let mut n = buf.len() as u32;
        if GetUserNameExW(NameDisplay, Some(PWSTR(buf.as_mut_ptr())), &mut n) && n > 0 {
            return String::from_utf16_lossy(&buf[..n as usize]);
        }
    }
    std::env::var("USERNAME").unwrap_or_default()
}

/// ¿Está registrado el protocolo (ms-settings, ms-contact-support…) en este Windows?

// ---------------------------------------------------------------- inicio con Windows
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

pub fn autostart_enabled() -> bool {
    reg_string(HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\Run", "macbar").is_some()
}

pub fn set_autostart(on: bool) {
    unsafe {
        if on {
            let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            let v: Vec<u16> = format!("\"{exe}\"").encode_utf16().chain([0]).collect();
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("macbar"), REG_SZ.0, Some(v.as_ptr() as _), (v.len() * 2) as u32);
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("macbar"));
        }
    }
}

// ---------------------------------------------------------------- energía
pub fn lock() { unsafe { let _ = windows::Win32::System::Shutdown::LockWorkStation(); } }
pub fn sign_out() {
    use windows::Win32::System::Shutdown::{ExitWindowsEx, EWX_LOGOFF, SHUTDOWN_REASON};
    unsafe { let _ = ExitWindowsEx(EWX_LOGOFF, SHUTDOWN_REASON(0)); }
}
pub fn sleep() { unsafe { let _ = windows::Win32::System::Power::SetSuspendState(false, false, false); } }
pub fn restart() { let _ = std::process::Command::new("shutdown").args(["/r", "/t", "0"]).spawn(); }
pub fn shutdown() { let _ = std::process::Command::new("shutdown").args(["/s", "/t", "0"]).spawn(); }

// ---------------------------------------------------------------- configuración
pub fn config_dir() -> String { std::env::var("APPDATA").unwrap_or_default() + "\\macbar" }
pub fn config_path() -> String { config_dir() + "\\config.ini" }

pub fn config_value(k: &str) -> Option<String> {
    let txt = std::fs::read_to_string(config_path()).ok()?;
    txt.lines().find_map(|l| Some(l.trim().strip_prefix(k)?.trim().strip_prefix('=')?.trim().to_string()))
}

pub fn edit_config() {
    let p = config_path();
    if !std::path::Path::new(&p).exists() {
        let _ = std::fs::create_dir_all(config_dir());
        let _ = std::fs::write(&p, "# Ciudad para el tiempo (o usa lat = ... y lon = ...)\nciudad = Madrid\n");
    }
    open_args("notepad.exe", &format!("\"{p}\""));
}
