//! Nota flotante (como Raycast Notes): una ventanita oscura siempre encima con un EDIT nativo.
//!
//! No sale en capturas, grabaciones ni al compartir pantalla (`WDA_EXCLUDEFROMCAPTURE`). El texto se
//! guarda en `%APPDATA%\macbar\nota.txt` (UTF-8) poco después de dejar de escribir y al cerrar, y la
//! posición en `%APPDATA%\macbar\nota.ini`. Usa el bucle de mensajes de la app: nada bloquea.
//!
//! El EDIT mide siempre lo que mediría con la nota en su alto máximo; la ventana, más baja, lo
//! recorta. Así el EDIT no se desplaza solo mientras la ventana crece detrás del texto, y al llegar
//! al máximo se comporta como un EDIT normal (con su propio desplazamiento).
//! Todo es autocontenido: no usa nada del resto de la barra.

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Instant;
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

// ---------------------------------------------------------------- medidas (DIP) y tiempos

const WIDTH: f32 = 300.0;
const HEADER: f32 = 30.0;
/// Margen interior a los lados y abajo.
const PAD: f32 = 12.0;
/// Hueco entre la cabecera y el texto (la cabecera ya deja aire).
const PAD_TOP: f32 = 2.0;
const MIN_H: f32 = 120.0;
const MAX_H: f32 = 420.0;
/// Ancho de la zona de la × (a la derecha de la cabecera).
const CLOSE_W: f32 = 34.0;
/// Primera vez: alto bajo la barra y separación del borde derecho.
const FIRST_Y: f32 = 52.0;
const FIRST_RIGHT: f32 = 12.0;

const FADE_IN_MS: f32 = 150.0;
const FADE_OUT_MS: f32 = 100.0;
const GROW_MS: f32 = 140.0;
const SAVE_MS: u32 = 600;

const TIMER_ANIM: usize = 1;
const TIMER_SAVE: usize = 2;
const ID_EDIT: usize = 100;

// Mensajes del EDIT (se fijan aquí para no depender de dónde los ponga windows-rs)
const EM_GETSEL: u32 = 0x00B0;
const EM_SETSEL: u32 = 0x00B1;
const EM_SCROLLCARET: u32 = 0x00B7;
const EM_GETLINECOUNT: u32 = 0x00BA;
const EM_REPLACESEL: u32 = 0x00C2;
const EM_SETLIMITTEXT: u32 = 0x00C5;
const EM_SETMARGINS: u32 = 0x00D3;
const EC_LEFTMARGIN: usize = 1;
const EC_RIGHTMARGIN: usize = 2;
const EN_CHANGE: u32 = 0x0300;
const WM_MOUSELEAVE: u32 = 0x02A3;
const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_WANTRETURN: u32 = 0x1000;

/// Color en el orden de GDI (0x00BBGGRR).
const fn rgb(r: u8, g: u8, b: u8) -> COLORREF { COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16) }
const BG: COLORREF = rgb(0x1C, 0x1C, 0x1E);
const INK: COLORREF = rgb(0xF2, 0xF2, 0xF7);
const TITLE: COLORREF = rgb(0x8E, 0x8E, 0x93);
const DIM: COLORREF = rgb(0x63, 0x63, 0x66);
const BORDER: COLORREF = rgb(0x3A, 0x3A, 0x3C);

// ---------------------------------------------------------------- API

/// Abre la nota (o la cierra si ya está abierta). Se llama desde el hilo de la interfaz (STA, con
/// bucle de mensajes propio de la app).
pub fn toggle() {
    if is_open() { close(); return; }
    let h = st(|s| s.hwnd.get());
    if alive(h) {
        // Se estaba fundiendo al cerrar: vuelve desde la opacidad de ahora
        st(|s| s.fade.set(Some((Instant::now(), s.alpha.get(), false))));
        unsafe {
            SetTimer(Some(h), TIMER_ANIM, 15, None);
            let _ = SetForegroundWindow(h);
            let _ = SetFocus(Some(st(|s| s.edit.get())));
        }
        return;
    }
    open();
}

/// ¿Está abierta? (una nota que se está fundiendo al cerrar ya cuenta como cerrada).
pub fn is_open() -> bool {
    let (h, fade) = st(|s| (s.hwnd.get(), s.fade.get()));
    alive(h) && !matches!(fade, Some((_, _, true)))
}

// ---------------------------------------------------------------- funciones puras

/// DIP → píxeles físicos.
fn dip(v: f32, dpi: u32) -> i32 { (v * dpi as f32 / 96.0).round() as i32 }

/// Parte fija de arriba (cabecera + hueco), en píxeles: ahí empieza el EDIT.
fn text_top(dpi: u32) -> i32 { dip(HEADER + PAD_TOP, dpi) }

/// Alto (fijo) del EDIT: el que tiene con la nota en su alto máximo.
fn edit_height(dpi: u32) -> i32 { dip(MAX_H, dpi) - text_top(dpi) - dip(PAD, dpi) }

