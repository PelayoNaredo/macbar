//! Cesta flotante (como Dropover o Droppy): mientras se arrastran archivos, si se agita el cursor
//! aparece a su lado una tarjeta donde soltarlos; lo soltado va a la bandeja de la isla.
//!
//! Detección sin ganchos globales: un hilo mira cada 25 ms si el botón principal está pulsado (si no
//! lo está, no hace nada más) y, con él pulsado, cuenta los cambios de sentido horizontales del
//! cursor. Para no saltar al seleccionar texto o mover ventanas se exige además una señal de arrastre
//! OLE: la imagen de arrastre del shell (`SysDragImage`) visible o que el cursor sea uno de los de
//! arrastrar y soltar de ole32 (por handle o, de otro proceso, por la huella de sus bits). Sin señal
//! clara (y sin cursor normal del sistema ni ventana moviéndose) solo vale un agitado más largo.
//!
//! La tarjeta es una ventana capa con alfa por píxel pintada con Direct2D sobre un DIB y subida con
//! `UpdateLayeredWindow` (igual que la lupa del cuentagotas), registrada como destino OLE propio.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::{Duration, Instant};
use windows::core::{implement, w, Ref, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::Matrix3x2;

// ---------------------------------------------------------------- ajustes

/// Cada cuánto mira el vigilante el ratón.
const POLL_MS: u64 = 25;
/// Recorrido mínimo (px físicos) de cada tramo para que cuente un cambio de sentido.
const MIN_RUN: i32 = 40;
/// Agitado normal (con señal de arrastre): estos cambios de sentido en esta ventana de tiempo.
const FAST_TURNS: usize = 3;
const FAST_MS: u64 = 700;
/// Agitado estricto (sin señal clara de arrastre).
const STRICT_TURNS: usize = 5;
const STRICT_MS: u64 = 1000;

/// Tarjeta (DIP).
const CARD_W: f32 = 150.0;
const CARD_H: f32 = 130.0;
const RADIUS: f32 = 18.0;
/// Margen transparente alrededor de la tarjeta (sombra, rebote y respiración).
const MARGIN: f32 = 26.0;
/// Separación entre el cursor y la tarjeta.
const OFFSET: f32 = 28.0;

const ENTER_MS: f32 = 200.0;
const EXIT_MS: f32 = 120.0;
/// Si el arrastre no entra en este tiempo, se va.
const IDLE_MS: f32 = 4000.0;
/// Tiempo que se ve el «tick» tras soltar antes del fundido.
const DONE_HOLD_MS: f32 = 420.0;
/// Con el arrastre encima y el botón ya suelto, lo que se espera a que llegue el Drop.
const DROP_WAIT_MS: f32 = 600.0;
/// Periodo de la respiración (s).
const BREATH_S: f32 = 1.4;

const WM_BASKET_SHOW: u32 = WM_APP + 0x42;
const TIMER_ID: usize = 1;

/// Glifos de Segoe Fluent Icons: bandeja y tick.
const GLYPH_TRAY: &str = "\u{E8B7}";
const GLYPH_CHECK: &str = "\u{E73E}";

static STARTED: AtomicBool = AtomicBool::new(false);
/// Ventana de la cesta (para el aviso del vigilante).
static HWND_BASKET: AtomicIsize = AtomicIsize::new(0);
/// La cesta está en pantalla (el vigilante no vuelve a avisar).
static SHOWN: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------- funciones puras

/// Detector de agitado: cuenta cambios de sentido horizontales con tramos de al menos `MIN_RUN`.
#[derive(Default, Debug)]
struct Shake {
    started: bool,
    /// Sentido del tramo actual (-1, 0 = aún sin sentido, 1).
    dir: i32,
    /// Punto más lejano del tramo actual (o el de partida mientras no hay sentido).
    ext: i32,
    /// Momentos (ms) de los cambios de sentido recientes.
    turns: VecDeque<u64>,
}

impl Shake {
    fn reset(&mut self) { *self = Shake::default(); }

    /// Añade una muestra (x del cursor en px físicos, t en ms).
    fn feed(&mut self, x: i32, t: u64) {
        while self.turns.front().is_some_and(|&o| t.saturating_sub(o) > STRICT_MS.max(FAST_MS)) { self.turns.pop_front(); }
        if !self.started {
            self.started = true;
            self.ext = x;
            return;
        }
        match self.dir {
            0 => {
                if (x - self.ext).abs() >= MIN_RUN { self.dir = (x - self.ext).signum(); self.ext = x; }
            }
            d => {
                if (x - self.ext) * d > 0 {
                    self.ext = x;
                } else if (self.ext - x) * d >= MIN_RUN {
                    // Volvió MIN_RUN desde el extremo: cambio de sentido
                    self.turns.push_back(t);
                    self.dir = -d;
                    self.ext = x;
                }
            }
        }
    }

    /// Cambios de sentido en los últimos `window` ms.
    fn turns(&self, now: u64, window: u64) -> usize { self.turns.iter().filter(|&&o| now.saturating_sub(o) <= window).count() }
}

/// Qué dice el sistema sobre lo que se está haciendo con el botón pulsado.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Signal {
    /// Hay un arrastre OLE en curso (imagen de arrastre o cursor de arrastrar y soltar).
    Drag,
    /// Seguro que no: cursor normal (flecha, texto, mano, redimensionar…), oculto o ventana moviéndose.
    Plain,
    /// No se sabe (cursor propio de alguna app).
    Unknown,
}

/// ¿Hay que enseñar la cesta? `fast`/`slow` = cambios de sentido en `FAST_MS`/`STRICT_MS`.
fn should_show(fast: usize, slow: usize, sig: Signal) -> bool {
    match sig {
        Signal::Drag => fast >= FAST_TURNS,
        Signal::Unknown => slow >= STRICT_TURNS,
        Signal::Plain => false,
    }
}

