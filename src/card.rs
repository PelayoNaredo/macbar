//! Tarjeta de la isla: la isla crece hacia abajo hasta convertirse en una tarjeta con controles
//! (como la Dynamic Island expandida). Ver PLAN_TARJETA_ISLA.md.
//!
//! Es una ventana propia con alfa por píxel (Direct2D sobre un DIB + `UpdateLayeredWindow`), así que
//! lo transparente deja pasar el ratón. Nace con la forma exacta de la isla: solo cuando su primer
//! fotograma ya está en pantalla, la barra deja de pintar la isla (`owns_island`). Al cerrar, al revés.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU32};

/// Ancho y alto de la tarjeta de música (DIP).
pub const CARD_W: f32 = 380.0;
pub const CARD_H: f32 = 168.0;
/// Margen alrededor de la tarjeta para la sombra (salvo por el lado de la barra).
const M: f32 = 16.0;
const RADIUS: f32 = 24.0;
const TIMER_POLL: usize = 1;
const TIMER_ANIM: usize = 2;

/// Cómo se abrió: con clic aguanta más fuera antes de cerrarse que si se abrió pasando el ratón.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Origin { Click, #[allow(dead_code)] Hover }

/// Motivo del cierre.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Why { Leave, ClickOutside, Escape, Toggle, Empty, Fullscreen, OtherPanel, BarMoved }

/// Lo que se mira para decidir si cerrar (ver la tabla del plan, sección 4).
#[derive(Clone, Copy, Debug)]
pub struct Watch {
    pub origin: Origin,
    /// El ratón está sobre la tarjeta.
    pub inside: bool,
    /// Milisegundos seguidos con el ratón fuera.
    pub outside_ms: u32,
    /// Arrastrando algo dentro (la barra de progreso): nada la cierra salvo lo imprescindible.
    pub dragging: bool,
    pub click_outside: bool,
    pub escape: bool,
    pub fullscreen: bool,
    pub other_panel: bool,
    /// Hay algo que enseñar (música sonando o en pausa).
    pub has_content: bool,
    /// Milisegundos seguidos sin contenido.
    pub empty_ms: u32,
}

/// Plazo con el ratón fuera antes de cerrarse.
pub fn grace_ms(o: Origin) -> u32 { match o { Origin::Click => 700, Origin::Hover => 250 } }

/// ¿Hay que cerrar ya? Pantalla completa y otro panel mandan siempre; un arrastre en curso impide
/// el resto; luego clic fuera, quedarse sin contenido y salir con el ratón.
pub fn should_close(w: &Watch) -> Option<Why> {
    if w.fullscreen { return Some(Why::Fullscreen); }
    if w.other_panel { return Some(Why::OtherPanel); }
    if w.escape { return Some(Why::Escape); }
    if w.dragging { return None; }
    if w.click_outside { return Some(Why::ClickOutside); }
    if !w.has_content && w.empty_ms >= 1500 { return Some(Why::Empty); }
    if !w.inside && w.outside_ms >= grace_ms(w.origin) { return Some(Why::Leave); }
    None
}

/// Rectángulo de la ventana (px de pantalla: izq, arriba, der, abajo) y posición de la tarjeta dentro
/// de ella. La tarjeta se centra bajo la isla (`isl`, px), se encaja en el monitor con `edge` px de
/// margen y su borde del lado de la barra coincide con el de la isla. `m` = margen de la sombra.
pub fn place(isl: (i32, i32, i32, i32), size: (i32, i32), m: i32, edge: i32, mon: (i32, i32, i32, i32), bottom: bool) -> ((i32, i32, i32, i32), (i32, i32)) {
    let (w, h) = size;
    let cx = (isl.0 + isl.2) / 2;
    let lo = mon.0 + edge;
    let hi = (mon.2 - edge - w).max(lo);
    let left = (cx - w / 2).clamp(lo, hi);
    let (top, bot) = if bottom { (isl.3 - h, isl.3) } else { (isl.1, isl.1 + h) };
    let win = if bottom { (left - m, top - m, left + w + m, bot) } else { (left - m, top, left + w + m, bot + m) };
    (win, (left - win.0, top - win.1))
}

pub static OWNS: AtomicBool = AtomicBool::new(false);
/// Cuánto está desplegada la isla de música ahora mismo (0..1, bits de f32): lo apunta la barra.
pub static ISL_OPEN: AtomicU32 = AtomicU32::new(0);
/// ¿La isla la pinta la tarjeta (la barra no debe pintarla)?
pub fn owns_island() -> bool { OWNS.load(Ordering::Relaxed) }

/// ¿El clic en la isla abre la tarjeta? (ajuste «tarjeta_clic»; «pausar» = como antes)
pub fn click_opens() -> bool { !sys::config_value("tarjeta_clic").is_some_and(|v| v.eq_ignore_ascii_case("pausar")) }

// ---------------------------------------------------------------- superficie con alfa por píxel
struct Surface { dc: HDC, bmp: HBITMAP, old: HGDIOBJ, w: i32, h: i32 }

impl Surface {
    fn new(w: i32, h: i32) -> Option<Surface> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() { return None; }
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = match CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                Ok(b) => { let _ = DeleteObject(b.into()); let _ = DeleteDC(dc); return None; }
                Err(_) => { let _ = DeleteDC(dc); return None; }
            };
            let old = SelectObject(dc, bmp.into());
            Some(Surface { dc, bmp, old, w, h })
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) { unsafe { SelectObject(self.dc, self.old); let _ = DeleteObject(self.bmp.into()); let _ = DeleteDC(self.dc); } }
}

struct CardFonts { title: IDWriteTextFormat, sub: IDWriteTextFormat, small: IDWriteTextFormat, tiny: IDWriteTextFormat, icon: IDWriteTextFormat, icon_l: IDWriteTextFormat,
    // Los de la isla (para el primer y el último fotograma, idénticos a los de la barra)
    i_title: IDWriteTextFormat, i_small: IDWriteTextFormat, i_icon: IDWriteTextFormat }

/// Muelle con un pelín de rebote (el mismo que la isla). Devuelve true mientras se mueve.
fn spring(x: &mut f32, v: &mut f32, target: f32, dt: f32) -> bool {
    let d = target - *x;
    if d.abs() < 0.3 && v.abs() < 6.0 { *x = target; *v = 0.0; return false; }
    let (k, c) = (380.0_f32, 2.0 * 380.0_f32.sqrt() * 0.72);
    *v += (k * d - c * *v) * dt;
    *x += *v * dt;
    true
}

/// Acercamiento suave sin rebote (brillos, escalas pequeñas).
fn ease_to(x: &mut f32, target: f32, tau: f32, dt: f32) -> bool {
    if (target - *x).abs() < 0.002 { *x = target; return false; }
    *x += (target - *x) * (1.0 - (-dt / tau).exp());
    true
}

fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