/// Alto de la ventana (píxeles físicos) para `lines` líneas de `line_px` píxeles: cabecera, márgenes y
/// texto, entre 120 y 420 DIP.
fn note_height(lines: i32, line_px: i32, dpi: u32) -> i32 {
    let want = text_top(dpi) + lines.max(1) * line_px.max(1) + dip(PAD, dpi);
    want.clamp(dip(MIN_H, dpi), dip(MAX_H, dpi))
}

/// Dónde va la nota la primera vez: arriba a la derecha del monitor principal, bajo la barra.
/// `work` = área de trabajo y `mon` = monitor entero, (izq, arriba, der, abajo).
fn first_position(w: i32, dpi: u32, work: (i32, i32, i32, i32), mon: (i32, i32, i32, i32)) -> (i32, i32) {
    (work.2 - w - dip(FIRST_RIGHT, dpi), mon.1 + dip(FIRST_Y, dpi))
}

/// Encaja la posición guardada: vale si la cabecera (`w` × `hdr`) queda entera de alto y bastante de
/// ancho dentro de algún monitor (así siempre se puede agarrar); si no, la nota (`w` × `h`) va al
/// centro del principal.
fn fit_position(pos: (i32, i32), w: i32, hdr: i32, h: i32, monitors: &[(i32, i32, i32, i32)], primary: (i32, i32, i32, i32)) -> (i32, i32) {
    let (x, y) = pos;
    let need = w.min(80);
    let ok = monitors.iter().any(|&(l, t, r, b)| {
        let ancho = (x + w).min(r) - x.max(l);
        ancho >= need && y >= t && y + hdr <= b
    });
    if ok { return pos; }
    let (l, t, r, b) = primary;
    (l + (r - l - w) / 2, t + (b - t - h) / 2)
}

/// Lee `x = …` e `y = …` del ini de la posición.
fn parse_pos(txt: &str) -> Option<(i32, i32)> {
    let (mut x, mut y) = (None, None);
    for line in txt.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim().parse::<i32>().ok();
        match k.trim() { "x" => x = v, "y" => y = v, _ => {} }
    }
    Some((x?, y?))
}

/// Texto del fichero → EDIT (el EDIT quiere CRLF).
fn to_edit(s: &str) -> String {
    s.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n").replace('\n', "\r\n")
}

/// Texto del EDIT → fichero (saltos de línea de toda la vida).
fn from_edit(s: &str) -> String { s.replace("\r\n", "\n") }

/// Comienzo de la palabra que hay antes de `caret` (para Ctrl+Retroceso): salta los espacios y luego
/// la palabra; un salto de línea cuenta como palabra propia.
fn word_start(text: &[u16], caret: usize) -> usize {
    let caret = caret.min(text.len());
    let is_space = |c: u16| c == b' ' as u16 || c == b'\t' as u16;
    let is_nl = |c: u16| c == b'\r' as u16 || c == b'\n' as u16;
    let mut i = caret;
    while i > 0 && is_space(text[i - 1]) { i -= 1; }
    if i > 0 && is_nl(text[i - 1]) {
        // Borra el salto entero (CRLF son dos caracteres)
        i -= 1;
        if i > 0 && text[i] == b'\n' as u16 && text[i - 1] == b'\r' as u16 { i -= 1; }
        return i;
    }
    while i > 0 && !is_space(text[i - 1]) && !is_nl(text[i - 1]) { i -= 1; }
    i
}

fn ease_out(t: f32) -> f32 { let u = 1.0 - t.clamp(0.0, 1.0); 1.0 - u * u * u }

// ---------------------------------------------------------------- estado (solo hilo de la interfaz)

/// Todo en `Cell`: los procedimientos de ventana se reentran (SetWindowPos manda WM_SIZE, etc.) y así
/// nunca hay un préstamo pendiente que pueda fallar.
#[derive(Default)]
struct St {
    hwnd: Cell<HWND>,
    edit: Cell<HWND>,
    dpi: Cell<u32>,
    f_text: Cell<HFONT>,
    f_title: Cell<HFONT>,
    f_small: Cell<HFONT>,
    f_icon: Cell<HFONT>,
    line_px: Cell<i32>,
    bg: Cell<HBRUSH>,
    alpha: Cell<u8>,
    /// Fundido en curso: (inicio, opacidad de partida, ¿de salida?).
    fade: Cell<Option<(Instant, u8, bool)>>,
    /// Cambio de alto en curso: (inicio, alto de partida, alto de destino).
    grow: Cell<Option<(Instant, i32, i32)>>,
    /// Ratón encima de la × y botón pulsado sobre ella.
    hot: Cell<bool>,
    down: Cell<bool>,
    /// Hay texto sin guardar.
    dirty: Cell<bool>,
    /// Poniendo el texto inicial: los EN_CHANGE no cuentan.
    loading: Cell<bool>,
    /// Se pudo excluir de la captura (se enseña el aviso en la cabecera).
    hidden: Cell<bool>,
}

thread_local! { static ST: St = St::default(); }

fn st<R>(f: impl FnOnce(&St) -> R) -> R { ST.with(f) }

