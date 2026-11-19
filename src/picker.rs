//! Cuentagotas de color de pantalla (como el de PowerToys o ColorSlurp, con aire de macOS).
//!
//! Dos ventanas: una capa casi invisible que cubre todo el escritorio virtual (recoge el ratón y pone
//! el cursor de cruz) y, encima, la lupa: una ventana pequeña con alfa por píxel que se pinta con
//! Direct2D sobre un DIB y se sube con `UpdateLayeredWindow`. Las dos se excluyen de la captura
//! (`WDA_EXCLUDEFROMCAPTURE`), así que el `BitBlt` de la pantalla ve lo que hay debajo.
//! Todo es autocontenido: no usa nada del resto de la barra.

use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};
use windows::core::{w, Interface, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::{Matrix3x2, Vector2};

/// Píxeles por lado de la zona capturada (impar: hay un píxel central).
const N: i32 = 13;
const HALF: i32 = N / 2;
const ENTER_MS: f32 = 180.0;
const EXIT_MS: f32 = 140.0;
/// Constante de tiempo con la que la lupa persigue al cursor (s).
const FOLLOW_TAU: f32 = 0.045;
/// Si el ratón sigue pulsado al acabar la salida, se espera a que se suelte (como mucho esto).
const RELEASE_WAIT_MS: f32 = 1500.0;

/// Abre el cuentagotas (bloquea el hilo que lo llama, con su propio bucle de mensajes).
/// Devuelve el color elegido (r, g, b) o None si se cancela con Esc o clic derecho.
pub fn pick() -> Option<(u8, u8, u8)> {
    EVENTS.with(|e| e.borrow_mut().clear());
    let mut p = Picker::new()?;
    let (res, quit) = p.run();
    drop(p);
    // Si alguien pidió cerrar el hilo mientras estaba abierto, el aviso se devuelve a la cola
    if let Some(code) = quit { unsafe { PostQuitMessage(code); } }
    res
}

// ---------------------------------------------------------------- funciones puras

/// `#RRGGBB` en mayúsculas.
fn hex(r: u8, g: u8, b: u8) -> String { format!("#{r:02X}{g:02X}{b:02X}") }

/// Esquina superior izquierda de la caja visible de la lupa (`w`×`h`) para un cursor en (`cx`, `cy`):
/// abajo a la derecha del cursor, a `off` píxeles; si no cabe, al otro lado (izquierda / arriba), y al
/// final se encaja dentro del monitor `mon` = (izq, arriba, der, abajo).
fn place(cx: i32, cy: i32, w: i32, h: i32, off: i32, mon: (i32, i32, i32, i32)) -> (i32, i32) {
    let (l, t, r, b) = mon;
    let mut x = cx + off;
    if x + w > r { x = cx - off - w; }
    let mut y = cy + off;
    if y + h > b { y = cy - off - h; }
    // Monitor diminuto: al menos que no se salga (si no cabe, manda la esquina de arriba a la izquierda)
    (x.min(r - w).max(l), y.min(b - h).max(t))
}

/// Deshace el velo de la capa (negro con alfa 1/255) si no se pudo excluir de la captura.
fn unblend(c: u8) -> u8 { ((c as u32 * 255 + 127) / 254).min(255) as u8 }

fn ease_out(t: f32) -> f32 { let u = 1.0 - t.clamp(0.0, 1.0); 1.0 - u * u * u }
fn ease_back(t: f32) -> f32 { let t = t.clamp(0.0, 1.0) - 1.0; 1.0 + 2.70158 * t * t * t + 1.70158 * t * t }

fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F { D2D1_COLOR_F { r, g, b, a } }
fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F { D2D_RECT_F { left: l, top: t, right: r, bottom: b } }
fn ellipse(x: f32, y: f32, r: f32) -> D2D1_ELLIPSE { D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: r, radiusY: r } }
fn rrect(r: D2D_RECT_F, rad: f32) -> D2D1_ROUNDED_RECT { D2D1_ROUNDED_RECT { rect: r, radiusX: rad, radiusY: rad } }

/// Medidas de la lupa en píxeles físicos para una escala de monitor.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Geo {
    s: f32,
    /// Lado de cada píxel ampliado (entero: así la cuadrícula queda nítida).
    cell: i32,
    /// Diámetro de la lupa.
    d: i32,
    /// Margen transparente alrededor (sombra y rebote de la escala).
    m: i32,
    gap: i32,
    pill_h: i32,
    off: i32,
    /// Tamaño de la ventana.
    w: i32,
    h: i32,
}