/// Esquina superior izquierda de la tarjeta (`w`×`h`) para un cursor en (`cx`, `cy`): a la derecha
/// del cursor y a su altura, separada `off`; si no cabe, a la izquierda; al final se encaja en `area`
/// = (izq, arriba, der, abajo).
fn place(cx: i32, cy: i32, w: i32, h: i32, off: i32, area: (i32, i32, i32, i32)) -> (i32, i32) {
    let (l, t, r, b) = area;
    let mut x = cx + off;
    if x + w > r { x = cx - off - w; }
    let y = cy - h / 2;
    (x.min(r - w).max(l), y.min(b - h).max(t))
}

/// ¿Se cierra ya? (sin soltar nada encima). `up_ms` = tiempo desde que se vio el botón suelto.
fn close_now(shown_ms: f32, entered: bool, hover: bool, down: bool, up_ms: Option<f32>) -> bool {
    if !entered && shown_ms >= IDLE_MS { return true; }
    if down { return false; }
    // Botón suelto: si el arrastre estaba encima, se da un momento a que llegue el Drop
    !(hover && up_ms.unwrap_or(0.0) < DROP_WAIT_MS)
}

fn ease_out(t: f32) -> f32 { let u = 1.0 - t.clamp(0.0, 1.0); 1.0 - u * u * u }
fn ease_back(t: f32) -> f32 { let t = t.clamp(0.0, 1.0) - 1.0; 1.0 + 2.70158 * t * t * t + 1.70158 * t * t }

/// Entrada: (escala, opacidad) a los `ms` de aparecer (0,8 → 1 con un pelín de rebote, y fundido).
fn enter(ms: f32) -> (f32, f32) { (0.8 + 0.2 * ease_back(ms / ENTER_MS), ease_out(ms / (ENTER_MS * 0.7))) }

/// Respiración con el arrastre encima: multiplicador de escala (`k` = cuánto está encima, 0..1).
fn breathe(t_s: f32, k: f32) -> f32 { 1.0 + 0.03 * k.clamp(0.0, 1.0) * (0.5 - 0.5 * (std::f32::consts::TAU * t_s / BREATH_S).cos()) }

/// Huella FNV-1a (para comparar cursores de otros procesos por sus bits).
fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes { h ^= b as u64; h = h.wrapping_mul(0x0000_0100_0000_01B3); }
    h
}
const FNV_SEED: u64 = 0xCBF2_9CE4_8422_2325;

/// Medidas en píxeles físicos para una escala de monitor.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Geo { s: f32, cw: i32, ch: i32, m: i32, off: i32, w: i32, h: i32 }

impl Geo {
    fn new(s: f32) -> Geo {
        let px = |v: f32| (v * s).round() as i32;
        let (cw, ch, m) = (px(CARD_W), px(CARD_H), px(MARGIN));
        Geo { s, cw, ch, m, off: px(OFFSET), w: cw + 2 * m, h: ch + 2 * m }
    }
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F { D2D1_COLOR_F { r, g, b, a } }
fn mix(a: D2D1_COLOR_F, b: D2D1_COLOR_F, k: f32) -> D2D1_COLOR_F {
    let k = k.clamp(0.0, 1.0);
    rgba(a.r + (b.r - a.r) * k, a.g + (b.g - a.g) * k, a.b + (b.b - a.b) * k, a.a + (b.a - a.a) * k)
}
fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F { D2D_RECT_F { left: l, top: t, right: r, bottom: b } }
fn rrect(r: D2D_RECT_F, rad: f32) -> D2D1_ROUNDED_RECT { D2D1_ROUNDED_RECT { rect: r, radiusX: rad, radiusY: rad } }
fn ellipse(x: f32, y: f32, r: f32) -> D2D1_ELLIPSE { D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: x, Y: y }, radiusX: r, radiusY: r } }
/// Escala `k` alrededor de (`x`, `y`).
fn scale_about(k: f32, x: f32, y: f32) -> Matrix3x2 { Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: x * (1.0 - k), M32: y * (1.0 - k) } }
/// `a` y después `b`.
fn mul(a: &Matrix3x2, b: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: a.M11 * b.M11 + a.M12 * b.M21,
        M12: a.M11 * b.M12 + a.M12 * b.M22,
        M21: a.M21 * b.M11 + a.M22 * b.M21,
        M22: a.M21 * b.M12 + a.M22 * b.M22,
        M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31,
        M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    }
}
fn wide(s: &str) -> Vec<u16> { s.encode_utf16().collect() }

// ---------------------------------------------------------------- vigilante (su propio hilo)

/// ¿Está pulsado el botón principal? (GetAsyncKeyState mira el botón físico: con los botones
/// cambiados, el principal es el derecho).
fn primary_down() -> bool {
    unsafe {
        let vk = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
        GetAsyncKeyState(vk.0 as i32) < 0
    }
}

/// Cursores conocidos para leer la señal de arrastre.
struct Cursors {
    /// Cursores normales del sistema (sus handles son globales).
    plain: Vec<isize>,
    /// Cursores de arrastre de ole32 cargados en este proceso.
    drag: Vec<isize>,
    /// Huellas de los cursores de arrastre de ole32 a varios tamaños (ordenadas).
    prints: Vec<u64>,
    /// Último cursor mirado por huella y lo que salió (para no repetir el trabajo).
    last: (isize, Signal),
}