fn alive(h: HWND) -> bool { !h.is_invalid() && unsafe { IsWindow(Some(h)).as_bool() } }

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

// ---------------------------------------------------------------- ficheros

fn data_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(base).join("macbar"))
}

/// Escribe a un temporal y lo cambia por el bueno: un corte a medias no deja la nota vacía.
fn write_atomic(name: &str, bytes: &[u8]) -> bool {
    let Some(dir) = data_dir() else { return false };
    if std::fs::create_dir_all(&dir).is_err() { return false; }
    let (dst, tmp) = (dir.join(name), dir.join(format!("{name}.tmp")));
    std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, &dst).is_ok()
}

fn load_text() -> String {
    data_dir().and_then(|d| std::fs::read(d.join("nota.txt")).ok()).map(|b| to_edit(&String::from_utf8_lossy(&b))).unwrap_or_default()
}

fn load_pos() -> Option<(i32, i32)> {
    let txt = std::fs::read_to_string(data_dir()?.join("nota.ini")).ok()?;
    parse_pos(&txt)
}

fn edit_text(edit: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(edit).max(0) as usize;
        let mut buf = vec![0u16; n + 1];
        let got = GetWindowTextW(edit, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..got.min(n)])
    }
}

fn save_text() {
    let (edit, dirty) = st(|s| (s.edit.get(), s.dirty.get()));
    if !dirty || !alive(edit) { return; }
    let txt = from_edit(&edit_text(edit));
    if write_atomic("nota.txt", txt.as_bytes()) { st(|s| s.dirty.set(false)); }
}

fn save_pos(hwnd: HWND) {
    let mut rc = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rc) }.is_err() { return; }
    let _ = write_atomic("nota.ini", format!("x = {}\ny = {}\n", rc.left, rc.top).as_bytes());
}

// ---------------------------------------------------------------- monitores y fuentes

fn rect_tuple(r: RECT) -> (i32, i32, i32, i32) { (r.left, r.top, r.right, r.bottom) }

unsafe extern "system" fn enum_monitor(hm: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
    unsafe {
        let v = &mut *(lp.0 as *mut Vec<(i32, i32, i32, i32)>);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(hm, &mut mi).as_bool() { v.push(rect_tuple(mi.rcWork)); }
    }
    BOOL(1)
}

/// Áreas de trabajo de todos los monitores.
fn monitors() -> Vec<(i32, i32, i32, i32)> {
    let mut v: Vec<(i32, i32, i32, i32)> = Vec::new();
    unsafe { let _ = EnumDisplayMonitors(None, None, Some(enum_monitor), LPARAM(&mut v as *mut _ as isize)); }
    v
}

/// (área de trabajo, monitor entero, ppp) del monitor que contiene el punto (o el más cercano).
fn monitor_at(pt: POINT, flags: MONITOR_FROM_FLAGS) -> ((i32, i32, i32, i32), (i32, i32, i32, i32), u32) {
    unsafe {
        let hm = MonitorFromPoint(pt, flags);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let ok = GetMonitorInfoW(hm, &mut mi).as_bool();
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_err() || dx == 0 { dx = 96; }
        if !ok {
            let r = (0, 0, GetSystemMetrics(SM_CXSCREEN).max(1), GetSystemMetrics(SM_CYSCREEN).max(1));
            return (r, r, dx);
        }
        (rect_tuple(mi.rcWork), rect_tuple(mi.rcMonitor), dx)
    }
}

/// Fuente GDI de `tenths` décimas de punto a estos ppp.
fn make_font(face: &str, tenths: i32, weight: i32, dpi: u32) -> HFONT {
    let mut lf = LOGFONTW {
        lfHeight: -((tenths as i64 * dpi as i64 + 360) / 720) as i32,
        lfWeight: weight,
        lfCharSet: DEFAULT_CHARSET,
        lfQuality: CLEARTYPE_QUALITY,
        ..Default::default()
    };
    for (d, c) in lf.lfFaceName.iter_mut().zip(face.encode_utf16().take(31)) { *d = c; }
    unsafe { CreateFontIndirectW(&lf) }
}

/// Segoe Fluent Icons (Windows 11) si está; si no, Segoe MDL2 Assets (tiene los mismos glifos).
fn icon_face() -> &'static str {
    const FLUENT: &str = "Segoe Fluent Icons";
    unsafe {
        let f = make_font(FLUENT, 100, 400, 96);
        if f.is_invalid() { return "Segoe MDL2 Assets"; }
        let dc = GetDC(None);
        let mut found = false;
        if !dc.is_invalid() {
            let old = SelectObject(dc, f.into());
            let mut name = [0u16; 64];
            let n = GetTextFaceW(dc, Some(&mut name)).max(0) as usize;
            let n = name[..n.min(64)].iter().position(|&c| c == 0).unwrap_or(n.min(64));
            found = String::from_utf16_lossy(&name[..n]).eq_ignore_ascii_case(FLUENT);
            SelectObject(dc, old);
            ReleaseDC(None, dc);
        }
        let _ = DeleteObject(f.into());
        if found { FLUENT } else { "Segoe MDL2 Assets" }
    }
}