struct Card {
    hwnd: HWND,
    fac: ID2D1Factory,
    rt: Option<ID2D1DCRenderTarget>,
    surf: Option<Surface>,
    dw: IDWriteFactory,
    cache: RefCell<Caches>,
    f: CardFonts,
    /// Ventana (px de pantalla) y posición de la tarjeta dentro (px).
    win: (i32, i32, i32, i32),
    off: (i32, i32),
    s: f32,
    bottom: bool,
    origin: Origin,
    open: bool,
    opened: Instant,
    closed_at: Instant,
    // Forma: ancho y alto animados (DIP) con su velocidad
    w: f32, vw: f32, h: f32, vh: f32,
    // Ratón
    hover: Option<u8>,
    pressed: Option<u8>,
    glow: [f32; 8],
    hits: Vec<(D2D_RECT_F, u8)>,
    outside_since: Option<Instant>,
    empty_since: Option<Instant>,
    btn_prev: (bool, bool, bool),
    // Música
    art: Option<(u64, ID2D1Bitmap)>,
    art_at: Instant,
    art_k: f32,
    shown_title: (String, String),
    prev_title: (String, String),
    title_at: Instant,
    lines: (String, f32, Vec<String>),
    play: (bool, Instant),
    last: Instant,
    /// Arrastrando la barra de progreso: fracción (0..1) bajo el ratón.
    drag: Option<f32>,
    /// Barra de progreso «activa» (ratón encima o arrastrando): engorda y sale el círculo.
    bar_k: f32,
    bar_at: Instant,
    /// Tras un salto, la posición que se enseña mientras el reproductor se pone al día.
    hold: Option<(f32, Instant)>,
    /// Extremos de la barra de progreso (DIP) y duración, para convertir el ratón en tiempo.
    bar_span: (f32, f32, f32),
    /// Ventana que tenía el foco antes de abrir (se le devuelve al cerrar).
    prev_fg: HWND,
}

thread_local! { static CARD: RefCell<Option<Card>> = const { RefCell::new(None) }; }

pub fn is_open() -> bool { CARD.with(|c| c.borrow().as_ref().is_some_and(|c| c.open)) }

/// Isla de la barra en px de pantalla, su grado de despliegue (0..1) y la escala de la barra.
fn island_px() -> Option<((i32, i32, i32, i32), f32, f32)> {
    let bar = bar_hwnd();
    let s = scale_of(bar);
    let mut br = RECT::default();
    unsafe { GetWindowRect(bar, &mut br).ok()?; }
    let r = GFX.with(|g| g.try_borrow().ok().and_then(|g| g.as_ref().map(|g| g.isl_rect)))?;
    let t = f32::from_bits(ISL_OPEN.load(Ordering::Relaxed));
    let px = |v: f32| (v * s).round() as i32;
    Some(((br.left + px(r.left), br.top + px(r.top), br.left + px(r.right), br.top + px(r.bottom)), t, s))
}

fn ensure() -> Option<()> {
    if CARD.with(|c| c.borrow().is_some()) { return Some(()); }
    unsafe {
        let hinst = GetModuleHandleW(None).ok()?;
        let class = w!("MacBarCard");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, hCursor: LoadCursorW(None, IDC_ARROW).ok()?, ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class, w!("Isla de macbar"), WS_POPUP, 0, 0, 1, 1, None, None, Some(hinst.into()), None).ok()?;
        let fac: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let mk = |fam, size, wt| make_format(&dw, fam, size, wt);
        let f = CardFonts {
            title: mk(FONT, 15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD), sub: mk(FONT, 12.5, DWRITE_FONT_WEIGHT_NORMAL), small: mk(FONT, 11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD), tiny: mk(FONT, 8.5, DWRITE_FONT_WEIGHT_BOLD),
            icon: mk(ICONS, 17.0, DWRITE_FONT_WEIGHT_NORMAL), icon_l: mk(ICONS, 24.0, DWRITE_FONT_WEIGHT_NORMAL),
            i_title: mk(FONT, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD), i_small: mk(FONT, 11.5, DWRITE_FONT_WEIGHT_NORMAL), i_icon: mk(ICONS, 12.5, DWRITE_FONT_WEIGHT_NORMAL),
        };
        let old = Instant::now() - Duration::from_secs(5);
        let c = Card { hwnd, fac, rt: None, surf: None, dw, cache: RefCell::new(Caches::default()), f, win: (0, 0, 1, 1), off: (0, 0), s: 1.0, bottom: false,
            origin: Origin::Click, open: false, opened: old, closed_at: old, w: 0.0, vw: 0.0, h: PILL_H, vh: 0.0, hover: None, pressed: None, glow: [0.0; 8], hits: Vec::new(),
            outside_since: None, empty_since: None, btn_prev: (false, false, false), art: None, art_at: old, art_k: 1.0, shown_title: Default::default(), prev_title: Default::default(),
            title_at: old, lines: (String::new(), 0.0, Vec::new()), play: (false, old), last: Instant::now(),
            drag: None, bar_k: 0.0, bar_at: old, hold: None, bar_span: (0.0, 0.0, 0.0), prev_fg: HWND::default() };
        CARD.with(|x| *x.borrow_mut() = Some(c));
    }
    Some(())
}

/// Abre la tarjeta desde la isla (o la cierra si ya estaba abierta).
fn dbg(m: &str) { if std::env::var_os("MACBAR_CARDTEST").is_some() { use std::io::Write; if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("macbar_card.log")) { let _ = writeln!(f, "{m}"); } } }

