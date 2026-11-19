//! Iconos ocultos de la bandeja en un panel propio.
//!
//! Windows no da otra vía, así que se leen del panel de desbordamiento de Windows con UI Automation.
//! Sus iconos solo existen mientras ese panel está abierto: se abre un instante VUELTO TRANSPARENTE,
//! se lee o se pulsa el icono, y se deja exactamente como estaba (también al arrancar, por si
//! macbar se cerró a mitad).

use super::*;
use std::collections::HashMap;
use std::sync::mpsc::{channel as tr_channel, Sender as TrSender};

/// Iconos pequeños, del tamaño de los de la barra, sin nombre: hasta 8 por fila
const COLS: usize = 8;
const CELL_W: f32 = 34.0;
const CELL_H: f32 = 34.0;
const TPAD: f32 = 6.0;
const ICON: f32 = 17.0;

fn cols(n: usize) -> usize { n.clamp(1, COLS) }
const WM_TRAY_READY: u32 = WM_USER + 30;

/// Icono tal como lo ve la barra: nombre (tooltip) y su dibujo (bitmap del ejecutable o glifo del sistema).
#[derive(Clone)]
struct Item { name: String, short: String, icon: Option<Arc<Vec<u8>>>, glyph: Option<&'static str>, host: Option<trayhost::TrayIcon>, own: bool }

/// El icono de la propia macbar va siempre el primero: abre su menú (widgets, inicio con Windows, configuración, salir).
fn with_own(mut v: Vec<Item>) -> Vec<Item> {
    v.insert(0, Item { name: "macbar".into(), short: "macbar".into(), icon: None, glyph: Some("\u{E713}"), host: None, own: true });
    v
}

/// Iconos ocultos desde el receptor propio: instantáneo, con su dibujo y su clic reales.
fn host_items() -> Option<Vec<Item>> {
    if !trayhost::active() { return None; }
    let v = trayhost::hidden_icons();
    if v.is_empty() { return None; }
    Some(v.into_iter().map(|h| {
        let mut name = h.tip.lines().next().unwrap_or("").trim().to_string();
        if name.is_empty() {
            name = file_description(&h.exe);
            if name.is_empty() { name = h.exe.rsplit('\\').next().unwrap_or("").trim_end_matches(".exe").to_string(); }
        }
        let glyph = if h.px.is_none() { system_glyph(&name).or(Some("\u{E71D}")) } else { None };
        Item { short: short_name(&name), name, icon: h.px.clone(), glyph, host: Some(h), own: false }
    }).collect())
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());
static TR_HWND: AtomicIsize = AtomicIsize::new(0);

enum TJob { Load, Click(usize, bool) }
static TJOBS: OnceLock<TrSender<TJob>> = OnceLock::new();

struct Tr {
    hwnd: HWND,
    target: Option<ID2D1HwndRenderTarget>,
    dw: IDWriteFactory,
    cache: RefCell<Caches>,
    f_lbl: IDWriteTextFormat, f_foot: IDWriteTextFormat, f_icon: IDWriteTextFormat,
    bmps: Vec<Option<ID2D1Bitmap>>,
    hover: Option<usize>,
    anchor_right: i32,
    return_to: HWND,
    hidden_at: Option<Instant>,
}
thread_local! { static TR: RefCell<Option<Tr>> = const { RefCell::new(None) }; }

// ---------------------------------------------------------------- panel de Windows (desbordamiento)
fn overflow_window() -> Option<HWND> {
    unsafe { FindWindowW(w!("TopLevelWindowForOverflowXamlIsland"), None).ok().filter(|h| !h.0.is_null()) }
}

/// Deja el panel de Windows opaco y sin el estilo que le pusimos. Se llama también al arrancar.
pub fn repair() {
    unsafe {
        let Some(h) = overflow_window() else { return };
        let ex = GetWindowLongW(h, GWL_EXSTYLE);
        if ex & WS_EX_LAYERED.0 as i32 != 0 {
            let _ = SetLayeredWindowAttributes(h, COLORREF(0), 255, LWA_ALPHA);
            SetWindowLongW(h, GWL_EXSTYLE, ex & !(WS_EX_LAYERED.0 as i32));
        }
    }
}

fn read_elements(uia: &IUIAutomation) -> Vec<(String, IUIAutomationElement)> {
    unsafe {
        let Some(h) = overflow_window() else { return Vec::new() };
        let Ok(root) = uia.ElementFromHandle(h) else { return Vec::new() };
        let var = {
            use windows::Win32::System::Variant::*;
            let mut v = VARIANT::default();
            { let i = &mut *v.Anonymous.Anonymous; i.vt = VT_I4; i.Anonymous.lVal = UIA_ButtonControlTypeId.0; }
            v
        };
        let Ok(cond) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &var) else { return Vec::new() };
        let Ok(all) = root.FindAll(TreeScope_Descendants, &cond) else { return Vec::new() };
        let mut out = Vec::new();
        for i in 0..all.Length().unwrap_or(0) {
            let Ok(el) = all.GetElement(i) else { continue };
            if el.CurrentAutomationId().map(|b| b.to_string()).unwrap_or_default() != "NotifyItemIcon" { continue; }
            out.push((el.CurrentName().map(|b| b.to_string()).unwrap_or_default().trim().to_string(), el));
        }
        out
    }
}

