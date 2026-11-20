//! Tarjeta de la isla: la isla crece hacia abajo hasta convertirse en una tarjeta con controles
//! (como la Dynamic Island expandida). Ver PLAN_TARJETA_ISLA.md.
//!
//! Es una ventana propia con alfa por píxel (Direct2D sobre un DIB + `UpdateLayeredWindow`), así que
//! lo transparente deja pasar el ratón. Nace con la forma exacta de la isla: solo cuando su primer
//! fotograma ya está en pantalla, la barra deja de pintar la isla (`owns_island`). Al cerrar, al revés.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU32};

/// Ancho y alto máximos de la tarjeta (los de la música, DIP): la ventana tiene este tamaño y la
/// forma se anima al de cada pestaña.
pub const CARD_W: f32 = 380.0;
pub const CARD_H: f32 = 168.0;

/// Pestañas de la tarjeta (orden fijo).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab { Media, Timer, Shelf, Glance }

impl Tab { fn idx(self) -> u8 { match self { Tab::Media => 0, Tab::Timer => 1, Tab::Shelf => 2, Tab::Glance => 3 } } }

/// Alto máximo de la ventana (la bandeja con dos filas es lo más alto).
pub const WIN_H: f32 = 230.0;

/// Alto de la bandeja con `n` archivos: cabecera y una fila de 84 DIP por cada cuatro (vacía, la zona de soltar).
pub fn shelf_h(n: usize) -> f32 { if n == 0 { 168.0 } else { 52.0 + n.div_ceil(4) as f32 * 84.0 + 10.0 } }

/// Tamaño de la tarjeta en cada pestaña (DIP).
pub fn tab_size(t: Tab) -> (f32, f32) {
    match t { Tab::Media => (CARD_W, CARD_H), Tab::Timer => (340.0, 150.0), Tab::Shelf => (CARD_W, shelf_h(live::SHELF.lock().unwrap().len())), Tab::Glance => (360.0, 150.0) }
}

/// Pestaña con la que se abre: la del modo de la isla (1 = música, 2 = temporizador), si tiene contenido;
/// si no, la primera que lo tenga.
pub fn initial_tab(isl_mode: u8, has_media: bool, has_timer: bool, has_shelf: bool) -> Option<Tab> {
    match isl_mode {
        2 if has_timer => Some(Tab::Timer),
        1 if has_media => Some(Tab::Media),
        3 if has_shelf => Some(Tab::Shelf),
        _ if has_media => Some(Tab::Media),
        _ if has_timer => Some(Tab::Timer),
        _ if has_shelf => Some(Tab::Shelf),
        _ => None,
    }
}
/// Margen alrededor de la tarjeta para la sombra (salvo por el lado de la barra).
const M: f32 = 16.0;
const RADIUS: f32 = 24.0;
const TIMER_POLL: usize = 1;
const TIMER_ANIM: usize = 2;

/// Cómo se abrió: con clic aguanta más fuera antes de cerrarse que si se abrió pasando el ratón.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Origin { Click, #[allow(dead_code)] Hover, Drop }

/// Motivo del cierre.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Why { Leave, ClickOutside, Escape, Toggle, Empty, Fullscreen, OtherPanel, BarMoved, Alert }

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
pub fn grace_ms(o: Origin) -> u32 { match o { Origin::Click | Origin::Drop => 700, Origin::Hover => 250 } }

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
/// Modo de la isla ahora: 0 = nada, 1 = música, 2 = temporizador, 3 = bandeja, 4 = aviso.
pub static ISL_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// ¿La isla la pinta la tarjeta (la barra no debe pintarla)?
pub fn owns_island() -> bool { OWNS.load(Ordering::Relaxed) }

// ---------------------------------------------------------------- ajustes
/// Bits: 1 = el clic en la isla abre la tarjeta, 2 = se abre al pasar el ratón, 4 = Vistazo con la isla vacía.
/// 0xFF = sin leer todavía de config.ini.
static OPTS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0xFF);
pub const OPT_CLICK: u8 = 1;
pub const OPT_HOVER: u8 = 2;
pub const OPT_GLANCE: u8 = 4;

fn opts() -> u8 {
    let v = OPTS.load(Ordering::Relaxed);
    if v != 0xFF { return v; }
    let is = |k: &str, val: &str| sys::config_value(k).is_some_and(|x| x.eq_ignore_ascii_case(val));
    let mut b = 0;
    if !is("tarjeta_clic", "pausar") { b |= OPT_CLICK; }
    if is("tarjeta_hover", "si") { b |= OPT_HOVER; }
    if is("tarjeta_vistazo", "si") { b |= OPT_GLANCE; }
    OPTS.store(b, Ordering::Relaxed);
    b
}
pub fn opt(bit: u8) -> bool { opts() & bit != 0 }

/// Cambia un ajuste y lo guarda en config.ini.
pub fn toggle_opt(bit: u8) {
    let b = opts() ^ bit;
    OPTS.store(b, Ordering::Relaxed);
    match bit {
        OPT_CLICK => sys::set_config_value("tarjeta_clic", if b & bit != 0 { "abrir" } else { "pausar" }),
        OPT_HOVER => sys::set_config_value("tarjeta_hover", if b & bit != 0 { "si" } else { "no" }),
        _ => sys::set_config_value("tarjeta_vistazo", if b & bit != 0 { "si" } else { "no" }),
    }
}

/// ¿El clic en la isla abre la tarjeta? (ajuste «tarjeta_clic»; «pausar» = como antes)
pub fn click_opens() -> bool { opt(OPT_CLICK) }
pub fn hover_opens() -> bool { opt(OPT_HOVER) && !crate::profiles::presenting() }
pub fn glance_on() -> bool { opt(OPT_GLANCE) }

/// Valor (0..1) de la barra de un aviso, como en la isla.
fn hud_value(h: live::Hud) -> f32 {
    use live::Hud::*;
    match h {
        Volume(v, m) => if m { 0.0 } else { v },
        Bright(b) => b,
        Power(pc, _) | LowBat(pc) | Claude(pc) | Mem(pc) | Disk(pc) => pc as f32 / 100.0,
        Full => 1.0,
        _ => 0.0,
    }
}

/// Avisos que cierran la tarjeta para verse en la isla como siempre.
fn hud_closes(h: live::Hud, tab: Tab) -> bool {
    matches!(h, live::Hud::LowBat(_) | live::Hud::Present(true)) || (h == live::Hud::TimerDone && tab != Tab::Timer)
}

/// Avisos que no hace falta repetir dentro de la tarjeta (la propia pestaña ya lo enseña).
fn hud_skip(h: live::Hud, tab: Tab) -> bool {
    matches!(h, live::Hud::Shelf) || (tab == Tab::Timer && matches!(h, live::Hud::Pomo(..) | live::Hud::TimerDone))
}

// ---------------------------------------------------------------- superficie con alfa por píxel
struct Surface { dc: HDC, bmp: HBITMAP, old: HGDIOBJ, w: i32, h: i32, bits: *const u8 }

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
            Some(Surface { dc, bmp, old, w, h, bits: bits as *const u8 })
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) { unsafe { SelectObject(self.dc, self.old); let _ = DeleteObject(self.bmp.into()); let _ = DeleteDC(self.dc); } }
}

struct CardFonts { clock: IDWriteTextFormat, icon_s: IDWriteTextFormat, big: IDWriteTextFormat, lbl: IDWriteTextFormat, title: IDWriteTextFormat, sub: IDWriteTextFormat, small: IDWriteTextFormat, tiny: IDWriteTextFormat, icon: IDWriteTextFormat, icon_l: IDWriteTextFormat,
    // Los de la isla (para el primer y el último fotograma, idénticos a los de la barra)
    i_title: IDWriteTextFormat, i_small: IDWriteTextFormat, i_icon: IDWriteTextFormat }