pub fn open(origin: Origin) {
    dbg(&format!("open: fullscreen={} media={} abierta={}", crate::fullscreen(), MEDIA.lock().unwrap().is_some(), is_open()));
    if crate::fullscreen() || MEDIA.lock().unwrap().is_none() { return; }
    if is_open() { close(Why::Toggle); return; }
    if ensure().is_none() { dbg("ensure falló"); return; }
    let Some((isl, _, s)) = island_px() else { dbg("sin isla"); return };
    dbg(&format!("isla {isl:?} escala {s}"));
    // Un panel a la vez
    ccenter::close();
    tray::close();
    popup::hide_all();
    let bottom = crate::bar_bottom();
    let mon = unsafe {
        let hm = MonitorFromWindow(bar_hwnd(), MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(hm, &mut mi);
        (mi.rcMonitor.left, mi.rcMonitor.top, mi.rcMonitor.right, mi.rcMonitor.bottom)
    };
    let size = ((CARD_W * s).round() as i32, (CARD_H * s).round() as i32);
    let (win, off) = place(isl, size, (M * s).round() as i32, (8.0 * s).round() as i32, mon, bottom);
    let hwnd = CARD.with(|c| {
        let mut c = c.borrow_mut();
        let c = c.as_mut()?;
        let was_closing = c.surf.is_some() && !c.open;
        // Si se estaba cerrando, vuelve a crecer desde donde iba; si no, nace con la forma de la isla
        if !was_closing || c.win != win {
            c.w = (isl.2 - isl.0) as f32 / s;
            c.h = PILL_H;
            c.vw = 0.0;
            c.vh = 0.0;
        }
        let (ww, wh) = (win.2 - win.0, win.3 - win.1);
        if c.surf.as_ref().is_none_or(|x| x.w != ww || x.h != wh) { c.surf = Surface::new(ww, wh); }
        c.win = win;
        c.off = off;
        c.s = s;
        c.bottom = bottom;
        c.origin = origin;
        c.open = true;
        c.opened = Instant::now();
        c.hover = None;
        c.pressed = None;
        c.outside_since = None;
        c.empty_since = None;
        // Lo pulsado ahora mismo (el clic que la abrió) no cuenta como «clic fuera»
        c.btn_prev = (true, true, true);
        c.last = Instant::now();
        c.drag = None;
        c.hold = None;
        Some(c.hwnd)
    });
    let Some(hwnd) = hwnd else { return };
    // Primer fotograma (idéntico a la isla) ya en pantalla; luego la barra deja de pintarla
    render();
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        OWNS.store(true, Ordering::Relaxed);
        let _ = InvalidateRect(Some(bar_hwnd()), None, false);
        SetTimer(Some(hwnd), TIMER_POLL, 30, None);
        SetTimer(Some(hwnd), TIMER_ANIM, 15, None);
        // Abierta con clic: recibe el teclado (espacio, flechas, Esc); al cerrar devuelve el foco
        if origin == Origin::Click {
            let fg = GetForegroundWindow();
            CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.prev_fg = if fg == hwnd { c.prev_fg } else { fg }; });
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

/// Cierra la tarjeta: vuelve a encogerse hasta la isla. Algunos motivos la quitan al momento.
pub fn close(why: Why) {
    let instant = matches!(why, Why::Fullscreen | Why::BarMoved);
    let hwnd = CARD.with(|c| c.borrow_mut().as_mut().filter(|c| c.open).map(|c| { c.open = false; c.closed_at = Instant::now(); c.pressed = None; c.drag = None; c.hwnd }));
    let Some(hwnd) = hwnd else { return };
    if instant { finish(hwnd); return; }
    unsafe { SetTimer(Some(hwnd), TIMER_ANIM, 15, None); }
}

/// Fin del cierre: la barra vuelve a pintar la isla y, después, se oculta la tarjeta.
fn finish(hwnd: HWND) {
    OWNS.store(false, Ordering::Relaxed);
    let prev = CARD.with(|c| c.borrow_mut().as_mut().map(|c| std::mem::take(&mut c.prev_fg))).unwrap_or_default();
    unsafe {
        if GetForegroundWindow() == hwnd && !prev.0.is_null() && IsWindow(Some(prev)).as_bool() { let _ = SetForegroundWindow(prev); }
        if GetCapture() == hwnd { let _ = ReleaseCapture(); }
        let _ = InvalidateRect(Some(bar_hwnd()), None, false);
        let _ = UpdateWindow(bar_hwnd());
        let _ = ShowWindow(hwnd, SW_HIDE);
        let _ = KillTimer(Some(hwnd), TIMER_POLL);
        let _ = KillTimer(Some(hwnd), TIMER_ANIM);
    }
    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.open = false; c.art = None; c.rt = None; });
}

/// Forma actual de la tarjeta (DIP, dentro de la ventana).
fn shape(c: &Card, isl_w: f32) -> D2D_RECT_F {
    let s = c.s;
    let (ox, oy) = (c.off.0 as f32 / s, c.off.1 as f32 / s);
    // Centro: del de la isla al de la tarjeta según lo que ha crecido
    let isl_cx = island_px().map(|(r, _, _)| ((r.0 + r.2) as f32 / 2.0 - c.win.0 as f32) / s).unwrap_or(ox + CARD_W / 2.0);
    let kw = if CARD_W > isl_w + 1.0 { ((c.w - isl_w) / (CARD_W - isl_w)).clamp(0.0, 1.0) } else { 1.0 };
    let cx = lerp(isl_cx, ox + CARD_W / 2.0, kw);
    let w = c.w.max(0.0);
    if c.bottom {
        let b = oy + CARD_H;
        rect(cx - w / 2.0, b - c.h, cx + w / 2.0, b)
    } else {
        rect(cx - w / 2.0, oy, cx + w / 2.0, oy + c.h)
    }
}

fn fmt_time(secs: f32) -> String { let s = secs.max(0.0).round() as u32; format!("{}:{:02}", s / 60, s % 60) }

/// Título en dos líneas como mucho (la segunda con puntos suspensivos si no cabe).
fn wrap2(p: &Painter, f: &IDWriteTextFormat, s: &str, maxw: f32) -> Vec<String> {
    let fit = |t: &str| p.width(f, t) <= maxw;
    let ell = |t: &str| -> String {
        if fit(t) { return t.to_string(); }
        let mut cs: Vec<char> = t.chars().collect();
        while !cs.is_empty() {
            cs.pop();
            let c = format!("{}…", cs.iter().collect::<String>().trim_end());
            if fit(&c) { return c; }
        }
        "…".into()
    };
    if fit(s) { return vec![s.to_string()]; }
    let words: Vec<&str> = s.split(' ').collect();
    let mut l1 = String::new();
    let mut i = 0;
    while i < words.len() {
        let cand = if l1.is_empty() { words[i].to_string() } else { format!("{l1} {}", words[i]) };
        if !fit(&cand) { break; }
        l1 = cand;
        i += 1;
    }
    if l1.is_empty() { return vec![ell(s)]; }
    let rest = words[i..].join(" ");
    if rest.is_empty() { vec![l1] } else { vec![l1, ell(&rest)] }
}

/// Texto con su altura de mayúscula centrada en [top, top + bh].
fn txt(p: &Painter, f: &IDWriteTextFormat, s: &str, x: f32, top: f32, bh: f32, b: &ID2D1Brush) {
    p.draw_at(f, s, x, top + bh / 2.0 + p.text_top(f) - 50.0, b, None);
}

/// Glifo centrado por su tinta en (cx, cy).
fn glyph_c(p: &Painter, f: &IDWriteTextFormat, g: &str, cx: f32, cy: f32, b: &ID2D1Brush) {
    let [l, t, iw, ih] = p.ink(f, g);
    p.draw_at(f, g, cx - iw / 2.0 - l, p.snap(cy - ih / 2.0 - t), b, None);
}

/// Carátula como cuadrado de esquinas `rad` (con rad = size/2 es un círculo).
fn draw_art(rt: &ID2D1RenderTarget, bmp: &ID2D1Bitmap, x: f32, y: f32, size: f32, rad: f32, a: f32) {
    unsafe {
        let props = D2D1_BRUSH_PROPERTIES { opacity: a, transform: Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 } };
        let Ok(br) = rt.CreateBitmapBrush(bmp, None, Some(&props)) else { return };
        let sz = bmp.GetSize();
        let k = size / sz.width.max(1.0);
        br.SetTransform(&Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: x, M32: y });
        br.SetInterpolationMode(D2D1_BITMAP_INTERPOLATION_MODE_LINEAR);
        rt.FillRoundedRectangle(&rrect(rect(x, y, x + size, y + size), rad.min(size / 2.0)), &br);
    }
}