/// Abre el panel de Windows transparente y espera a que sus iconos existan. Devuelve los elementos.
fn open_invisible(uia: &IUIAutomation) -> Option<Vec<(String, IUIAutomationElement)>> {
    unsafe {
        let h = overflow_window()?;
        let ex = GetWindowLongW(h, GWL_EXSTYLE);
        SetWindowLongW(h, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as i32);
        let _ = SetLayeredWindowAttributes(h, COLORREF(0), 0, LWA_ALPHA);
        sys::combo(&[VK_LWIN, sys::vk(b'B')]);
        std::thread::sleep(Duration::from_millis(180));
        sys::combo(&[VK_RETURN]);
        // Los iconos aparecen en unas décimas: se espera lo justo
        let t = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(60));
            let v = read_elements(uia);
            if !v.is_empty() { return Some(v); }
            if t.elapsed() > Duration::from_millis(1200) { return None; }
        }
    }
}

/// Espera a que el panel de Windows se cierre (o lo cierra) y lo deja opaco otra vez.
fn close_invisible(max: Duration) {
    unsafe {
        let t = Instant::now();
        while let Some(h) = overflow_window() {
            if !IsWindowVisible(h).as_bool() { break; }
            if GetForegroundWindow() == h && t.elapsed() > Duration::from_millis(300) { sys::combo(&[VK_ESCAPE]); }
            if t.elapsed() > max { break; }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    repair();
}

// ---------------------------------------------------------------- de nombre a dibujo
/// Nombres que no coinciden con el ejecutable (p. ej. la app sale en español y el proceso en inglés).
const ALIASES: [(&str, &str); 4] = [("enlace móvil", "phoneexperiencehost"), ("phone link", "phoneexperiencehost"), ("onedrive", "onedrive"), ("wallpaper engine", "wallpaper64")];

fn system_glyph(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    if n.contains("bluetooth") { return Some("\u{E702}"); }
    if n.contains("seguridad") || n.contains("security") || n.contains("defender") { return Some("\u{EA18}"); }
    if n.contains("hardware") || n.contains("expulsar") || n.contains("usb") { return Some("\u{E88E}"); }
    if n.contains("red") && !n.contains("redes sociales") || n.contains("network") || n.contains("wi-fi") { return Some("\u{E701}"); }
    if n.contains("volumen") || n.contains("volume") || n.contains("altavoz") { return Some("\u{E767}"); }
    if n.contains("batería") || n.contains("battery") { return Some("\u{E83F}"); }
    None
}

fn file_description(path: &str) -> String {
    use windows::Win32::Storage::FileSystem::*;
    unsafe {
        let p = HSTRING::from(path);
        let size = GetFileVersionInfoSizeW(&p, None);
        if size == 0 { return String::new(); }
        let mut buf = vec![0u8; size as usize];
        if GetFileVersionInfoW(&p, None, size, buf.as_mut_ptr() as _).is_err() { return String::new(); }
        let mut tr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let mut sub = "\\StringFileInfo\\040904B0\\FileDescription".to_string();
        if VerQueryValueW(buf.as_ptr() as _, w!("\\VarFileInfo\\Translation"), &mut tr, &mut len).as_bool() && len >= 4 {
            let t = &*(tr as *const [u16; 2]);
            sub = format!("\\StringFileInfo\\{:04X}{:04X}\\FileDescription", t[0], t[1]);
        }
        let mut val: *mut core::ffi::c_void = std::ptr::null_mut();
        if VerQueryValueW(buf.as_ptr() as _, &HSTRING::from(sub), &mut val, &mut len).as_bool() && len > 0 {
            let s = std::slice::from_raw_parts(val as *const u16, len as usize);
            return String::from_utf16_lossy(s).trim_end_matches('\0').trim().to_string();
        }
        String::new()
    }
}

/// Procesos en marcha: (nombre del ejecutable sin .exe, descripción, ruta). Se calcula una vez por carga.
fn processes() -> Vec<(String, String, String)> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            if let Some(path) = sys::process_path(e.th32ProcessID) {
                if seen.insert(path.to_lowercase()) {
                    let file = path.rsplit('\\').next().unwrap_or("").to_lowercase();
                    let stem = file.trim_end_matches(".exe").to_string();
                    let desc = file_description(&path).to_lowercase();
                    out.push((stem, desc, path));
                }
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Ejecutable más probable de un icono de bandeja por su nombre.
fn exe_for(name: &str, procs: &[(String, String, String)]) -> Option<String> {
    let key = name.split(" - ").next().unwrap_or(name).trim().to_lowercase();
    if key.is_empty() { return None; }
    let alias = ALIASES.iter().find(|(a, _)| key.starts_with(a)).map(|(_, s)| *s);
    let first = key.split_whitespace().next().unwrap_or(&key).to_string();
    procs.iter().find(|(stem, desc, _)| {
        if matches!(stem.as_str(), "explorer" | "svchost" | "runtimebroker" | "applicationframehost") { return false; }
        alias == Some(stem.as_str()) || *stem == key.replace(' ', "") || (!desc.is_empty() && (*desc == key || key.starts_with(desc.as_str())))
            || (first.len() >= 4 && (stem.starts_with(&first) || desc.starts_with(&first)))
    }).map(|(_, _, p)| p.clone())
}

/// Icono del ejecutable a 64 px, en BGRA premultiplicado (formato de Direct2D).
fn exe_icon(path: &str) -> Option<Vec<u8>> {
    unsafe {
        let mut icons = [HICON::default(); 1];
        let mut ids = [0u32; 1];
        let mut wide_path: Vec<u16> = path.encode_utf16().collect();
        wide_path.resize(260, 0);
        let n = PrivateExtractIconsW(&wide_path.try_into().ok()?, 0, 64, 64, Some(&mut icons), Some(ids.as_mut_ptr()), 0);
        if n == 0 || n == u32::MAX || icons[0].is_invalid() { return None; }
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let bmp = wic.CreateBitmapFromHICON(icons[0]).ok();
        let _ = DestroyIcon(icons[0]);
        let src: IWICBitmapSource = bmp?.into();
        wic_square(&wic, &src)
    }
}

fn short_name(name: &str) -> String {
    let s = name.split(" - ").next().unwrap_or(name).trim();
    if s.is_empty() { i18n::t("Sin nombre").into() } else { s.to_string() }
}

// ---------------------------------------------------------------- hilo de trabajo
fn worker(rx: std::sync::mpsc::Receiver<TJob>) {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let Ok(uia) = (unsafe { CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }) else { return };
    let mut icon_cache: HashMap<String, Option<Arc<Vec<u8>>>> = HashMap::new();
    while let Ok(job) = rx.recv() {
        match job {
            TJob::Load => {
                let els = open_invisible(&uia).unwrap_or_default();
                let names: Vec<String> = els.iter().map(|(n, _)| n.clone()).collect();
                drop(els);
                close_invisible(Duration::from_millis(1500));
                let need_procs = names.iter().any(|n| system_glyph(n).is_none() && !icon_cache.contains_key(&short_name(n)));
                let procs = if need_procs { processes() } else { Vec::new() };
                let items: Vec<Item> = names.iter().map(|n| {
                    let short = short_name(n);
                    let glyph = system_glyph(n);
                    let icon = if glyph.is_some() { None } else {
                        icon_cache.entry(short.clone()).or_insert_with(|| exe_for(n, &procs).and_then(|p| exe_icon(&p)).map(Arc::new)).clone()
                    };
                    Item { name: n.clone(), short, icon, glyph: glyph.or(Some("\u{E71D}")), host: None, own: false }
                }).collect();
                *ITEMS.lock().unwrap() = with_own(items);
                unsafe { let _ = PostMessageW(Some(HWND(TR_HWND.load(Ordering::Relaxed) as _)), WM_TRAY_READY, WPARAM(0), LPARAM(0)); }
            }
            TJob::Click(idx, right) => {
                let Some(els) = open_invisible(&uia) else { close_invisible(Duration::from_millis(800)); continue };
                // Se busca por posición y se confirma por nombre (los iconos pueden haber cambiado)
                let want = ITEMS.lock().unwrap().get(idx).map(|i| i.name.clone()).unwrap_or_default();
                let el = els.iter().find(|(n, _)| *n == want).or_else(|| els.get(idx.saturating_sub(1))) /* el 0 es el de macbar */.map(|(_, e)| e.clone());
                if let Some(el) = el {
                    unsafe {
                        if right {
                            // Clic derecho real sobre el icono (Windows no ofrece otra forma de abrir su menú)
                            if let Ok(r) = el.CurrentBoundingRectangle() {
                                let _ = SetCursorPos((r.left + r.right) / 2, (r.top + r.bottom) / 2);
                                std::thread::sleep(Duration::from_millis(40));
                                let mi = |f: MOUSE_EVENT_FLAGS| INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: f, ..Default::default() } } };
                                SendInput(&[mi(MOUSEEVENTF_RIGHTDOWN), mi(MOUSEEVENTF_RIGHTUP)], size_of::<INPUT>() as i32);
                            }
                        } else if let Ok(p) = el.GetCurrentPattern(UIA_InvokePatternId) {
                            if let Ok(inv) = windows::core::Interface::cast::<IUIAutomationInvokePattern>(&p) { let _ = inv.Invoke(); }
                        }
                    }
                }
                drop(els);
                // El menú de la app puede tardar en cerrarse: se espera hasta 20 s; un clic normal, 3 s
                close_invisible(Duration::from_secs(if right { 20 } else { 3 }));
            }
        }
    }
}