fn delete_fonts(fonts: [HFONT; 4]) {
    for f in fonts {
        if !f.is_invalid() { unsafe { let _ = DeleteObject(f.into()); } }
    }
}

/// Suelta las fuentes (al destruir la ventana).
fn free_fonts() {
    let d = HFONT::default();
    delete_fonts(st(|s| [s.f_text.replace(d), s.f_title.replace(d), s.f_small.replace(d), s.f_icon.replace(d)]));
}

/// (Re)hace las fuentes para estos ppp, se las da al EDIT y mide el alto de línea. Las viejas se
/// borran después: el EDIT nunca se queda con una fuente ya borrada.
fn set_fonts(dpi: u32) {
    let icons = icon_face();
    let f_text = make_font("Segoe UI", 105, 400, dpi);
    let old = st(|s| {
        s.dpi.set(dpi);
        [s.f_text.replace(f_text), s.f_title.replace(make_font("Segoe UI", 90, 600, dpi)),
         s.f_small.replace(make_font("Segoe UI", 80, 400, dpi)), s.f_icon.replace(make_font(icons, 80, 400, dpi))]
    });
    let edit = st(|s| s.edit.get());
    let mut line = dip(19.0, dpi);
    if alive(edit) {
        unsafe {
            SendMessageW(edit, WM_SETFONT, Some(WPARAM(f_text.0 as usize)), Some(LPARAM(1)));
            // Sin márgenes propios: los 12 DIP los pone la colocación del EDIT
            SendMessageW(edit, EM_SETMARGINS, Some(WPARAM(EC_LEFTMARGIN | EC_RIGHTMARGIN)), Some(LPARAM(0)));
            let dc = GetDC(Some(edit));
            if !dc.is_invalid() {
                let old = SelectObject(dc, f_text.into());
                let mut tm = TEXTMETRICW::default();
                if GetTextMetricsW(dc, &mut tm).as_bool() && tm.tmHeight > 0 { line = tm.tmHeight; }
                SelectObject(dc, old);
                ReleaseDC(Some(edit), dc);
            }
        }
    }
    st(|s| s.line_px.set(line));
    delete_fonts(old);
}

// ---------------------------------------------------------------- abrir y cerrar

fn open() {
    unsafe {
        let Ok(hm) = GetModuleHandleW(None) else { return };
        let hinst: HINSTANCE = hm.into();
        let class = w!("MacBarNote");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(note_proc),
            hInstance: hinst,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        // Si ya estaba registrada, falla sin más
        RegisterClassW(&wc);

        // Posición: la guardada (si sigue viéndose) o arriba a la derecha del principal
        let (pwork, pmon, pdpi) = monitor_at(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let saved = load_pos();
        let mut dpi = match saved { Some((x, y)) => monitor_at(POINT { x, y }, MONITOR_DEFAULTTONEAREST).2, None => pdpi };
        let (mut x, mut y) = match saved {
            Some(p) => fit_position(p, dip(WIDTH, dpi), dip(HEADER, dpi), dip(MIN_H, dpi), &monitors(), pwork),
            None => first_position(dip(WIDTH, dpi), dpi, pwork, pmon),
        };
        // Los ppp que mandan son los del monitor donde acaba la cabecera
        let d2 = monitor_at(POINT { x: x + dip(WIDTH, dpi) / 2, y: y + dip(HEADER, dpi) / 2 }, MONITOR_DEFAULTTONEAREST).2;
        if d2 != dpi {
            dpi = d2;
            if saved.is_none() { (x, y) = first_position(dip(WIDTH, dpi), dpi, pwork, pmon); }
        }

        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED, class, w!("Nota"), WS_POPUP | WS_CLIPCHILDREN,
            x, y, dip(WIDTH, dpi), dip(MIN_H, dpi), None, None, Some(hinst), None) else { return };
        // Invisible hasta el fundido
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
        let round: i32 = 2; // DWMWCP_ROUND
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &BORDER as *const _ as _, 4);
        // MACBAR_NOTE_SHOW=1 la deja visible en capturas (solo para comprobarla a la vista)
        let hidden = std::env::var_os("MACBAR_NOTE_SHOW").is_none() && SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_ok();

        let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN);
        let edit = CreateWindowExW(WINDOW_EX_STYLE(0), w!("EDIT"), PCWSTR::null(), style, 0, 0, 10, 10,
            Some(hwnd), Some(HMENU(ID_EDIT as _)), Some(hinst), None);
        let Ok(edit) = edit else { let _ = DestroyWindow(hwnd); return };

        let old_bg = st(|s| {
            s.hwnd.set(hwnd);
            s.edit.set(edit);
            s.hidden.set(hidden);
            s.alpha.set(0);
            s.hot.set(false);
            s.down.set(false);
            s.dirty.set(false);
            s.grow.set(None);
            s.bg.replace(CreateSolidBrush(BG))
        });
        if !old_bg.is_invalid() { let _ = DeleteObject(old_bg.into()); }
        let _ = SetWindowSubclass(edit, Some(edit_proc), 1, 0);
        SendMessageW(edit, EM_SETLIMITTEXT, Some(WPARAM(0)), Some(LPARAM(0)));
        let real = GetDpiForWindow(hwnd);
        set_fonts(if real == 0 { dpi } else { real });
        place_edit();

        let txt = wide(&load_text());
        st(|s| s.loading.set(true));
        let _ = SetWindowTextW(edit, PCWSTR(txt.as_ptr()));
        st(|s| s.loading.set(false));
        // Alto según el texto, de golpe (aún no se ve)
        let h = target_height();
        resize_height(hwnd, h);
        // Cursor al final
        let n = GetWindowTextLengthW(edit).max(0) as usize;
        SendMessageW(edit, EM_SETSEL, Some(WPARAM(n)), Some(LPARAM(n as isize)));
        SendMessageW(edit, EM_SCROLLCARET, None, None);

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(edit));
        st(|s| s.fade.set(Some((Instant::now(), 0, false))));
        SetTimer(Some(hwnd), TIMER_ANIM, 15, None);
    }
}