/// Flecha circular con «15» dentro (saltar 15 s atrás o adelante), como la de los reproductores de iOS.
fn skip_icon(rt: &ID2D1RenderTarget, fac: &ID2D1Factory, p: &Painter, f: &IDWriteTextFormat, cx: f32, cy: f32, back: bool, b: &ID2D1Brush) {
    let r = 9.5;
    // Hacia delante la flecha va en el sentido del reloj y acaba arriba a la izquierda; hacia atrás, en espejo
    let mx = |x: f32| if back { 2.0 * cx - x } else { x };
    let at = |deg: f32| { let a = deg.to_radians(); Vector2 { X: mx(cx + r * a.cos()), Y: cy + r * a.sin() } };
    unsafe {
        if let Ok(geo) = fac.CreatePathGeometry() {
            if let Ok(sk) = geo.Open() {
                sk.BeginFigure(at(-62.0), D2D1_FIGURE_BEGIN_HOLLOW);
                sk.AddArc(&D2D1_ARC_SEGMENT { point: at(245.0), size: D2D_SIZE_F { width: r, height: r }, rotationAngle: 0.0,
                    sweepDirection: if back { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_CLOCKWISE }, arcSize: D2D1_ARC_SIZE_LARGE });
                sk.EndFigure(D2D1_FIGURE_END_OPEN);
                let _ = sk.Close();
                rt.DrawGeometry(&geo, b, 1.6, None);
            }
        }
        // Punta de flecha en el extremo, siguiendo la tangente
        let a = 245.0_f32.to_radians();
        let tip0 = at(245.0);
        let (tx, ty) = (-a.sin() * if back { -1.0 } else { 1.0 }, a.cos());
        let (nx, ny) = (-ty, tx);
        if let Ok(geo) = fac.CreatePathGeometry() {
            if let Ok(sk) = geo.Open() {
                sk.BeginFigure(Vector2 { X: tip0.X + tx * 3.6, Y: tip0.Y + ty * 3.6 }, D2D1_FIGURE_BEGIN_FILLED);
                sk.AddLines(&[Vector2 { X: tip0.X + nx * 3.0 - tx * 0.6, Y: tip0.Y + ny * 3.0 - ty * 0.6 }, Vector2 { X: tip0.X - nx * 3.0 - tx * 0.6, Y: tip0.Y - ny * 3.0 - ty * 0.6 }]);
                sk.EndFigure(D2D1_FIGURE_END_CLOSED);
                let _ = sk.Close();
                rt.FillGeometry(&geo, b, None);
            }
        }
    }
    let w = p.width(f, "15");
    txt(p, f, "15", cx - w / 2.0, cy - 6.0, 12.5, b);
}