fn tjob(j: TJob) { if let Some(tx) = TJOBS.get() { let _ = tx.send(j); } }

// ---------------------------------------------------------------- ventana del panel
/// Abre o cierra el panel. `anchor_right` es el borde derecho del botón ^ en píxeles de pantalla.
pub fn toggle(anchor_right: i32) {
    let visible = is_open();
    if visible { hide(); return; }
    let just = TR.with(|t| t.borrow().as_ref().and_then(|t| t.hidden_at).is_some_and(|x| x.elapsed() < Duration::from_millis(350)));
    if just { return; }
    if TR.with(|t| t.borrow().is_none()) { create(); }
    TR.with(|t| if let Some(t) = t.borrow_mut().as_mut() { t.anchor_right = anchor_right; t.return_to = last_fg(); });
    // Con el receptor propio los iconos ya están aquí: se abre al instante
    if let Some(items) = host_items() {
        *ITEMS.lock().unwrap() = with_own(items);
        show_now();
        return;
    }
    // Si no, se leen del panel de Windows (invisible) y luego se muestra el nuestro
    tjob(TJob::Load);
}

pub fn prewarm() { if TR.with(|t| t.borrow().is_none()) { create(); } }
pub fn close() { hide(); }

pub fn is_open() -> bool { TR.with(|t| t.borrow().as_ref().is_some_and(|t| popup::shown(t.hwnd))) }