/// ¿Windows tiene desactivadas las animaciones? (Accesibilidad > Efectos visuales; MACBAR_REDUCED=1 lo simula)
fn reduced_motion() -> bool {
    if std::env::var_os("MACBAR_REDUCED").is_some() { return true; }
    let mut on = windows::core::BOOL(1);
    unsafe { let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
    !on.as_bool()
}

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
    glow: [f32; 48],
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
    /// Tras un salto, la posición que se enseña mientras el reproductor se pone al día.
    hold: Option<(f32, Instant)>,
    /// Extremos de la barra de progreso (DIP) y duración, para convertir el ratón en tiempo.
    bar_span: (f32, f32, f32),
    /// Ventana que tenía el foco antes de abrir (se le devuelve al cerrar).
    prev_fg: HWND,
    tab: Tab,
    /// Pestaña de la que se viene, cuándo se cambió y hacia qué lado (+1 derecha, -1 izquierda).
    tab_prev: Option<(Tab, Instant, f32)>,
    /// Píldora de la pestaña activa (x animada, DIP relativos a la tarjeta).
    tab_x: f32,
    /// Cuenta atrás grande: los dígitos ruedan al cambiar.
    roll: Roll,
    /// Trazo de puntas redondas (anillos).
    cap: Option<ID2D1StrokeStyle>,
    /// Trazo discontinuo (zona de soltar).
    dash: Option<ID2D1StrokeStyle>,
    /// Arrastrando archivos encima de la tarjeta.
    dropping: bool,
    /// Iconos de la bandeja en la GPU (versión de la bandeja, iconos).
    shelf_bmps: (u64, Vec<Option<ID2D1Bitmap>>),
    /// Cuándo apareció cada archivo de la bandeja (los nuevos entran uno tras otro).
    seen: HashMap<String, Instant>,
    /// Pulsado sobre un archivo: cuál y dónde (para empezar a arrastrarlo fuera).
    press_item: Option<(usize, i32, i32)>,
    /// Hay un menú nativo abierto (sus clics no cuentan como «clic fuera»).
    menu_open: bool,
    /// Aviso dentro de la tarjeta: el último (para su salida), cuánto se ve (0..1) y su barra animada.
    hud_last: Option<live::Hud>,
    hud_k: f32,
    hud_val: f32,
    /// Windows tiene las animaciones desactivadas (se mira al abrir).
    reduced: bool,
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
            clock: mk(FONT, 36.0, DWRITE_FONT_WEIGHT_SEMI_BOLD), icon_s: mk(ICONS, 13.0, DWRITE_FONT_WEIGHT_NORMAL), big: mk(FONT, 20.0, DWRITE_FONT_WEIGHT_SEMI_BOLD), lbl: mk(FONT, 12.5, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            title: mk(FONT, 15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD), sub: mk(FONT, 12.5, DWRITE_FONT_WEIGHT_NORMAL), small: mk(FONT, 11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD), tiny: mk(FONT, 8.5, DWRITE_FONT_WEIGHT_BOLD),
            icon: mk(ICONS, 17.0, DWRITE_FONT_WEIGHT_NORMAL), icon_l: mk(ICONS, 24.0, DWRITE_FONT_WEIGHT_NORMAL),
            i_title: mk(FONT, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD), i_small: mk(FONT, 11.5, DWRITE_FONT_WEIGHT_NORMAL), i_icon: mk(ICONS, 12.5, DWRITE_FONT_WEIGHT_NORMAL),
        };
        let dash = fac.CreateStrokeStyle(&D2D1_STROKE_STYLE_PROPERTIES { startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, dashCap: D2D1_CAP_STYLE_ROUND, dashStyle: D2D1_DASH_STYLE_DASH, ..Default::default() }, None).ok();
        let cap = fac.CreateStrokeStyle(&D2D1_STROKE_STYLE_PROPERTIES { startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, ..Default::default() }, None).ok();
        let old = Instant::now() - Duration::from_secs(5);
        let c = Card { hwnd, fac, rt: None, surf: None, dw, cache: RefCell::new(Caches::default()), f, win: (0, 0, 1, 1), off: (0, 0), s: 1.0, bottom: false,
            origin: Origin::Click, open: false, opened: old, closed_at: old, w: 0.0, vw: 0.0, h: PILL_H, vh: 0.0, hover: None, pressed: None, glow: [0.0; 48], hits: Vec::new(),
            outside_since: None, empty_since: None, btn_prev: (false, false, false), art: None, art_at: old, art_k: 1.0, shown_title: Default::default(), prev_title: Default::default(),
            title_at: old, lines: (String::new(), 0.0, Vec::new()), play: (false, old), last: Instant::now(),
            drag: None, bar_k: 0.0, hold: None, bar_span: (0.0, 0.0, 0.0), prev_fg: HWND::default(),
            tab: Tab::Media, tab_prev: None, tab_x: f32::NAN, roll: Roll::default(),
            cap, dash, dropping: false, shelf_bmps: (u64::MAX, Vec::new()), seen: HashMap::new(), press_item: None, menu_open: false, hud_last: None, hud_k: 0.0, hud_val: 0.0, reduced: false };
        CARD.with(|x| *x.borrow_mut() = Some(c));
        // Soltar archivos sobre la tarjeta: a la bandeja
        dnd::register(hwnd, true);
    }
    Some(())
}

/// Abre la tarjeta desde la isla (o la cierra si ya estaba abierta).
fn dbg(m: &str) { if std::env::var_os("MACBAR_CARDTEST").is_some() { use std::io::Write; if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("macbar_card.log")) { let _ = writeln!(f, "{m}"); } } }

pub fn open(origin: Origin) { open_tab(origin, None) }

/// Abre la tarjeta en una pestaña (None = la del modo de la isla).
pub fn open_tab(origin: Origin, want: Option<Tab>) {
    let (has_media, has_timer, has_shelf) = (MEDIA.lock().unwrap().is_some(), live::timer_left().is_some(), !live::SHELF.lock().unwrap().is_empty());
    dbg(&format!("open: fullscreen={} media={has_media} timer={has_timer} abierta={}", crate::fullscreen(), is_open()));
    if crate::fullscreen() { return; }
    if is_open() { if origin != Origin::Drop { close(Why::Toggle); } return; }
    let tab = match want {
        Some(Tab::Timer) if has_timer => Tab::Timer,
        Some(Tab::Media) if has_media => Tab::Media,
        // Al arrastrar archivos se abre la bandeja aunque esté vacía (es donde se sueltan)
        Some(Tab::Shelf) if has_shelf || origin == Origin::Drop => Tab::Shelf,
        Some(Tab::Glance) => Tab::Glance,
        _ => match initial_tab(ISL_MODE.load(Ordering::Relaxed), has_media, has_timer, has_shelf) { Some(t) => t, None => return },
    };
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
    let size = ((CARD_W * s).round() as i32, (WIN_H * s).round() as i32);
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
        c.tab = tab;
        c.tab_prev = None;
        c.tab_x = f32::NAN;
        c.dropping = origin == Origin::Drop;
        c.press_item = None;
        c.menu_open = false;
        c.hud_last = None;
        c.hud_k = 0.0;
        c.reduced = reduced_motion();
        // Los archivos que ya estaban no hacen su entrada
        let old = Instant::now() - Duration::from_secs(5);
        c.seen = live::SHELF.lock().unwrap().iter().map(|x| (x.0.clone(), old)).collect();
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
    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.open = false; c.art = None; c.rt = None; c.shelf_bmps.0 = u64::MAX; });
}