/// Pinta un fotograma y lo sube a la ventana. Devuelve true mientras algo se mueve.
fn render() -> bool {
    let mut done: Option<HWND> = None;
    let moving = CARD.with(|cc| {
        let mut cc = cc.borrow_mut();
        let Some(c) = cc.as_mut() else { return false };
        let Some(surf) = c.surf.as_ref() else { return false };
        let (sw, shh) = (surf.w, surf.h);
        let dc = surf.dc;
        if c.rt.is_none() {
            let props = D2D1_RENDER_TARGET_PROPERTIES { r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT, pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0, ..Default::default() };
            c.rt = unsafe { c.fac.CreateDCRenderTarget(&props).ok() };
            c.art = None;
        }
        let Some(rt) = c.rt.clone() else { return false };
        let s = c.s;
        let now = Instant::now();
        let dt = (now - c.last).as_secs_f32().min(1.0 / 30.0);
        c.last = now;
        let mut moving = false;

        let (isl_px, isl_open) = island_px().map(|(r, t, _)| (r, t)).unwrap_or(((0, 0, 0, 0), 0.0));
        let isl_w = (isl_px.2 - isl_px.0) as f32 / s;
        // Isla en coordenadas de la ventana (DIP)
        let isl = rect((isl_px.0 - c.win.0) as f32 / s, (isl_px.1 - c.win.1) as f32 / s, (isl_px.2 - c.win.0) as f32 / s, (isl_px.3 - c.win.1) as f32 / s);
        let isl_cy = (isl.top + isl.bottom) / 2.0;

        // ---- Forma: al abrir, primero se ensancha y 30 ms después cae; al cerrar, al revés
        let (tw, th) = if c.open {
            (CARD_W, if c.opened.elapsed().as_secs_f32() > 0.03 { CARD_H } else { PILL_H })
        } else {
            (if c.closed_at.elapsed().as_secs_f32() > 0.03 { isl_w } else { c.w.max(isl_w) }, PILL_H)
        };
        moving |= spring(&mut c.w, &mut c.vw, tw, dt);
        moving |= spring(&mut c.h, &mut c.vh, th, dt);
        if !c.open && !moving { done = Some(c.hwnd); }

        let media = MEDIA.lock().unwrap().clone();
        // Carátula grande (256 px) en la GPU, solo cuando cambia
        if let Some(m) = &media {
            if c.art.as_ref().map(|a| a.0) != Some(m.art_ver) {
                let had = c.art.is_some();
                c.art = ART_BIG.lock().unwrap().as_ref().filter(|a| a.0 == m.art_ver).and_then(|(v, px)| {
                    let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                    unsafe { rt.CreateBitmap(D2D_SIZE_U { width: ART_BIG_PX, height: ART_BIG_PX }, Some(px.as_ptr() as _), ART_BIG_PX * 4, &bp).ok().map(|b| (*v, b)) }
                });
                if had && c.art.is_some() { c.art_at = now; }
            }
            let tt = (m.title.clone(), m.artist.clone());
            if c.shown_title.0.is_empty() { c.shown_title = tt.clone(); }
            if c.shown_title != tt {
                c.prev_title = std::mem::replace(&mut c.shown_title, tt);
                c.title_at = now;
                // Canción nueva a mitad de un arrastre: se cancela (el salto sería en otra canción)
                c.drag = None;
                c.hold = None;
                c.pressed = None;
            }
            if c.play.0 != m.playing { c.play = (m.playing, now); }
        }
        let playing = media.as_ref().is_some_and(|m| m.playing);
        // En pausa la carátula se encoge un poco (como en el iPhone)
        moving |= ease_to(&mut c.art_k, if playing || media.is_none() { 1.0 } else { 0.92 }, 0.12, dt);
        // Brillo de «ratón encima» de cada botón
        for i in 0..8 {
            let t = if c.hover == Some(i as u8) { 1.0 } else { 0.0 };
            let tau = if t > c.glow[i] { 0.05 } else { 0.12 };
            moving |= ease_to(&mut c.glow[i], t, tau, dt);
        }
        // Barra de progreso activa (ratón encima o arrastrando): engorda y sale el círculo
        moving |= ease_to(&mut c.bar_k, if c.hover == Some(5) || c.drag.is_some() { 1.0 } else { 0.0 }, 0.07, dt);
        if c.hold.is_some_and(|h| h.1.elapsed().as_secs_f32() > 1.6) { c.hold = None; }
        let can_seek = media.as_ref().is_some_and(|m| CAN_SEEK.load(Ordering::Relaxed) && *SEEK_BAD_FOR.lock().unwrap() != m.title) && media_secs(false).is_some();
        let (tint, eq) = GFX.with(|g| g.try_borrow().ok().and_then(|g| g.as_ref().map(|g| (g.tint_now, g.eq)))).unwrap_or(([1.0; 3], [0.0; 5]));

        let sh = shape(c, isl_w);
        let kh = ((c.h - PILL_H) / (CARD_H - PILL_H)).clamp(0.0, 1.0);
        let kw = if CARD_W > isl_w + 1.0 { ((c.w - isl_w) / (CARD_W - isl_w)).clamp(0.0, 1.0) } else { 1.0 };
        // Contenido de la isla: se va en cuanto empieza a crecer; el de la tarjeta entra pasado el 55 %
        let a_isl = (1.0 - kw.max(kh) * 3.2).clamp(0.0, 1.0);
        let row = |i: f32| ease_out(((kh - 0.55 - 0.06 * i) / 0.4).clamp(0.0, 1.0));
        let (ox, oy) = (c.off.0 as f32 / s, c.off.1 as f32 / s);
        // Esquina de arriba a la izquierda de la tarjeta ya abierta
        let (cl, ct) = (ox, oy);
        // Con la isla casi a cero (sin nada que enseñar), la forma se desvanece como la de la barra
        let pill_a = (sh.right - sh.left).clamp(0.0, 26.0) / 26.0;

        let p = Painter { rt: &rt, dw: &c.dw, cache: &c.cache, h: 100.0, s };
        let mut hits: Vec<(D2D_RECT_F, u8)> = Vec::new();
        unsafe {
            let full = RECT { left: 0, top: 0, right: sw, bottom: shh };
            if rt.BindDC(dc, &full).is_err() { c.rt = None; return true; }
            rt.SetDpi(96.0 * s, 96.0 * s);
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            rt.BeginDraw();
            rt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
            let rad = (PILL_H / 2.0 + (RADIUS - PILL_H / 2.0) * kh).min((sh.bottom - sh.top) / 2.0);
            popup::push_alpha(&rt, pill_a);
            // Sombra suave (solo cuando ya es tarjeta): varias capas tenues que se abren
            if kh > 0.01 {
                for i in 1..=7 {
                    let e = i as f32 * 2.2;
                    let r = rect(sh.left - e, sh.top - e + 5.0, sh.right + e, sh.bottom + e + 5.0);
                    rt.FillRoundedRectangle(&rrect(r, rad + e), &p.brush(rgba(0.0, 0.0, 0.0, 0.055 * kh)));
                }
            }
            rt.FillRoundedRectangle(&rrect(sh, rad), &p.brush(rgba(0.0, 0.0, 0.0, 1.0)));
            // Fondo teñido del color de la carátula, de arriba hacia abajo
            let kc = row(0.0);
            if kc > 0.01 && media.is_some() {
                let g = p.vgrad(sh.top, sh.top + (sh.bottom - sh.top) * 0.8, rgba(tint[0] * 0.55, tint[1] * 0.55, tint[2] * 0.55, 0.42 * kc), rgba(0.0, 0.0, 0.0, 0.0));
                rt.FillRoundedRectangle(&rrect(sh, rad), &g);
            }
            // Borde fino con luz arriba (el de la isla)
            let px = 1.0 / s;
            let edge = p.vgrad(sh.top, sh.bottom, rgba(1.0, 1.0, 1.0, 0.14), rgba(1.0, 1.0, 1.0, lerp(0.03, 0.05, kh)));
            let inner = rect(sh.left + px / 2.0, sh.top + px / 2.0, sh.right - px / 2.0, sh.bottom - px / 2.0);
            rt.DrawRoundedRectangle(&rrect(inner, rad - px / 2.0), &edge, px, None);
            rt.PushAxisAlignedClip(&rect(sh.left + 1.0, sh.top, sh.right - 1.0, sh.bottom), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);

            let wa = |a: f32| p.brush(rgba(1.0, 1.0, 1.0, a));
            // ---- Lo que enseñaba la isla (primer y último fotograma idénticos a los de la barra)
            if a_isl > 0.01 {
                if let Some(m) = &media {
                    let (il, iw) = (isl.left, isl.right - isl.left);
                    if isl_open < 0.5 {
                        let fade = (1.0 - isl_open * 2.0) * a_isl;
                        let bx0 = il + iw - 13.0 - 22.0;
                        // El ecualizador se dibuja como en la barra (centrado en la isla)
                        let wb = p.brush(rgba(tint[0], tint[1], tint[2], 0.96 * fade));
                        draw_eq(&rt, &eq, bx0, isl_cy, &wb);
                    }
                    if isl_open > 0.35 {
                        let k = ((isl_open - 0.35) / 0.65).clamp(0.0, 1.0) * a_isl;
                        let title = trunc(&m.title, 34);
                        let artist = trunc(&m.artist, 22);
                        let tx = il + 4.0 + 22.0 + 10.0 - (1.0 - k.min(1.0)) * 10.0;
                        txt(&p, &c.f.i_title, &title, tx, isl.top, PILL_H, &wa(0.96 * k));
                        let tw = p.width(&c.f.i_title, &title);
                        if !artist.is_empty() { txt(&p, &c.f.i_small, &artist, tx + tw + 8.0, isl.top, PILL_H, &wa(0.52 * k)); }
                        let mut bx = il + iw - 4.0 - 78.0;
                        let play = if m.playing { "\u{E769}" } else { "\u{E768}" };
                        for (gl, en) in [("\u{E892}", m.can_prev), (play, true), ("\u{E893}", m.can_next)] {
                            glyph_c(&p, &c.f.i_icon, gl, bx + 13.0, isl_cy, &wa(if en { 0.96 } else { 0.25 } * k));
                            bx += 26.0;
                        }
                    }
                }
            }

            // ---- La carátula viaja de la isla a la tarjeta como una sola pieza
            let art_from = (isl.left + 4.0, isl_cy - 11.0, 22.0, 11.0);
            let pop = { let e = c.art_at.elapsed().as_secs_f32(); if e < 0.45 { moving = true; 0.88 + 0.12 * ease_back(e / 0.45) } else { 1.0 } };
            let asz = 76.0 * c.art_k * pop;
            let art_to = (cl + 16.0 + (76.0 - asz) / 2.0, ct + 16.0 + (76.0 - asz) / 2.0, asz, 14.0);
            let ka = kh;
            let (ax, ay, asz2, arad) = (lerp(art_from.0, art_to.0, ka), lerp(art_from.1, art_to.1, ka), lerp(art_from.2, art_to.2, ka), lerp(art_from.3, art_to.3, ka));
            match (&c.art, &media) {
                (Some((_, bmp)), Some(_)) => {
                    // Sombra de la carátula grande
                    if ka > 0.3 { rt.FillRoundedRectangle(&rrect(rect(ax, ay + 3.0, ax + asz2, ay + asz2 + 3.0), arad), &p.brush(rgba(0.0, 0.0, 0.0, 0.35 * ka))); }
                    draw_art(&rt, bmp, ax, ay, asz2, arad, 1.0);
                }
                (None, Some(_)) => {
                    rt.FillRoundedRectangle(&rrect(rect(ax, ay, ax + asz2, ay + asz2), arad), &p.brush(rgba(1.0, 1.0, 1.0, 0.10)));
                    if ka > 0.5 { glyph_c(&p, &c.f.icon_l, "\u{E8D6}", ax + asz2 / 2.0, ay + asz2 / 2.0, &wa(0.6 * row(0.0))); }
                }
                _ => {}
            }
            if ka > 0.9 && c.open { hits.push((rect(art_to.0, art_to.1, art_to.0 + 76.0, art_to.1 + 76.0), 4)); }

            // ---- Contenido de la tarjeta
            let lift = |k: f32| 6.0 * (1.0 - k);
            if let Some(m) = &media {
                let tx = cl + 16.0 + 76.0 + 14.0;
                let maxw = cl + CARD_W - 16.0 - tx;
                // Título y artista: fundido cruzado al cambiar de canción
                let k1 = row(1.0);
                if k1 > 0.01 {
                    let ch = ease_out(c.title_at.elapsed().as_secs_f32() / 0.28);
                    if ch < 1.0 { moving = true; }
                    let mut block = |t: &(String, String), a: f32, dy: f32, cache: bool| {
                        if a <= 0.01 { return; }
                        let lines = if cache {
                            if c.lines.0 != t.0 || c.lines.1 != maxw { c.lines = (t.0.clone(), maxw, wrap2(&p, &c.f.title, &t.0, maxw)); }
                            c.lines.2.clone()
                        } else { wrap2(&p, &c.f.title, &t.0, maxw) };
                        let y0 = ct + 20.0 + lift(k1) + dy;
                        for (i, l) in lines.iter().enumerate() { txt(&p, &c.f.title, l, tx, y0 + i as f32 * 20.0, 18.0, &wa(0.96 * a)); }
                        let sub = if m.album.is_empty() || m.album == t.1 || t.1.is_empty() { t.1.clone() } else { format!("{} · {}", t.1, m.album) };
                        let sub = if p.width(&c.f.sub, &sub) > maxw { t.1.clone() } else { sub };
                        let yl = y0 + lines.len() as f32 * 20.0 + 3.0;
                        txt(&p, &c.f.sub, &trunc(&sub, 48), tx, yl, 16.0, &wa(0.56 * a));
                    };
                    if ch < 1.0 { let old = c.prev_title.clone(); block(&old, k1 * (1.0 - ch), -6.0 * ch, false); }
                    let cur = c.shown_title.clone();
                    block(&cur, k1 * ch, 6.0 * (1.0 - ch), true);
                }
                // Progreso: tiempo, barra y lo que queda (o el ecualizador si no hay línea de tiempo)
                let k2 = row(2.0);
                if k2 > 0.01 {
                    let y = ct + 112.0 + lift(k2);
                    match media_secs(m.playing) {
                        Some((pos, dur)) => {
                            // Arrastrando: el tiempo de destino; tras un salto: el destino mientras el reproductor se pone al día
                            let pos = match (c.drag, c.hold) {
                                (Some(fr), _) => fr * dur,
                                (None, Some((t, at))) => (t + if m.playing { at.elapsed().as_secs_f32() } else { 0.0 }).min(dur),
                                _ => pos,
                            };
                            let el = fmt_time(pos);
                            let rem = format!("-{}", fmt_time(dur - pos));
                            let tb = wa((0.55 + 0.35 * c.bar_k) * k2);
                            txt(&p, &c.f.small, &el, cl + 16.0, y - 8.0, 16.0, &tb);
                            let rw = p.width(&c.f.small, &rem);
                            txt(&p, &c.f.small, &rem, cl + CARD_W - 16.0 - rw, y - 8.0, 16.0, &tb);
                            let (l, r) = (cl + 16.0 + 40.0, cl + CARD_W - 16.0 - 44.0);
                            c.bar_span = (l, r, dur);
                            let fr = (pos / dur.max(0.1)).clamp(0.0, 1.0);
                            // Engorda de 4 a 8 DIP al pasar el ratón (solo si se puede saltar)
                            let bh = 2.0 + if can_seek { 2.0 * c.bar_k } else { 0.0 };
                            rt.FillRoundedRectangle(&rrect(rect(l, y - bh, r, y + bh), bh), &wa(0.18 * k2));
                            let fx = (l + (r - l) * fr).max(l + bh * 2.0);
                            if fr > 0.0 { rt.FillRoundedRectangle(&rrect(rect(l, y - bh, fx, y + bh), bh), &wa(0.9 * k2)); }
                            if can_seek {
                                // El círculo de arrastre aparece con un pop
                                let kr = 6.5 * ease_back(c.bar_k);
                                if kr > 0.3 {
                                    let e = D2D1_ELLIPSE { point: Vector2 { X: fx, Y: y }, radiusX: kr, radiusY: kr };
                                    rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: fx, Y: y + 1.0 }, radiusX: kr + 0.5, radiusY: kr + 0.5 }, &p.brush(rgba(0.0, 0.0, 0.0, 0.35 * k2)));
                                    rt.FillEllipse(&e, &wa(k2));
                                }
                                if k2 > 0.9 && c.open { hits.push((rect(l - 8.0, y - 11.0, r + 8.0, y + 11.0), 5)); }
                            }
                        }
                        None => {
                            let wb = p.brush(rgba(tint[0], tint[1], tint[2], 0.96 * k2));
                            draw_eq(&rt, &eq, cl + CARD_W / 2.0 - 11.0, y, &wb);
                        }
                    }
                }
                // Controles: anterior, reproducir o pausa y siguiente
                let k3 = row(3.0);
                if k3 > 0.01 {
                    let y = ct + 143.0 + lift(k3);
                    let ccx = cl + CARD_W / 2.0;
                    let pp = { let e = c.play.1.elapsed().as_secs_f32(); if e < 0.32 { moving = true; 0.7 + 0.3 * ease_back(e / 0.32) } else { 1.0 } };
                    for (id, dx, gl, en, big) in [(1u8, -60.0, "\u{E892}", m.can_prev, false), (2, 0.0, if m.playing { "\u{E769}" } else { "\u{E768}" }, true, true), (3, 60.0, "\u{E893}", m.can_next, false)] {
                        let bx = ccx + dx;
                        let r = if big { 21.0 } else { 17.0 };
                        let gk = c.glow[id as usize];
                        if en && gk > 0.01 { rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: bx, Y: y }, radiusX: r, radiusY: r }, &wa(0.10 * gk * k3)); }
                        let mut sc = if c.pressed == Some(id) && c.hover == Some(id) { 0.88 } else { 1.0 };
                        if id == 2 { sc *= pp; }
                        let mut base = Matrix3x2::default();
                        rt.GetTransform(&mut base);
                        if sc < 0.999 { rt.SetTransform(&(Matrix3x2 { M11: sc, M12: 0.0, M21: 0.0, M22: sc, M31: bx * (1.0 - sc), M32: y * (1.0 - sc) } * base)); }
                        glyph_c(&p, if big { &c.f.icon_l } else { &c.f.icon }, gl, bx, y, &wa(if en { 0.96 } else { 0.25 } * k3));
                        rt.SetTransform(&base);
                        if en && k3 > 0.9 && c.open { hits.push((rect(bx - r, y - r, bx + r, y + r), id)); }
                    }
                    // -15 s / +15 s: solo si el reproductor deja saltar
                    if can_seek {
                        for (id, dx, back) in [(6u8, -118.0, true), (7, 118.0, false)] {
                            let bx = ccx + dx;
                            let gk = c.glow[id as usize];
                            if gk > 0.01 { rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: bx, Y: y }, radiusX: 16.0, radiusY: 16.0 }, &wa(0.10 * gk * k3)); }
                            let sc = if c.pressed == Some(id) && c.hover == Some(id) { 0.88 } else { 1.0 };
                            let mut base = Matrix3x2::default();
                            rt.GetTransform(&mut base);
                            if sc < 0.999 { rt.SetTransform(&(Matrix3x2 { M11: sc, M12: 0.0, M21: 0.0, M22: sc, M31: bx * (1.0 - sc), M32: y * (1.0 - sc) } * base)); }
                            skip_icon(&rt, &c.fac, &p, &c.f.tiny, bx, y, back, &wa(0.9 * k3));
                            rt.SetTransform(&base);
                            if k3 > 0.9 && c.open { hits.push((rect(bx - 16.0, y - 16.0, bx + 16.0, y + 16.0), id)); }
                        }
                    }
                }
            } else {
                // La música se cerró: lo dice un momento y luego la tarjeta se recoge
                let k1 = row(1.0);
                let t = i18n::t("Nada sonando");
                let tw = p.width(&c.f.title, t);
                txt(&p, &c.f.title, t, cl + CARD_W / 2.0 - tw / 2.0, ct + CARD_H / 2.0 - 9.0, 18.0, &wa(0.6 * k1));
            }
            rt.PopAxisAlignedClip();
            rt.PopLayer();
            let ok = rt.EndDraw(None, None).is_ok();
            if !ok { c.rt = None; c.art = None; moving = true; }
            // A la pantalla
            let dst = POINT { x: c.win.0, y: c.win.1 };
            let size = SIZE { cx: sw, cy: shh };
            let src = POINT::default();
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            if ok {
                let screen = GetDC(None);
                let _ = UpdateLayeredWindow(c.hwnd, Some(screen), Some(&dst), Some(&size), Some(dc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
                ReleaseDC(None, screen);
            }
        }
        c.hits = hits;
        // Sin línea de tiempo y sonando, el ecualizador necesita fotogramas
        if media.as_ref().is_some_and(|m| m.playing) && media_secs(true).is_none() && kh > 0.5 { moving = true; }
        moving
    });
    if let Some(h) = done { finish(h); return false; }
    moving
}