impl Cursors {
    fn load() -> Cursors {
        let mut c = Cursors { plain: Vec::new(), drag: Vec::new(), prints: Vec::new(), last: (0, Signal::Unknown) };
        unsafe {
            for id in [IDC_ARROW, IDC_IBEAM, IDC_HAND, IDC_SIZEALL, IDC_SIZEWE, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZENESW, IDC_CROSS, IDC_WAIT, IDC_APPSTARTING, IDC_UPARROW, IDC_HELP, IDC_PIN, IDC_PERSON] {
                if let Ok(h) = LoadCursorW(None, id) { c.plain.push(h.0 as isize); }
            }
            // ole32 trae los cursores de DoDragDrop como recursos 1..7 (no permitido, mover, copiar, enlace…)
            if let Ok(m) = GetModuleHandleW(w!("ole32.dll")) {
                let hi: HINSTANCE = m.into();
                for id in 1..=7usize {
                    let name = PCWSTR(id as *const u16);
                    if let Ok(h) = LoadCursorW(Some(hi), name) {
                        c.drag.push(h.0 as isize);
                        if let Some(p) = print(h) { c.prints.push(p); }
                    }
                    // Otro proceso los carga al tamaño de su escala: huellas a los tamaños habituales
                    for sz in (16..=256).step_by(8) {
                        if let Ok(h) = LoadImageW(Some(hi), name, IMAGE_CURSOR, sz, sz, LR_DEFAULTCOLOR) {
                            let hc = HCURSOR(h.0);
                            if let Some(p) = print(hc) { c.prints.push(p); }
                            let _ = DestroyCursor(hc);
                        }
                    }
                }
            }
        }
        c.prints.sort_unstable();
        c.prints.dedup();
        c
    }

    fn signal(&mut self) -> Signal {
        unsafe {
            // Moviendo o redimensionando una ventana (agitarla minimiza las demás en Windows)
            let mut gi = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            if GetGUIThreadInfo(0, &mut gi).is_ok() && gi.flags.0 & GUI_INMOVESIZE.0 != 0 { return Signal::Plain; }
            let mut ci = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
            if GetCursorInfo(&mut ci).is_err() { return Signal::Unknown; }
            let h = ci.hCursor.0 as isize;
            if ci.flags.0 & CURSOR_SHOWING.0 == 0 || h == 0 || self.plain.contains(&h) { return Signal::Plain; }
            if drag_image_visible() || self.drag.contains(&h) { return Signal::Drag; }
            if self.last.0 == h { return self.last.1; }
            let sig = match print(ci.hCursor) {
                Some(p) if self.prints.binary_search(&p).is_ok() => Signal::Drag,
                _ => Signal::Unknown,
            };
            self.last = (h, sig);
            sig
        }
    }
}

/// ¿Hay una imagen de arrastre del shell en pantalla? (la crea quien arrastra con DragDropHelper).
fn drag_image_visible() -> bool {
    unsafe {
        let mut after: Option<HWND> = None;
        for _ in 0..16 {
            match FindWindowExW(None, after, w!("SysDragImage"), None) {
                Ok(h) if !h.0.is_null() => {
                    if IsWindowVisible(h).as_bool() { return true; }
                    after = Some(h);
                }
                _ => return false,
            }
        }
        false
    }
}

/// Huella de la forma de un cursor (máscara, color y punto activo).
fn print(h: HCURSOR) -> Option<u64> {
    unsafe {
        let mut ii = ICONINFO::default();
        GetIconInfo(HICON(h.0), &mut ii).ok()?;
        let mut hash = fnv(FNV_SEED, &[ii.xHotspot.to_le_bytes(), ii.yHotspot.to_le_bytes()].concat());
        let ok = bitmap_into(ii.hbmMask, &mut hash) && bitmap_into(ii.hbmColor, &mut hash);
        if !ii.hbmMask.is_invalid() { let _ = DeleteObject(ii.hbmMask.into()); }
        if !ii.hbmColor.is_invalid() { let _ = DeleteObject(ii.hbmColor.into()); }
        ok.then_some(hash)
    }
}

/// Mete en la huella las medidas y los bits de un mapa de bits (vacío = nada que meter).
fn bitmap_into(hb: HBITMAP, hash: &mut u64) -> bool {
    if hb.is_invalid() { return true; }
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(hb.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut BITMAP as *mut _)) == 0 { return false; }
        let n = (bm.bmWidthBytes.max(0) as usize).saturating_mul(bm.bmHeight.max(0) as usize);
        if n == 0 || n > 4 << 20 { return false; }
        let mut buf = vec![0u8; n];
        let got = GetBitmapBits(hb, n as i32, buf.as_mut_ptr() as *mut _);
        if got <= 0 { return false; }
        *hash = fnv(*hash, &[bm.bmWidth.to_le_bytes(), bm.bmHeight.to_le_bytes(), (bm.bmBitsPixel as i32).to_le_bytes()].concat());
        *hash = fnv(*hash, &buf[..got as usize]);
        true
    }
}

/// Bucle del vigilante: con el botón suelto solo duerme y mira el botón.
fn watch() {
    let t0 = Instant::now();
    let mut shake = Shake::default();
    let (mut was_down, mut fired) = (false, false);
    let mut cursors: Option<Cursors> = None;
    loop {
        std::thread::sleep(Duration::from_millis(POLL_MS));
        if !primary_down() {
            if was_down { shake.reset(); was_down = false; fired = false; }
            continue;
        }
        if !was_down { was_down = true; shake.reset(); }
        // Una vez por pulsación
        if fired || SHOWN.load(Ordering::Relaxed) { continue; }
        let mut p = POINT::default();
        if unsafe { GetCursorPos(&mut p) }.is_err() { continue; }
        let now = t0.elapsed().as_millis() as u64;
        shake.feed(p.x, now);
        let fast = shake.turns(now, FAST_MS);
        if fast < FAST_TURNS { continue; }
        if crate::profiles::presenting() || crate::fullscreen() { fired = true; continue; }
        let sig = cursors.get_or_insert_with(Cursors::load).signal();
        if !should_show(fast, shake.turns(now, STRICT_MS), sig) { continue; }
        fired = true;
        let hwnd = HWND(HWND_BASKET.load(Ordering::Relaxed) as *mut _);
        if hwnd.0.is_null() { continue; }
        unsafe { let _ = PostMessageW(Some(hwnd), WM_BASKET_SHOW, WPARAM(p.x as isize as usize), LPARAM(p.y as isize)); }
    }
}