/// Forma actual de la tarjeta (DIP, dentro de la ventana).
fn shape(c: &Card, isl_w: f32) -> D2D_RECT_F {
    let s = c.s;
    let (ox, oy) = (c.off.0 as f32 / s, c.off.1 as f32 / s);
    // Centro: del de la isla al de la tarjeta según lo que ha crecido
    let isl_cx = island_px().map(|(r, _, _)| ((r.0 + r.2) as f32 / 2.0 - c.win.0 as f32) / s).unwrap_or(ox + CARD_W / 2.0);
    let tw = tab_size(c.tab).0;
    let kw = if tw > isl_w + 1.0 { ((c.w - isl_w) / (tw - isl_w)).clamp(0.0, 1.0) } else { 1.0 };
    let cx = lerp(isl_cx, ox + CARD_W / 2.0, kw);
    let w = c.w.max(0.0);
    if c.bottom {
        let b = oy + WIN_H;
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

/// Anillo de progreso: fondo tenue y un arco que se llena en el sentido del reloj desde arriba.
#[allow(clippy::too_many_arguments)]
fn ring(rt: &ID2D1RenderTarget, fac: &ID2D1Factory, cap: Option<&ID2D1StrokeStyle>, cx: f32, cy: f32, r: f32, sw: f32, fr: f32, fg: &ID2D1Brush, bg: &ID2D1Brush) {
    unsafe {
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
        rt.DrawEllipse(&e, bg, sw, None);
        if fr >= 0.999 { rt.DrawEllipse(&e, fg, sw, None); return; }
        if fr <= 0.002 { return; }
        let Ok(geo) = fac.CreatePathGeometry() else { return };
        let Ok(sk) = geo.Open() else { return };
        let a = fr * std::f32::consts::TAU;
        sk.BeginFigure(Vector2 { X: cx, Y: cy - r }, D2D1_FIGURE_BEGIN_HOLLOW);
        sk.AddArc(&D2D1_ARC_SEGMENT { point: Vector2 { X: cx + r * a.sin(), Y: cy - r * a.cos() }, size: D2D_SIZE_F { width: r, height: r }, rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE, arcSize: if fr > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL } });
        sk.EndFigure(D2D1_FIGURE_END_OPEN);
        let _ = sk.Close();
        rt.DrawGeometry(&geo, fg, sw, cap);
    }
}

/// Dibuja `f` escalado `sc` alrededor de (cx, cy) (botones que se hunden al pulsarlos).
fn scaled(rt: &ID2D1RenderTarget, cx: f32, cy: f32, sc: f32, f: impl FnOnce()) {
    unsafe {
        let mut base = Matrix3x2::default();
        rt.GetTransform(&mut base);
        if sc < 0.999 { rt.SetTransform(&(Matrix3x2 { M11: sc, M12: 0.0, M21: 0.0, M22: sc, M31: cx * (1.0 - sc), M32: cy * (1.0 - sc) } * base)); }
        f();
        rt.SetTransform(&base);
    }
}

/// Pinta un fotograma y lo sube a la ventana. Devuelve true mientras algo se mueve.
fn render() -> bool {
    let mut finished: Option<HWND> = None;
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
            c.shelf_bmps.0 = u64::MAX;
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

        let media = MEDIA.lock().unwrap().clone();
        let timer = live::timer_left();
        // Temporizador recién terminado (para «Repetir»)
        let done = if timer.is_none() { (*live::LAST_DONE.lock().unwrap()).filter(|d| d.1.elapsed().as_secs() < 60) } else { None };
        // La pestaña se quedó sin contenido: se pasa a la otra si tiene
        let shelf: Vec<(String, Option<Arc<Vec<u8>>>)> = live::SHELF.lock().unwrap().clone();
        let has = |t: Tab| match t { Tab::Media => media.is_some(), Tab::Timer => timer.is_some() || done.is_some(), Tab::Shelf => !shelf.is_empty(), Tab::Glance => false };
        if c.open && !has(c.tab) && !(c.tab == Tab::Shelf && c.dropping) {
            // La pestaña se quedó vacía: a la siguiente que tenga algo (la bandeja vacía se queda si se acaba de vaciar)
            let to = [Tab::Media, Tab::Timer, Tab::Shelf].into_iter().find(|t| *t != c.tab && has(*t));
            let keep = c.tab == Tab::Shelf || c.tab == Tab::Glance || (c.tab == Tab::Timer && media.is_none() && shelf.is_empty());
            if let Some(t) = to.filter(|_| !keep) { c.tab_prev = Some((c.tab, now, if t.idx() > c.tab.idx() { 1.0 } else { -1.0 })); c.tab = t; }
        }
        let tabs: Vec<Tab> = [Tab::Media, Tab::Timer, Tab::Shelf, Tab::Glance].into_iter().filter(|t| has(*t) || *t == c.tab).collect();
        // Iconos de la bandeja en la GPU, solo cuando cambia
        let sv = live::SHELF_VER.load(Ordering::Relaxed);
        if c.shelf_bmps.0 != sv {
            let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
            c.shelf_bmps = (sv, shelf.iter().map(|(_, px)| px.as_ref().and_then(|px| unsafe { rt.CreateBitmap(D2D_SIZE_U { width: ART_PX, height: ART_PX }, Some(px.as_ptr() as _), ART_PX * 4, &bp).ok() })).collect());
        }
        // Archivos nuevos: entran uno tras otro (40 ms entre cada uno)
        let mut k_new = 0;
        for (p, _) in &shelf { if !c.seen.contains_key(p) { c.seen.insert(p.clone(), now + Duration::from_millis(40 * k_new)); k_new += 1; } }
        // Cambio de pestaña: fundido cruzado con un desplazamiento hacia su lado
        let tk = c.tab_prev.map(|(_, t, _)| ease_out(t.elapsed().as_secs_f32() / 0.22)).unwrap_or(1.0);
        if tk >= 1.0 { c.tab_prev = None; } else { moving = true; }
        let (tab_w, tab_h) = tab_size(c.tab);

        // ---- Forma: al abrir, primero se ensancha y 30 ms después cae; al cerrar, al revés
        let (tw, th) = if c.open {
            (tab_w, if c.opened.elapsed().as_secs_f32() > 0.03 { tab_h } else { PILL_H })
        } else {
            (if c.closed_at.elapsed().as_secs_f32() > 0.03 { isl_w } else { c.w.max(isl_w) }, PILL_H)
        };
        // Con las animaciones de Windows desactivadas: sin muelles ni viajes, solo un fundido de 120 ms
        let fade = if c.reduced {
            if c.open { c.w = tab_w; c.h = tab_h; }
            let e = if c.open { c.opened } else { c.closed_at }.elapsed().as_secs_f32();
            let k = ease_out(e / 0.12);
            if k < 1.0 { moving = true; } else if !c.open { finished = Some(c.hwnd); }
            if c.open { k } else { 1.0 - k }
        } else {
            moving |= spring(&mut c.w, &mut c.vw, tw, dt);
            moving |= spring(&mut c.h, &mut c.vh, th, dt);
            if !c.open && !moving { finished = Some(c.hwnd); }
            1.0
        };

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
        for i in 0..48 {
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
        let kh = ((c.h - PILL_H) / (tab_h - PILL_H)).clamp(0.0, 1.0);
        let kw = if tab_w > isl_w + 1.0 { ((c.w - isl_w) / (tab_w - isl_w)).clamp(0.0, 1.0) } else { 1.0 };
        // Contenido de la isla: se va en cuanto empieza a crecer; el de la tarjeta entra pasado el 55 %
        let a_isl = if c.reduced { 0.0 } else { (1.0 - kw.max(kh) * 3.2).clamp(0.0, 1.0) };
        let row = |i: f32| ease_out(((kh - 0.55 - 0.06 * i) / 0.4).clamp(0.0, 1.0));
        let (ox, oy) = (c.off.0 as f32 / s, c.off.1 as f32 / s);
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
            popup::push_alpha(&rt, pill_a * fade);
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
            // (solo en la pestaña de música; en la del temporizador, un velo naranja muy tenue)
            let media_a = (if c.tab == Tab::Media { tk } else { 0.0 }) + c.tab_prev.filter(|x| x.0 == Tab::Media).map(|_| 1.0 - tk).unwrap_or(0.0);
            if kc > 0.01 && media.is_some() && media_a > 0.01 {
                let g = p.vgrad(sh.top, sh.top + (sh.bottom - sh.top) * 0.8, rgba(tint[0] * 0.55, tint[1] * 0.55, tint[2] * 0.55, 0.42 * kc * media_a), rgba(0.0, 0.0, 0.0, 0.0));
                rt.FillRoundedRectangle(&rrect(sh, rad), &g);
            }
            // Borde fino con luz arriba (el de la isla)
            let px = 1.0 / s;
            let edge = p.vgrad(sh.top, sh.bottom, rgba(1.0, 1.0, 1.0, 0.14), rgba(1.0, 1.0, 1.0, lerp(0.03, 0.05, kh)));
            let inner = rect(sh.left + px / 2.0, sh.top + px / 2.0, sh.right - px / 2.0, sh.bottom - px / 2.0);
            rt.DrawRoundedRectangle(&rrect(inner, rad - px / 2.0), &edge, px, None);
            rt.PushAxisAlignedClip(&rect(sh.left + 1.0, sh.top, sh.right - 1.0, sh.bottom), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);

            let wa = |a: f32| p.brush(rgba(1.0, 1.0, 1.0, a));
            let isl_mode = ISL_MODE.load(Ordering::Relaxed);
            let bottom = c.bottom;
            // Esquina y tamaño de la tarjeta en cada pestaña
            let place_of = |t: Tab| { let (w, h) = tab_size(t); (ox + (CARD_W - w) / 2.0, if bottom { oy + WIN_H - h } else { oy }, w, h) };
            // Color del temporizador: verde en los descansos, rojo en los últimos 10 s de uno normal
            let pomo = live::POMO.load(Ordering::Relaxed);
            let paused = live::timer_paused();
            let tcol = |left: u32| if pomo == 2 { (0.30, 0.85, 0.45) } else if pomo == 0 && left <= 10 { (1.0, 0.36, 0.33) } else { (1.0, 0.62, 0.22) };
            let col_b = |c3: (f32, f32, f32), a: f32| p.brush(rgba(c3.0, c3.1, c3.2, a));
            let cap = c.cap.clone();
            let art_from = (isl.left + 4.0, isl_cy - 11.0, 22.0, 11.0);

            // ---- Lo que enseñaba la isla (primer y último fotograma idénticos a los de la barra)
            if a_isl > 0.01 {
                let (il, iw) = (isl.left, isl.right - isl.left);
                match isl_mode {
                    1 => if let Some(m) = &media {
                        if isl_open < 0.5 {
                            let fade = (1.0 - isl_open * 2.0) * a_isl;
                            if let Some((left, _)) = timer {
                                // Música con temporizador: la cuenta en lugar del ecualizador
                                let cnt = live::mmss(left);
                                txt(&p, &c.f.i_title, &cnt, il + iw - 13.0 - p.width(&c.f.i_title, &cnt), isl.top, PILL_H, &col_b((1.0, 0.62, 0.22), fade));
                            } else {
                                let wb = p.brush(rgba(tint[0], tint[1], tint[2], 0.96 * fade));
                                draw_eq(&rt, &eq, il + iw - 13.0 - 22.0, isl_cy, &wb);
                            }
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
                        // Cerrando desde otra pestaña: la carátula de la isla vuelve con un fundido
                        if c.tab != Tab::Media {
                            if let Some((_, bmp)) = &c.art { draw_art(&rt, bmp, art_from.0, art_from.1, 22.0, 11.0, a_isl); }
                        }
                    },
                    2 => if let Some((left, total)) = timer {
                        let col = tcol(left);
                        let cnt = live::mmss(left);
                        let cw = p.width(&c.f.i_title, &cnt);
                        txt(&p, &c.f.i_title, &cnt, il + iw - 14.0 - cw, isl.top, PILL_H, &col_b(col, a_isl));
                        if c.tab != Tab::Timer {
                            ring(&rt, &c.fac, cap.as_ref(), il + 20.0, isl_cy, 7.5, 2.2, left as f32 / total.max(1) as f32, &col_b(col, a_isl), &col_b(col, 0.25 * a_isl));
                        }
                        if isl_open > 0.3 {
                            let k = ((isl_open - 0.3) / 0.7).clamp(0.0, 1.0) * a_isl;
                            let lbl = match pomo { 1 => i18n::t("Concentración"), 2 => i18n::t("Descanso"), _ => i18n::t("Temporizador") };
                            txt(&p, &c.f.i_small, lbl, il + 38.0, isl.top, PILL_H, &wa(0.6 * k));
                            glyph_c(&p, &c.f.i_icon, "\u{E711}", il + iw - 14.0 - cw - 8.0 - 13.0, isl_cy, &wa(0.96 * k));
                        }
                    },
                    3 => {
                        let cnt = shelf.len().to_string();
                        glyph_c(&p, &c.f.i_icon, "\u{E8B7}", il + 20.0, isl_cy, &wa(0.96 * a_isl));
                        if isl_open < 0.5 { txt(&p, &c.f.i_title, &cnt, il + iw - 14.0 - p.width(&c.f.i_title, &cnt), isl.top, PILL_H, &wa(0.96 * (1.0 - isl_open * 2.0) * a_isl)); }
                    }
                    _ => {}
                }
            }

            // ---- Pestañas: la que se va y la que entra
            let tabs_on = tabs.len() > 1;
            let mut layers: Vec<(Tab, f32, f32, bool)> = Vec::new();
            if let Some((pt, _, dir)) = c.tab_prev { layers.push((pt, 1.0 - tk, -12.0 * tk * dir, false)); }
            layers.push((c.tab, tk, 12.0 * (1.0 - tk) * c.tab_prev.map(|x| x.2).unwrap_or(0.0), c.open && tk > 0.9));
            let lift = |k: f32| 6.0 * (1.0 - k);
            for (tab, la, dx, live_t) in layers {
                if la <= 0.01 { continue; }
                popup::push_alpha(&rt, la);
                let tm = Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: dx, M32: 0.0 };
                rt.SetTransform(&tm);
                let (cl, ct, cw, chh) = place_of(tab);
                match tab {
                    Tab::Media => {
                        // La carátula viaja de la isla a la tarjeta como una sola pieza
                        let pop = { let e = c.art_at.elapsed().as_secs_f32(); if e < 0.45 { moving = true; 0.88 + 0.12 * ease_back(e / 0.45) } else { 1.0 } };
                        let asz = 76.0 * c.art_k * pop;
                        let art_to = (cl + 16.0 + (76.0 - asz) / 2.0, ct + 16.0 + (76.0 - asz) / 2.0, asz, 14.0);
                        let ka = if c.tab == Tab::Media { kh } else { 1.0 };
                        let (ax, ay, asz2, arad) = (lerp(art_from.0, art_to.0, ka), lerp(art_from.1, art_to.1, ka), lerp(art_from.2, art_to.2, ka), lerp(art_from.3, art_to.3, ka));
                        match (&c.art, &media) {
                            (Some((_, bmp)), Some(_)) => {
                                if ka > 0.3 { rt.FillRoundedRectangle(&rrect(rect(ax, ay + 3.0, ax + asz2, ay + asz2 + 3.0), arad), &p.brush(rgba(0.0, 0.0, 0.0, 0.35 * ka))); }
                                draw_art(&rt, bmp, ax, ay, asz2, arad, 1.0);
                            }
                            (None, Some(_)) => {
                                rt.FillRoundedRectangle(&rrect(rect(ax, ay, ax + asz2, ay + asz2), arad), &p.brush(rgba(1.0, 1.0, 1.0, 0.10)));
                                if ka > 0.5 { glyph_c(&p, &c.f.icon_l, "\u{E8D6}", ax + asz2 / 2.0, ay + asz2 / 2.0, &wa(0.6 * row(0.0))); }
                            }
                            _ => {}
                        }
                        if ka > 0.9 && live_t && media.is_some() { hits.push((rect(art_to.0, art_to.1, art_to.0 + 76.0, art_to.1 + 76.0), 4)); }
                        if let Some(m) = &media {
                            let tx = cl + 16.0 + 76.0 + 14.0;
                            // Con pestañas arriba a la derecha, el título deja su hueco
                            let maxw = cl + cw - 16.0 - tx - if tabs_on { 64.0 } else { 0.0 };
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
                                        // Arrastrando: el destino; tras un salto: el destino mientras el reproductor se pone al día
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
                                        txt(&p, &c.f.small, &rem, cl + cw - 16.0 - rw, y - 8.0, 16.0, &tb);
                                        let (l, r) = (cl + 16.0 + 40.0, cl + cw - 16.0 - 44.0);
                                        if live_t { c.bar_span = (l, r, dur); }
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
                                                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: fx, Y: y + 1.0 }, radiusX: kr + 0.5, radiusY: kr + 0.5 }, &p.brush(rgba(0.0, 0.0, 0.0, 0.35 * k2)));
                                                rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: fx, Y: y }, radiusX: kr, radiusY: kr }, &wa(k2));
                                            }
                                            if k2 > 0.9 && live_t { hits.push((rect(l - 8.0, y - 11.0, r + 8.0, y + 11.0), 5)); }
                                        }
                                    }
                                    None => {
                                        let wb = p.brush(rgba(tint[0], tint[1], tint[2], 0.96 * k2));
                                        draw_eq(&rt, &eq, cl + cw / 2.0 - 11.0, y, &wb);
                                    }
                                }
                            }
                            // Controles: anterior, reproducir o pausa y siguiente (y ±15 s si se puede saltar)
                            let k3 = row(3.0);
                            if k3 > 0.01 {
                                let y = ct + 143.0 + lift(k3);
                                let ccx = cl + cw / 2.0;
                                let pp = { let e = c.play.1.elapsed().as_secs_f32(); if e < 0.32 { moving = true; 0.7 + 0.3 * ease_back(e / 0.32) } else { 1.0 } };
                                for (id, dx, gl, en, big) in [(1u8, -60.0, "\u{E892}", m.can_prev, false), (2, 0.0, if m.playing { "\u{E769}" } else { "\u{E768}" }, true, true), (3, 60.0, "\u{E893}", m.can_next, false)] {
                                    let bx = ccx + dx;
                                    let r = if big { 21.0 } else { 17.0 };
                                    let gk = c.glow[id as usize];
                                    if en && gk > 0.01 { rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: bx, Y: y }, radiusX: r, radiusY: r }, &wa(0.10 * gk * k3)); }
                                    let mut sc = if c.pressed == Some(id) && c.hover == Some(id) { 0.88 } else { 1.0 };
                                    if id == 2 { sc *= pp; }
                                    scaled(&rt, bx, y, sc, || glyph_c(&p, if big { &c.f.icon_l } else { &c.f.icon }, gl, bx, y, &wa(if en { 0.96 } else { 0.25 } * k3)));
                                    if en && k3 > 0.9 && live_t { hits.push((rect(bx - r, y - r, bx + r, y + r), id)); }
                                }
                                if can_seek {
                                    for (id, dx, back) in [(6u8, -118.0, true), (7, 118.0, false)] {
                                        let bx = ccx + dx;
                                        let gk = c.glow[id as usize];
                                        if gk > 0.01 { rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: bx, Y: y }, radiusX: 16.0, radiusY: 16.0 }, &wa(0.10 * gk * k3)); }
                                        let sc = if c.pressed == Some(id) && c.hover == Some(id) { 0.88 } else { 1.0 };
                                        scaled(&rt, bx, y, sc, || skip_icon(&rt, &c.fac, &p, &c.f.tiny, bx, y, back, &wa(0.9 * k3)));
                                        if k3 > 0.9 && live_t { hits.push((rect(bx - 16.0, y - 16.0, bx + 16.0, y + 16.0), id)); }
                                    }
                                }
                            }
                        } else {
                            // La música se cerró: lo dice un momento y luego la tarjeta se recoge
                            let t = i18n::t("Nada sonando");
                            let tw = p.width(&c.f.title, t);
                            txt(&p, &c.f.title, t, cl + cw / 2.0 - tw / 2.0, ct + chh / 2.0 - 9.0, 18.0, &wa(0.6 * row(1.0)));
                        }
                    }
                    Tab::Glance => {
                        let k1 = row(1.0);
                        let t = GetLocalTime();
                        // Hora grande y segundos al lado, en gris
                        let hm = format!("{}:{:02}", t.wHour, t.wMinute);
                        txt(&p, &c.f.clock, &hm, cl + 20.0, ct + 18.0 + lift(k1), 40.0, &wa(0.96 * k1));
                        let hw = p.width(&c.f.clock, &hm);
                        txt(&p, &c.f.sub, &format!(":{:02}", t.wSecond), cl + 22.0 + hw, ct + 30.0 + lift(k1), 24.0, &wa(0.45 * k1));
                        let (wd, mo) = (i18n::weekday(t.wDayOfWeek as usize), i18n::month(t.wMonth as usize + 11));
                        let date = if i18n::en() { format!("{wd}, {mo} {}", t.wDay) } else { format!("{wd}, {} de {mo}", t.wDay) };
                        let date: String = date.chars().enumerate().map(|(i, ch)| if i == 0 { ch.to_uppercase().next().unwrap_or(ch) } else { ch }).collect();
                        let k2 = row(2.0);
                        txt(&p, &c.f.sub, &date, cl + 20.0, ct + 62.0 + lift(k2), 16.0, &wa(0.6 * k2));
                        // A la derecha: el tiempo, la batería y el uso semanal de Claude
                        // Con el selector de pestañas arriba a la derecha, el tiempo se aparta
                        let rx0 = cl + cw - 20.0 - if tabs_on { 30.0 * tabs.len() as f32 + 4.0 } else { 0.0 };
                        if let Some(w) = *WEATHER.lock().unwrap() {
                            let temp = format!("{}°", w.temp.round() as i32);
                            let tw = p.width(&c.f.big, &temp);
                            txt(&p, &c.f.big, &temp, rx0 - tw, ct + 20.0 + lift(k1), 26.0, &wa(0.96 * k1));
                            weather::draw_icon(&rt, &p, &c.f.icon_l, rx0 - tw - 34.0, ct + 20.0 + lift(k1), 30.0, 26.0, w.code, w.is_day, &wa(0.9 * k1));
                        }
                        let mut gx = rx0;
                        let usage = if sys::demo() { Some(36.0) } else { GFX.with(|g| g.try_borrow().ok().and_then(|g| g.as_ref().and_then(|g| g.usage.map(|u| u.0)))) };
                        // Uso semanal de Claude (naranja Claude) y batería (verde)
                        for (val, col) in [(usage.map(|u| u / 100.0), (0.85, 0.47, 0.34)), (battery().map(|b| b.0 as f32 / 100.0), (0.30, 0.85, 0.45))] {
                            let Some(v) = val else { continue };
                            let pct = format!("{}%", (v * 100.0).round() as i32);
                            let pw = p.width(&c.f.small, &pct);
                            txt(&p, &c.f.small, &pct, gx - pw, ct + 58.0 + lift(k2), 16.0, &wa(0.8 * k2));
                            let rcx = gx - pw - 12.0;
                            ring(&rt, &c.fac, cap.as_ref(), rcx, ct + 66.0 + lift(k2), 6.5, 2.2, v.clamp(0.0, 1.0), &col_b(col, k2), &col_b(col, 0.22 * k2));
                            gx = rcx - 16.0;
                        }
                        // Atajos: cafeína, pomodoro, cuentagotas y captura
                        let k3 = row(3.0);
                        if k3 > 0.01 {
                            let by = ct + chh - 46.0 + lift(k3);
                            let mut bx = cl + 20.0;
                            let caf = live::caffeine_on();
                            for (id, gl, lbl, on) in [(40u8, "\u{EC32}", i18n::t("Cafeína"), caf), (41, "\u{E916}", i18n::t("Pomodoro"), false), (42, "\u{E790}", i18n::t("Color"), false), (43, "\u{E722}", i18n::t("Captura"), false)] {
                                let w = p.width(&c.f.small, lbl) + 34.0;
                                let r = rect(bx, by, bx + w, by + 30.0);
                                let gk = c.glow[id as usize];
                                let sc = if c.pressed == Some(id) && c.hover == Some(id) { 0.92 } else { 1.0 };
                                scaled(&rt, bx + w / 2.0, by + 15.0, sc, || {
                                    let bg = if on { col_b((1.0, 0.62, 0.22), (0.85 + 0.15 * gk) * k3) } else { wa((0.10 + 0.08 * gk) * k3) };
                                    rt.FillRoundedRectangle(&rrect(r, 15.0), &bg);
                                    let ink = if on { p.brush(rgba(0.0, 0.0, 0.0, 0.9 * k3)) } else { wa(0.92 * k3) };
                                    glyph_c(&p, &c.f.icon_s, gl, bx + 15.0, by + 15.0, &ink);
                                    txt(&p, &c.f.small, lbl, bx + 26.0, by, 30.0, &ink);
                                });
                                if k3 > 0.9 && live_t { hits.push((r, id)); }
                                bx += w + 6.0;
                            }
                        }
                    }                    Tab::Shelf => {
                        let n = shelf.len();
                        let k1 = row(1.0);
                        // Cabecera: «Bandeja», cuántos y, a la derecha, Compartir y Vaciar
                        txt(&p, &c.f.title, i18n::t("Bandeja"), cl + 18.0, ct + 16.0 + lift(k1), 18.0, &wa(0.96 * k1));
                        if n > 0 {
                            let tw = p.width(&c.f.title, i18n::t("Bandeja"));
                            txt(&p, &c.f.sub, &n.to_string(), cl + 18.0 + tw + 8.0, ct + 17.0 + lift(k1), 18.0, &wa(0.45 * k1));
                            let mut bx = cl + cw - 16.0 - if tabs_on { 30.0 * tabs.len() as f32 + 6.0 } else { 0.0 };
                            for (id, gl, lbl) in [(29u8, "\u{E74D}", i18n::t("Vaciar")), (28, "\u{E72D}", i18n::t("Compartir"))] {
                                let w = p.width(&c.f.small, lbl) + 36.0;
                                let r = rect(bx - w, ct + 12.0, bx, ct + 38.0);
                                let gk = c.glow[id as usize];
                                let sc = if c.pressed == Some(id) && c.hover == Some(id) { 0.92 } else { 1.0 };
                                scaled(&rt, (r.left + r.right) / 2.0, ct + 25.0, sc, || {
                                    rt.FillRoundedRectangle(&rrect(r, 13.0), &wa((0.10 + 0.08 * gk) * k1));
                                    glyph_c(&p, &c.f.icon_s, gl, r.left + 15.0, ct + 25.0, &wa(0.9 * k1));
                                    txt(&p, &c.f.small, lbl, r.left + 26.0, ct + 12.0, 26.0, &wa(0.9 * k1));
                                });
                                if k1 > 0.9 && live_t { hits.push((r, id)); }
                                bx -= w + 6.0;
                            }
                        }
                        let k2 = row(2.0);
                        // Zona de soltar: vacía, o encima de los archivos mientras se arrastra algo
                        if n == 0 || c.dropping {
                            let z = rect(cl + 16.0, ct + 50.0, cl + cw - 16.0, ct + chh - 14.0);
                            let beat = if c.dropping { moving = true; 0.5 + 0.5 * (c.opened.elapsed().as_secs_f32() * 3.0).sin() } else { 0.0 };
                            if n > 0 { rt.FillRoundedRectangle(&rrect(z, 14.0), &p.brush(rgba(0.0, 0.0, 0.0, 0.72 * k2))); }
                            let acc = crate::profiles::accent(1.0);
                            let edge = if c.dropping { p.brush(rgba(acc.r, acc.g, acc.b, (0.55 + 0.35 * beat) * k2)) } else { wa(0.28 * k2) };
                            if c.dropping { rt.FillRoundedRectangle(&rrect(z, 14.0), &p.brush(rgba(acc.r, acc.g, acc.b, (0.06 + 0.06 * beat) * k2))); }
                            rt.DrawRoundedRectangle(&rrect(z, 14.0), &edge, 1.5, c.dash.as_ref());
                            let zc = (z.top + z.bottom) / 2.0;
                            let up = if c.dropping { 3.0 * beat } else { 0.0 };
                            glyph_c(&p, &c.f.icon_l, "\u{E896}", (z.left + z.right) / 2.0, zc - 12.0 - up, &wa((if c.dropping { 0.95 } else { 0.55 }) * k2));
                            let t = if c.dropping { i18n::t("Suelta para guardar aquí") } else { i18n::t("Arrastra archivos aquí") };
                            let tw = p.width(&c.f.sub, t);
                            txt(&p, &c.f.sub, t, (z.left + z.right) / 2.0 - tw / 2.0, zc + 6.0, 16.0, &wa((if c.dropping { 0.9 } else { 0.5 }) * k2));
                        } else {
                            for (i, (path, _)) in shelf.iter().enumerate() {
                                let (col, rw) = ((i % 4) as f32, (i / 4) as f32);
                                let x = cl + 16.0 + col * 87.0;
                                let y = ct + 50.0 + rw * 84.0 + lift(k2);
                                let id = 32 + i as u8;
                                let gk = c.glow[id as usize];
                                let cell = rect(x + 2.0, y, x + 85.0, y + 80.0);
                                if gk > 0.01 { rt.FillRoundedRectangle(&rrect(cell, 12.0), &wa(0.08 * gk * k2)); }
                                // Los nuevos entran creciendo con un pequeño rebote
                                let born = c.seen.get(path).copied().unwrap_or(now);
                                let e = now.saturating_duration_since(born).as_secs_f32();
                                let kb = if born > now { 0.0 } else if e < 0.4 { moving = true; ease_back(e / 0.4) } else { 1.0 };
                                if born > now { moving = true; }
                                let sc = (0.55 + 0.45 * kb) * if c.pressed == Some(id) && c.hover == Some(id) { 0.92 } else { 1.0 };
                                let (icx, icy) = (x + 43.5, y + 30.0);
                                popup::push_alpha(&rt, kb.clamp(0.0, 1.0));
                                scaled(&rt, icx, icy, sc, || {
                                    match c.shelf_bmps.1.get(i).and_then(|b| b.as_ref()) {
                                        Some(b) => draw_art(&rt, b, icx - 24.0, icy - 24.0, 48.0, 6.0, k2),
                                        None => glyph_c(&p, &c.f.icon_l, "\u{E8A5}", icx, icy, &wa(0.8 * k2)),
                                    }
                                });
                                // Nombre en una línea, recortado al ancho de la celda
                                let name = path.rsplit('\\').next().unwrap_or(path);
                                let mut nm = trunc(name, 16);
                                while p.width(&c.f.small, &nm) > 78.0 && nm.chars().count() > 4 { let k = nm.chars().count(); nm = trunc(name, k - 2); }
                                let nw = p.width(&c.f.small, &nm);
                                txt(&p, &c.f.small, &nm, icx - nw / 2.0, y + 60.0, 14.0, &wa(0.78 * k2));
                                rt.PopLayer();
                                if k2 > 0.9 && live_t && kb >= 1.0 { hits.push((cell, id)); }
                            }
                        }
                    }                    Tab::Timer => {
                        let is_done = timer.is_none() && done.is_some();
                        let none = timer.is_none() && !is_done;
                        let (left, total) = timer.unwrap_or((0, done.map(|d| d.0).unwrap_or(1)));
                        let col = if is_done { (1.0, 0.36, 0.33) } else { tcol(left) };
                        // Terminado: el anillo se sacude como una alarma (cada 1,6 s)
                        let shake = match done {
                            Some((_, at)) if is_done => { moving = true; let ph = at.elapsed().as_secs_f32() % 1.6; if ph < 0.55 { (ph * 42.0).sin() * 5.0 * (1.0 - ph / 0.55) } else { 0.0 } }
                            _ => 0.0,
                        };
                        let k1 = row(1.0);
                        // El anillo viaja desde el de la isla y crece hasta 72 DIP
                        let kr = if c.tab == Tab::Timer { kh } else { 1.0 };
                        let from = (isl.left + 20.0, isl_cy, 7.5, 2.2);
                        let rcy = ct + chh / 2.0 - 2.0;
                        let to = (cl + 22.0 + 36.0 + shake, rcy, 34.0, 5.0);
                        let (rx, ry, rr, sw) = (lerp(from.0, to.0, kr), lerp(from.1, to.1, kr), lerp(from.2, to.2, kr), lerp(from.3, to.3, kr));
                        let fr = if is_done { 1.0 } else if none { 0.0 } else { left as f32 / total.max(1) as f32 };
                        let ring_a = if none { k1 } else { 1.0 };
                        ring(&rt, &c.fac, cap.as_ref(), rx, ry, rr, sw, fr, &col_b(col, if paused { 0.55 } else { 1.0 } * ring_a), &col_b(col, 0.22 * ring_a));
                        // La cuenta, grande dentro del anillo; los dígitos ruedan al cambiar
                        if kr > 0.6 && !none {
                            let ka = ((kr - 0.6) / 0.4).clamp(0.0, 1.0);
                            let txt_s = live::mmss(left);
                            let f = if left >= 3600 { &c.f.title } else { &c.f.big };
                            let tw = p.width(f, &txt_s);
                            let base = Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: dx, M32: ry - 50.0 };
                            rt.SetTransform(&base);
                            if c.roll.draw(&p, &rt, &base, f, &txt_s, rx - tw / 2.0, 50.0 - 12.0, 50.0 + 12.0, &wa(if paused { 0.5 } else { 0.96 } * ka), None) { moving = true; }
                            rt.SetTransform(&tm);
                        }
                        // A la derecha: qué es, cuánto y los botones
                        let x0 = cl + 22.0 + 72.0 + 22.0;
                        let lbl = if is_done { i18n::t("Se acabó el tiempo") } else if none { i18n::t("Temporizador") } else if paused { i18n::t("En pausa") } else {
                            match pomo { 1 => i18n::t("Concentración"), 2 => i18n::t("Descanso"), _ => i18n::t("Temporizador") }
                        };
                        txt(&p, &c.f.title, lbl, x0, ct + 22.0 + lift(k1), 18.0, &wa(0.96 * k1));
                        let k2 = row(2.0);
                        let sy = ct + 46.0 + lift(k2);
                        if pomo != 0 && !none && !is_done {
                            // Cuatro puntos, uno por bloque de la tanda, y los bloques de hoy
                            let d = live::POMO_DONE.load(Ordering::Relaxed) % 4;
                            for j in 0..4u8 {
                                let on = j < d || (j == d && pomo == 1);
                                let e = D2D1_ELLIPSE { point: Vector2 { X: x0 + 3.0 + j as f32 * 10.0, Y: sy + 8.0 }, radiusX: 3.0, radiusY: 3.0 };
                                rt.FillEllipse(&e, &col_b(col, if on { 1.0 } else { 0.25 } * k2));
                            }
                            let n = live::pomo_today();
                            let t = if n == 1 { i18n::t("1 bloque hoy").to_string() } else { i18n::tf("{} bloques hoy", &[&n]) };
                            txt(&p, &c.f.sub, &t, x0 + 46.0, sy, 16.0, &wa(0.5 * k2));
                        } else if none {
                            txt(&p, &c.f.sub, i18n::t("Elige cuánto tiempo"), x0, sy, 16.0, &wa(0.5 * k2));
                        } else {
                            txt(&p, &c.f.sub, &i18n::tf("de {}", &[&live::mmss(total)]), x0, sy, 16.0, &wa(0.5 * k2));
                        }
                        // Botones
                        let k3 = row(3.0);
                        if k3 > 0.01 {
                            let by = ct + chh - 22.0 - 15.0 + lift(k3);
                            let mut bx = x0;
                            let mut btn = |hits: &mut Vec<(D2D_RECT_F, u8)>, w: f32, label: Option<&str>, glyph: Option<&str>, id: u8, accent: Option<(f32, f32, f32)>| {
                                let r = rect(bx, by, bx + w, by + 30.0);
                                let gk = c.glow[id as usize];
                                let sc = if c.pressed == Some(id) && c.hover == Some(id) { 0.92 } else { 1.0 };
                                let (cx2, cy2) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
                                scaled(&rt, cx2, cy2, sc, || {
                                    let bg = match accent { Some(a3) => col_b(a3, (0.85 + 0.15 * gk) * k3), None => wa((0.10 + 0.08 * gk) * k3) };
                                    rt.FillRoundedRectangle(&rrect(r, 15.0), &bg);
                                    let ink = if accent.is_some() { p.brush(rgba(0.0, 0.0, 0.0, 0.9 * k3)) } else { wa(0.96 * k3) };
                                    match (label, glyph) {
                                        (Some(l), _) => { let lw = p.width(&c.f.lbl, l); txt(&p, &c.f.lbl, l, cx2 - lw / 2.0, r.top, 30.0, &ink); }
                                        (None, Some(g)) => glyph_c(&p, &c.f.icon, g, cx2, cy2, &ink),
                                        _ => {}
                                    }
                                });
                                if k3 > 0.9 && live_t { hits.push((r, id)); }
                                bx += w + 6.0;
                            };
                            if none {
                                for (i, m) in [1u32, 5, 10, 25].into_iter().enumerate() { btn(&mut hits, if m >= 10 { 52.0 } else { 46.0 }, Some(&format!("{m} min")), None, 16 + i as u8, None); }
                            } else if is_done {
                                btn(&mut hits, 92.0, Some(i18n::t("Repetir")), None, 23, Some((1.0, 0.62, 0.22)));
                                btn(&mut hits, 32.0, None, Some("\u{E711}"), 22, None);
                            } else {
                                btn(&mut hits, 58.0, Some(i18n::t("+1 min")), None, 20, None);
                                btn(&mut hits, 32.0, None, Some(if paused { "\u{E768}" } else { "\u{E769}" }), 21, None);
                                btn(&mut hits, 32.0, None, Some("\u{E711}"), 22, None);
                                if pomo != 0 { btn(&mut hits, 62.0, Some(i18n::t("Saltar")), None, 23, None); }
                            }
                        }
                        // Sin temporizador: también se puede empezar un pomodoro
                        if none && k2 > 0.01 {
                            let t = i18n::t("Pomodoro");
                            let w = p.width(&c.f.small, t) + 20.0;
                            let r = rect(cl + cw - 16.0 - w, sy - 2.0, cl + cw - 16.0, sy + 20.0);
                            rt.FillRoundedRectangle(&rrect(r, 11.0), &col_b((1.0, 0.62, 0.22), (0.18 + 0.12 * c.glow[15]) * k2));
                            txt(&p, &c.f.small, t, r.left + 10.0, r.top, 22.0, &col_b((1.0, 0.62, 0.22), k2));
                            if k2 > 0.9 && live_t { hits.push((r, 15)); }
                        }
                    }
                }
                rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 });
                rt.PopLayer();
            }

            // ---- Selector de pestañas (solo si hay más de una con contenido): arriba a la derecha
            if tabs_on {
                let ka = row(1.0);
                if ka > 0.01 {
                    let (r0, t0) = (sh.right - 16.0, sh.top + 12.0);
                    let n = tabs.len();
                    // De derecha a izquierda: la última pestaña pegada al borde
                    let x_of = |i: usize| r0 - 13.0 - (n - 1 - i) as f32 * 30.0;
                    let ai = tabs.iter().position(|t| *t == c.tab).unwrap_or(0);
                    let target = x_of(ai) - r0;
                    if c.tab_x.is_nan() { c.tab_x = target; }
                    let d = target - c.tab_x;
                    if d.abs() > 0.2 { c.tab_x += d * (1.0 - (-dt / 0.06).exp()); moving = true; } else { c.tab_x = target; }
                    rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: r0 + c.tab_x, Y: t0 + 13.0 }, radiusX: 13.0, radiusY: 13.0 }, &wa(0.16 * ka));
                    for (i, t) in tabs.iter().enumerate() {
                        let (id, gl) = match t { Tab::Media => (10u8, "\u{E8D6}"), Tab::Timer => (11, "\u{E916}"), Tab::Shelf => (12, "\u{E8B7}"), Tab::Glance => (13, "\u{E823}") };
                        let x = x_of(i);
                        let gk = c.glow[id as usize];
                        if gk > 0.01 && c.tab != *t { rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: x, Y: t0 + 13.0 }, radiusX: 13.0, radiusY: 13.0 }, &wa(0.08 * gk * ka)); }
                        glyph_c(&p, &c.f.icon_s, gl, x, t0 + 13.0, &wa(if c.tab == *t { 0.96 } else { 0.5 } * ka));
                        if ka > 0.9 && c.open { hits.push((rect(x - 13.0, t0, x + 13.0, t0 + 26.0), id)); }
                    }
                }
            }            // ---- Aviso dentro de la tarjeta: la isla no se ve mientras está abierta, así que baja aquí
            {
                let hud = live::current().filter(|h| !hud_skip(*h, c.tab));
                if hud.is_some() { c.hud_last = hud; }
                if ease_to(&mut c.hud_k, if hud.is_some() && c.open { 1.0 } else { 0.0 }, 0.08, dt) { moving = true; }
                if hud.is_some() { moving = true; }
                if let Some(h) = c.hud_last.filter(|_| c.hud_k > 0.01) {
                    let k = c.hud_k;
                    let (gl, lbl, bar) = hud_parts(h);
                    if ease_to(&mut c.hud_val, hud_value(h), 0.07, dt) { moving = true; }
                    let drop = -40.0 * (1.0 - ease_out(k));
                    let r = rect(sh.left + 10.0, sh.top + 10.0 + drop, sh.right - 10.0, sh.top + 44.0 + drop);
                    rt.FillRoundedRectangle(&rrect(rect(r.left, r.top + 3.0, r.right, r.bottom + 3.0), 17.0), &p.brush(rgba(0.0, 0.0, 0.0, 0.4 * k)));
                    rt.FillRoundedRectangle(&rrect(r, 17.0), &p.brush(rgba(0.13, 0.13, 0.14, 0.98 * k)));
                    rt.DrawRoundedRectangle(&rrect(r, 17.0), &wa(0.08 * k), 1.0 / s, None);
                    let cy2 = (r.top + r.bottom) / 2.0;
                    if !gl.is_empty() { glyph_c(&p, &c.f.icon_s, gl, r.left + 20.0, cy2, &wa(0.96 * k)); }
                    txt(&p, &c.f.lbl, &lbl, r.left + 36.0, r.top, 34.0, &wa(0.96 * k));
                    if bar {
                        let pct = format!("{}%", (c.hud_val * 100.0).round() as i32);
                        let pw = p.width(&c.f.lbl, "100%");
                        txt(&p, &c.f.lbl, &pct, r.right - 14.0 - p.width(&c.f.lbl, &pct), r.top, 34.0, &wa(0.96 * k));
                        let (bl, br) = (r.right - 14.0 - pw - 10.0 - 84.0, r.right - 14.0 - pw - 10.0);
                        rt.FillRoundedRectangle(&rrect(rect(bl, cy2 - 2.5, br, cy2 + 2.5), 2.5), &wa(0.18 * k));
                        let fx = bl + (br - bl) * c.hud_val.clamp(0.0, 1.0);
                        if fx > bl + 0.5 { rt.FillRoundedRectangle(&rrect(rect(bl, cy2 - 2.5, fx.max(bl + 5.0), cy2 + 2.5), 2.5), &wa(0.96 * k)); }
                    }
                }
            }            rt.PopAxisAlignedClip();
            rt.PopLayer();
            let ok = rt.EndDraw(None, None).is_ok();
            if !ok { c.rt = None; c.art = None; c.shelf_bmps.0 = u64::MAX; moving = true; }
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
        // Capturas para el README: MACBAR_CARDSHOT=carpeta guarda el lienzo (con su transparencia) de cada
        // pestaña la primera vez que queda quieta
        if !moving && c.open && kh >= 1.0 {
            if let (Ok(dir), Some(sf)) = (std::env::var("MACBAR_CARDSHOT"), c.surf.as_ref()) {
                let path = format!("{dir}\\card_{:?}.raw", c.tab);
                if !std::path::Path::new(&path).exists() {
                    let n = (sf.w * sf.h * 4) as usize;
                    let px = unsafe { std::slice::from_raw_parts(sf.bits, n) };
                    let mut out = Vec::with_capacity(n + 8);
                    out.extend_from_slice(&sf.w.to_le_bytes());
                    out.extend_from_slice(&sf.h.to_le_bytes());
                    out.extend_from_slice(px);
                    let _ = std::fs::write(&path, out);
                }
            }
        }
        // Sin línea de tiempo y sonando, el ecualizador necesita fotogramas
        if media.as_ref().is_some_and(|m| m.playing) && media_secs(true).is_none() && kh > 0.5 { moving = true; }
        moving
    });
    if let Some(h) = finished { finish(h); return false; }
    moving
}