impl Geo {
    fn new(s: f32) -> Geo {
        let px = |v: f32| (v * s).round() as i32;
        let cell = px(10.0).max(4);
        let d = cell * N;
        let (m, gap, pill_h, off) = (px(20.0), px(8.0), px(28.0), px(20.0));
        Geo { s, cell, d, m, gap, pill_h, off, w: d + 2 * m, h: d + gap + pill_h + 2 * m }
    }
    /// Alto de la parte visible (lupa + pastilla).
    fn body_h(&self) -> i32 { self.d + self.gap + self.pill_h }
}

// ---------------------------------------------------------------- eventos (ventanas y gancho → bucle)

#[derive(Clone, Copy, Debug)]
enum Ev { Pick, Cancel, Nudge(i32, i32), Reframe }

thread_local! { static EVENTS: RefCell<Vec<Ev>> = const { RefCell::new(Vec::new()) }; }

fn push(e: Ev) { EVENTS.with(|v| v.borrow_mut().push(e)); }

/// Teclas que usa el cuentagotas (Mayús + flecha mueve 10 px).
fn key_event(vk: u32) -> Option<Ev> {
    let step = if unsafe { GetAsyncKeyState(VK_SHIFT.0 as i32) } < 0 { 10 } else { 1 };
    match VIRTUAL_KEY(vk as u16) {
        VK_ESCAPE => Some(Ev::Cancel),
        VK_RETURN => Some(Ev::Pick),
        VK_LEFT => Some(Ev::Nudge(-step, 0)),
        VK_RIGHT => Some(Ev::Nudge(step, 0)),
        VK_UP => Some(Ev::Nudge(0, -step)),
        VK_DOWN => Some(Ev::Nudge(0, step)),
        _ => None,
    }
}