/// Sondeo cada 30 ms mientras está abierta: ratón dentro o fuera, clics fuera, Esc y lo que la cierra.
fn poll() {
    let mut pt = POINT::default();
    unsafe { let _ = GetCursorPos(&mut pt); }
    let down = |vk: VIRTUAL_KEY| unsafe { GetAsyncKeyState(vk.0 as i32) < 0 };
    let (l, r, esc) = (down(VK_LBUTTON), down(VK_RBUTTON), down(VK_ESCAPE));
    let other = ccenter::is_open() || tray::is_open() || popup::open_kind().is_some();
    let has = MEDIA.lock().unwrap().is_some();
    let why = CARD.with(|cc| {
        let mut cc = cc.borrow_mut();
        let c = cc.as_mut()?;
        if !c.open { return None; }
        let isl_w = island_px().map(|(r, _, _)| (r.2 - r.0) as f32 / c.s).unwrap_or(0.0);
        let sh = shape(c, isl_w);
        let (x, y) = ((pt.x - c.win.0) as f32 / c.s, (pt.y - c.win.1) as f32 / c.s);
        let inside = x >= sh.left && x <= sh.right && y >= sh.top - 2.0 && y <= sh.bottom + 2.0;
        let now = Instant::now();
        let inside = inside || std::env::var_os("MACBAR_CARDTEST").is_some();
        if inside { c.outside_since = None; } else if c.outside_since.is_none() { c.outside_since = Some(now); }
        if has { c.empty_since = None; } else if c.empty_since.is_none() { c.empty_since = Some(now); }
        let pressed_now = (l && !c.btn_prev.0) || (r && !c.btn_prev.1);
        let escape = esc && !c.btn_prev.2;
        c.btn_prev = (l, r, esc);
        let ms = |t: Option<Instant>| t.map(|t| t.elapsed().as_millis() as u32).unwrap_or(0);
        // En la prueba visual (MACBAR_CARDTEST) el ratón no está encima: cuenta como dentro
        let inside = inside || std::env::var_os("MACBAR_CARDTEST").is_some();
        should_close(&Watch {
            origin: c.origin, inside, outside_ms: ms(c.outside_since), dragging: c.drag.is_some(),
            click_outside: pressed_now && !inside, escape, fullscreen: crate::fullscreen(), other_panel: other,
            has_content: has, empty_ms: ms(c.empty_since),
        })
    });
    if let Some(w) = why { close(w); }
}