fn close() {
    let h = st(|s| s.hwnd.get());
    if !alive(h) { return; }
    save_text();
    save_pos(h);
    unsafe {
        let _ = KillTimer(Some(h), TIMER_SAVE);
        st(|s| s.fade.set(Some((Instant::now(), s.alpha.get(), true))));
        SetTimer(Some(h), TIMER_ANIM, 15, None);
    }
}

// ---------------------------------------------------------------- colocación y animación

fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rc = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut rc) };
    (rc.right - rc.left, rc.bottom - rc.top)
}

/// EDIT a 12 DIP de los lados, bajo la cabecera, con su alto fijo (el de la nota en el máximo).
fn place_edit() {
    let (hwnd, edit, dpi) = st(|s| (s.hwnd.get(), s.edit.get(), s.dpi.get()));
    if !alive(edit) { return; }
    let (w, _) = client_size(hwnd);
    let pad = dip(PAD, dpi);
    unsafe { let _ = MoveWindow(edit, pad, text_top(dpi), (w - 2 * pad).max(1), edit_height(dpi), true); }
}

/// Alto que pide el texto de ahora.
fn target_height() -> i32 {
    let (edit, dpi, line) = st(|s| (s.edit.get(), s.dpi.get(), s.line_px.get()));
    let lines = if alive(edit) { unsafe { SendMessageW(edit, EM_GETLINECOUNT, None, None).0 as i32 } } else { 1 };
    note_height(lines, line, dpi)
}

fn resize_height(hwnd: HWND, h: i32) {
    let mut rc = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rc) }.is_err() { return; }
    if rc.bottom - rc.top == h { return; }
    unsafe { let _ = SetWindowPos(hwnd, None, 0, 0, rc.right - rc.left, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE); }
}

/// Tras un cambio de texto: si el alto que toca es otro, se anima hacia él.
fn retarget() {
    let hwnd = st(|s| s.hwnd.get());
    if !alive(hwnd) { return; }
    let to = target_height();
    let mut rc = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut rc) }.is_err() { return; }
    let cur = rc.bottom - rc.top;
    let goal = st(|s| s.grow.get()).map(|g| g.2).unwrap_or(cur);
    if goal == to { return; }
    if cur == to { st(|s| s.grow.set(None)); return; }
    st(|s| s.grow.set(Some((Instant::now(), cur, to))));
    unsafe { SetTimer(Some(hwnd), TIMER_ANIM, 15, None); }
}

/// Un paso de las animaciones (fundido y alto).
fn tick(hwnd: HWND) {
    if let Some((t0, from, out)) = st(|s| s.fade.get()) {
        let dur = if out { FADE_OUT_MS } else { FADE_IN_MS };
        let k = ease_out(t0.elapsed().as_secs_f32() * 1000.0 / dur);
        let to = if out { 0.0 } else { 255.0 };
        let a = (from as f32 + (to - from as f32) * k).round().clamp(0.0, 255.0) as u8;
        st(|s| s.alpha.set(a));
        unsafe { let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), a, LWA_ALPHA); }
        if k >= 1.0 {
            st(|s| s.fade.set(None));
            if out { unsafe { let _ = DestroyWindow(hwnd); } return; }
        }
    }
    if let Some((t0, from, to)) = st(|s| s.grow.get()) {
        let k = ease_out(t0.elapsed().as_secs_f32() * 1000.0 / GROW_MS);
        resize_height(hwnd, from + ((to - from) as f32 * k).round() as i32);
        if k >= 1.0 { st(|s| s.grow.set(None)); }
    }
    if st(|s| s.fade.get().is_none() && s.grow.get().is_none()) {
        unsafe { let _ = KillTimer(Some(hwnd), TIMER_ANIM); }
    }
}

// ---------------------------------------------------------------- dibujo de la cabecera