/// Gancho de teclado: las teclas del cuentagotas llegan aunque Windows no deje traer la capa al
/// frente, y no se escapan a la ventana de debajo.
unsafe extern "system" fn kb_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code == HC_ACTION as i32 && lp.0 != 0 {
            let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
            if let Some(ev) = key_event(k.vkCode) {
                let m = wp.0 as u32;
                if m == WM_KEYDOWN || m == WM_SYSKEYDOWN { push(ev); }
                return LRESULT(1);
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_SETCURSOR => {
                if let Ok(c) = LoadCursorW(None, IDC_CROSS) { SetCursor(Some(c)); }
                LRESULT(1)
            }
            WM_LBUTTONDOWN => { SetCapture(hwnd); push(Ev::Pick); LRESULT(0) }
            WM_RBUTTONDOWN => { SetCapture(hwnd); push(Ev::Cancel); LRESULT(0) }
            WM_LBUTTONUP | WM_RBUTTONUP => { let _ = ReleaseCapture(); LRESULT(0) }
            WM_KEYDOWN | WM_SYSKEYDOWN => match key_event(wp.0 as u32) {
                Some(ev) => { push(ev); LRESULT(0) }
                None => DefWindowProcW(hwnd, msg, wp, lp),
            },
            WM_DISPLAYCHANGE => { push(Ev::Reframe); DefWindowProcW(hwnd, msg, wp, lp) }
            WM_CLOSE => { push(Ev::Cancel); LRESULT(0) }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

unsafe extern "system" fn loupe_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // La lupa no estorba al ratón: todo va a la capa de debajo
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

// ---------------------------------------------------------------- superficies GDI

/// DIB de 32 bits (de arriba abajo) seleccionado en un DC de memoria.
struct Surface { dc: HDC, bmp: HBITMAP, old: HGDIOBJ, bits: *mut u32 }

impl Surface {
    fn new(w: i32, h: i32) -> Option<Surface> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() { return None; }
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = match CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                Ok(b) => { let _ = DeleteObject(b.into()); let _ = DeleteDC(dc); return None; }
                Err(_) => { let _ = DeleteDC(dc); return None; }
            };
            let old = SelectObject(dc, bmp.into());
            Some(Surface { dc, bmp, old, bits: bits as *mut u32 })
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

// ---------------------------------------------------------------- el cuentagotas

struct Picker {
    overlay: HWND,
    loupe: HWND,
    hook: Option<HHOOK>,
    screen: HDC,
    prev_dpi: DPI_AWARENESS_CONTEXT,
    fac: ID2D1Factory,
    dw: IDWriteFactory,
    family: Vec<u16>,
    rt: Option<ID2D1DCRenderTarget>,
    fmt: Option<IDWriteTextFormat>,
    grab: Option<Surface>,
    canvas: Option<Surface>,
    geo: Geo,
    hmon: HMONITOR,
    mon: (i32, i32, i32, i32),
    /// Píxeles capturados alrededor del cursor (BGRA, alfa a 0).
    px: [u32; (N * N) as usize],
    cur: POINT,
    /// Posición actual de la ventana de la lupa (persigue a la de destino).
    pos: (f32, f32),
    snap: bool,
    refresh: bool,
    /// La capa no se pudo excluir de la captura: hay que deshacer su velo.
    compensate: bool,
    /// La lupa se puede deslizar por encima del cursor (excluida de la captura).
    glide: bool,
    t0: Instant,
    last: Instant,
    /// Salida en curso: (inicio, escala y opacidad de partida).
    exit: Option<(Instant, f32, f32)>,
    result: Option<(u8, u8, u8)>,
    dirty: bool,
}

impl Picker {
    fn new() -> Option<Picker> {
        unsafe {
            let fac = D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            let family = pick_family(&dw);
            // Coordenadas físicas en este hilo, pase lo que pase con el resto del proceso
            let prev_dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let now = Instant::now();
            let mut p = Picker {
                overlay: HWND::default(), loupe: HWND::default(), hook: None, screen: HDC::default(), prev_dpi,
                fac, dw, family, rt: None, fmt: None, grab: None, canvas: None,
                geo: Geo::new(1.0), hmon: HMONITOR::default(), mon: (0, 0, 0, 0),
                px: [0; (N * N) as usize], cur: POINT::default(), pos: (0.0, 0.0), snap: true, refresh: true,
                compensate: false, glide: true, t0: now, last: now, exit: None, result: None, dirty: true,
            };
            // Si algo falla a medias, el Drop deshace lo que se llegó a crear
            p.init()?;
            Some(p)
        }
    }

    fn init(&mut self) -> Option<()> {
        unsafe {
            let hinst: HINSTANCE = GetModuleHandleW(None).ok()?.into();
            let (oc, lc) = (w!("MacBarPickerOverlay"), w!("MacBarPickerLoupe"));
            let cross = LoadCursorW(None, IDC_CROSS).unwrap_or_default();
            // Fondo negro: el velo (alfa 1/255) es siempre el mismo, y se puede deshacer si hace falta
            let black = HBRUSH(GetStockObject(BLACK_BRUSH).0);
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(overlay_proc), hInstance: hinst, lpszClassName: oc, hCursor: cross, hbrBackground: black, ..Default::default() });
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(loupe_proc), hInstance: hinst, lpszClassName: lc, ..Default::default() });

            let (vx, vy, vw, vh) = virtual_screen();
            self.overlay = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW, oc, w!("macbar picker"), WS_POPUP,
                vx, vy, vw, vh, None, None, Some(hinst), None).ok()?;
            // Alfa 1 (no 0): así la capa sigue recibiendo el ratón
            SetLayeredWindowAttributes(self.overlay, COLORREF(0), 1, LWA_ALPHA).ok()?;
            self.compensate = SetWindowDisplayAffinity(self.overlay, WDA_EXCLUDEFROMCAPTURE).is_err();
            // La lupa es "propiedad" de la capa: siempre queda encima de ella
            self.loupe = CreateWindowExW(WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, lc, w!("macbar loupe"), WS_POPUP,
                0, 0, 1, 1, Some(self.overlay), None, Some(hinst), None).ok()?;
            // Si no se puede excluir, la colocación la mantiene lejos del cursor, pero sin deslizarse por encima
            // MACBAR_PICKER_SHOW=1 deja la lupa visible en capturas (para comprobarla a la vista)
            self.glide = std::env::var_os("MACBAR_PICKER_SHOW").is_none() && SetWindowDisplayAffinity(self.loupe, WDA_EXCLUDEFROMCAPTURE).is_ok();

            self.screen = GetDC(None);
            if self.screen.is_invalid() { return None; }
            self.grab = Some(Surface::new(N, N)?);
            self.rt = Some(self.make_rt()?);

            // Primer fotograma (invisible) antes de enseñar nada
            let _ = GetCursorPos(&mut self.cur);
            self.update();
            self.t0 = Instant::now();
            self.render();
            let _ = SetWindowPos(self.overlay, Some(HWND_TOPMOST), vx, vy, vw, vh, SWP_SHOWWINDOW);
            let _ = ShowWindow(self.loupe, SW_SHOWNOACTIVATE);
            let _ = SetForegroundWindow(self.overlay);
            let _ = SetFocus(Some(self.overlay));
            SetCursor(Some(cross));
            self.hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_hook), Some(hinst), 0).ok();
            self.t0 = Instant::now();
            self.last = self.t0;
            Some(())
        }
    }

    fn make_rt(&self) -> Option<ID2D1DCRenderTarget> {
        unsafe {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0, dpiY: 96.0,
                ..Default::default()
            };
            let rt = self.fac.CreateDCRenderTarget(&props).ok()?;
            // Se pinta en píxeles físicos: la escala la pone `Geo`
            rt.SetDpi(96.0, 96.0);
            // Sobre fondo transparente, ClearType deja bordes de colores
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            Some(rt)
        }
    }

    /// Bucle propio: devuelve el resultado y, si llegó un WM_QUIT, su código.
    fn run(&mut self) -> (Option<(u8, u8, u8)>, Option<i32>) {
        let mut msg = MSG::default();
        loop {
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT { return (None, Some(msg.wParam.0 as i32)); }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                // Alguien cerró la capa por su cuenta
                if !IsWindow(Some(self.overlay)).as_bool() { return (None, None); }
            }
            for ev in EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut())) { self.handle(ev); }
            self.update();
            if let Some(r) = self.finished() { return (r, None); }
            if self.dirty { self.render(); }
            unsafe {
                if self.animating() {
                    // Al ritmo de la composición: movimiento sin tirones
                    if DwmFlush().is_err() { std::thread::sleep(Duration::from_millis(8)); }
                } else {
                    // Quieto: se duerme hasta el próximo mensaje, y cada 200 ms se vuelve a mirar la
                    // pantalla (por si lo de debajo cambia, un vídeo por ejemplo)
                    if MsgWaitForMultipleObjects(None, false, 200, QS_ALLINPUT) == WAIT_TIMEOUT { self.refresh = true; }
                }
            }
        }
    }

    fn handle(&mut self, ev: Ev) {
        if self.exit.is_some() { return; }
        unsafe {
            match ev {
                Ev::Pick => {
                    let _ = GetCursorPos(&mut self.cur);
                    self.capture();
                    self.result = Some(self.center());
                    self.start_exit();
                }
                Ev::Cancel => { self.result = None; self.start_exit(); }
                Ev::Nudge(dx, dy) => {
                    let mut p = POINT::default();
                    if GetCursorPos(&mut p).is_ok() { let _ = SetCursorPos(p.x + dx, p.y + dy); }
                }
                Ev::Reframe => {
                    let (vx, vy, vw, vh) = virtual_screen();
                    let _ = SetWindowPos(self.overlay, Some(HWND_TOPMOST), vx, vy, vw, vh, SWP_NOACTIVATE);
                    self.hmon = HMONITOR::default();
                    self.refresh = true;
                }
            }
        }
    }

    fn start_exit(&mut self) {
        let (sc, a) = self.anim();
        self.exit = Some((Instant::now(), sc, a));
        self.dirty = true;
    }

    /// Escala y opacidad de ahora (entrada con rebote, salida encogiéndose).
    fn anim(&self) -> (f32, f32) {
        if let Some((t, s0, a0)) = self.exit {
            let k = ease_out(t.elapsed().as_secs_f32() * 1000.0 / EXIT_MS);
            (s0 * (1.0 - 0.3 * k), a0 * (1.0 - k))
        } else {
            let ms = self.t0.elapsed().as_secs_f32() * 1000.0;
            (0.85 + 0.15 * ease_back(ms / ENTER_MS), ease_out(ms / (ENTER_MS * 0.65)))
        }
    }

    fn entering(&self) -> bool { self.exit.is_none() && self.t0.elapsed().as_secs_f32() * 1000.0 < ENTER_MS }

    fn target(&self) -> (f32, f32) {
        let g = &self.geo;
        let (x, y) = place(self.cur.x, self.cur.y, g.d, g.body_h(), g.off, self.mon);
        ((x - g.m) as f32, (y - g.m) as f32)
    }

    fn animating(&self) -> bool {
        let t = self.target();
        self.entering() || self.exit.is_some() || (self.pos.0 - t.0).abs() > 0.5 || (self.pos.1 - t.1).abs() > 0.5
    }

    /// ¿Terminó? (la salida acabó y ya no hay botones pulsados).
    fn finished(&self) -> Option<Option<(u8, u8, u8)>> {
        let (t, _, _) = self.exit?;
        let ms = t.elapsed().as_secs_f32() * 1000.0;
        if ms < EXIT_MS { return None; }
        let held = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0 };
        if held && ms < RELEASE_WAIT_MS { return None; }
        Some(self.result)
    }

    fn update(&mut self) {
        let dt = self.last.elapsed().as_secs_f32().min(0.1);
        self.last = Instant::now();
        if self.exit.is_some() { self.dirty = true; return; }
        let mut p = POINT::default();
        if unsafe { GetCursorPos(&mut p) }.is_ok() && (p.x != self.cur.x || p.y != self.cur.y) { self.cur = p; self.refresh = true; }
        if self.refresh {
            self.refresh = false;
            self.monitor();
            if self.capture() { self.dirty = true; }
        }
        let t = self.target();
        let before = (self.pos.0.round(), self.pos.1.round());
        if self.snap || !self.glide {
            self.pos = t;
            self.snap = false;
        } else {
            let k = 1.0 - (-dt / FOLLOW_TAU).exp();
            self.pos.0 += (t.0 - self.pos.0) * k;
            self.pos.1 += (t.1 - self.pos.1) * k;
            if (t.0 - self.pos.0).abs() <= 0.5 && (t.1 - self.pos.1).abs() <= 0.5 { self.pos = t; }
        }
        if before != (self.pos.0.round(), self.pos.1.round()) || self.entering() { self.dirty = true; }
    }

    /// Monitor bajo el cursor: si cambia, la lupa se rehace a su escala y salta (sin deslizarse).
    fn monitor(&mut self) {
        unsafe {
            let hm = MonitorFromPoint(self.cur, MONITOR_DEFAULTTONEAREST);
            if hm == self.hmon && self.canvas.is_some() { return; }
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(hm, &mut mi).as_bool() { return; }
            let (mut dx, mut dy) = (96u32, 96u32);
            if GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_err() || dx == 0 { dx = 96; }
            let geo = Geo::new(dx as f32 / 96.0);
            if geo != self.geo || self.canvas.is_none() || self.fmt.is_none() {
                let Some(c) = Surface::new(geo.w, geo.h) else { return };
                self.canvas = Some(c);
                self.fmt = self.dw.CreateTextFormat(PCWSTR(self.family.as_ptr()), None, DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 13.0 * geo.s, w!("")).ok();
                self.geo = geo;
            }
            let r = mi.rcMonitor;
            self.mon = (r.left, r.top, r.right, r.bottom);
            self.hmon = hm;
            self.snap = true;
            self.dirty = true;
        }
    }

    /// Copia la zona de N×N alrededor del cursor. true = cambió algo.
    fn capture(&mut self) -> bool {
        let Some(g) = &self.grab else { return false };
        unsafe {
            if BitBlt(g.dc, 0, 0, N, N, Some(self.screen), self.cur.x - HALF, self.cur.y - HALF, SRCCOPY | CAPTUREBLT).is_err() { return false; }
            let _ = GdiFlush();
            let src = std::slice::from_raw_parts(g.bits, (N * N) as usize);
            let mut px = [0u32; (N * N) as usize];
            for (d, s) in px.iter_mut().zip(src) {
                let v = *s & 0x00FF_FFFF;
                *d = if self.compensate {
                    let ch = |sh: u32| (unblend(((v >> sh) & 0xFF) as u8) as u32) << sh;
                    ch(0) | ch(8) | ch(16)
                } else { v };
            }
            let changed = px != self.px;
            self.px = px;
            changed
        }
    }

    fn center(&self) -> (u8, u8, u8) {
        let v = self.px[(HALF * N + HALF) as usize];
        (((v >> 16) & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, (v & 0xFF) as u8)
    }

    fn render(&mut self) {
        self.dirty = false;
        if self.rt.is_none() { self.rt = self.make_rt(); }
        let (Some(rt), Some(c)) = (self.rt.clone(), self.canvas.as_ref()) else { return };
        let g = self.geo;
        let (sc, alpha) = self.anim();
        unsafe {
            let rc = RECT { left: 0, top: 0, right: g.w, bottom: g.h };
            if rt.BindDC(c.dc, &rc).is_err() { self.rt = None; self.dirty = true; return; }
            rt.BeginDraw();
            rt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
            // Escala alrededor del centro de la lupa
            let (px, py) = (g.w as f32 / 2.0, g.m as f32 + g.d as f32 / 2.0);
            rt.SetTransform(&Matrix3x2 { M11: sc, M12: 0.0, M21: 0.0, M22: sc, M31: px * (1.0 - sc), M32: py * (1.0 - sc) });
            let _ = self.draw(&rt);
            rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 });
            // Si el fotograma no sale, se rehace el render target y se repite
            if rt.EndDraw(None, None).is_err() { self.rt = None; self.dirty = true; return; }
            let dst = POINT { x: self.pos.0.round() as i32, y: self.pos.1.round() as i32 };
            let size = SIZE { cx: g.w, cy: g.h };
            let src = POINT::default();
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = UpdateLayeredWindow(self.loupe, Some(self.screen), Some(&dst), Some(&size), Some(c.dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
        }
    }

    /// Lupa + pastilla, en píxeles físicos de la ventana.
    fn draw(&self, rt: &ID2D1DCRenderTarget) -> Option<()> {
        let g = &self.geo;
        let s = g.s;
        let brush = |c: D2D1_COLOR_F| unsafe { rt.CreateSolidColorBrush(&c, None).ok() };
        let (x0, y0) = (g.m as f32, g.m as f32);
        let d = g.d as f32;
        let r = d / 2.0;
        let (cx, cy) = (x0 + r, y0 + r);
        unsafe {
            // Sombra suave: círculos concéntricos muy tenues, algo desplazados hacia abajo
            let sh = brush(rgba(0.0, 0.0, 0.0, 0.03))?;
            for i in 0..9 {
                let e = r + i as f32 * 1.2 * s;
                rt.FillEllipse(&ellipse(cx, cy + 2.5 * s, e), &sh);
            }

            // Píxeles ampliados, sin suavizado, recortados al círculo
            let props = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE }, dpiX: 96.0, dpiY: 96.0 };
            let bmp = rt.CreateBitmap(D2D_SIZE_U { width: N as u32, height: N as u32 }, Some(self.px.as_ptr() as _), (N * 4) as u32, &props).ok()?;
            let mask: ID2D1Geometry = self.fac.CreateEllipseGeometry(&ellipse(cx, cy, r)).ok()?.cast().ok()?;
            let mut lp = D2D1_LAYER_PARAMETERS {
                contentBounds: rect(-1e6, -1e6, 1e6, 1e6),
                geometricMask: ManuallyDrop::new(Some(mask)),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 },
                opacity: 1.0,
                opacityBrush: ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS_NONE,
            };
            rt.PushLayer(&lp, None);
            ManuallyDrop::drop(&mut lp.geometricMask);
            rt.DrawBitmap(&bmp, Some(&rect(x0, y0, x0 + d, y0 + d)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None);
            // Retícula fina (gris: se ve igual sobre claro que sobre oscuro)
            if let Some(grid) = brush(rgba(0.5, 0.5, 0.5, 0.26)) {
                for i in 1..N {
                    let o = (i * g.cell) as f32;
                    rt.FillRectangle(&rect(x0 + o, y0, x0 + o + 1.0, y0 + d), &grid);
                    rt.FillRectangle(&rect(x0, y0 + o, x0 + d, y0 + o + 1.0), &grid);
                }
            }
            // Píxel central: marco blanco con un filo negro por fuera (se ve sobre cualquier color)
            let (ccx, ccy, cs) = (x0 + (HALF * g.cell) as f32, y0 + (HALF * g.cell) as f32, g.cell as f32);
            let bw = (1.5 * s).max(1.5);
            if let Some(k) = brush(rgba(0.0, 0.0, 0.0, 0.6)) {
                let e = bw / 2.0 + 0.5;
                rt.DrawRectangle(&rect(ccx - e, ccy - e, ccx + cs + 1.0 + e, ccy + cs + 1.0 + e), &k, 1.0, None);
            }
            if let Some(wb) = brush(rgba(1.0, 1.0, 1.0, 0.95)) {
                rt.DrawRectangle(&rect(ccx, ccy, ccx + cs + 1.0, ccy + cs + 1.0), &wb, bw, None);
            }
            rt.PopLayer();

            // Aro blanco con filos oscuros dentro y fuera
            let ring = 3.0 * s;
            rt.DrawEllipse(&ellipse(cx, cy, r - ring / 2.0), &brush(rgba(1.0, 1.0, 1.0, 0.95))?, ring, None);
            if let Some(k) = brush(rgba(0.0, 0.0, 0.0, 0.18)) { rt.DrawEllipse(&ellipse(cx, cy, r - ring - 0.5), &k, 1.0, None); }
            if let Some(k) = brush(rgba(0.0, 0.0, 0.0, 0.35)) { rt.DrawEllipse(&ellipse(cx, cy, r + 0.5), &k, 1.0, None); }

            // Pastilla: muestra de color + hex
            let (cr, cg, cb) = self.center();
            let txt: Vec<u16> = hex(cr, cg, cb).encode_utf16().collect();
            let fmt = self.fmt.as_ref()?;
            let layout = self.dw.CreateTextLayout(&txt, fmt, 1000.0, 200.0).ok()?;
            let mut tm = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut tm).ok()?;
            let (pad, sw, sgap) = (10.0 * s, 14.0 * s, 7.0 * s);
            let pw = pad + sw + sgap + tm.widthIncludingTrailingWhitespace + pad * 1.15;
            let ph = g.pill_h as f32;
            let (pl, pt) = ((cx - pw / 2.0).round(), y0 + d + g.gap as f32);
            let pill = rect(pl, pt, pl + pw, pt + ph);
            if let Some(psh) = brush(rgba(0.0, 0.0, 0.0, 0.035)) {
                for i in 0..6 {
                    let e = i as f32 * s;
                    rt.FillRoundedRectangle(&rrect(rect(pill.left - e, pill.top - e + 2.0 * s, pill.right + e, pill.bottom + e + 2.0 * s), ph / 2.0 + e), &psh);
                }
            }
            rt.FillRoundedRectangle(&rrect(pill, ph / 2.0), &brush(rgba(0.11, 0.11, 0.12, 0.86))?);
            if let Some(edge) = brush(rgba(1.0, 1.0, 1.0, 0.12)) {
                rt.DrawRoundedRectangle(&rrect(rect(pill.left + 0.5, pill.top + 0.5, pill.right - 0.5, pill.bottom - 0.5), ph / 2.0 - 0.5), &edge, 1.0, None);
            }
            let (scx, scy) = (pl + pad + sw / 2.0, pt + ph / 2.0);
            rt.FillEllipse(&ellipse(scx, scy, sw / 2.0), &brush(rgba(cr as f32 / 255.0, cg as f32 / 255.0, cb as f32 / 255.0, 1.0))?);
            if let Some(k) = brush(rgba(1.0, 1.0, 1.0, 0.55)) { rt.DrawEllipse(&ellipse(scx, scy, sw / 2.0), &k, 1.0, None); }
            let ink = brush(rgba(1.0, 1.0, 1.0, 0.95))?;
            rt.DrawTextLayout(Vector2 { X: pl + pad + sw + sgap, Y: (pt + (ph - tm.height) / 2.0).round() }, &layout, &ink, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
        Some(())
    }
}