fn hide() { hide_ex(true); }

/// Oculta el panel. `restore`: devolver el foco a la app de antes (no si un icono va a abrir algo).
fn hide_ex(restore: bool) {
    let hw = TR.with(|t| t.borrow_mut().as_mut().map(|t| { t.hidden_at = Some(Instant::now()); (t.hwnd, t.return_to) }));
    let Some((hwnd, ret)) = hw else { return };
    unsafe {
        if !popup::shown(hwnd) { return; }
        popup::dismiss(hwnd);
        if restore && (GetForegroundWindow().0.is_null() || sys::window_pid(GetForegroundWindow()) == std::process::id()) {
            if !ret.0.is_null() { let _ = SetForegroundWindow(ret); }
        }
    }
}

fn create() {
    unsafe {
        repair();
        let hwnd = popup::create_window(w!("MacBarTrayPanel"), w!("Iconos ocultos"), Some(wndproc));
        TR_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();
        let t = Tr {
            hwnd, target: None,
            f_lbl: make_format(&dw, FONT, 11.5, DWRITE_FONT_WEIGHT_NORMAL),
            f_foot: make_format(&dw, FONT, 12.0, DWRITE_FONT_WEIGHT_MEDIUM),
            f_icon: make_format(&dw, ICONS, 14.5, DWRITE_FONT_WEIGHT_NORMAL),
            dw, cache: RefCell::new(Caches::default()), bmps: Vec::new(), hover: None,
            anchor_right: 0, return_to: HWND::default(), hidden_at: None,
        };
        TR.with(|c| *c.borrow_mut() = Some(t));
        // El canal existe ANTES de lanzar el hilo: así el primer trabajo nunca se pierde
        let (tx, rx) = tr_channel::<TJob>();
        let _ = TJOBS.set(tx);
        std::thread::spawn(move || worker(rx));
    }
}