/// Sondeo cada 30 ms mientras está abierta: ratón dentro o fuera, clics fuera, Esc y lo que la cierra.
fn poll() {
    let mut pt = POINT::default();
    unsafe { let _ = GetCursorPos(&mut pt); }
    let down = |vk: VIRTUAL_KEY| unsafe { GetAsyncKeyState(vk.0 as i32) < 0 };
    let (l, r, esc) = (down(VK_LBUTTON), down(VK_RBUTTON), down(VK_ESCAPE));
    let other = ccenter::is_open() || tray::is_open() || popup::open_kind().is_some();
    let timer_on = live::timer_left().is_some();
    let done_recent = (*live::LAST_DONE.lock().unwrap()).is_some_and(|d| d.1.elapsed().as_secs_f32() < 6.0);
    let media_on = MEDIA.lock().unwrap().is_some();
    let shelf_on = !live::SHELF.lock().unwrap().is_empty();
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
        // Con contenido: música, temporizador, uno recién terminado (6 s) o los atajos del temporizador con el ratón encima
        let has = media_on || timer_on || done_recent || shelf_on || (matches!(c.tab, Tab::Timer | Tab::Shelf | Tab::Glance) && inside);
        if live::current().is_some_and(|h| hud_closes(h, c.tab)) { return Some(Why::Alert); }
        if has { c.empty_since = None; } else if c.empty_since.is_none() { c.empty_since = Some(now); }
        let inside = inside || c.dropping;
        let pressed_now = !c.menu_open && ((l && !c.btn_prev.0) || (r && !c.btn_prev.1));
        let escape = esc && !c.btn_prev.2;
        c.btn_prev = (l, r, esc);
        let ms = |t: Option<Instant>| t.map(|t| t.elapsed().as_millis() as u32).unwrap_or(0);
        // En la prueba visual (MACBAR_CARDTEST) el ratón no está encima: cuenta como dentro
        let inside = inside || std::env::var_os("MACBAR_CARDTEST").is_some();
        should_close(&Watch {
            origin: c.origin, inside, outside_ms: ms(c.outside_since), dragging: c.drag.is_some() || c.dropping || c.menu_open,
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

/// Cambia de pestaña con su fundido hacia el lado que toca.
fn switch_tab(t: Tab) {
    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() {
        if c.tab == t { return; }
        let dir = if t.idx() > c.tab.idx() { 1.0 } else { -1.0 };
        c.tab_prev = Some((c.tab, Instant::now(), dir));
        c.tab = t;
        c.hover = None;
    });
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
        if c.bottom { y >= oy + WIN_H - PILL_H } else { y <= oy + PILL_H }
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
                    else if is_open() && (MEDIA.lock().unwrap().as_ref().is_some_and(|m| m.playing) || (live::timer_left().is_some() && !live::timer_paused()) || CARD.with(|c| c.borrow().as_ref().is_some_and(|c| c.tab == Tab::Glance))) { SetTimer(Some(hwnd), TIMER_ANIM, 500, None); }
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
                // Pulsado en un archivo y movido más de 5 px: se arrastra fuera de la bandeja
                let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                let start = CARD.with(|c| c.borrow().as_ref().and_then(|c| c.press_item.filter(|(_, px, py)| (x - px).abs() > 5 || (y - py).abs() > 5).map(|p| p.0)));
                if let Some(i) = start {
                    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.press_item = None; c.pressed = None; });
                    let path = live::SHELF.lock().unwrap().get(i).map(|x| x.0.clone());
                    if let Some(path) = path {
                        // Si se movió a otra carpeta, ya no está donde estaba: sale de la bandeja
                        if dnd::drag_out(&path).is_some_and(|e| e.0 & windows::Win32::System::Ole::DROPEFFECT_MOVE.0 != 0 && !std::path::Path::new(&path).exists()) { live::shelf_remove(&path); }
                    }
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
                if let Some(i @ 32..=39) = h {
                    let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
                    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.press_item = Some(((i - 32) as usize, x, y)); });
                }
                if h == Some(5) {
                    let fr = bar_frac(lp);
                    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.drag = Some(fr); });
                    SetCapture(hwnd);
                }
                SetTimer(Some(hwnd), TIMER_ANIM, 1, None);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let pressed = CARD.with(|c| c.borrow_mut().as_mut().and_then(|c| { c.press_item = None; c.pressed.take() }));
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
                    Some(10) => switch_tab(Tab::Media),
                    Some(11) => switch_tab(Tab::Timer),
                    Some(12) => switch_tab(Tab::Shelf),
                    Some(13) => switch_tab(Tab::Glance),
                    Some(40) => live::caffeine_toggle(),
                    Some(41) => { live::pomodoro_start(); switch_tab(Tab::Timer); }
                    Some(42) => { close(Why::Toggle); crate::pick_color(); }
                    Some(43) => { close(Why::Toggle); sys::open("ms-screenclip:"); }
                    Some(28) => { let paths: Vec<String> = live::SHELF.lock().unwrap().iter().map(|x| x.0.clone()).collect(); let _ = share::files(bar_hwnd(), &paths); close(Why::Toggle); }
                    Some(29) => live::shelf_clear(),
                    Some(i @ 32..=39) => {
                        let p = live::SHELF.lock().unwrap().get((i - 32) as usize).map(|x| x.0.clone());
                        if let Some(p) = p { sys::open(&p); close(Why::Toggle); }
                    }
                    Some(15) => live::pomodoro_start(),
                    Some(i @ 16..=19) => live::timer_start([1, 5, 10, 25][(i - 16) as usize] * 60),
                    Some(20) => live::timer_add(60),
                    Some(21) => live::timer_pause_toggle(),
                    Some(22) => if live::timer_left().is_some() { live::timer_stop(); } else { *live::LAST_DONE.lock().unwrap() = None; live::dismiss(); close(Why::Toggle); },
                    Some(23) => if live::POMO.load(Ordering::Relaxed) != 0 { live::pomo_skip(); } else {
                        // Repetir el temporizador que acaba de terminar
                        let d = live::LAST_DONE.lock().unwrap().take();
                        if let Some((total, _)) = d { live::dismiss(); live::timer_start(total); }
                    },
                    Some(4) => { if let Some(m) = MEDIA.lock().unwrap().clone() { open_player(&m.app); } close(Why::Toggle); }
                    _ => if h.is_none() && pressed.is_none() && in_island_band(lp) { close(Why::Toggle); },
                }
                SetTimer(Some(hwnd), TIMER_ANIM, 1, None);
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                if let Some(i @ 32..=39) = hit_at(lp) { item_menu(hwnd, (i - 32) as usize); }
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