impl Drop for Picker {
    fn drop(&mut self) {
        unsafe {
            if let Some(h) = self.hook.take() { let _ = UnhookWindowsHookEx(h); }
            self.rt = None;
            self.canvas = None;
            self.grab = None;
            if GetCapture() == self.overlay && !self.overlay.is_invalid() { let _ = ReleaseCapture(); }
            if !self.loupe.is_invalid() { let _ = DestroyWindow(self.loupe); }
            if !self.overlay.is_invalid() { let _ = DestroyWindow(self.overlay); }
            if !self.screen.is_invalid() { ReleaseDC(None, self.screen); }
            if !self.prev_dpi.0.is_null() { SetThreadDpiAwarenessContext(self.prev_dpi); }
        }
        EVENTS.with(|e| e.borrow_mut().clear());
    }
}

/// Escritorio virtual (todos los monitores), en píxeles físicos.
fn virtual_screen() -> (i32, i32, i32, i32) {
    unsafe {
        (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN),
         GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1), GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1))
    }
}

/// Segoe UI Variable (Windows 11) si está; si no, Segoe UI. Terminada en 0, lista para PCWSTR.
fn pick_family(dw: &IDWriteFactory) -> Vec<u16> {
    let want = "Segoe UI Variable Text";
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let found = unsafe {
        let mut col: Option<IDWriteFontCollection> = None;
        dw.GetSystemFontCollection(&mut col, false).ok().and(col).is_some_and(|c| {
            let (mut idx, mut ok) = (0u32, BOOL(0));
            let name = wide(want);
            c.FindFamilyName(PCWSTR(name.as_ptr()), &mut idx, &mut ok).is_ok() && ok.as_bool()
        })
    };
    wide(if found { want } else { "Segoe UI" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_en_mayusculas_y_con_ceros() {
        assert_eq!(hex(0, 0, 0), "#000000");
        assert_eq!(hex(255, 255, 255), "#FFFFFF");
        assert_eq!(hex(0x1A, 0x0B, 0xC3), "#1A0BC3");
        assert_eq!(hex(1, 2, 3), "#010203");
    }

    const MON: (i32, i32, i32, i32) = (0, 0, 1920, 1080);

    #[test]
    fn lupa_abajo_a_la_derecha_si_cabe() {
        assert_eq!(place(500, 400, 130, 166, 20, MON), (520, 420));
    }

    #[test]
    fn lupa_salta_al_otro_lado_en_los_bordes() {
        // Borde derecho: a la izquierda del cursor
        assert_eq!(place(1900, 400, 130, 166, 20, MON), (1750, 420));
        // Borde de abajo: encima del cursor
        assert_eq!(place(500, 1070, 130, 166, 20, MON), (520, 884));
        // Esquina de abajo a la derecha: arriba a la izquierda
        assert_eq!(place(1919, 1079, 130, 166, 20, MON), (1769, 893));
    }

    #[test]
    fn lupa_no_sale_del_monitor_ni_tapa_el_cursor() {
        // Monitor secundario a la izquierda, con coordenadas negativas
        let mon = (-1280, -200, 0, 824);
        for &(cx, cy) in &[(-1280, -200), (-1, 823), (-640, 300), (-1279, 823), (-1, -200)] {
            let (x, y) = place(cx, cy, 130, 166, 20, mon);
            assert!(x >= mon.0 && x + 130 <= mon.2, "x fuera: {x} para {cx},{cy}");
            assert!(y >= mon.1 && y + 166 <= mon.3, "y fuera: {y} para {cx},{cy}");
            // El píxel central (y su vecindad capturada) nunca queda debajo de la lupa
            let dentro = cx + HALF >= x && cx - HALF < x + 130 && cy + HALF >= y && cy - HALF < y + 166;
            assert!(!dentro, "la lupa tapa el cursor en {cx},{cy}");
        }
    }

    #[test]
    fn lupa_en_monitor_diminuto_se_encaja() {
        assert_eq!(place(50, 50, 130, 166, 20, (0, 0, 100, 100)), (0, 0));
    }

    #[test]
    fn velo_se_deshace() {
        assert_eq!(unblend(0), 0);
        assert_eq!(unblend(254), 255);
        assert_eq!(unblend(255), 255);
        // 128 * 254/255 ≈ 127,5 → la pantalla da 127 o 128; ambos vuelven a ~128
        assert!((127..=129).contains(&unblend(127)));
    }

    #[test]
    fn geometria_entera_a_cualquier_escala() {
        for s in [1.0, 1.25, 1.5, 1.75, 2.0, 3.0] {
            let g = Geo::new(s);
            assert_eq!(g.d, g.cell * N);
            assert_eq!(g.w, g.d + 2 * g.m);
        }
        assert_eq!(Geo::new(1.0).d, 130);
    }
}