// ---------------------------------------------------------------- destino de soltar

/// Lo que cuenta el destino OLE (mismo hilo que la ventana).
struct Flags { hover: Cell<bool>, entered: Cell<bool>, dropped: Cell<bool> }

thread_local! {
    static FLAGS: Flags = const { Flags { hover: Cell::new(false), entered: Cell::new(false), dropped: Cell::new(false) } };
    static STATE: RefCell<Option<Basket>> = const { RefCell::new(None) };
}

fn reset_flags() { FLAGS.with(|f| { f.hover.set(false); f.entered.set(false); f.dropped.set(false); }); }

#[implement(IDropTarget)]
struct Target { files: Cell<bool> }

fn set_effect(effect: *mut DROPEFFECT, ok: bool) {
    if !effect.is_null() { unsafe { *effect = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE }; } }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let files = data.as_ref().is_some_and(crate::dnd::has_files);
        self.files.set(files);
        FLAGS.with(|f| { f.entered.set(true); f.hover.set(files); });
        set_effect(effect, files);
        Ok(())
    }
    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        set_effect(effect, self.files.get());
        Ok(())
    }
    fn DragLeave(&self) -> windows::core::Result<()> {
        FLAGS.with(|f| f.hover.set(false));
        Ok(())
    }
    fn Drop(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let paths = data.as_ref().map(crate::dnd::paths_of).unwrap_or_default();
        let ok = !paths.is_empty();
        FLAGS.with(|f| f.hover.set(false));
        if ok {
            crate::live::shelf_add(paths);
            FLAGS.with(|f| f.dropped.set(true));
        }
        set_effect(effect, ok);
        Ok(())
    }
}

// ---------------------------------------------------------------- la cesta (hilo de la interfaz)

/// DIB de 32 bits (de arriba abajo) seleccionado en un DC de memoria.
struct Surface { dc: HDC, bmp: HBITMAP, old: HGDIOBJ }

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
            Some(Surface { dc, bmp, old })
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

struct Basket {
    hwnd: HWND,
    fac: ID2D1Factory,
    dw: IDWriteFactory,
    family: Vec<u16>,
    rt: Option<ID2D1DCRenderTarget>,
    canvas: Option<Surface>,
    geo: Geo,
    text: Option<IDWriteTextFormat>,
    icon: Option<IDWriteTextFormat>,
    check: Option<IDWriteTextFormat>,
    dash: Option<ID2D1StrokeStyle>,
    /// Esquina de la ventana (px físicos).
    pos: POINT,
    visible: bool,
    t0: Instant,
    last: Instant,
    /// Cuánto está el arrastre encima (0..1, suavizado).
    hover_k: f32,
    up_at: Option<Instant>,
    done_at: Option<Instant>,
    /// Salida en curso: (inicio, escala y opacidad de partida).
    exit: Option<(Instant, f32, f32)>,
}

fn ms_since(t: Instant) -> f32 { t.elapsed().as_secs_f32() * 1000.0 }