/// Archivos arrastrados que llegan a la isla: la tarjeta se abre en la bandeja (sin quitar el foco).
pub fn open_for_drop() {
    if crate::profiles::presenting() { return; }
    if is_open() { if CARD.with(|c| c.borrow().as_ref().is_some_and(|c| c.tab != Tab::Shelf)) { switch_tab(Tab::Shelf); } drop_hover(true); return; }
    open_tab(Origin::Drop, Some(Tab::Shelf));
}

/// Archivos arrastrados encima de la tarjeta (o que se van).
pub fn drop_hover(on: bool) {
    let h = CARD.with(|c| c.borrow_mut().as_mut().map(|c| { c.dropping = on; if on && c.tab != Tab::Shelf { c.tab_prev = Some((c.tab, Instant::now(), 1.0)); c.tab = Tab::Shelf; } c.hwnd }));
    if let Some(h) = h { unsafe { SetTimer(Some(h), TIMER_ANIM, 1, None); } }
}

/// Se soltaron archivos: la tarjeta se queda abierta enseñándolos (cuenta como si el ratón siguiera encima un rato).
pub fn dropped() {
    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.outside_since = None; });
}

/// Menú de un archivo de la bandeja (clic derecho).
fn item_menu(hwnd: HWND, i: usize) {
    let Some(path) = live::SHELF.lock().unwrap().get(i).map(|x| x.0.clone()) else { return };
    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.menu_open = true; });
    let pick = unsafe {
        let m = CreatePopupMenu().ok();
        m.map(|m| {
            for (id, t) in [(1usize, i18n::t("Abrir")), (2, i18n::t("Mostrar en la carpeta")), (3, i18n::t("Compartir")), (4, i18n::t("Quitar de la bandeja"))] {
                let w = HSTRING::from(t);
                let _ = AppendMenuW(m, MF_STRING, id, PCWSTR(w.as_ptr()));
            }
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let r = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, None, hwnd, None).0 as usize;
            let _ = DestroyMenu(m);
            r
        }).unwrap_or(0)
    };
    CARD.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.menu_open = false; c.btn_prev = (true, true, true); });
    match pick {
        1 => { sys::open(&path); close(Why::Toggle); }
        2 => { sys::open_args("explorer.exe", &format!("/select,\"{path}\"")); close(Why::Toggle); }
        3 => { let _ = share::files(bar_hwnd(), &[path]); close(Why::Toggle); }
        4 => live::shelf_remove(&path),
        _ => {}
    }
}