/// Fracción (0..1) de la barra de progreso bajo el ratón.
fn bar_frac(lp: LPARAM) -> f32 {
    CARD.with(|cc| cc.borrow().as_ref().map(|c| {
        let x = (lp.0 & 0xFFFF) as i16 as f32 / c.s;
        let (l, r, _) = c.bar_span;
        if r > l { ((x - l) / (r - l)).clamp(0.0, 1.0) } else { 0.0 }
    }).unwrap_or(0.0))
}

/// Posición que se ve ahora (con el salto pendiente si lo hay), en segundos, y la duración.
fn shown_pos() -> Option<(f32, f32)> {
    let playing = MEDIA.lock().unwrap().as_ref().is_some_and(|m| m.playing);
    let (pos, dur) = media_secs(playing)?;
    let hold = CARD.with(|c| c.borrow().as_ref().and_then(|c| c.hold));
    Some((hold.map(|(t, at)| t + if playing { at.elapsed().as_secs_f32() } else { 0.0 }).unwrap_or(pos).min(dur), dur))
}

/// Salta `d` segundos (atrás si es negativo) desde lo que se ve.
fn jump(d: f32) {
    let Some((pos, dur)) = shown_pos() else { return };
    let t = (pos + d).clamp(0.0, (dur - 1.0).max(0.0));
    seek_to(t);
    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.hold = Some((t, Instant::now())); });
}

fn can_seek_now() -> bool {
    MEDIA.lock().unwrap().as_ref().is_some_and(|m| CAN_SEEK.load(Ordering::Relaxed) && *SEEK_BAD_FOR.lock().unwrap() != m.title) && media_secs(false).is_some()
}

fn hit_at(lp: LPARAM) -> Option<u8> {
    CARD.with(|cc| {
        let cc = cc.borrow();
        let c = cc.as_ref()?;
        let x = (lp.0 & 0xFFFF) as i16 as f32 / c.s;
        let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / c.s;
        c.hits.iter().rev().find(|(r, _)| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom).map(|(_, i)| *i)
    })
}