impl Basket {
    fn new(hwnd: HWND, fac: ID2D1Factory, dw: IDWriteFactory) -> Basket {
        let family = pick_family(&dw);
        let dash = unsafe {
            fac.CreateStrokeStyle(&D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, dashCap: D2D1_CAP_STYLE_ROUND,
                dashStyle: D2D1_DASH_STYLE_CUSTOM, ..Default::default()
            }, Some(&[2.6, 2.6])).ok()
        };
        let now = Instant::now();
        Basket {
            hwnd, fac, dw, family, rt: None, canvas: None, geo: Geo::new(1.0), text: None, icon: None, check: None, dash,
            pos: POINT::default(), visible: false, t0: now, last: now, hover_k: 0.0, up_at: None, done_at: None, exit: None,
        }
    }

    /// Rehace lienzo y tipos de letra para la escala `s` (si hace falta).
    fn rescale(&mut self, s: f32) -> bool {
        let geo = Geo::new(s);
        if geo == self.geo && self.canvas.is_some() && self.text.is_some() && self.icon.is_some() && self.check.is_some() { return true; }
        let Some(c) = Surface::new(geo.w, geo.h) else { return false };
        let fmt = |family: PCWSTR, weight: DWRITE_FONT_WEIGHT, size: f32| unsafe {
            let f = self.dw.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("")).ok()?;
            let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            Some(f)
        };
        self.text = fmt(PCWSTR(self.family.as_ptr()), DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.0 * s);
        self.icon = fmt(w!("Segoe Fluent Icons"), DWRITE_FONT_WEIGHT_REGULAR, 32.0 * s);
        self.check = fmt(w!("Segoe Fluent Icons"), DWRITE_FONT_WEIGHT_REGULAR, 20.0 * s);
        self.canvas = Some(c);
        self.geo = geo;
        self.text.is_some() && self.icon.is_some() && self.check.is_some()
    }

    fn show(&mut self, cx: i32, cy: i32) {
        if self.visible && self.exit.is_none() { return; }
        if crate::profiles::presenting() || crate::fullscreen() { self.hide(); return; }
        unsafe {
            let hm = MonitorFromPoint(POINT { x: cx, y: cy }, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(hm, &mut mi).as_bool() { self.hide(); return; }
            let (mut dx, mut dy) = (96u32, 96u32);
            if GetDpiForMonitor(hm, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_err() || dx == 0 { dx = 96; }
            if !self.rescale(dx as f32 / 96.0) { self.hide(); return; }
            let g = self.geo;
            let wa = mi.rcWork;
            let (x, y) = place(cx, cy, g.cw, g.ch, g.off, (wa.left, wa.top, wa.right, wa.bottom));
            self.pos = POINT { x: x - g.m, y: y - g.m };
            reset_flags();
            let now = Instant::now();
            (self.t0, self.last, self.hover_k, self.up_at, self.done_at, self.exit) = (now, now, 0.0, None, None, None);
            // Primer fotograma (opacidad 0) antes de enseñarla
            self.render();
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            SetTimer(Some(self.hwnd), TIMER_ID, 15, None);
        }
        self.visible = true;
        SHOWN.store(true, Ordering::Relaxed);
    }

    fn hide(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ID);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.visible = false;
        self.exit = None;
        self.done_at = None;
        reset_flags();
        SHOWN.store(false, Ordering::Relaxed);
    }

    fn start_exit(&mut self) {
        if self.exit.is_some() { return; }
        let (sc, a) = self.anim();
        self.exit = Some((Instant::now(), sc, a));
    }

    /// Escala y opacidad de ahora.
    fn anim(&self) -> (f32, f32) {
        if let Some((t, s0, a0)) = self.exit {
            let k = ease_out(ms_since(t) / EXIT_MS);
            return (s0 * (1.0 - 0.04 * k), a0 * (1.0 - k));
        }
        let (sc, a) = enter(ms_since(self.t0));
        (sc * breathe(self.t0.elapsed().as_secs_f32(), self.hover_k), a)
    }

    fn tick(&mut self) {
        if !self.visible { unsafe { let _ = KillTimer(Some(self.hwnd), TIMER_ID); } return; }
        let dt = self.last.elapsed().as_secs_f32().min(0.1);
        self.last = Instant::now();
        if let Some((t, _, _)) = self.exit {
            if ms_since(t) >= EXIT_MS { self.hide(); } else { self.render(); }
            return;
        }
        let (hover, entered, dropped) = FLAGS.with(|f| (f.hover.get(), f.entered.get(), f.dropped.get()));
        // Soltado encima: tick y, al rato, fundido
        if dropped && self.done_at.is_none() { self.done_at = Some(Instant::now()); }
        if let Some(d) = self.done_at {
            if ms_since(d) >= DONE_HOLD_MS { self.start_exit(); }
            self.render();
            return;
        }
        // Prueba visual (MACBAR_BASKETTEST): como si siguiera el arrastre, con el ratón encima del segundo 1,5 al 3
        let test = std::env::var_os("MACBAR_BASKETTEST").is_some();
        if test { let ms = ms_since(self.t0); FLAGS.with(|f| { f.hover.set((1500.0..3000.0).contains(&ms)); f.entered.set(true); }); }
        let hover = if test { FLAGS.with(|f| f.hover.get()) } else { hover };
        let down = primary_down() || (test && ms_since(self.t0) < 3800.0);
        if down { self.up_at = None; } else if self.up_at.is_none() { self.up_at = Some(Instant::now()); }
        if crate::profiles::presenting() || crate::fullscreen() || close_now(ms_since(self.t0), entered, hover, down, self.up_at.map(ms_since)) {
            self.start_exit();
        }
        let target = if hover { 1.0 } else { 0.0 };
        let k = 1.0 - (-dt / 0.07).exp();
        self.hover_k += (target - self.hover_k) * k;
        if (target - self.hover_k).abs() < 0.002 { self.hover_k = target; }
        if ms_since(self.t0) < ENTER_MS + 30.0 || self.hover_k > 0.0 || self.exit.is_some() { self.render(); }
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

    fn render(&mut self) {
        if self.rt.is_none() { self.rt = self.make_rt(); }
        let (Some(rt), Some(c)) = (self.rt.clone(), self.canvas.as_ref()) else { return };
        let g = self.geo;
        let (sc, alpha) = self.anim();
        unsafe {
            let rc = RECT { left: 0, top: 0, right: g.w, bottom: g.h };
            if rt.BindDC(c.dc, &rc).is_err() { self.rt = None; return; }
            rt.BeginDraw();
            rt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
            let base = scale_about(sc, g.w as f32 / 2.0, g.h as f32 / 2.0);
            rt.SetTransform(&base);
            let _ = self.draw(&rt, &base);
            rt.SetTransform(&scale_about(1.0, 0.0, 0.0));
            // Si el fotograma no sale, se rehace el render target en el siguiente
            if rt.EndDraw(None, None).is_err() { self.rt = None; return; }
            let size = SIZE { cx: g.w, cy: g.h };
            let src = POINT::default();
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = UpdateLayeredWindow(self.hwnd, None, Some(&self.pos), Some(&size), Some(c.dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
        }
    }

    /// Tarjeta, en píxeles físicos de la ventana (`base` = transformación de la escala general).
    fn draw(&self, rt: &ID2D1DCRenderTarget, base: &Matrix3x2) -> Option<()> {
        let g = &self.geo;
        let s = g.s;
        let hk = self.hover_k;
        let brush = |c: D2D1_COLOR_F| unsafe { rt.CreateSolidColorBrush(&c, None).ok() };
        let (l, t) = (g.m as f32, g.m as f32);
        let (r, b) = (l + g.cw as f32, t + g.ch as f32);
        let (cx, cw, ch) = ((l + r) / 2.0, g.cw as f32, g.ch as f32);
        let rad = RADIUS * s;
        // Tras soltar: el icono y el texto se van y entra el tick
        let done_ms = self.done_at.map(ms_since);
        let fade = done_ms.map_or(1.0, |m| 1.0 - ease_out(m / 120.0));
        unsafe {
            // Sombra suave: capas muy tenues, algo desplazadas hacia abajo
            if let Some(sh) = brush(rgba(0.0, 0.0, 0.0, 0.028)) {
                for i in 0..10 {
                    let e = i as f32 * 1.4 * s;
                    rt.FillRoundedRectangle(&rrect(rect(l - e, t - e + 3.0 * s, r + e, b + e + 3.0 * s), rad + e), &sh);
                }
            }
            // Halo del acento con el arrastre encima
            if hk > 0.0 {
                if let Some(gl) = brush(crate::profiles::accent(0.05 * hk)) {
                    for i in 1..6 {
                        let e = i as f32 * 1.5 * s;
                        rt.FillRoundedRectangle(&rrect(rect(l - e, t - e, r + e, b + e), rad + e), &gl);
                    }
                }
            }
            let card = rect(l, t, r, b);
            rt.FillRoundedRectangle(&rrect(card, rad), &brush(rgba(28.0 / 255.0, 28.0 / 255.0, 30.0 / 255.0, 0.985))?);
            if let Some(edge) = brush(rgba(1.0, 1.0, 1.0, 0.09)) {
                rt.DrawRoundedRectangle(&rrect(rect(l + 0.5, t + 0.5, r - 0.5, b - 0.5), rad - 0.5), &edge, 1.0, None);
            }

            // Borde discontinuo interior: gris en reposo, acento con el arrastre encima
            let ins = 10.0 * s;
            let inner = rect(l + ins, t + ins, r - ins, b - ins);
            let irad = (rad - ins).max(6.0 * s);
            if hk > 0.0 {
                if let Some(f) = brush(crate::profiles::accent(0.12 * hk)) { rt.FillRoundedRectangle(&rrect(inner, irad), &f); }
            }
            let line = mix(rgba(1.0, 1.0, 1.0, 0.26), crate::profiles::accent(0.95), hk);
            if let Some(br) = brush(line) {
                rt.DrawRoundedRectangle(&rrect(inner, irad), &br, (1.5 + 0.5 * hk) * s, self.dash.as_ref());
            }

            // Icono de bandeja y «Suelta aquí»
            let icy = t + ch * 0.42;
            if fade > 0.0 {
                if let (Some(fmt), Some(ib)) = (self.icon.as_ref(), brush(mix(rgba(1.0, 1.0, 1.0, 0.88), crate::profiles::accent(1.0), hk))) {
                    ib.SetOpacity(fade);
                    rt.DrawText(&wide(GLYPH_TRAY), fmt, &rect(l, icy - 24.0 * s, r, icy + 24.0 * s), &ib, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                }
                if let (Some(fmt), Some(tb)) = (self.text.as_ref(), brush(rgba(1.0, 1.0, 1.0, 0.70 + 0.25 * hk))) {
                    tb.SetOpacity(fade);
                    let ty = t + ch * 0.74;
                    rt.DrawText(&wide(crate::i18n::t("Suelta aquí")), fmt, &rect(l + 6.0 * s, ty - 10.0 * s, l + cw - 6.0 * s, ty + 10.0 * s), &tb, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                }
            }

            // Tick: círculo del acento que entra con un pequeño rebote
            if let Some(m) = done_ms {
                let pop = 0.5 + 0.5 * ease_back(m / 220.0);
                let a = ease_out(m / 120.0);
                let cy = t + ch * 0.5;
                rt.SetTransform(&mul(&scale_about(pop, cx, cy), base));
                if let Some(cb) = brush(crate::profiles::accent(a)) { rt.FillEllipse(&ellipse(cx, cy, 22.0 * s), &cb); }
                if let (Some(fmt), Some(wb)) = (self.check.as_ref(), brush(rgba(1.0, 1.0, 1.0, a))) {
                    rt.DrawText(&wide(GLYPH_CHECK), fmt, &rect(cx - 22.0 * s, cy - 22.0 * s, cx + 22.0 * s, cy + 22.0 * s), &wb, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
                }
                rt.SetTransform(base);
            }
        }
        Some(())
    }
}

/// Segoe UI Variable (Windows 11) si está; si no, Segoe UI. Terminada en 0, lista para PCWSTR.
fn pick_family(dw: &IDWriteFactory) -> Vec<u16> {
    let want = "Segoe UI Variable Text";
    let wide0 = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let found = unsafe {
        let mut col: Option<IDWriteFontCollection> = None;
        dw.GetSystemFontCollection(&mut col, false).ok().and(col).is_some_and(|c| {
            let (mut idx, mut ok) = (0u32, BOOL(0));
            let name = wide0(want);
            c.FindFamilyName(PCWSTR(name.as_ptr()), &mut idx, &mut ok).is_ok() && ok.as_bool()
        })
    };
    wide0(if found { want } else { "Segoe UI" })
}

fn with_state(f: impl FnOnce(&mut Basket)) {
    STATE.with(|s| if let Ok(mut g) = s.try_borrow_mut() { if let Some(b) = g.as_mut() { f(b); } });
}

unsafe extern "system" fn basket_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_BASKET_SHOW => { with_state(|b| b.show(wp.0 as isize as i32, lp.0 as i32)); LRESULT(0) }
            WM_TIMER if wp.0 == TIMER_ID => { with_state(|b| b.tick()); LRESULT(0) }
            // Nunca roba el foco a la app desde la que se arrastra
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            // El tamaño y la posición los pone UpdateLayeredWindow a la escala de cada monitor
            WM_DPICHANGED => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Arranca el vigilante (un hilo que sondea el ratón). Lo llamo yo una vez al arrancar, desde el hilo de la interfaz.
/// Prueba visual: enseña la cesta en (x, y) de pantalla.
pub fn test_show(x: i32, y: i32) {
    let hwnd = HWND(HWND_BASKET.load(Ordering::Relaxed) as *mut _);
    if !hwnd.0.is_null() { unsafe { let _ = PostMessageW(Some(hwnd), WM_BASKET_SHOW, WPARAM(x as isize as usize), LPARAM(y as isize)); } }
}

pub fn start() {
    if STARTED.swap(true, Ordering::Relaxed) { return; }
    unsafe {
        // El destino OLE vive en este hilo (ya inicializado por la barra; repetirlo no hace daño)
        let _ = OleInitialize(None);
        let Ok(fac) = D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) else { return };
        let Ok(dw) = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) else { return };
        let Ok(hm) = GetModuleHandleW(None) else { return };
        let hinst: HINSTANCE = hm.into();
        let class = w!("MacBarBasket");
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(basket_proc), hInstance: hinst, lpszClassName: class, hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() });
        let Ok(hwnd) = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class, w!("macbar basket"), WS_POPUP,
            0, 0, 1, 1, None, None, Some(hinst), None) else { return };
        let t: IDropTarget = Target { files: Cell::new(false) }.into();
        let _ = RegisterDragDrop(hwnd, &t);
        STATE.with(|s| if let Ok(mut g) = s.try_borrow_mut() { *g = Some(Basket::new(hwnd, fac, dw)); });
        HWND_BASKET.store(hwnd.0 as isize, Ordering::Relaxed);
    }
    let _ = std::thread::Builder::new().name("macbar-cesta".into()).spawn(watch);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pasa una ruta de puntos (x, ms) por el detector y devuelve los cambios de sentido recientes.
    fn run(path: &[(i32, u64)], window: u64) -> usize {
        let mut s = Shake::default();
        for &(x, t) in path { s.feed(x, t); }
        s.turns(path.last().map_or(0, |p| p.1), window)
    }

    /// Zigzag de `legs` tramos de `amp` px, cada uno en `leg_ms`, con muestras cada 25 ms.
    fn zigzag(amp: i32, legs: usize, leg_ms: u64) -> Vec<(i32, u64)> {
        let steps = (leg_ms / 25).max(1) as i32;
        let mut out = vec![(0, 0)];
        let mut t = 0;
        for i in 0..legs {
            let (from, to) = if i % 2 == 0 { (0, amp) } else { (amp, 0) };
            for k in 1..=steps {
                t += 25;
                out.push((from + (to - from) * k / steps, t));
            }
        }
        out
    }

    #[test]
    fn agitado_rapido_cuenta_cambios_de_sentido() {
        // 4 tramos de 120 px a 125 ms: 3 cambios de sentido en ~500 ms
        let p = zigzag(120, 4, 125);
        assert_eq!(run(&p, FAST_MS), 3);
        assert!(should_show(run(&p, FAST_MS), run(&p, STRICT_MS), Signal::Drag));
    }

    #[test]
    fn temblor_pequeno_no_es_agitar() {
        // Tramos de 30 px (por debajo de MIN_RUN)
        let p = zigzag(30, 12, 75);
        assert_eq!(run(&p, FAST_MS), 0);
    }

    #[test]
    fn vaiven_lento_no_es_agitar() {
        // Tramos de 400 ms: en 700 ms caben como mucho 2 cambios de sentido
        let p = zigzag(200, 6, 400);
        assert!(run(&p, FAST_MS) < FAST_TURNS);
    }

    #[test]
    fn arrastre_en_linea_recta_no_cuenta() {
        let p: Vec<(i32, u64)> = (0..40).map(|i| (i * 30, i as u64 * 25)).collect();
        assert_eq!(run(&p, FAST_MS), 0);
        // Ida y vuelta una sola vez: un cambio de sentido
        let mut p2 = p.clone();
        p2.extend((1..40).map(|i| (39 * 30 - i * 30, 975 + i as u64 * 25)));
        assert_eq!(run(&p2, STRICT_MS), 1);
    }

    #[test]
    fn los_cambios_viejos_caducan() {
        let mut s = Shake::default();
        for &(x, t) in &zigzag(120, 4, 125) { s.feed(x, t); }
        // Quieto mucho rato
        s.feed(0, 5000);
        assert_eq!(s.turns(5000, FAST_MS), 0);
        assert_eq!(s.turns(5000, STRICT_MS), 0);
    }

    #[test]
    fn sin_senal_de_arrastre_hace_falta_agitar_mas() {
        let p = zigzag(120, 4, 125);
        let (f, sl) = (run(&p, FAST_MS), run(&p, STRICT_MS));
        assert!(!should_show(f, sl, Signal::Unknown));
        assert!(!should_show(f, sl, Signal::Plain));
        let p = zigzag(120, 6, 125);
        let (f, sl) = (run(&p, FAST_MS), run(&p, STRICT_MS));
        assert_eq!(sl, 5);
        assert!(should_show(f, sl, Signal::Unknown));
        // Con cursor normal (texto, mover ventana…) nunca
        assert!(!should_show(f, sl, Signal::Plain));
    }

    const MON: (i32, i32, i32, i32) = (0, 0, 1920, 1040);

    #[test]
    fn cesta_a_la_derecha_y_a_la_altura_del_cursor() {
        assert_eq!(place(500, 400, 150, 130, 28, MON), (528, 335));
    }

    #[test]
    fn cesta_salta_a_la_izquierda_y_se_encaja() {
        // Borde derecho: a la izquierda del cursor
        assert_eq!(place(1900, 400, 150, 130, 28, MON), (1722, 335));
        // Arriba del todo: no se sale por arriba
        assert_eq!(place(500, 10, 150, 130, 28, MON), (528, 0));
        // Abajo del todo (zona de trabajo sin la barra de tareas)
        assert_eq!(place(500, 1039, 150, 130, 28, MON), (528, 910));
    }

    #[test]
    fn cesta_no_sale_del_monitor_ni_tapa_el_cursor() {
        // Monitor secundario a la izquierda, con coordenadas negativas, a escala 1,5
        let g = Geo::new(1.5);
        let mon = (-1920, -300, 0, 780);
        for &(cx, cy) in &[(-1920, -300), (-1, 779), (-960, 200), (-1919, 779), (-1, -300), (-200, 0)] {
            let (x, y) = place(cx, cy, g.cw, g.ch, g.off, mon);
            assert!(x >= mon.0 && x + g.cw <= mon.2, "x fuera: {x} para {cx},{cy}");
            assert!(y >= mon.1 && y + g.ch <= mon.3, "y fuera: {y} para {cx},{cy}");
            let dentro = cx >= x && cx < x + g.cw && cy >= y && cy < y + g.ch;
            assert!(!dentro, "la cesta tapa el cursor en {cx},{cy}");
        }
    }

    #[test]
    fn cesta_en_monitor_diminuto_se_encaja() {
        assert_eq!(place(50, 50, 150, 130, 28, (0, 0, 100, 100)), (0, 0));
    }

    #[test]
    fn entrada_con_rebote_y_fundido() {
        let (s0, a0) = enter(0.0);
        assert!((s0 - 0.8).abs() < 1e-4 && a0.abs() < 1e-6);
        let (s1, a1) = enter(ENTER_MS);
        assert!((s1 - 1.0).abs() < 1e-4 && (a1 - 1.0).abs() < 1e-4);
        // Algún momento de la entrada pasa un pelín de 1 (el rebote), sin pasarse
        let peak = (0..=200).map(|i| enter(i as f32).0).fold(0.0f32, f32::max);
        assert!(peak > 1.0 && peak < 1.04, "pico {peak}");
    }

    #[test]
    fn respiracion_solo_con_el_arrastre_encima() {
        assert_eq!(breathe(0.37, 0.0), 1.0);
        assert_eq!(breathe(0.0, 1.0), 1.0);
        let max = (0..140).map(|i| breathe(i as f32 / 100.0, 1.0)).fold(0.0f32, f32::max);
        assert!(max > 1.02 && max <= 1.03 + 1e-6);
    }

    #[test]
    fn cierre_por_tiempo_y_al_soltar_fuera() {
        // Recién salida, con el botón pulsado: se queda
        assert!(!close_now(100.0, false, false, true, None));
        // 4 s sin que entre el arrastre
        assert!(close_now(IDLE_MS, false, false, true, None));
        // Si ya entró, no caduca mientras siga el botón pulsado
        assert!(!close_now(9000.0, true, false, true, None));
        // Botón suelto fuera: se va
        assert!(close_now(500.0, true, false, false, Some(0.0)));
        assert!(close_now(500.0, false, false, false, None));
        // Suelto encima: espera al Drop un momento, y luego se rinde
        assert!(!close_now(500.0, true, true, false, Some(100.0)));
        assert!(close_now(500.0, true, true, false, Some(DROP_WAIT_MS)));
    }

    #[test]
    fn geometria_a_cualquier_escala() {
        let g = Geo::new(1.0);
        assert_eq!((g.cw, g.ch, g.w, g.h), (150, 130, 202, 182));
        for s in [1.25, 1.5, 1.75, 2.0, 3.0] {
            let g = Geo::new(s);
            assert_eq!(g.w, g.cw + 2 * g.m);
            assert_eq!(g.h, g.ch + 2 * g.m);
        }
    }

    #[test]
    fn transformaciones_se_componen() {
        let a = scale_about(2.0, 10.0, 10.0);
        let b = scale_about(0.5, 0.0, 0.0);
        let m = mul(&a, &b);
        // (10,10) es fijo en `a`; luego `b` lo deja en (5,5)
        let (x, y) = (10.0 * m.M11 + 10.0 * m.M21 + m.M31, 10.0 * m.M12 + 10.0 * m.M22 + m.M32);
        assert!((x - 5.0).abs() < 1e-5 && (y - 5.0).abs() < 1e-5);
    }

    #[test]
    fn cursores_de_arrastre_de_ole32_tienen_huella() {
        // Solo carga recursos: no toca el cursor ni el ratón
        let _ = unsafe { windows::Win32::System::LibraryLoader::LoadLibraryW(w!("ole32.dll")) };
        let c = Cursors::load();
        assert!(!c.drag.is_empty() && !c.prints.is_empty(), "sin cursores de ole32");
        assert!(!c.plain.is_empty());
        // Ningún cursor normal del sistema se confunde con uno de arrastre
        for &h in &c.plain {
            if let Some(p) = print(HCURSOR(h as *mut _)) { assert!(c.prints.binary_search(&p).is_err()); }
        }
        // El de copiar cargado aparte da la misma huella
        let hi: HINSTANCE = unsafe { GetModuleHandleW(w!("ole32.dll")) }.unwrap().into();
        let h = unsafe { LoadCursorW(Some(hi), PCWSTR(3usize as *const u16)) }.unwrap();
        assert!(c.prints.binary_search(&print(h).unwrap()).is_ok());
    }

    #[test]
    fn huella_distingue_bits() {
        assert_eq!(fnv(FNV_SEED, b""), FNV_SEED);
        assert_ne!(fnv(FNV_SEED, b"ab"), fnv(FNV_SEED, b"ba"));
        assert_eq!(fnv(FNV_SEED, b"abc"), fnv(fnv(FNV_SEED, b"a"), b"bc"));
    }
}
