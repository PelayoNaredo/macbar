//! Receptor de iconos de bandeja (como hacen RetroBar y otras barras alternativas).
//!
//! Las apps mandan sus iconos a la primera ventana "Shell_TrayWnd" que encuentran. macbar crea la suya
//! (oculta, en la misma posición que la barra de tareas), se queda con una copia de cada icono y
//! REENVÍA TODO al Explorador, que sigue funcionando igual. Al arrancar pide a las apps que vuelvan a
//! registrar sus iconos (el mismo aviso que manda el Explorador cuando se reinicia).
//!
//! Así el panel tiene los iconos reales, sus nombres y sus clics, sin tocar el panel de Windows.
//! Si macbar se cierra, su ventana desaparece y las apps vuelven a hablar solo con el Explorador.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64};

#[repr(C)]
#[derive(Clone, Copy)]
struct Nid32 {
    cb_size: u32, hwnd: u32, uid: u32, flags: u32, cbmsg: u32, hicon: u32,
    tip: [u16; 128], state: u32, state_mask: u32, info: [u16; 256], version: u32,
    info_title: [u16; 64], info_flags: u32, guid: windows::core::GUID, balloon_icon: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TrayData { sig: u32, msg: u32, nid: Nid32 }

const NIF_MESSAGE: u32 = 1;
const NIF_ICON: u32 = 2;
const NIF_TIP: u32 = 4;
const NIF_STATE: u32 = 8;
const NIF_GUID: u32 = 0x20;
const NIS_HIDDEN: u32 = 1;
const TIMER_TOP: usize = 1;
const TIMER_REBROADCAST: usize = 2;

/// Un icono de bandeja tal como lo registró su app.
#[derive(Clone)]
pub struct TrayIcon {
    pub hwnd: isize,
    pub uid: u32,
    pub guid: Option<windows::core::GUID>,
    pub cbmsg: u32,
    pub version: u32,
    pub tip: String,
    pub px: Option<Arc<Vec<u8>>>,
    pub hidden_state: bool,
    pub exe: String,
    pub pid: u32,
    pub seq: u64,
}

pub static ICONS: Mutex<Vec<TrayIcon>> = Mutex::new(Vec::new());
static OWN: AtomicIsize = AtomicIsize::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SEQ: AtomicU64 = AtomicU64::new(0);
/// Mientras somos nosotros quienes mandamos el aviso "TaskbarCreated", no se reacciona a él.
static OWN_BROADCAST: AtomicBool = AtomicBool::new(false);

pub fn active() -> bool { ACTIVE.load(Ordering::Relaxed) }

fn own() -> HWND { HWND(OWN.load(Ordering::Relaxed) as _) }

/// La barra de tareas del Explorador (la primera "Shell_TrayWnd" que no es la nuestra).
pub fn explorer_tray() -> Option<HWND> {
    unsafe {
        let mut after: Option<HWND> = None;
        loop {
            let h = FindWindowExW(None, after, w!("Shell_TrayWnd"), None).ok()?;
            if h.0.is_null() { return None; }
            if h != own() && sys::window_pid(h) != std::process::id() { return Some(h); }
            after = Some(h);
        }
    }
}

fn taskbar_created() -> u32 { unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) } }

/// Arranca el receptor en su propio hilo: así un cuelgue de la barra nunca bloquea a las apps.
pub fn start() {
    std::thread::spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Some(ex) = explorer_tray() else { return };
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: w!("Shell_TrayWnd"), ..Default::default() };
        RegisterClassW(&wc);
        let mut r = RECT::default();
        let _ = GetWindowRect(ex, &mut r);
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, w!("Shell_TrayWnd"), PCWSTR::null(), WS_POPUP,
            r.left, r.top, r.right - r.left, r.bottom - r.top, None, None, Some(hinst.into()), None) else { return };
        OWN.store(hwnd.0 as isize, Ordering::Relaxed);
        // Hijo con el mismo nombre que el del Explorador: hay apps que lo buscan para situarse
        let _ = CreateWindowExW(WINDOW_EX_STYLE(0), w!("Static"), PCWSTR::null(), WS_CHILD, 0, 0, 1, 1, Some(hwnd), None, Some(hinst.into()), None);
        raise(hwnd);
        ACTIVE.store(true, Ordering::Relaxed);
        SetTimer(Some(hwnd), TIMER_TOP, 400, None);
        broadcast();
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