fn panel_size(n: usize) -> (f32, f32) {
    let rows = n.div_ceil(cols(n)).max(1);
    let w = if n == 0 { 170.0 } else { TPAD * 2.0 + cols(n) as f32 * CELL_W };
    (w, TPAD * 2.0 + rows as f32 * CELL_H)
}

fn show_now() {
    let s = scale_of(bar_hwnd());
    let n = ITEMS.lock().unwrap().len();
    let (w, h) = panel_size(n);
    let Some((hwnd, ar)) = TR.with(|t| t.borrow_mut().as_mut().map(|t| { t.bmps.clear(); t.target = None; t.hover = None; (t.hwnd, t.anchor_right) })) else { return };
    let (wpx, hpx) = ((w * s) as i32, (h * s) as i32);
    let x = (ar - wpx).max(4);
    unsafe {
        popup::begin(hwnd, x, ((BAR_H + 4.0) * s) as i32, wpx, hpx);
        let _ = SetForegroundWindow(hwnd);
    }
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        BeginPaint(hwnd, &mut ps);
        TR.with(|tr| {
            let mut tr = tr.borrow_mut();
            let Some(t) = tr.as_mut() else { return };
            let s = scale_of(hwnd);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            if t.target.is_none() {
                let Ok(fac) = D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) else { return };
                let props = D2D1_RENDER_TARGET_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, ..Default::default() };
                let hp = D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd, pixelSize: D2D_SIZE_U { width: (rc.right - rc.left) as u32, height: (rc.bottom - rc.top) as u32 }, presentOptions: D2D1_PRESENT_OPTIONS_NONE };
                t.target = fac.CreateHwndRenderTarget(&props, &hp).ok();
                if let Some(rt) = &t.target { rt.SetDpi(96.0 * s, 96.0 * s); rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE); }
                t.bmps.clear();
            }
            let Some(rt) = t.target.clone() else { return };
            let items = ITEMS.lock().unwrap().clone();
            if t.bmps.len() != items.len() {
                let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                t.bmps = items.iter().map(|it| it.icon.as_ref().and_then(|px| rt.CreateBitmap(D2D_SIZE_U { width: ART_PX, height: ART_PX }, Some(px.as_ptr() as _), ART_PX * 4, &bp).ok())).collect();
            }
            let (w, ph) = panel_size(items.len());
            let ink = popup::panel_ink();
            let p = Painter { rt: &rt, dw: &t.dw, cache: &t.cache, h: 100.0, s };
            let white = p.brush(popup::ink_color(ink, 0.95));
            let dim = p.brush(popup::ink_color(ink, 0.55));
            let hov = p.brush(popup::ink_color(ink, 0.10));
            let tile = p.brush(popup::ink_color(ink, 0.07));
            rt.BeginDraw();
            rt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
            let tint = p.brush(popup::tint());
            rt.FillRectangle(&rect(0.0, 0.0, w, ph), &tint); popup::glass_edge(&rt, w, ph, s); rt.SetTransform(&popup::content_matrix(hwnd, w));
            popup::push_reveal(&rt, hwnd, ph, popup::alpha(hwnd));
            let text_c = |f: &IDWriteTextFormat, s: &str, cx: f32, top: f32, bh: f32, b: &ID2D1Brush| {
                let tw = p.width(f, s);
                let cap = p.text_top(f) - 50.0;
                p.draw_at(f, s, cx - tw / 2.0, top + bh / 2.0 + cap, b, None);
            };
            if items.is_empty() {
                text_c(&t.f_foot, i18n::t("No hay iconos ocultos"), w / 2.0, TPAD, CELL_H, &dim);
                let _ = (&tile, &t.f_lbl);
            }
            let nc = cols(items.len());
            // Al abrir, los iconos aparecen en cascada con un pequeño rebote (uno tras otro)
            let cm = popup::content_matrix(hwnd, w);
            let opened = popup::reveal_elapsed(hwnd);
            for (i, it) in items.iter().enumerate() {
                let (col, row) = ((i % nc) as f32, (i / nc) as f32);
                let x0 = TPAD + col * CELL_W;
                let y0 = TPAD + row * CELL_H;
                let (cx, cy) = (x0 + CELL_W / 2.0, y0 + CELL_H / 2.0);
                if t.hover == Some(i) { rt.FillRoundedRectangle(&rrect(rect(x0 + 2.0, y0 + 2.0, x0 + CELL_W - 2.0, y0 + CELL_H - 2.0), 8.0), &hov); }
                let pop = opened.map(|e| { let d = (i as f32 * 0.012).min(0.08); 0.4 + 0.6 * crate::ease_back(((e - d) / 0.24).clamp(0.0, 1.0)) }).unwrap_or(1.0);
                if pop < 0.999 { rt.SetTransform(&(Matrix3x2 { M11: pop, M12: 0.0, M21: 0.0, M22: pop, M31: cx * (1.0 - pop), M32: cy * (1.0 - pop) } * cm)); }
                match (t.bmps.get(i).and_then(|b| b.clone()), it.glyph) {
                    (Some(b), _) => p.art(&b, p.snap(cx - ICON / 2.0), p.snap(cy - ICON / 2.0), ICON, Some(3.0)),
                    (None, Some(g)) => {
                        let [l, tt, iw, ih] = p.ink(&t.f_icon, g);
                        p.draw_at(&t.f_icon, g, cx - iw / 2.0 - l, p.snap(cy - ih / 2.0 - tt), &white, None);
                    }
                    _ => {}
                }
                if pop < 0.999 { rt.SetTransform(&cm); }
            }
            rt.PopLayer();
            rt.SetTransform(&popup::IDENTITY);
            if rt.EndDraw(None, None).is_err() { t.target = None; }
        });
        let _ = EndPaint(hwnd, &ps);
    }
}