fn close_rect(w: i32, dpi: u32) -> RECT {
    RECT { left: w - dip(CLOSE_W, dpi), top: 0, right: w, bottom: dip(HEADER, dpi) }
}

fn in_close(hwnd: HWND, x: i32, y: i32) -> bool {
    let dpi = st(|s| s.dpi.get());
    let (w, _) = client_size(hwnd);
    let r = close_rect(w, dpi);
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn draw_text(dc: HDC, font: HFONT, color: COLORREF, s: &str, rc: &mut RECT, fmt: DRAW_TEXT_FORMAT) {
    let mut t: Vec<u16> = s.encode_utf16().collect();
    if t.is_empty() { return; }
    unsafe {
        let old = SelectObject(dc, font.into());
        SetTextColor(dc, color);
        DrawTextW(dc, &mut t, rc, fmt);
        SelectObject(dc, old);
    }
}

/// Pinta el fondo y la cabecera (título, aviso de captura y ×) con doble búfer.
fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        if dc.is_invalid() { return; }
        let (w, h) = client_size(hwnd);
        let (dpi, bg, hot, hidden) = st(|s| (s.dpi.get(), s.bg.get(), s.hot.get(), s.hidden.get()));
        let (f_title, f_small, f_icon) = st(|s| (s.f_title.get(), s.f_small.get(), s.f_icon.get()));
        let mem = CreateCompatibleDC(Some(dc));
        let bmp = CreateCompatibleBitmap(dc, w.max(1), h.max(1));
        if mem.is_invalid() || bmp.is_invalid() {
            if !bmp.is_invalid() { let _ = DeleteObject(bmp.into()); }
            if !mem.is_invalid() { let _ = DeleteDC(mem); }
            let _ = EndPaint(hwnd, &ps);
            return;
        }
        let old_bmp = SelectObject(mem, bmp.into());
        let full = RECT { left: 0, top: 0, right: w, bottom: h };
        FillRect(mem, &full, bg);
        SetBkMode(mem, TRANSPARENT);

        let hdr = dip(HEADER, dpi);
        let pad = dip(PAD, dpi);
        let mut r = RECT { left: pad, top: 0, right: w, bottom: hdr };
        draw_text(mem, f_title, TITLE, crate::i18n::t("Nota"), &mut r, DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);

        // ×: más clara con el ratón encima
        let cr = close_rect(w, dpi);
        let mut rc = cr;
        draw_text(mem, f_icon, if hot { INK } else { TITLE }, "\u{E8BB}", &mut rc, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);

        // Aviso: ojo tachado + «oculta al compartir», pegado a la ×
        if hidden {
            let label = crate::i18n::t("oculta al compartir");
            let mut m = RECT { left: 0, top: 0, right: w, bottom: hdr };
            draw_text(mem, f_small, DIM, label, &mut m, DT_SINGLELINE | DT_CALCRECT | DT_NOPREFIX);
            let tw = m.right - m.left;
            let right = cr.left - dip(2.0, dpi);
            let mut tr = RECT { left: right - tw, top: 0, right, bottom: hdr };
            draw_text(mem, f_small, DIM, label, &mut tr, DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);
            let iw = dip(14.0, dpi);
            let mut ir = RECT { left: tr.left - dip(4.0, dpi) - iw, top: 0, right: tr.left - dip(4.0, dpi), bottom: hdr };
            draw_text(mem, f_icon, DIM, "\u{ED1A}", &mut ir, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
        }

        let _ = BitBlt(dc, 0, 0, w, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        let _ = EndPaint(hwnd, &ps);
    }
}

fn invalidate_header(hwnd: HWND) {
    let dpi = st(|s| s.dpi.get());
    let (w, _) = client_size(hwnd);
    let r = RECT { left: 0, top: 0, right: w, bottom: dip(HEADER, dpi) };
    unsafe { let _ = InvalidateRect(Some(hwnd), Some(&r), false); }
}

// ---------------------------------------------------------------- procedimientos de ventana

fn lo(v: usize) -> u32 { (v & 0xFFFF) as u32 }
fn hi(v: usize) -> u32 { ((v >> 16) & 0xFFFF) as u32 }
/// Coordenadas con signo de un LPARAM (los monitores a la izquierda dan negativas).
fn point_of(lp: LPARAM) -> (i32, i32) { ((lp.0 & 0xFFFF) as u16 as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32) }

unsafe extern "system" fn note_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_NCHITTEST => {
                let (sx, sy) = point_of(lp);
                let mut p = POINT { x: sx, y: sy };
                let _ = ScreenToClient(hwnd, &mut p);
                let hdr = dip(HEADER, st(|s| s.dpi.get()));
                // La cabecera arrastra la ventana, salvo la ×
                if p.y >= 0 && p.y < hdr && !in_close(hwnd, p.x, p.y) { return LRESULT(HTCAPTION as isize); }
                LRESULT(HTCLIENT as isize)
            }
            // Doble clic en la cabecera: nada de maximizar
            WM_NCLBUTTONDBLCLK => LRESULT(0),
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => { paint(hwnd); LRESULT(0) }
            WM_CTLCOLOREDIT => {
                let dc = HDC(wp.0 as _);
                SetTextColor(dc, INK);
                SetBkColor(dc, BG);
                LRESULT(st(|s| s.bg.get()).0 as isize)
            }
            WM_COMMAND => {
                if hi(wp.0) == EN_CHANGE && lo(wp.0) as usize == ID_EDIT && !st(|s| s.loading.get()) {
                    st(|s| s.dirty.set(true));
                    // Se guarda cuando se deja de escribir un rato
                    SetTimer(Some(hwnd), TIMER_SAVE, SAVE_MS, None);
                    retarget();
                }
                LRESULT(0)
            }
            WM_TIMER => {
                match wp.0 {
                    TIMER_ANIM => tick(hwnd),
                    TIMER_SAVE => { let _ = KillTimer(Some(hwnd), TIMER_SAVE); save_text(); }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_SIZE => { place_edit(); LRESULT(0) }
            WM_MOUSEMOVE => {
                let (x, y) = point_of(lp);
                let hot = in_close(hwnd, x, y);
                if hot != st(|s| s.hot.replace(hot)) { invalidate_header(hwnd); }
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                if st(|s| s.hot.replace(false)) { invalidate_header(hwnd); }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point_of(lp);
                if in_close(hwnd, x, y) {
                    st(|s| s.down.set(true));
                    SetCapture(hwnd);
                } else {
                    // Clic en el margen: al texto
                    let _ = SetFocus(Some(st(|s| s.edit.get())));
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                if st(|s| s.down.replace(false)) {
                    let _ = ReleaseCapture();
                    let (x, y) = point_of(lp);
                    if in_close(hwnd, x, y) { close(); }
                }
                LRESULT(0)
            }
            WM_CAPTURECHANGED => { st(|s| s.down.set(false)); LRESULT(0) }
            WM_ACTIVATE => {
                if lo(wp.0) != WA_INACTIVE { let _ = SetFocus(Some(st(|s| s.edit.get()))); }
                LRESULT(0)
            }
            WM_SETFOCUS => { let _ = SetFocus(Some(st(|s| s.edit.get()))); LRESULT(0) }
            WM_EXITSIZEMOVE => { save_pos(hwnd); LRESULT(0) }
            WM_DPICHANGED => {
                let dpi = lo(wp.0).max(1);
                let r = &*(lp.0 as *const RECT);
                set_fonts(dpi);
                st(|s| s.grow.set(None));
                let _ = SetWindowPos(hwnd, None, r.left, r.top, dip(WIDTH, dpi), dip(MIN_H, dpi), SWP_NOZORDER | SWP_NOACTIVATE);
                place_edit();
                resize_height(hwnd, target_height());
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            // Si la sesión se cierra con la nota abierta, que no se pierda lo último
            WM_ENDSESSION => { if wp.0 != 0 { save_text(); save_pos(hwnd); } LRESULT(0) }
            WM_CLOSE => { close(); LRESULT(0) }
            WM_DESTROY => {
                save_text();
                let edit = st(|s| s.edit.get());
                if alive(edit) { let _ = RemoveWindowSubclass(edit, Some(edit_proc), 1); }
                free_fonts();
                st(|s| {
                    s.hwnd.set(HWND::default());
                    s.edit.set(HWND::default());
                    s.fade.set(None);
                    s.grow.set(None);
                    let b = s.bg.replace(HBRUSH::default());
                    if !b.is_invalid() { let _ = DeleteObject(b.into()); }
                });
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Teclas que el EDIT nativo no trae: Ctrl+A, Ctrl+Retroceso y Esc (cierra la nota).
unsafe extern "system" fn edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        match msg {
            WM_KEYDOWN => {
                let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                let vk = wp.0 as u16;
                if vk == VK_ESCAPE.0 {
                    let _ = PostMessageW(Some(st(|s| s.hwnd.get())), WM_CLOSE, WPARAM(0), LPARAM(0));
                    return LRESULT(0);
                }
                if ctrl && vk == b'A' as u16 {
                    SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                    return LRESULT(0);
                }
                if ctrl && vk == VK_BACK.0 {
                    let (mut a, mut b) = (0u32, 0u32);
                    SendMessageW(hwnd, EM_GETSEL, Some(WPARAM(&mut a as *mut u32 as usize)), Some(LPARAM(&mut b as *mut u32 as isize)));
                    if a == b {
                        let txt: Vec<u16> = edit_text(hwnd).encode_utf16().collect();
                        let start = word_start(&txt, b as usize);
                        SendMessageW(hwnd, EM_SETSEL, Some(WPARAM(start)), Some(LPARAM(b as isize)));
                    }
                    let empty = [0u16; 1];
                    SendMessageW(hwnd, EM_REPLACESEL, Some(WPARAM(1)), Some(LPARAM(empty.as_ptr() as isize)));
                    return LRESULT(0);
                }
            }
            // Los caracteres que dejarían esas teclas (Esc, Ctrl+A, Ctrl+Retroceso) no se escriben
            WM_CHAR if matches!(wp.0, 0x1B | 0x01 | 0x7F) => return LRESULT(0),
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alto_minimo_con_poco_texto() {
        // 96 ppp, líneas de 19 px: 1 línea → 32 + 19 + 12 = 63 < 120
        assert_eq!(note_height(0, 19, 96), 120);
        assert_eq!(note_height(1, 19, 96), 120);
        assert_eq!(note_height(4, 19, 96), 120);
    }

    #[test]
    fn alto_crece_con_las_lineas() {
        // 32 de arriba + n·19 + 12 de abajo
        assert_eq!(note_height(5, 19, 96), 32 + 5 * 19 + 12);
        assert_eq!(note_height(10, 19, 96), 32 + 10 * 19 + 12);
        assert!(note_height(11, 19, 96) > note_height(10, 19, 96));
    }

    #[test]
    fn alto_se_para_en_el_maximo() {
        assert_eq!(note_height(19, 19, 96), 32 + 19 * 19 + 12);
        assert_eq!(note_height(20, 19, 96), 420);
        assert_eq!(note_height(500, 19, 96), 420);
        // A 150 % todo escala
        assert_eq!(note_height(1, 28, 144), 180);
        assert_eq!(note_height(500, 28, 144), 630);
    }

    #[test]
    fn edit_llega_justo_al_margen_de_abajo_en_el_maximo() {
        for dpi in [96, 120, 144, 168, 192] {
            assert_eq!(text_top(dpi) + edit_height(dpi) + dip(PAD, dpi), dip(MAX_H, dpi));
        }
    }

    const MON: (i32, i32, i32, i32) = (0, 0, 1920, 1040);

    #[test]
    fn primera_vez_arriba_a_la_derecha() {
        assert_eq!(first_position(300, 96, MON, (0, 0, 1920, 1080)), (1920 - 300 - 12, 52));
        assert_eq!(first_position(450, 144, (0, 0, 2880, 1560), (0, 0, 2880, 1620)), (2880 - 450 - 18, 78));
    }

    #[test]
    fn posicion_guardada_visible_se_respeta() {
        assert_eq!(fit_position((100, 200), 300, 30, 120, &[MON], MON), (100, 200));
        // Medio fuera por la derecha, pero se puede agarrar
        assert_eq!(fit_position((1800, 200), 300, 30, 120, &[MON], MON), (1800, 200));
        // Monitor secundario a la izquierda, con coordenadas negativas
        let izq = (-1280, -200, 0, 824);
        assert_eq!(fit_position((-900, -100), 300, 30, 120, &[MON, izq], MON), (-900, -100));
    }

    #[test]
    fn posicion_perdida_se_centra_en_el_principal() {
        let centro = ((1920 - 300) / 2, (1040 - 120) / 2);
        // Monitor desenchufado
        assert_eq!(fit_position((-900, -100), 300, 30, 120, &[MON], MON), centro);
        // Cabecera por encima del borde de arriba (no se podría agarrar)
        assert_eq!(fit_position((100, -10), 300, 30, 120, &[MON], MON), centro);
        // Casi entera fuera por la derecha
        assert_eq!(fit_position((1880, 200), 300, 30, 120, &[MON], MON), centro);
        // Cabecera bajo el área de trabajo
        assert_eq!(fit_position((100, 1030), 300, 30, 120, &[MON], MON), centro);
        // Sin monitores (no debería pasar): al principal
        assert_eq!(fit_position((100, 200), 300, 30, 120, &[], MON), centro);
    }

    #[test]
    fn ini_de_posicion() {
        assert_eq!(parse_pos("x = 10\ny = -20\n"), Some((10, -20)));
        assert_eq!(parse_pos("y=5\r\nx=7"), Some((7, 5)));
        assert_eq!(parse_pos("x = 10\n"), None);
        assert_eq!(parse_pos("x = a\ny = 2"), None);
        assert_eq!(parse_pos(""), None);
    }

    #[test]
    fn saltos_de_linea_ida_y_vuelta() {
        assert_eq!(to_edit("a\nb\r\nc\rd"), "a\r\nb\r\nc\r\nd");
        assert_eq!(to_edit("\u{feff}hola"), "hola");
        assert_eq!(from_edit("a\r\nb\r\n"), "a\nb\n");
        assert_eq!(from_edit(&to_edit("uno\ndos\n\ntres")), "uno\ndos\n\ntres");
    }

    #[test]
    fn ctrl_retroceso_borra_la_palabra() {
        let t: Vec<u16> = "hola mundo  ".encode_utf16().collect();
        assert_eq!(word_start(&t, t.len()), 5);
        assert_eq!(word_start(&t, 4), 0);
        assert_eq!(word_start(&t, 0), 0);
        let t: Vec<u16> = "uno\r\ndos".encode_utf16().collect();
        assert_eq!(word_start(&t, 8), 5);
        // Justo tras el salto: se borra el CRLF entero
        assert_eq!(word_start(&t, 5), 3);
        assert_eq!(word_start(&t, 99), 5);
    }
}