/// Pide a todas las apps que vuelvan a registrar sus iconos.
fn broadcast() {
    OWN_BROADCAST.store(true, Ordering::Relaxed);
    unsafe { let _ = SendNotifyMessageW(HWND_BROADCAST, taskbar_created(), WPARAM(0), LPARAM(0)); }
    // El aviso nos llega a nosotros también (asíncrono): se ignora durante un momento
    std::thread::spawn(|| { std::thread::sleep(Duration::from_millis(1500)); OWN_BROADCAST.store(false, Ordering::Relaxed); });
}

/// Llamadas en curso que deben llegar al Explorador sin pasar por nuestra ventana.
static DIRECT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Ejecuta `f` con el Explorador como primera "Shell_TrayWnd". `SHAppBarMessage` habla con la primera
/// que encuentra y, si es la nuestra, el reenvío de las peticiones del hueco de la barra (ABM_SETPOS…)
/// se acepta pero el Explorador no lo aplica: tras suspender, la barra perdía su hueco para siempre.
pub fn direct<R>(f: impl FnOnce() -> R) -> R {
    let own = own();
    if own.0.is_null() { return f(); }
    DIRECT.fetch_add(1, Ordering::SeqCst);
    unsafe {
        let fl = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        let _ = SetWindowPos(own, Some(HWND_NOTOPMOST), 0, 0, 0, 0, fl);
        let _ = SetWindowPos(own, Some(HWND_BOTTOM), 0, 0, 0, 0, fl);
    }
    let r = f();
    if DIRECT.fetch_sub(1, Ordering::SeqCst) == 1 { raise(own); }
    r
}

/// Nos pone por delante de la barra del Explorador (las apps eligen la primera que encuentran)
/// y copia su posición, para quien pregunte dónde está la barra de tareas.
fn raise(hwnd: HWND) {
    if DIRECT.load(Ordering::SeqCst) > 0 { return; }
    unsafe {
        let Some(ex) = explorer_tray() else { return };
        let first = FindWindowW(w!("Shell_TrayWnd"), None).unwrap_or_default();
        let (mut a, mut b) = (RECT::default(), RECT::default());
        let _ = GetWindowRect(ex, &mut a);
        let _ = GetWindowRect(hwnd, &mut b);
        if first != hwnd || a != b {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), a.left, a.top, a.right - a.left, a.bottom - a.top, SWP_NOACTIVATE);
        }
    }
}

fn wstr(s: &[u16]) -> String {
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..n]).trim().to_string()
}

fn icon_px(h: u32) -> Option<Vec<u8>> {
    if h == 0 { return None; }
    unsafe {
        let hi = HICON(h as isize as _);
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let src: IWICBitmapSource = wic.CreateBitmapFromHICON(hi).ok()?.into();
        wic_square(&wic, &src)
    }
}

/// Guarda o actualiza el icono que acaba de mandar una app.
fn record(d: &TrayData) {
    let n = &d.nid;
    let by_guid = n.flags & NIF_GUID != 0;
    let same = |i: &TrayIcon| if by_guid { i.guid == Some(n.guid) } else { i.guid.is_none() && i.hwnd == n.hwnd as isize && i.uid == n.uid };
    let mut list = ICONS.lock().unwrap();
    match d.msg {
        0 | 1 => {
            // NIM_ADD / NIM_MODIFY: el icono se convierte YA, porque la app puede destruirlo al volver
            let px = (n.flags & NIF_ICON != 0).then(|| icon_px(n.hicon)).flatten().map(Arc::new);
            let idx = list.iter().position(same);
            // Hay apps que, al aviso de "barra nueva", MODIFICAN su icono en vez de añadirlo
            // (el Explorador ya lo tenía): eso también cuenta como alta.
            let it = match idx {
                Some(i) => &mut list[i],
                None => {
                    let hwnd = HWND(n.hwnd as isize as _);
                    let pid = sys::window_pid(hwnd);
                    list.push(TrayIcon {
                        hwnd: n.hwnd as isize, uid: n.uid, guid: by_guid.then_some(n.guid), cbmsg: 0, version: 0,
                        tip: String::new(), px: None, hidden_state: false,
                        exe: sys::process_path(pid).unwrap_or_default(), pid, seq: SEQ.fetch_add(1, Ordering::Relaxed),
                    });
                    list.last_mut().unwrap()
                }
            };
            if n.hwnd != 0 { it.hwnd = n.hwnd as isize; }
            if n.flags & NIF_MESSAGE != 0 { it.cbmsg = n.cbmsg; }
            if n.flags & NIF_TIP != 0 { it.tip = wstr(&n.tip); }
            if n.flags & NIF_STATE != 0 && n.state_mask & NIS_HIDDEN != 0 { it.hidden_state = n.state & NIS_HIDDEN != 0; }
            if px.is_some() { it.px = px; }
        }
        2 => list.retain(|i| !same(i)),
        4 => { if let Some(i) = list.iter_mut().find(|i| same(i)) { i.version = n.version; } }
        _ => {}
    }
    // Ventanas que ya no existen (apps cerradas sin quitar su icono)
    list.retain(|i| unsafe { IsWindow(Some(HWND(i.hwnd as _))).as_bool() });
}