/// Prueba visual: cambia de pestaña.
pub fn test_switch(t: Tab) {
    switch_tab(t);
    let h = CARD.with(|c| c.borrow().as_ref().map(|c| c.hwnd));
    if let Some(h) = h { unsafe { SetTimer(Some(h), TIMER_ANIM, 1, None); } }
}

/// Algo cambió (música, aviso nuevo): la tarjeta abierta se repinta.
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
    fn pestana_inicial() {
        // Isla del temporizador con temporizador: su pestaña; si no hay, la de la música
        assert_eq!(initial_tab(2, true, true, false), Some(Tab::Timer));
        assert_eq!(initial_tab(2, true, false, false), Some(Tab::Media));
        assert_eq!(initial_tab(1, true, true, true), Some(Tab::Media));
        // Desde un aviso o sin nada: la primera con contenido
        assert_eq!(initial_tab(4, false, true, false), Some(Tab::Timer));
        assert_eq!(initial_tab(0, false, false, false), None);
        // Cada pestaña cabe en la ventana (que tiene el tamaño de la mayor)
        for t in [Tab::Media, Tab::Timer] { let (w, h) = tab_size(t); assert!(w <= CARD_W && h <= WIN_H); }
        // La bandeja llena (8 archivos, dos filas) cabe justa
        assert!(shelf_h(8) <= WIN_H);
        assert_eq!(shelf_h(4), shelf_h(1));
        assert!(shelf_h(5) > shelf_h(4));
        assert_eq!(initial_tab(3, true, false, true), Some(Tab::Shelf));
        assert_eq!(initial_tab(0, false, false, true), Some(Tab::Shelf));
    }

    #[test]
    fn avisos_en_la_tarjeta() {
        // Batería baja y presentación cierran la tarjeta; «se acabó el tiempo» solo si no está en su pestaña
        assert!(hud_closes(live::Hud::LowBat(8), Tab::Media));
        assert!(hud_closes(live::Hud::TimerDone, Tab::Media));
        assert!(!hud_closes(live::Hud::TimerDone, Tab::Timer));
        assert!(!hud_closes(live::Hud::Volume(0.4, false), Tab::Media));
        // La bandeja y el pomodoro no se repiten donde ya se ven
        assert!(hud_skip(live::Hud::Shelf, Tab::Shelf));
        assert!(hud_skip(live::Hud::Pomo(true, 25), Tab::Timer));
        assert!(!hud_skip(live::Hud::Pomo(true, 25), Tab::Media));
        assert_eq!(hud_value(live::Hud::Volume(0.4, true)), 0.0);
        assert_eq!(hud_value(live::Hud::Power(80, true)), 0.8);
    }

    #[test]
    fn pausa_y_minuto_extra() {
        live::timer_start(120);
        live::timer_pause_toggle();
        assert!(live::timer_paused());
        let (l1, t1) = live::timer_left().unwrap();
        std::thread::sleep(Duration::from_millis(1100));
        // En pausa no corre
        assert_eq!(live::timer_left().unwrap().0, l1);
        live::timer_add(60);
        assert_eq!(live::timer_left().unwrap(), (l1 + 60, t1 + 60));
        live::timer_pause_toggle();
        assert!(!live::timer_paused());
        assert!(live::timer_left().unwrap().0 >= l1 + 59);
        live::timer_stop();
        assert!(live::timer_left().is_none());
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