fn item_at(hwnd: HWND, lp: LPARAM) -> Option<usize> {
    let s = scale_of(hwnd);
    let x = (lp.0 & 0xFFFF) as i16 as f32 / s - TPAD;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / s - TPAD;
    if x < 0.0 || y < 0.0 { return None; }
    let nc = cols(ITEMS.lock().unwrap().len());
    let (c, r) = ((x / CELL_W) as usize, (y / CELL_H) as usize);
    if c >= nc { return None; }
    let i = r * nc + c;
    (i < ITEMS.lock().unwrap().len()).then_some(i)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => { paint(hwnd); LRESULT(0) }
            WM_ERASEBKGND => LRESULT(1),
            WM_TRAY_READY => { show_now(); LRESULT(0) }
            WM_TIMER => { if wp.0 == popup::TIMER_REVEAL { popup::step(hwnd); } LRESULT(0) }
            WM_ACTIVATE => { if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE { hide(); } LRESULT(0) }
            WM_KEYDOWN => { if wp.0 as u16 == VK_ESCAPE.0 { hide(); } LRESULT(0) }
            WM_MOUSEMOVE => {
                let h = item_at(hwnd, lp);
                let ch = TR.with(|t| t.borrow_mut().as_mut().map(|t| { let c = t.hover != h; t.hover = h; c }).unwrap_or(false));
                if ch { let _ = InvalidateRect(Some(hwnd), None, false); }
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
                LRESULT(0)
            }
            0x02A3 => { TR.with(|t| if let Some(t) = t.borrow_mut().as_mut() { t.hover = None; }); let _ = InvalidateRect(Some(hwnd), None, false); LRESULT(0) }
            WM_LBUTTONUP | WM_RBUTTONUP => {
                if let Some(i) = item_at(hwnd, lp) {
                    let right = msg == WM_RBUTTONUP;
                    if ITEMS.lock().unwrap().get(i).is_some_and(|it| it.own) {
                        hide_ex(false);
                        crate::macbar_menu();
                        return LRESULT(0);
                    }
                    let host = ITEMS.lock().unwrap().get(i).and_then(|it| it.host.clone());
                    match host {
                        // Icono del receptor propio: el clic va directo a la app, como desde la barra de tareas
                        // Los iconos del propio Explorador (Bluetooth…) no atienden avisos de fuera: van por su panel
                        Some(h) if h.cbmsg != 0 && !h.exe.to_lowercase().ends_with("\\explorer.exe") => {
                            let mut pt = POINT::default();
                            let _ = GetCursorPos(&mut pt);
                            // El permiso para ponerse delante se da mientras somos la ventana activa
                            let _ = AllowSetForegroundWindow(h.pid);
                            hide_ex(false);
                            trayhost::click(&h, right, pt.x, pt.y);
                        }
                        // Sin mensaje de aviso conocido (o sin receptor): se pulsa en el panel de Windows
                        _ => { hide(); tjob(TJob::Click(i, right)); }
                    }
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