fn forward(msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(ex) = explorer_tray() else { return LRESULT(0) };
    let mut res = 0usize;
    unsafe {
        if InSendMessage().as_bool() {
            let _ = SendMessageTimeoutW(ex, msg, wp, lp, SMTO_ABORTIFHUNG, 3000, Some(&mut res));
        } else {
            let _ = PostMessageW(Some(ex), msg, wp, lp);
        }
    }
    LRESULT(res as isize)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COPYDATA => {
                let cds = &*(lp.0 as *const windows::Win32::System::DataExchange::COPYDATASTRUCT);
                if cds.dwData == 1 && cds.cbData as usize >= 8 + 24 && !cds.lpData.is_null() {
                    // Copia en una estructura completa: las apps viejas mandan versiones más cortas
                    let mut d: TrayData = std::mem::zeroed();
                    let n = (cds.cbData as usize).min(size_of::<TrayData>());
                    std::ptr::copy_nonoverlapping(cds.lpData as *const u8, &mut d as *mut _ as *mut u8, n);
                    if d.sig == 0x34753423 { record(&d); }
                }
                forward(msg, wp, lp)
            }
            WM_TIMER => {
                if wp.0 == TIMER_TOP { raise(hwnd); }
                if wp.0 == TIMER_REBROADCAST { let _ = KillTimer(Some(hwnd), TIMER_REBROADCAST); raise(hwnd); broadcast(); }
                LRESULT(0)
            }
            // Mensajes que las apps (o el propio Explorador) mandan a "la barra de tareas": se pasan tal cual
            WM_COMMAND => forward(msg, wp, lp),
            m if (WM_USER..0x8000).contains(&m) => forward(msg, wp, lp),
            m if m == taskbar_created() => {
                // El Explorador se ha reiniciado: volvemos a ponernos delante y a pedir los iconos
                if !OWN_BROADCAST.load(Ordering::Relaxed) {
                    ICONS.lock().unwrap().clear();
                    SetTimer(Some(hwnd), TIMER_REBROADCAST, 2000, None);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

// ---------------------------------------------------------------- clics
const NIN_SELECT: u32 = WM_USER;

/// Manda a la app el clic en su icono, igual que lo haría la barra de tareas. (x, y): punto de pantalla
/// donde la app debe abrir su menú.
pub fn click(it: &TrayIcon, right: bool, x: i32, y: i32) {
    if it.cbmsg == 0 { return; }
    unsafe {
        // La app necesita permiso para ponerse delante (abrir su ventana o su menú)
        let _ = AllowSetForegroundWindow(it.pid);
        let h = HWND(it.hwnd as _);
        let msgs: &[u32] = if right { &[WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU] } else { &[WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT] };
        for &m in msgs {
            // WM_CONTEXTMENU y NIN_SELECT solo existen desde la versión 3 del protocolo
            if (m == WM_CONTEXTMENU || m == NIN_SELECT) && it.version < 3 { continue; }
            let (w, l) = if it.version >= 4 {
                (((y as u16 as usize) << 16) | x as u16 as usize, ((it.uid as usize & 0xFFFF) << 16 | m as usize) as isize)
            } else {
                (it.uid as usize, m as isize)
            };
            let _ = PostMessageW(Some(h), it.cbmsg, WPARAM(w), LPARAM(l));
        }
    }
}

// ---------------------------------------------------------------- ¿visible u oculto en la bandeja?
/// Ajustes de cada icono en el registro (Windows 11): ruta, identificador y si está anclado a la vista.
struct Setting { exe: String, uid: Option<u32>, guid: Option<String>, promoted: bool }

fn expand_known(p: &str) -> String {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let sys_root = env("SystemRoot");
    let map = [
        ("{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}", env("ProgramFiles(x86)")),
        ("{6D809377-6AF0-444B-8957-A3773F02200E}", env("ProgramFiles")),
        ("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}", format!("{sys_root}\\System32")),
        ("{F38BF404-1D43-42F2-9305-67DE0B28FC23}", sys_root.clone()),
    ];
    for (g, base) in map {
        if let Some(rest) = p.strip_prefix(g) { return format!("{base}{rest}"); }
    }
    p.to_string()
}

fn settings() -> Vec<Setting> {
    use windows::Win32::System::Registry::*;
    let mut out = Vec::new();
    unsafe {
        let mut root = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, w!("Control Panel\\NotifyIconSettings"), Some(0), KEY_READ, &mut root).is_err() { return out; }
        let mut i = 0u32;
        loop {
            let mut name = [0u16; 64];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(root, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None).is_err() { break; }
            i += 1;
            let sub = HSTRING::from(String::from_utf16_lossy(&name[..len as usize]));
            let sz = |v: PCWSTR| -> Option<String> {
                let mut buf = vec![0u16; 1024];
                let mut size = (buf.len() * 2) as u32;
                RegGetValueW(root, &sub, v, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut size)).ok().ok()?;
                Some(String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]))
            };
            let dw = |v: PCWSTR| -> Option<u32> {
                let (mut x, mut size) = (0u32, 4u32);
                RegGetValueW(root, &sub, v, RRF_RT_REG_DWORD, None, Some(&mut x as *mut _ as _), Some(&mut size)).ok().ok()?;
                Some(x)
            };
            let Some(exe) = sz(w!("ExecutablePath")) else { continue };
            out.push(Setting { exe: expand_known(&exe).to_lowercase(), uid: dw(w!("UID")), guid: sz(w!("IconGuid")).map(|g| g.to_uppercase()), promoted: dw(w!("IsPromoted")) == Some(1) });
        }
        let _ = RegCloseKey(root);
    }
    out
}