/// ¿El punto (coordenadas de cliente) cae en la franja de la isla, sin botón? Un clic ahí la cierra.
fn in_island_band(lp: LPARAM) -> bool {
    CARD.with(|cc| cc.borrow().as_ref().is_some_and(|c| {
        let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / c.s;
        let oy = c.off.1 as f32 / c.s;
        if c.bottom { y >= oy + CARD_H - PILL_H } else { y <= oy + PILL_H }
    }))
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER => {
                if wp.0 == TIMER_POLL { poll(); }
                if wp.0 == TIMER_ANIM {
                    let _ = KillTimer(Some(hwnd), TIMER_ANIM);
                    if render() { SetTimer(Some(hwnd), TIMER_ANIM, 15, None); }
                    // Quieta y sonando: un repintado por segundo para el progreso
                    else if is_open() && MEDIA.lock().unwrap().as_ref().is_some_and(|m| m.playing) { SetTimer(Some(hwnd), TIMER_ANIM, 1000, None); }
                }
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_MOUSEMOVE => {
                // Arrastrando la barra de progreso: sigue al ratón aunque se salga de ella
                let dragging = CARD.with(|c| c.borrow().as_ref().is_some_and(|c| c.drag.is_some()));
                if dragging {
                    let fr = bar_frac(lp);
                    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.drag = Some(fr); });
                    SetTimer(Some(hwnd), TIMER_ANIM, 1, None);
                    return LRESULT(0);
                }
                let h = hit_at(lp);
                let ch = CARD.with(|c| c.borrow_mut().as_mut().map(|c| { let x = c.hover != h; c.hover = h; x }).unwrap_or(false));
                if ch { SetTimer(Some(hwnd), TIMER_ANIM, 1, None); }
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
                LRESULT(0)
            }
            0x02A3 => {
                CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.hover = None; });
                SetTimer(Some(hwnd), TIMER_ANIM, 1, None);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let h = hit_at(lp);
                CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.pressed = h; });
                if h == Some(5) {
                    let fr = bar_frac(lp);
                    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.drag = Some(fr); });
                    SetCapture(hwnd);
                }
                SetTimer(Some(hwnd), TIMER_ANIM, 1, None);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let pressed = CARD.with(|c| c.borrow_mut().as_mut().and_then(|c| c.pressed.take()));
                // Fin del arrastre: salta adonde se soltó
                let drag = CARD.with(|c| c.borrow_mut().as_mut().and_then(|c| c.drag.take().map(|fr| (fr, c.bar_span.2))));
                if let Some((fr, dur)) = drag {
                    if GetCapture() == hwnd { let _ = ReleaseCapture(); }
                    let t = fr * dur;
                    seek_to(t);
                    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.hold = Some((t, Instant::now())); });
                    SetTimer(Some(hwnd), TIMER_ANIM, 1, None);
                    return LRESULT(0);
                }
                let h = hit_at(lp);
                match h.filter(|h| Some(*h) == pressed) {
                    Some(1) => media(Cmd::Prev),
                    Some(2) => media(Cmd::Toggle),
                    Some(3) => media(Cmd::Next),
                    Some(6) => jump(-15.0),
                    Some(7) => jump(15.0),
                    Some(4) => { if let Some(m) = MEDIA.lock().unwrap().clone() { open_player(&m.app); } close(Why::Toggle); }
                    _ => if h.is_none() && pressed.is_none() && in_island_band(lp) { close(Why::Toggle); },
                }
                SetTimer(Some(hwnd), TIMER_ANIM, 1, None);
                LRESULT(0)
            }
            // Con el foco (abierta con clic): espacio pausa, flechas saltan 5 s o cambian el volumen, Esc cierra
            WM_KEYDOWN => {
                match VIRTUAL_KEY(wp.0 as u16) {
                    VK_SPACE => media(Cmd::Toggle),
                    VK_LEFT if can_seek_now() => jump(-5.0),
                    VK_RIGHT if can_seek_now() => jump(5.0),
                    VK_UP => live::nudge_volume(3),
                    VK_DOWN => live::nudge_volume(-3),
                    VK_ESCAPE => close(Why::Escape),
                    _ => {}
                }
                SetTimer(Some(hwnd), TIMER_ANIM, 1, None);
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                // Si otro se queda el ratón a mitad del arrastre, se cancela sin saltar
                if lp.0 != hwnd.0 as isize { CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.drag = None; c.pressed = None; }); }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_DISPLAYCHANGE | WM_DPICHANGED => { close(Why::BarMoved); LRESULT(0) }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Prueba visual: simula el ratón encima de un botón (sin pulsarlo).
pub fn test_hover(id: Option<u8>) {
    let h = CARD.with(|c| c.borrow_mut().as_mut().map(|c| { c.hover = id; c.hwnd }));
    if let Some(h) = h { unsafe { SetTimer(Some(h), TIMER_ANIM, 1, None); } }
}

/// Cuando cambia la música, la tarjeta abierta se repinta.
pub fn media_changed() {
    let h = CARD.with(|c| c.borrow().as_ref().filter(|c| c.open).map(|c| c.hwnd));
    if let Some(h) = h { unsafe { SetTimer(Some(h), TIMER_ANIM, 1, None); } }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w() -> Watch { Watch { origin: Origin::Click, inside: true, outside_ms: 0, dragging: false, click_outside: false, escape: false, fullscreen: false, other_panel: false, has_content: true, empty_ms: 0 } }

    #[test]
    fn cierre_por_raton() {
        assert_eq!(should_close(&w()), None);
        // Abierta con clic: aguanta 700 ms fuera
        assert_eq!(should_close(&Watch { inside: false, outside_ms: 600, ..w() }), None);
        assert_eq!(should_close(&Watch { inside: false, outside_ms: 700, ..w() }), Some(Why::Leave));
        // Abierta pasando el ratón: 250 ms
        assert_eq!(should_close(&Watch { origin: Origin::Hover, inside: false, outside_ms: 260, ..w() }), Some(Why::Leave));
        assert_eq!(should_close(&Watch { origin: Origin::Hover, inside: false, outside_ms: 200, ..w() }), None);
        // El ratón vuelve dentro: el plazo no cuenta
        assert_eq!(should_close(&Watch { inside: true, outside_ms: 5000, ..w() }), None);
    }

    #[test]
    fn cierre_por_eventos() {
        assert_eq!(should_close(&Watch { click_outside: true, ..w() }), Some(Why::ClickOutside));
        assert_eq!(should_close(&Watch { escape: true, ..w() }), Some(Why::Escape));
        assert_eq!(should_close(&Watch { fullscreen: true, dragging: true, ..w() }), Some(Why::Fullscreen));
        assert_eq!(should_close(&Watch { other_panel: true, ..w() }), Some(Why::OtherPanel));
        // Arrastrando: ni el clic fuera ni salir con el ratón la cierran
        assert_eq!(should_close(&Watch { dragging: true, click_outside: true, inside: false, outside_ms: 9000, ..w() }), None);
        // Sin contenido: «Nada sonando» 1,5 s y se cierra
        assert_eq!(should_close(&Watch { has_content: false, empty_ms: 1000, ..w() }), None);
        assert_eq!(should_close(&Watch { has_content: false, empty_ms: 1500, ..w() }), Some(Why::Empty));
    }

    #[test]
    fn colocacion() {
        let mon = (0, 0, 2560, 1600);
        // Isla centrada arriba: la tarjeta se centra bajo ella y empieza donde empieza la isla
        let (win, off) = place((1180, 8, 1380, 56), (760, 336), 32, 16, mon, false);
        assert_eq!(win, (900 - 32, 8, 900 + 760 + 32, 8 + 336 + 32));
        assert_eq!(off, (32, 0));
        // Barra abajo: crece hacia arriba y su borde de abajo coincide con el de la isla
        let (win, off) = place((1180, 1544, 1380, 1592), (760, 336), 32, 16, mon, true);
        assert_eq!(win.3, 1592);
        assert_eq!(win.1, 1592 - 336 - 32);
        assert_eq!(off, (32, 32));
        // Isla pegada a un borde: se encaja dentro del monitor
        let (win, off) = place((0, 8, 100, 56), (760, 336), 32, 16, mon, false);
        assert_eq!(win.0 + off.0, 16);
        // Monitor con coordenadas negativas (a la izquierda del principal)
        let (win, off) = place((-1300, 8, -1100, 56), (760, 336), 32, 16, (-1920, 0, 0, 1080), false);
        assert_eq!(win.0 + off.0, -1200 - 380);
        // Monitor más estrecho que la tarjeta: no revienta
        let (win, off) = place((100, 8, 200, 56), (760, 336), 32, 16, (0, 0, 500, 900), false);
        assert_eq!(win.0 + off.0, 16);
    }
}