fn guid_str(g: &windows::core::GUID) -> String { format!("{{{g:?}}}").to_uppercase() }

/// Iconos que Windows guarda en "iconos ocultos" (no anclados a la barra), en orden de llegada.
pub fn hidden_icons() -> Vec<TrayIcon> {
    let st = settings();
    if let Some(p) = std::env::var_os("MACBAR_TRAYDUMP") {
        let all = ICONS.lock().unwrap();
        let txt: String = all.iter().map(|i| format!("{} | {} | uid={} guid={:?} cb={} v={} hid={} px={}\n", i.exe, i.tip, i.uid, i.guid.map(|g| guid_str(&g)), i.cbmsg, i.version, i.hidden_state, i.px.is_some())).collect();
        let _ = std::fs::write(p, txt);
    }
    let mut v: Vec<TrayIcon> = ICONS.lock().unwrap().iter().filter(|i| !i.hidden_state && (i.cbmsg != 0 || i.px.is_some())).cloned().collect();
    v.retain(|i| {
        let exe = i.exe.to_lowercase();
        let s = st.iter().find(|s| s.exe == exe && match (&i.guid, &s.guid) {
            (Some(g), Some(sg)) => guid_str(g) == *sg,
            (None, None) => s.uid == Some(i.uid),
            _ => false,
        });
        // Iconos del propio Windows (volumen, red…) sin ajuste guardado: viven fuera de "ocultos"
        if s.is_none() && exe.ends_with("\\explorer.exe") { return false; }
        // Sin ajuste guardado, Windows 11 lo deja oculto
        !s.is_some_and(|s| s.promoted)
    });
    v.sort_by_key(|i| i.seq);
    v
}
