//! Los seis widgets grandes del escritorio: calendario, tiempo, música con letra en vivo, sistema,
//! almacenamiento con papelera y proyectos (en local y en GitHub).
//! Todos son **responsive** (`Widget::layouts`): cada uno pide su tamaño con `desk::lay` y, según sea,
//! recoloca su contenido y añade o quita bloques (ver PLAN_TAMANOS_WIDGETS.md, §2). `W` y `H` son sus medidas.
#![allow(non_snake_case)]

use super::*;
use desk::{DFonts, Kind, Sz};
use deskdata::*;
use popup::Ui;
use std::os::windows::process::CommandExt;

const PAD: f32 = 20.0;

// Glifos de Segoe Fluent Icons
const G_PREV: &str = "\u{E892}";
const G_NEXT: &str = "\u{E893}";
const G_PLAY: &str = "\u{E768}";
const G_PAUSE: &str = "\u{E769}";
const G_MUSIC: &str = "\u{E8D6}";
const G_TRASH: &str = "\u{E74D}";
const G_DRIVE: &str = "\u{EDA2}";
const G_FOLDER: &str = "\u{E8DA}";
const G_OPEN: &str = "\u{E8A7}";
const G_DOWN: &str = "\u{E74B}";
const G_UP: &str = "\u{E74A}";
const G_LEFT: &str = "\u{E76B}";
const G_RIGHT: &str = "\u{E76C}";
const G_STOP: &str = "\u{E71A}";
const G_TIMER: &str = "\u{E916}";
const G_LOCK: &str = "\u{E72E}";
const G_CODE: &str = "\u{E943}";
const G_TERM: &str = "\u{E756}";
const G_ISSUE: &str = "\u{E8BD}";
const G_COPY: &str = "\u{E8C8}";

pub trait Widget {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32);
    fn click(&mut self, id: Option<u32>, right: bool, pt: POINT) -> Option<Box<dyn FnOnce()>>;
    fn wheel(&mut self, _d: i32) -> bool { false }
    /// Cada segundo. true = repintar.
    fn tick(&mut self) -> bool { true }
    /// true mientras haga falta animar (pide fotogramas).
    fn animating(&self, t: f32) -> bool { t < 1.3 }
    /// true durante una transición corta que merece 60 fotogramas (lo continuo va a 30).
    fn smooth(&self) -> bool { false }
    /// Milisegundos entre fotogramas de lo continuo (gráficas que se deslizan, letra…).
    fn frame_ms(&self) -> u32 { 33 }
    /// Con capa fija: `paint` dibuja en una imagen que solo se rehace cuando `layer_stale` lo pide, y
    /// `paint_live` dibuja encima, en cada fotograma, solo lo que se mueve.
    fn layered(&self) -> bool { false }
    fn layer_stale(&self) -> bool { true }
    fn paint_live(&mut self, _ui: &mut Ui, _d: &DFonts, _t: f32, _dt: f32) {}
    /// true si hay que repintar con cada movimiento del ratón (sondas que siguen al cursor).
    fn follows_mouse(&self) -> bool { false }
    /// true si el widget ya maqueta con su tamaño real (`desk::Sz`), es decir, es **responsive**.
    ///
    /// - false (por defecto): sigue pintando con sus medidas de siempre (`size(Kind::X)` en estos
    ///   ficheros es `desk::layout_size`, que entonces devuelve `desk::legacy_size`) y `desk` lo escala
    ///   sin deformar y centrado para caber en el tamaño elegido. Provisional hasta F2-F5.
    /// - true: `size(Kind::X)` pasa a devolver las medidas reales del tamaño elegido y no se escala
    ///   nada. El widget decide qué bloques caben con `let l = desk::lay(Kind::X);` y
    ///   `match l.sz { Sz::Pequeno => …, Sz::Mediano => …, … }`, con los ayudantes de `desk::Lay`
    ///   (`PAD` = 20, `inner()`, `is_wide()`, `is_tall()`, `cols()`, `rows()`, `cell(c, r, nc, nr)`).
    ///   Al cambiar de tamaño se crea un widget nuevo (`deskw::new`), así que el estado que dependa
    ///   del tamaño puede calcularse al construirlo o en cada `paint`.
    fn layouts(&self) -> bool { false }
}

// ---------------------------------------------------------------- piezas animadas comunes
/// Texto que "rueda" al cambiar: el viejo sube y se desvanece mientras el nuevo entra desde abajo.
#[derive(Default)]
pub(crate) struct Roll { cur: String, prev: String, at: Option<Instant> }

const ROLL_S: f32 = 0.42;

impl Roll {
    fn set(&mut self, s: &str) {
        if self.cur != s {
            if !self.cur.is_empty() { self.prev = std::mem::replace(&mut self.cur, s.to_string()); self.at = Some(Instant::now()); }
            else { self.cur = s.to_string(); }
        }
    }
    fn k(&self) -> f32 { self.at.map(|a| (a.elapsed().as_secs_f32() / ROLL_S).min(1.0)).unwrap_or(1.0) }
    pub(crate) fn busy(&self) -> bool { self.k() < 1.0 }
    /// Dibuja alineado a la izquierda (`right` = false) o a la derecha.
    pub(crate) fn draw(&mut self, ui: &Ui, f: &IDWriteTextFormat, s: &str, x: f32, top: f32, bh: f32, b: &ID2D1Brush, right: bool) {
        self.set(s);
        let k = self.k();
        let e = popup::ease_out(k);
        let dist = bh * 0.55;
        let put = |t: &str, dy: f32, a: f32| {
            if a <= 0.01 { return; }
            popup::push_alpha(ui.rt, a);
            if right { ui.text_r(f, t, x, top + dy, bh, b); } else { ui.text(f, t, x, top + dy, bh, b); }
            unsafe { ui.rt.PopLayer(); }
        };
        if k < 1.0 { put(&self.prev, -dist * e, 1.0 - popup::ease_out((k * 1.6).min(1.0))); }
        put(&self.cur.clone(), dist * (1.0 - e), if k < 1.0 { e } else { 1.0 });
    }
}

/// Transformación actual del lienzo (para componer escalas locales encima).
fn cur_transform(ui: &Ui) -> Matrix3x2 { let mut m = Matrix3x2::default(); unsafe { ui.rt.GetTransform(&mut m); } m }

/// Botón de texto que se hunde un poco al pulsarlo (envuelve `Ui::button`).
fn pbutton(ui: &mut Ui, r: D2D_RECT_F, glyph: Option<&str>, label: &str, id: u32, active: bool) {
    let p = desk::press(id);
    let base = cur_transform(ui);
    if p > 0.0 { unsafe { ui.rt.SetTransform(&mul(&scale_at(1.0 - 0.07 * p, (r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0), &base)); } }
    ui.button(r, glyph, label, id, active);
    if p > 0.0 { unsafe { ui.rt.SetTransform(&base); } }
}

pub fn new(k: Kind) -> Box<dyn Widget> {
    match k {
        Kind::Calendar => match desk::variant(k) { 1 => Box::new(crate::deskv::TodayW), 2 => Box::new(crate::deskv2::PosterW), _ => Box::new(CalendarW::default()) },
        Kind::Weather => match desk::variant(k) { 1 => Box::new(crate::deskv::SkyW), 2 => Box::new(crate::deskv::MinWeatherW::default()), _ => Box::new(WeatherW::default()) },
        Kind::Music => match desk::variant(k) { 1 => Box::new(crate::deskv::CoverW::default()), 2 => Box::new(crate::deskv2::VinylW::default()), 3 => Box::new(crate::deskv3::TapeW::default()), _ => Box::new(MusicW::default()) },
        Kind::System if desk::variant(k) == 1 => Box::new(crate::deskv::RingsW::default()),
        Kind::System if desk::variant(k) == 2 => Box::new(crate::deskv3::TermW::default()),
        Kind::System => Box::new(SystemW::default()),
        Kind::Storage if desk::variant(k) == 1 => Box::new(crate::deskv3::DriveDotsW::default()),
        Kind::Storage => Box::new(StorageW::default()),
        Kind::Projects => Box::new(ProjectsW::default()),
        Kind::Phone => { crate::phone::start(); Box::new(PhoneW::default()) }
        Kind::Visual => match desk::variant(k) {
            1 => Box::new(crate::deskv2::RadialW::default()),
            2 => Box::new(crate::deskv3::SpecDotsW::default()),
            3 => Box::new(crate::deskv3::OrbW::default()),
            _ => Box::new(crate::deskw2::VisualW::default()),
        },
        Kind::Photos if desk::variant(k) == 1 => Box::new(crate::deskv2::PolaroidW::default()),
        Kind::Photos => Box::new(crate::deskw2::PhotosW::default()),
        Kind::Clocks if desk::variant(k) == 1 => Box::new(crate::deskv3::EyeW::default()),
        Kind::Clocks => Box::new(crate::deskw3::ClocksW::default()),
        Kind::Todo => Box::new(crate::deskw3::TodoW::default()),
        Kind::Countdown if desk::variant(k) == 1 => Box::new(crate::deskv2::FlipW::default()),
        Kind::Countdown => Box::new(crate::deskw3::CountdownW::default()),
        Kind::News => Box::new(crate::news::NewsW::default()),
        Kind::Stocks => Box::new(crate::stocks::StocksW::default()),
    }
}

// ---------------------------------------------------------------- utilidades de dibujo
fn ez(t: f32, delay: f32, dur: f32) -> f32 { popup::ease_out(((t - delay) / dur).clamp(0.0, 1.0)) }

fn approach(x: &mut f32, target: f32, dt: f32, tau: f32) -> bool {
    if (target - *x).abs() < 0.002 { *x = target; return false; }
    *x += (target - *x) * (1.0 - (-dt / tau).exp());
    true
}

thread_local! {
    /// Trazo redondeado por fábrica de Direct2D (cada widget tiene la suya): se crea una vez
    static STROKES: RefCell<HashMap<usize, ID2D1StrokeStyle>> = RefCell::new(HashMap::new());
}

fn round_stroke(rt: &ID2D1RenderTarget) -> Option<ID2D1StrokeStyle> {
    use windows::core::Interface;
    unsafe {
        let f = rt.GetFactory().ok()?;
        let k = f.as_raw() as usize;
        if let Some(s) = STROKES.with(|m| m.borrow().get(&k).cloned()) { return Some(s); }
        let s = f.CreateStrokeStyle(&D2D1_STROKE_STYLE_PROPERTIES { startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, lineJoin: D2D1_LINE_JOIN_ROUND, ..Default::default() }, None).ok()?;
        STROKES.with(|m| m.borrow_mut().insert(k, s.clone()));
        Some(s)
    }
}

/// Polilínea suavizada (curvas de Bézier entre puntos). Con `base`, se cierra hacia abajo para rellenar.
fn smooth_path(rt: &ID2D1RenderTarget, pts: &[(f32, f32)], base: Option<f32>) -> Option<ID2D1PathGeometry> {
    if pts.len() < 2 { return None; }
    unsafe {
        let f = rt.GetFactory().ok()?;
        let g = f.CreatePathGeometry().ok()?;
        let s = g.Open().ok()?;
        let first = match base { Some(b) => Vector2 { X: pts[0].0, Y: b }, None => Vector2 { X: pts[0].0, Y: pts[0].1 } };
        s.BeginFigure(first, if base.is_some() { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
        if base.is_some() { s.AddLine(Vector2 { X: pts[0].0, Y: pts[0].1 }); }
        for i in 1..pts.len() {
            let (x0, y0) = pts[i - 1];
            let (x1, y1) = pts[i];
            let mx = (x0 + x1) / 2.0;
            s.AddBezier(&D2D1_BEZIER_SEGMENT { point1: Vector2 { X: mx, Y: y0 }, point2: Vector2 { X: mx, Y: y1 }, point3: Vector2 { X: x1, Y: y1 } });
        }
        if let Some(b) = base { s.AddLine(Vector2 { X: pts[pts.len() - 1].0, Y: b }); }
        s.EndFigure(if base.is_some() { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
        s.Close().ok()?;
        Some(g)
    }
}

pub(crate) fn bitmap(rt: &ID2D1RenderTarget, px: &[u8], side: u32) -> Option<ID2D1Bitmap> {
    let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
    unsafe { rt.CreateBitmap(D2D_SIZE_U { width: side, height: side }, Some(px.as_ptr() as _), side * 4, &bp).ok() }
}

pub(crate) fn draw_art(rt: &ID2D1RenderTarget, bmp: &ID2D1Bitmap, side: u32, x: f32, y: f32, size: f32, rad: f32) {
    unsafe {
        let Ok(br) = rt.CreateBitmapBrush(bmp, None, None) else { return };
        let s = size / side as f32;
        br.SetTransform(&Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: x, M32: y });
        rt.FillRoundedRectangle(&rrect(rect(x, y, x + size, y + size), rad), &br);
    }
}

fn ellipse(x: f32, y: f32, r: f32) -> D2D1_ELLIPSE { D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: r, radiusY: r } }

/// Cabecera común: título a la izquierda y un texto suave a la derecha.
pub(crate) fn header(ui: &Ui, d: &DFonts, title: &str, right: &str, w: f32) {
    ui.text(&d.title, title, PAD, PAD - 2.0, 22.0, &ui.white);
    if !right.is_empty() { ui.text_r(&ui.f.sub, right, w - PAD, PAD - 2.0, 22.0, &ui.dim); }
}

fn bar(ui: &Ui, x: f32, y: f32, w: f32, h: f32, frac: f32, fill: &ID2D1Brush) {
    unsafe {
        ui.rt.FillRoundedRectangle(&rrect(rect(x, y, x + w, y + h), h / 2.0), &ui.track);
        let fw = (w * frac.clamp(0.0, 1.0)).max(if frac > 0.0 { h } else { 0.0 });
        if fw > 0.0 { ui.rt.FillRoundedRectangle(&rrect(rect(x, y, x + fw, y + h), h / 2.0), fill); }
    }
}

/// Botón redondo con un glifo (controles de música, flechas del calendario).
fn round_btn(ui: &mut Ui, f: &IDWriteTextFormat, g: &str, cx: f32, cy: f32, r: f32, id: u32, filled: bool) {
    // Se hunde al pulsarlo y crece un pelín al pasar el ratón
    let k = ui.heat(id);
    let base = cur_transform(ui);
    let sc = (1.0 + 0.06 * k) * (1.0 - 0.14 * desk::press(id));
    unsafe { ui.rt.SetTransform(&mul(&scale_at(sc, cx, cy), &base)); }
    round_btn_inner(ui, f, g, cx, cy, r, id, filled, k);
    unsafe { ui.rt.SetTransform(&base); }
}

fn round_btn_inner(ui: &mut Ui, f: &IDWriteTextFormat, g: &str, cx: f32, cy: f32, r: f32, id: u32, filled: bool, k: f32) {
    unsafe {
        if filled { ui.rt.FillEllipse(&ellipse(cx, cy, r), &ui.wa(0.92)); }
        else if k > 0.01 { ui.rt.FillEllipse(&ellipse(cx, cy, r), &ui.wa(0.10 * k)); }
    }
    // Relleno: círculo de tinta con el glifo del color contrario
    let b = if filled { let c = 0.08 + 0.9 * ui.ink; ui.brush(rgba(c, c, c + 0.01, 1.0)) } else { ui.white.clone() };
    ui.glyph(f, g, cx - r, cy - r, r * 2.0, r * 2.0, &b);
    ui.hit(rect(cx - r, cy - r, cx + r, cy + r), id);
}


// ---------------------------------------------------------------- piezas comunes de los tamaños
/// Texto recortado con «…» para que quepa en `maxw` DIP (los tamaños estrechos quitan letras, nunca
/// encogen la letra).
fn fit(ui: &Ui, f: &IDWriteTextFormat, s: &str, maxw: f32) -> String {
    if maxw <= 0.0 { return String::new(); }
    if ui.p.width(f, s) <= maxw { return s.to_string(); }
    let mut cs: Vec<char> = s.chars().collect();
    while !cs.is_empty() {
        cs.pop();
        let t = cs.iter().collect::<String>().trim_end().to_string() + "…";
        if ui.p.width(f, &t) <= maxw { return t; }
    }
    String::new()
}

/// Parte un texto en dos líneas de `maxw` DIP como mucho (por palabras; la segunda se recorta).
fn wrap2(ui: &Ui, f: &IDWriteTextFormat, s: &str, maxw: f32) -> (String, String) {
    if ui.p.width(f, s) <= maxw { return (s.to_string(), String::new()); }
    let words: Vec<&str> = s.split_whitespace().collect();
    let mut a = String::new();
    let mut used = 0;
    for w in &words {
        let next = if a.is_empty() { w.to_string() } else { format!("{a} {w}") };
        if ui.p.width(f, &next) > maxw { break; }
        a = next;
        used += 1;
    }
    if used == 0 { return (fit(ui, f, s, maxw), String::new()); }
    (a, fit(ui, f, &words[used..].join(" "), maxw))
}

/// Línea fina horizontal entre `x0` y `x1` (como `Ui::sep`, pero solo en un trozo).
fn hline(ui: &Ui, x0: f32, x1: f32, y: f32) { unsafe { ui.rt.FillRectangle(&rect(x0, y, x1, y + 1.0 / ui.s), &ui.card_edge); } }
/// Línea fina vertical que separa columnas.
fn vline(ui: &Ui, x: f32, y0: f32, y1: f32) { unsafe { ui.rt.FillRectangle(&rect(x, y0, x + 1.0 / ui.s, y1), &ui.card_edge); } }

/// Rótulo de sección en versalitas suaves («A CONTINUACIÓN»).
fn caption(ui: &Ui, s: &str, x: f32, y: f32) { ui.text(&ui.f.small, s, x, y, 16.0, &ui.faint); }

/// Anillo de progreso: pista y arco desde las 12, con los extremos redondeados.
fn ring(ui: &Ui, cx: f32, cy: f32, r: f32, th: f32, frac: f32, b: &ID2D1Brush) {
    unsafe {
        ui.rt.DrawEllipse(&ellipse(cx, cy, r), &ui.track, th, None);
        if frac > 0.003 { if let Some(g) = arc_geo(ui.rt, cx, cy, r, frac) { ui.rt.DrawGeometry(&g, b, th, round_stroke(ui.rt).as_ref()); } }
    }
}

/// Relleno de un medidor: blanco y, del 90 % para arriba, naranja (como los discos casi llenos).
fn fill_brush(ui: &Ui, frac: f32) -> ID2D1Brush { if frac >= 0.9 { ui.brush(rgba(1.0, 0.62, 0.26, 1.0)) } else { ui.white.clone() } }

/// Anillo con su cifra dentro y el rótulo debajo (sistema, discos).
fn ring_stat(ui: &Ui, d: &DFonts, cx: f32, cy: f32, r: f32, frac: f32, val: &str, label: &str) {
    let th = if r >= 40.0 { 9.0 } else if r >= 32.0 { 7.0 } else { 6.0 };
    ring(ui, cx, cy, r, th, frac, &fill_brush(ui, frac));
    let (f, h) = if r >= 40.0 { (&d.big, 30.0) } else if r >= 32.0 { (&d.title, 22.0) } else { (&ui.f.lbl, 18.0) };
    ui.text_c(f, val, cx, cy - h / 2.0, h, &ui.white);
    if !label.is_empty() { ui.text_c(&ui.f.small, &fit(ui, &ui.f.small, label, r * 2.0 + 30.0), cx, cy + r + th / 2.0 + 6.0, 16.0, &ui.dim); }
}

/// Recuadro suave con un dato: rótulo arriba, valor grande y una línea de detalle.
fn tile(ui: &Ui, d: &DFonts, r: D2D_RECT_F, label: &str, value: &str, sub: &str, k: f32) {
    popup::push_alpha(ui.rt, k);
    unsafe { ui.rt.FillRoundedRectangle(&rrect(r, 12.0), &ui.wa(0.06)); }
    let w = r.right - r.left - 24.0;
    let y = r.top + (r.bottom - r.top - 66.0) / 2.0;
    caption(ui, &fit(ui, &ui.f.small, label, w), r.left + 12.0, y);
    ui.text(&d.title, &fit(ui, &d.title, value, w), r.left + 12.0, y + 18.0, 26.0, &ui.white);
    ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, sub, w), r.left + 12.0, y + 48.0, 16.0, &ui.dim);
    ui.rt_pop();
}

// ---------------------------------------------------------------- calendario
/// `slide`: avance del cambio de mes (±1 → 0); `dir`: hacia dónde (para dibujar también el mes que se va);
/// `timer_on`: fundido entre los botones de enfoque y el temporizador en marcha; `events`: próximas
/// fechas (las de la Cuenta atrás: nombre y día civil), que se releen cada pocos segundos.
#[derive(Default)]
struct CalendarW { month_off: i32, slide: f32, dir: f32, r_time: Roll, r_date: Roll, r_title: Roll, r_left: Roll, timer_on: f32, ring: f32, events: Vec<(String, i64)>, ev_at: Option<Instant> }

fn days_in(y: i32, m: u32) -> u32 {
    match m { 1 | 3 | 5 | 7 | 8 | 10 | 12 => 31, 4 | 6 | 9 | 11 => 30, _ => if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 } }
}

/// Semana ISO 8601.
fn iso_week(y: i32, m: u32, d: u32) -> u32 {
    let doy: u32 = (1..m).map(|mm| days_in(y, mm)).sum::<u32>() + d;
    let wd = (calendar::weekday(y, m, d) + 6) % 7 + 1; // 1 = lunes
    let w = (doy as i32 - wd as i32 + 10) / 7;
    if w < 1 { return iso_week(y - 1, 12, 31); }
    let jan1 = (calendar::weekday(y, 1, 1) + 6) % 7 + 1;
    let leap = days_in(y, 2) == 29;
    let max = if jan1 == 4 || (leap && jan1 == 3) { 53 } else { 52 };
    if w > max { 1 } else { w as u32 }
}

/// Año y mes desplazados `off` meses desde (y, m).
fn month_add(y: i32, m: u32, off: i32) -> (i32, u32) {
    let mut y = y;
    let mut mm = m as i32 + off;
    while mm < 1 { mm += 12; y -= 1; }
    while mm > 12 { mm -= 12; y += 1; }
    (y, mm as u32)
}

/// «hoy», «mañana», «en 5 días».
pub(crate) fn days_left(n: i64) -> String {
    match n { 0 => i18n::t("hoy").into(), 1 => i18n::t("mañana").into(), n => i18n::tf("en {} días", &[&n]) }
}

/// «Sáb 21 dic» / «Sat Dec 21».
fn date_short(day: i64) -> String {
    let (y, m, d) = crate::deskw3::civil_from_days(day);
    let wd = i18n::weekday_short(calendar::weekday(y, m, d) as usize);
    let mes = i18n::month_short((m - 1) as usize);
    if i18n::en() { format!("{wd} {mes} {d}") } else { format!("{wd} {d} {mes}") }
}

impl CalendarW {
    fn go(&mut self, d: i32) {
        self.month_off += d;
        // Si ya se estaba deslizando, sigue desde donde iba (no salta)
        self.slide = (self.slide + d as f32).clamp(-1.6, 1.6);
        self.dir = d as f32;
    }

    /// Relee las fechas de la Cuenta atrás (cada 5 s como mucho: leen widgets.ini).
    fn refresh_events(&mut self) {
        if self.ev_at.is_some_and(|a| a.elapsed().as_secs_f32() < 5.0) { return; }
        self.ev_at = Some(Instant::now());
        self.events = crate::deskw3::countdown_dates(crate::deskw3::today());
    }

    fn event_on(&self, day: i64) -> Option<&str> { self.events.iter().find(|e| e.1 == day).map(|e| e.0.as_str()) }

    /// Rejilla de un mes desplazada `dx`, con opacidad `a`. `t` lleva la cascada de entrada. `x0`/`gw`:
    /// dónde empieza y cuánto mide; `hy`: fila de las letras; `ch`: alto de cada semana. Los días con
    /// una fecha llevan un punto.
    fn grid(&self, ui: &mut Ui, y: i32, m: u32, dx: f32, a: f32, t: f32, x0: f32, gw: f32, hy: f32, ch: f32, small: bool) {
        if a <= 0.01 { return; }
        let now = unsafe { GetLocalTime() };
        let (ty, tm, td) = (now.wYear as i32, now.wMonth as u32, now.wDay as u32);
        let cw = gw / 7.0;
        let first = (calendar::weekday(y, m, 1) + 6) % 7; // 0 = lunes
        let rr = (ch * 0.48).min(cw * 0.5).min(12.0);
        let f = if small { ui.f.small.clone() } else { ui.f.lbl.clone() };
        popup::push_alpha(ui.rt, a);
        for day in 1..=days_in(y, m) {
            let idx = first + day - 1;
            let (c, r) = (idx % 7, idx / 7);
            let k = ez(t, 0.05 + r as f32 * 0.04 + c as f32 * 0.008, 0.35);
            let cx = x0 + cw * (c as f32 + 0.5) + dx;
            let cy = hy + 16.0 + ch * 0.56 + r as f32 * ch + 6.0 * (1.0 - k);
            let today = y == ty && m == tm && day == td;
            let past = (y, m, day) < (ty, tm, td);
            if today {
                // Hoy: el círculo azul entra con un pequeño rebote y late una vez
                let pop = crate::ease_back(((t - 0.35) / 0.4).clamp(0.0, 1.0));
                let beat = ((t - 0.75) / 0.5).clamp(0.0, 1.0);
                let halo = (beat * std::f32::consts::PI).sin();
                unsafe {
                    if halo > 0.01 { ui.rt.FillEllipse(&ellipse(cx, cy, rr + 6.0 * beat), &ui.brush(crate::profiles::accent(0.35 * halo))); }
                    ui.rt.FillEllipse(&ellipse(cx, cy, rr * pop), &ui.on);
                }
            }
            // Al pasar el ratón, un círculo suave tras el día (solo en el mes que está quieto)
            let id = 300 + day;
            let hk = if dx.abs() < 1.0 { ui.heat(id) } else { 0.0 };
            if hk > 0.01 && !today { unsafe { ui.rt.FillEllipse(&ellipse(cx, cy, rr - 1.0 + 1.5 * hk), &ui.wa(0.12 * hk)); } }
            let b = if today { ui.white.clone() } else if c >= 5 { ui.wa((0.42 + 0.4 * hk) * k) } else if past { ui.wa((0.55 + 0.4 * hk) * k) } else { ui.wa(0.95 * k) };
            ui.text_c(&f, &day.to_string(), cx, cy - 8.0, 16.0, &b);
            // Fecha apuntada ese día: un punto debajo del número
            if self.event_on(civil(y, m, day)).is_some() {
                let b = if today { ui.white.clone() } else { ui.on.clone() };
                unsafe { ui.rt.FillEllipse(&ellipse(cx, cy + rr * 0.78, 1.8), &b); }
            }
            if dx.abs() < 1.0 { ui.hits.push((rect(cx - rr - 1.0, cy - rr, cx + rr + 1.0, cy + rr), id)); }
        }
        unsafe { ui.rt.PopLayer(); }
    }

    /// Mes que se ve (con flechas o rueda): título, letras de los días y la rejilla, que se desliza al
    /// cambiar de mes. `gy`: fila del título; `ch`: alto de cada semana.
    fn month(&mut self, ui: &mut Ui, t: f32, x0: f32, x1: f32, gy: f32, ch: f32, small: bool, arrows: bool) {
        let now = unsafe { GetLocalTime() };
        let (ty, tm) = (now.wYear as i32, now.wMonth as u32);
        let (y, m) = month_add(ty, tm, self.month_off);
        let mn = i18n::month((m - 1) as usize);
        let title = format!("{}{} {}", mn[..1].to_uppercase(), &mn[1..], y);
        let f = if small { ui.f.lbl.clone() } else { ui.f.name.clone() };
        let white = ui.white.clone();
        self.r_title.draw(ui, &f, &title, x0, gy, 22.0, &white, false);
        if arrows {
            let fi = &ui.f.icon.clone();
            round_btn(ui, fi, G_LEFT, x1 - 46.0, gy + 11.0, 13.0, 1, false);
            round_btn(ui, fi, G_RIGHT, x1 - 13.0, gy + 11.0, 13.0, 2, false);
        }
        let gw = x1 - x0;
        let cw = gw / 7.0;
        let hy = gy + if small { 24.0 } else { 30.0 };
        for (i, l) in i18n::weekday_letters().iter().enumerate() {
            ui.text_c(&ui.f.small, l, x0 + cw * (i as f32 + 0.5), hy, 16.0, &ui.faint);
        }
        // Cambio de mes: el nuevo entra por un lado mientras el anterior sale por el otro
        let s = self.slide;
        unsafe { ui.rt.PushAxisAlignedClip(&rect(x0 - 6.0, hy + 14.0, x1 + 6.0, hy + 30.0 + 6.0 * ch), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE); }
        if s.abs() > 0.002 && self.dir != 0.0 {
            let (py, pm) = month_add(ty, tm, self.month_off - self.dir as i32);
            let off = s - self.dir;
            self.grid(ui, py, pm, off * gw * 0.6, (1.0 - (s.abs() - 1.0).abs()).clamp(0.0, 1.0), 9.0, x0, gw, hy, ch, small);
        }
        self.grid(ui, y, m, s * gw * 0.6, 1.0 - s.abs().min(1.0) * 0.85, t, x0, gw, hy, ch, small);
        unsafe { ui.rt.PopAxisAlignedClip(); }
    }

    /// Cabecera de los tamaños grandes: día de la semana y fecha a la izquierda, semana y hora a la derecha.
    fn header(&mut self, ui: &mut Ui, d: &DFonts, x0: f32, x1: f32) {
        let now = unsafe { GetLocalTime() };
        let (ty, tm, td) = (now.wYear as i32, now.wMonth as u32, now.wDay as u32);
        let red = ui.brush(rgba(1.0, 0.42, 0.40, 1.0));
        ui.text(&ui.f.lbl, &i18n::weekday(now.wDayOfWeek as usize).to_uppercase(), x0, PAD - 4.0, 18.0, &red);
        // Fecha y hora ruedan al cambiar (cada minuto, a medianoche…)
        let white = ui.white.clone();
        // "18 de noviembre" / "November 18"
        let mes = i18n::month((tm - 1) as usize);
        let fecha = if i18n::en() { format!("{mes} {td}") } else { format!("{td} de {mes}") };
        self.r_date.draw(ui, &d.big, &fecha, x0, PAD + 16.0, 34.0, &white, false);
        self.r_time.draw(ui, &d.big, &format!("{:02}:{:02}", now.wHour, now.wMinute), x1, PAD + 16.0, 34.0, &white, true);
        ui.text_r(&ui.f.sub, &i18n::tf("Semana {}", &[&iso_week(ty, tm, td)]), x1, PAD - 4.0, 18.0, &ui.dim);
    }

    /// Día grande (tamaños pequeños): día de la semana en rojo, el número y el mes.
    fn day_block(&self, ui: &Ui, d: &DFonts, x: f32, y: f32, t: f32) {
        let now = unsafe { GetLocalTime() };
        let red = ui.brush(rgba(1.0, 0.42, 0.40, 1.0));
        ui.text(&ui.f.lbl, &i18n::weekday(now.wDayOfWeek as usize).to_uppercase(), x, y - 4.0, 18.0, &red);
        let k = ez(t, 0.05, 0.45);
        popup::push_alpha(ui.rt, k);
        ui.text(&d.huge, &now.wDay.to_string(), x - 2.0, y + 14.0 + 6.0 * (1.0 - k), 50.0, &ui.white);
        ui.rt_pop();
        let mn = i18n::month((now.wMonth - 1) as usize);
        ui.text(&ui.f.name, &format!("{}{} {}", mn[..1].to_uppercase(), &mn[1..], now.wYear), x, y + 66.0, 20.0, &ui.dim);
    }

    /// La semana de hoy, de lunes a domingo: día, número (hoy en el círculo de acento) y, con `names`,
    /// el nombre de la fecha que caiga ese día (si no, un punto).
    fn week(&self, ui: &mut Ui, x0: f32, x1: f32, y: f32, names: bool, t: f32) {
        let today = crate::deskw3::today();
        let now = unsafe { GetLocalTime() };
        let wd = (now.wDayOfWeek as i64 + 6) % 7; // 0 = lunes
        let cw = (x1 - x0) / 7.0;
        for i in 0..7i64 {
            let day = today - wd + i;
            let (_, _, dd) = crate::deskw3::civil_from_days(day);
            let cx = x0 + cw * (i as f32 + 0.5);
            let k = ez(t, 0.1 + i as f32 * 0.04, 0.35);
            let yy = y + 6.0 * (1.0 - k);
            popup::push_alpha(ui.rt, k);
            let lab = if cw >= 56.0 { i18n::weekday_short(((i + 1) % 7) as usize).to_string() } else { i18n::weekday_letters()[i as usize].to_string() };
            ui.text_c(&ui.f.small, &lab, cx, yy, 16.0, if day == today { &ui.white } else { &ui.faint });
            let cy = yy + 34.0;
            if day == today { unsafe { ui.rt.FillEllipse(&ellipse(cx, cy, 14.0), &ui.on); } }
            let b = if day == today { ui.white.clone() } else if day < today { ui.wa(0.5) } else if i >= 5 { ui.wa(0.6) } else { ui.wa(0.95) };
            ui.text_c(&ui.f.name, &dd.to_string(), cx, cy - 10.0, 20.0, &b);
            if let Some(n) = self.event_on(day) {
                if names { ui.text_c(&ui.f.small, &fit(ui, &ui.f.small, n, cw - 6.0), cx, yy + 54.0, 16.0, &ui.white); }
                else { unsafe { ui.rt.FillEllipse(&ellipse(cx, cy + 19.0, 1.8), &ui.on); } }
            }
            ui.rt_pop();
        }
    }

    /// Próximas fechas en filas de `rh` desde `y` (como mucho `n`): nombre, cuánto falta y qué día.
    /// Al pulsar una, el calendario salta a su mes. Sin fechas, cómo añadirlas.
    fn agenda(&self, ui: &mut Ui, x0: f32, x1: f32, y: f32, n: usize, rh: f32, t: f32) {
        let today = crate::deskw3::today();
        if self.events.is_empty() {
            let hk = ui.heat(39);
            let r = rect(x0 - 8.0, y - 3.0, x1 + 8.0, y + 36.0);
            ui.card_k(r, hk);
            ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, i18n::t("Sin fechas a la vista"), x1 - x0), x0, y, 18.0, &ui.dim);
            ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, i18n::t("Añádelas en Cuenta atrás"), x1 - x0), x0, y + 17.0, 16.0, &ui.faint);
            ui.hit(r, 39);
            return;
        }
        for (i, (name, day)) in self.events.iter().take(n).enumerate() {
            let k = ez(t, 0.3 + i as f32 * 0.06, 0.35);
            let yy = y + i as f32 * rh + 6.0 * (1.0 - k);
            let id = 40 + i as u32;
            let hk = ui.heat(id);
            let r = rect(x0 - 8.0, yy - 3.0, x1 + 8.0, yy + rh - 5.0);
            popup::push_alpha(ui.rt, k);
            ui.card_k(r, hk);
            unsafe { ui.rt.FillEllipse(&ellipse(x0 + 3.0 + 2.0 * hk, yy + 9.0, 3.0), &ui.on); }
            let when = days_left(day - today);
            if x1 - x0 < 200.0 {
                // Estrecho: el nombre entero arriba y, debajo, cuánto falta
                ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, name, x1 - x0 - 14.0), x0 + 14.0, yy, 18.0, &ui.white);
                if rh >= 32.0 { ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &when, x1 - x0 - 14.0), x0 + 14.0, yy + 17.0, 16.0, &ui.dim); }
            } else {
                let ww = ui.p.width(&ui.f.sub, &when);
                ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, name, x1 - x0 - 14.0 - ww - 8.0), x0 + 14.0, yy, 18.0, &ui.white);
                ui.text_r(&ui.f.sub, &when, x1, yy, 18.0, &ui.dim);
                if rh >= 32.0 { ui.text(&ui.f.sub, &date_short(*day), x0 + 14.0, yy + 17.0, 16.0, &ui.faint); }
            }
            ui.rt_pop();
            ui.hit(r, id);
        }
    }

    /// Temporizador rápido: los botones y la cuenta atrás se funden entre sí (fila desde `by`).
    fn timer(&mut self, ui: &mut Ui, d: &DFonts, dt: f32, x0: f32, x1: f32, by: f32) {
        hline(ui, x0 - 4.0, x1 + 4.0, by - 12.0);
        let left = live::timer_left();
        approach(&mut self.timer_on, if left.is_some() { 1.0 } else { 0.0 }, dt, 0.12);
        let white = ui.white.clone();
        if self.timer_on < 0.99 {
            let a = 1.0 - self.timer_on;
            popup::push_alpha(ui.rt, a);
            let dy = 8.0 * self.timer_on;
            ui.glyph(&ui.f.icon_l, G_TIMER, x0, by + 2.0 + dy, 20.0, 30.0, &ui.dim);
            ui.text(&ui.f.lbl, i18n::t("Enfoque"), x0 + 26.0, by + 7.0 + dy, 20.0, &ui.dim);
            for (i, mins) in [5u32, 15, 25].iter().enumerate() {
                let x = x1 - (3 - i) as f32 * 62.0 + 6.0;
                let r = rect(x, by + 2.0 + dy, x + 56.0, by + 32.0 + dy);
                if left.is_none() { pbutton(ui, r, None, &format!("{mins} min"), 31 + i as u32, false); }
                else { ui.button(r, None, &format!("{mins} min"), 0, false); }
            }
            ui.rt_pop();
        }
        if self.timer_on > 0.01 {
            let (l, total) = left.unwrap_or((0, 1));
            let frac = if total > 0 { l as f32 / total as f32 } else { 0.0 };
            // El anillo se vacía con suavidad entre segundo y segundo
            approach(&mut self.ring, frac, dt, 0.25);
            let dy = -8.0 * (1.0 - self.timer_on);
            popup::push_alpha(ui.rt, self.timer_on);
            let orange = ui.brush(rgba(1.0, 0.62, 0.26, 1.0));
            unsafe {
                ui.rt.DrawEllipse(&ellipse(x0 + 14.0, by + 17.0 + dy, 12.0), &ui.track, 3.0, None);
                if let Some(g) = arc_geo(ui.rt, x0 + 14.0, by + 17.0 + dy, 12.0, self.ring) { ui.rt.DrawGeometry(&g, &orange, 3.0, round_stroke(ui.rt).as_ref()); }
            }
            let ft = d.title.clone();
            self.r_left.draw(ui, &ft, &live::mmss(l), x0 + 36.0, by + 6.0 + dy, 22.0, &white, false);
            let r = rect(x1 - 96.0, by + 2.0 + dy, x1, by + 32.0 + dy);
            if left.is_some() { pbutton(ui, r, Some(G_STOP), i18n::t("Parar"), 30, false); }
            ui.rt_pop();
        }
    }
}

/// Día civil (el de `deskw3`) de una fecha.
fn civil(y: i32, m: u32, d: u32) -> i64 { crate::deskw3::days_from_civil(y, m, d) }

impl Widget for CalendarW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let l = desk::lay(Kind::Calendar);
        let (W, H) = (l.w, l.h);
        approach(&mut self.slide, 0.0, dt, 0.13);
        self.refresh_events();
        let now = unsafe { GetLocalTime() };
        let week_no = i18n::tf("Semana {}", &[&iso_week(now.wYear as i32, now.wMonth as u32, now.wDay as u32)]);
        let today = crate::deskw3::today();
        match l.sz {
            // Día grande y la próxima fecha
            Sz::Pequeno => {
                self.day_block(ui, d, PAD, PAD, t);
                match self.events.first() {
                    Some((n, day)) => {
                        unsafe { ui.rt.FillEllipse(&ellipse(PAD + 3.0, H - PAD - 25.0, 3.0), &ui.on); }
                        ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, n, W - PAD * 2.0 - 12.0), PAD + 12.0, H - PAD - 34.0, 18.0, &ui.white);
                        ui.text(&ui.f.sub, &days_left(day - today), PAD + 12.0, H - PAD - 16.0, 16.0, &ui.dim);
                        ui.hit(rect(0.0, H - PAD - 40.0, W, H), 40);
                    }
                    None => ui.text(&ui.f.sub, &week_no, PAD, H - PAD - 16.0, 16.0, &ui.faint),
                }
            }
            // Día grande y las tres próximas fechas
            Sz::Mediano => {
                self.day_block(ui, d, PAD, PAD, t);
                ui.text(&ui.f.sub, &week_no, PAD, H - PAD - 16.0, 16.0, &ui.faint);
                vline(ui, 166.0, PAD, H - PAD);
                caption(ui, i18n::t("PRÓXIMAS FECHAS"), 186.0, PAD - 4.0);
                self.agenda(ui, 186.0, W - PAD, PAD + 20.0, 3, 37.0, t);
            }
            // Día grande, la semana en línea con sus fechas y lo que viene después
            Sz::Banda => {
                self.day_block(ui, d, PAD, PAD, t);
                ui.text(&ui.f.sub, &week_no, PAD, H - PAD - 16.0, 16.0, &ui.faint);
                vline(ui, 166.0, PAD, H - PAD);
                let x0 = 186.0;
                self.week(ui, x0, W - PAD, PAD - 4.0, true, t);
                hline(ui, x0, W - PAD, 100.0);
                caption(ui, i18n::t("PRÓXIMAS FECHAS"), x0, 108.0);
                // Las siguientes, una al lado de otra
                let n = self.events.len().min(3);
                if n == 0 {
                    ui.text(&ui.f.sub, i18n::t("Sin fechas a la vista · añádelas en Cuenta atrás"), x0, 128.0, 18.0, &ui.dim);
                    ui.hit(rect(x0, 104.0, W - PAD, H - PAD), 39);
                }
                let cw = (W - PAD - x0) / 3.0;
                for (i, (name, day)) in self.events.iter().take(3).enumerate() {
                    let x = x0 + i as f32 * cw;
                    let k = ez(t, 0.35 + i as f32 * 0.06, 0.35);
                    let hk = ui.heat(40 + i as u32);
                    let r = rect(x - 8.0, 124.0, x + cw - 12.0, H - PAD + 4.0);
                    popup::push_alpha(ui.rt, k);
                    ui.card_k(r, hk);
                    unsafe { ui.rt.FillEllipse(&ellipse(x + 3.0, 137.0, 3.0), &ui.on); }
                    // Nombre y cuánto falta; el día, si cabe
                    let full = format!("{} · {}", days_left(day - today), date_short(*day));
                    let short = days_left(day - today);
                    let nm = fit(ui, &ui.f.lbl, name, cw - 24.0 - ui.p.width(&ui.f.sub, &short) - 10.0);
                    ui.text(&ui.f.lbl, &nm, x + 12.0, 128.0, 18.0, &ui.white);
                    let nx = x + 12.0 + ui.p.width(&ui.f.lbl, &nm) + 8.0;
                    let when = if ui.p.width(&ui.f.sub, &full) <= x + cw - 16.0 - nx { full } else { short };
                    ui.text(&ui.f.sub, &when, nx, 128.0, 18.0, &ui.dim);
                    ui.rt_pop();
                    ui.hit(r, 40 + i as u32);
                }
            }
            // Mes pequeño y las dos próximas fechas
            Sz::Alto => {
                let red = ui.brush(rgba(1.0, 0.42, 0.40, 1.0));
                ui.text(&ui.f.lbl, &i18n::weekday(now.wDayOfWeek as usize).to_uppercase(), PAD, PAD - 4.0, 18.0, &red);
                let fecha = if i18n::en() { format!("{} {}", i18n::month_short((now.wMonth - 1) as usize), now.wDay) } else { format!("{} {}", now.wDay, i18n::month_short((now.wMonth - 1) as usize)) };
                let white = ui.white.clone();
                self.r_date.draw(ui, &d.big, &fecha, PAD, PAD + 14.0, 34.0, &white, false);
                self.month(ui, t, PAD, W - PAD, 76.0, 22.0, true, false);
                hline(ui, PAD - 4.0, W - PAD + 4.0, 262.0);
                self.agenda(ui, PAD, W - PAD, 272.0, 2, 36.0, t);
            }
            // Mes completo con sus fechas marcadas y el temporizador de enfoque
            Sz::Grande => {
                self.header(ui, d, PAD, W - PAD);
                self.month(ui, t, PAD, W - PAD, 92.0, 25.0, false, true);
                self.timer(ui, d, dt, PAD, W - PAD, H - PAD - 34.0);
            }
            // Mes, próximas fechas y temporizador
            Sz::Columna => {
                self.header(ui, d, PAD, W - PAD);
                self.month(ui, t, PAD, W - PAD, 92.0, 25.0, false, true);
                hline(ui, PAD - 4.0, W - PAD + 4.0, 300.0);
                caption(ui, i18n::t("PRÓXIMAS FECHAS"), PAD, 310.0);
                self.agenda(ui, PAD, W - PAD, 332.0, 4, 37.0, t);
                self.timer(ui, d, dt, PAD, W - PAD, H - PAD - 34.0);
            }
            // Mes a la izquierda; la semana, las próximas fechas y el temporizador a la derecha
            Sz::Extragrande => {
                let mx = 340.0;
                self.header(ui, d, PAD, mx);
                self.month(ui, t, PAD, mx, 92.0, 25.0, false, true);
                vline(ui, 370.0, PAD, H - PAD);
                let rx = 400.0;
                caption(ui, i18n::t("ESTA SEMANA"), rx, PAD - 4.0);
                self.week(ui, rx, W - PAD, PAD + 16.0, true, t);
                hline(ui, rx - 4.0, W - PAD + 4.0, 112.0);
                caption(ui, i18n::t("PRÓXIMAS FECHAS"), rx, 122.0);
                self.agenda(ui, rx, W - PAD, 144.0, 3, 37.0, t);
                self.timer(ui, d, dt, rx, W - PAD, H - PAD - 34.0);
            }
        }
    }
    fn click(&mut self, id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        match id? {
            1 => { self.go(-1); None }
            2 => { self.go(1); None }
            30 => Some(Box::new(live::timer_stop)),
            i @ 31..=33 => { let m = [5u32, 15, 25][(i - 31) as usize]; Some(Box::new(move || live::timer_start(m * 60))) }
            39 => Some(Box::new(crate::deskw3::countdown_edit)),
            // Una fecha: el calendario salta a su mes
            i @ 40..=49 => {
                let (_, day) = self.events.get((i - 40) as usize)?.clone();
                let (y, m, _) = crate::deskw3::civil_from_days(day);
                let now = unsafe { GetLocalTime() };
                let off = (y - now.wYear as i32) * 12 + m as i32 - now.wMonth as i32;
                if off != self.month_off { let d = off - self.month_off; self.month_off = off; self.slide = (d as f32).clamp(-1.0, 1.0); self.dir = d.signum() as f32; }
                None
            }
            _ => { if self.month_off != 0 { let d = -self.month_off; self.month_off = 0; self.slide = (d as f32).clamp(-1.0, 1.0); self.dir = d.signum() as f32; } None }
        }
    }
    // La rueda cambia de mes donde se ve un mes
    fn wheel(&mut self, d: i32) -> bool {
        if matches!(desk::lay(Kind::Calendar).sz, Sz::Pequeno | Sz::Mediano | Sz::Banda) { return false; }
        self.go(if d < 0 { 1 } else { -1 });
        true
    }
    fn animating(&self, t: f32) -> bool {
        t < 1.4 || self.slide.abs() > 0.002 || self.r_time.busy() || self.r_date.busy() || self.r_title.busy() || self.r_left.busy()
            || (self.timer_on > 0.001 && self.timer_on < 0.999) || (self.timer_on > 0.5)
    }
    fn smooth(&self) -> bool { self.slide.abs() > 0.002 || self.r_title.busy() || self.r_time.busy() || (self.timer_on > 0.001 && self.timer_on < 0.999) }
    fn layouts(&self) -> bool { true }
}

/// Arco en el sentido de las agujas desde las 12 (fracción 0..1).
fn arc_geo(rt: &ID2D1RenderTarget, cx: f32, cy: f32, r: f32, frac: f32) -> Option<ID2D1PathGeometry> {
    let frac = frac.clamp(0.001, 0.999);
    unsafe {
        let f = rt.GetFactory().ok()?;
        let g = f.CreatePathGeometry().ok()?;
        let s = g.Open().ok()?;
        s.BeginFigure(Vector2 { X: cx, Y: cy - r }, D2D1_FIGURE_BEGIN_HOLLOW);
        let a = frac * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        s.AddArc(&D2D1_ARC_SEGMENT { point: Vector2 { X: cx + r * a.cos(), Y: cy + r * a.sin() }, size: D2D_SIZE_F { width: r, height: r }, rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE, arcSize: if frac > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL } });
        s.EndFigure(D2D1_FIGURE_END_OPEN);
        s.Close().ok()?;
        Some(g)
    }
}

trait UiExt { fn rt_pop(&self); }
impl UiExt for Ui<'_> { fn rt_pop(&self) { unsafe { self.rt.PopLayer(); } } }

// ---------------------------------------------------------------- tiempo
/// `curve`/`lo`/`hi`: la curva que se ve, que se transforma con suavidad cuando llega otra previsión;
/// `probe`: hora bajo el ratón (desliza de una a otra) y su opacidad.
#[derive(Default)]
struct WeatherW { curve: Vec<f32>, lo: f32, hi: f32, r_temp: Roll, r_desc: Roll, probe_x: f32, probe_a: f32, morph: bool }

impl WeatherW {
    /// Temperatura grande con el icono a su derecha (`top`: arriba de la cifra). Al aparecer, la
    /// temperatura cuenta desde unos grados menos; luego rueda cuando cambia. Devuelve dónde acaba.
    fn now(&mut self, ui: &mut Ui, d: &DFonts, cur: &Weather, t: f32, x: f32, top: f32) -> f32 {
        let white = ui.white.clone();
        let temp = format!("{:.0}°", cur.temp - 6.0 * (1.0 - ez(t, 0.1, 0.8)));
        if t < 0.95 { ui.text(&d.huge, &temp, x - 2.0, top, 50.0, &white); self.r_temp.cur = temp.clone(); }
        else { self.r_temp.draw(ui, &d.huge, &temp, x - 2.0, top, 50.0, &white, false); }
        let tw = ui.p.width(&d.huge, &temp);
        // El icono entra con un pequeño salto
        let ip = crate::ease_back(ez(t, 0.2, 0.5));
        let (ix, iy, isz) = (x + tw + 4.0, top - 4.0, 58.0);
        unsafe {
            let base = cur_transform(ui);
            ui.rt.SetTransform(&mul(&scale_at(0.6 + 0.4 * ip, ix + isz / 2.0, iy + isz / 2.0), &base));
            weather::draw_icon_anim(ui.rt, &ui.p, &ui.f.icon_xl, ix, iy, isz, isz, cur.code, cur.is_day, &white, Some(t));
            ui.rt.SetTransform(&base);
        }
        ix + isz
    }

    /// Bloque de los tamaños de una celda de ancho: ciudad, temperatura con icono, cielo y máx/mín.
    fn compact(&mut self, ui: &mut Ui, d: &DFonts, cur: &Weather, fc: Option<&weather::Forecast>, city: &str, t: f32, x0: f32, x1: f32) {
        ui.text(&d.title, &fit(ui, &d.title, city, x1 - x0), x0, PAD - 2.0, 22.0, &ui.white);
        self.now(ui, d, cur, t, x0, PAD + 26.0);
        let white = ui.white.clone();
        let flbl = ui.f.lbl.clone();
        let desc = fit(ui, &flbl, weather_desc(cur.code), x1 - x0);
        self.r_desc.draw(ui, &flbl, &desc, x0, PAD + 92.0, 18.0, &white, false);
        if let Some(d0) = fc.and_then(|f| f.days.first()) {
            ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &i18n::tf("Máx {}°  ·  Mín {}°", &[&format!("{:.0}", d0.3), &format!("{:.0}", d0.2)]), x1 - x0), x0, PAD + 112.0, 18.0, &ui.dim);
        }
    }

    /// Cabecera de los tamaños grandes (como el widget de siempre): ciudad y cielo arriba, la
    /// temperatura con su icono y, a la derecha, los detalles que quepan; debajo, sol y luna.
    fn top(&mut self, ui: &mut Ui, d: &DFonts, cur: &Weather, f: &weather::Forecast, city: &str, t: f32, x0: f32, x1: f32) {
        let white = ui.white.clone();
        let ftitle = d.title.clone();
        let desc = weather_desc(cur.code);
        let dw = ui.p.width(&ftitle, desc);
        ui.text(&d.title, &fit(ui, &d.title, city, x1 - x0 - dw - 16.0), x0, PAD - 2.0, 22.0, &ui.white);
        self.r_desc.draw(ui, &ftitle, desc, x1, PAD - 2.0, 22.0, &white, true);
        let end = self.now(ui, d, cur, t, x0, PAD + 22.0);
        // A la derecha, de más a menos importante: solo las líneas que caben enteras
        let room = x1 - end - 12.0;
        let mut lines: Vec<String> = Vec::new();
        if let Some(d0) = f.days.first() { lines.push(i18n::tf("Máx {}°  ·  Mín {}°", &[&format!("{:.0}", d0.3), &format!("{:.0}", d0.2)])); }
        let rain_now = f.rain.first().copied().unwrap_or(0.0);
        let rain_max = f.rain.iter().take(12).cloned().fold(0.0f32, f32::max);
        let long = i18n::tf("Lluvia ahora {}%  ·  próximas 12 h {}%", &[&format!("{:.0}", rain_now), &format!("{:.0}", rain_max)]);
        if ui.p.width(&ui.f.sub, &long) <= room { lines.push(long); } else { lines.push(i18n::tf("Lluvia {}%", &[&format!("{:.0}", rain_max)])); }
        if let Some(d1) = f.days.get(1) {
            lines.push(i18n::tf("Mañana {}° / {}° · {}", &[&format!("{:.0}", d1.3), &format!("{:.0}", d1.2), &weather_desc(d1.1).to_lowercase()]));
        }
        let mut y = PAD + 24.0;
        for s in lines.iter() {
            if ui.p.width(&ui.f.sub, s) > room { continue; }
            ui.text_r(&ui.f.sub, s, x1, y, 18.0, &ui.dim);
            y += 18.0;
        }
        // Amanecer y anochecer, bajo la temperatura
        if let Some((rise, set)) = &f.sun {
            let ks = ez(t, 0.3, 0.4);
            popup::push_alpha(ui.rt, ks);
            let y = PAD + 76.0;
            ui.glyph(&ui.f.icon, "\u{E706}", x0 - 2.0, y, 16.0, 18.0, &ui.dim);
            ui.text(&ui.f.sub, rise, x0 + 18.0, y, 18.0, &ui.dim);
            let x2 = x0 + 18.0 + ui.p.width(&ui.f.sub, rise) + 16.0;
            ui.glyph(&ui.f.icon, "\u{E708}", x2, y, 16.0, 18.0, &ui.dim);
            ui.text(&ui.f.sub, set, x2 + 20.0, y, 18.0, &ui.dim);
            ui.rt_pop();
        }
    }

    /// Curva de las próximas 24 h entre `x0` y `x1` (de `top` a `bot`), con lluvia, máxima y mínima,
    /// las horas debajo y una sonda que sigue al ratón.
    fn curve(&mut self, ui: &mut Ui, f: &weather::Forecast, t: f32, dt: f32, x0: f32, x1: f32, top: f32, bot: f32) {
        let n = f.hours.len().min(25);
        if n < 2 { return; }
        let gw = x1 - x0;
        let temps: Vec<f32> = f.hours.iter().take(n).map(|h| h.1).collect();
        let lo_t = temps.iter().cloned().fold(f32::MAX, f32::min);
        let hi_t = temps.iter().cloned().fold(f32::MIN, f32::max).max(lo_t + 1.0);
        // La curva que se ve va hacia la nueva: al llegar otra previsión se transforma, no salta
        if self.curve.len() != n { self.curve = temps.clone(); self.lo = lo_t; self.hi = hi_t; }
        let mut mv = false;
        for i in 0..n { mv |= approach(&mut self.curve[i], temps[i], dt, 0.35); }
        mv |= approach(&mut self.lo, lo_t, dt, 0.35);
        mv |= approach(&mut self.hi, hi_t, dt, 0.35);
        self.morph = mv;
        let (lo, hi) = (self.lo, self.hi.max(self.lo + 1.0));
        let step = gw / (n - 1) as f32;
        let pts: Vec<(f32, f32)> = self.curve.iter().enumerate().map(|(i, v)| (x0 + i as f32 * step, bot - 16.0 - (v - lo) / (hi - lo) * (bot - top - 32.0))).collect();
        // Zonas para el ratón: una por hora
        for i in 0..n {
            let x = x0 + i as f32 * step;
            ui.hit(rect(x - step / 2.0, top - 10.0, x + step / 2.0, bot + 20.0), 100 + i as u32);
        }
        // Barras de lluvia al fondo
        for (i, r) in f.rain.iter().take(n).enumerate() {
            if *r < 5.0 { continue; }
            let h = r / 100.0 * 26.0 * ez(t, 0.2 + i as f32 * 0.01, 0.4);
            let x = x0 + i as f32 * step;
            unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(x - 3.0, bot - h, x + 3.0, bot), 2.0), &ui.brush(rgba(0.48, 0.72, 1.0, 0.45))); }
        }
        // La curva se dibuja de izquierda a derecha al aparecer
        let k = ez(t, 0.15, 1.0);
        unsafe {
            ui.rt.PushAxisAlignedClip(&rect(x0 - PAD, top - 10.0, x0 + gw * k + 8.0, bot + 2.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            if let Some(a) = smooth_path(ui.rt, &pts, Some(bot)) { ui.rt.FillGeometry(&a, &ui.p.vgrad(top, bot, ui.ic(0.13), ui.ic(0.0)), None); }
            if let Some(l) = smooth_path(ui.rt, &pts, None) {
                // Caliente arriba (naranja), frío abajo (azul)
                let g = ui.p.vgrad(top, bot - 16.0, rgba(1.0, 0.70, 0.36, 1.0), rgba(0.49, 0.72, 1.0, 1.0));
                ui.rt.DrawGeometry(&l, &g, 3.0, round_stroke(ui.rt).as_ref());
            }
            ui.rt.PopAxisAlignedClip();
        }
        // Ahora: punto que late
        let pulse = 0.5 + 0.5 * (t * 2.4).sin();
        unsafe {
            ui.rt.FillEllipse(&ellipse(pts[0].0, pts[0].1, 5.0 + 4.0 * pulse), &ui.wa(0.18 * (1.0 - pulse)));
            ui.rt.FillEllipse(&ellipse(pts[0].0, pts[0].1, 4.5), &ui.white);
        }
        // Máxima y mínima rotuladas
        let imax = temps.iter().enumerate().fold(0, |b, (i, v)| if *v > temps[b] { i } else { b });
        let imin = temps.iter().enumerate().fold(0, |b, (i, v)| if *v < temps[b] { i } else { b });
        for (i, below) in [(imax, false), (imin, true)] {
            if i == 0 || pts[i].0 > x0 + gw * k { continue; }
            let (x, y) = pts[i];
            let s = format!("{:.0}°", temps[i]);
            let x = x.clamp(x0 + 10.0, x1 - 10.0);
            ui.text_c(&ui.f.small, &s, x, if below { y + 6.0 } else { y - 22.0 }, 16.0, &ui.white);
        }
        // Horas (cada 3, o cada 6 si la curva es estrecha)
        let every = if gw / (n - 1) as f32 * 3.0 < 34.0 { 6 } else { 3 };
        for i in (0..n).step_by(every) {
            let label = if i == 0 { i18n::t("Ahora").to_string() } else { format!("{}", f.hours[i].0) };
            let x = (x0 + i as f32 * step).clamp(x0 + 14.0, x1 - 10.0);
            // Bajo la sonda, las horas se apagan para dejar sitio a su etiqueta
            let near = (1.0 - ((x - self.probe_x).abs() / 40.0)).clamp(0.0, 1.0) * self.probe_a;
            popup::push_alpha(ui.rt, 1.0 - near);
            ui.text_c(&ui.f.small, &label, x, bot + 4.0, 16.0, &ui.dim);
            ui.rt_pop();
        }
        // Sonda: al pasar el ratón por la curva, una línea fina sigue al cursor y muestra la hora,
        // su temperatura y la probabilidad de lluvia
        let hov = ui.hover.filter(|h| (100..100 + n as u32).contains(h)).map(|h| (h - 100) as usize);
        let mut pm = false;
        if let Some(i) = hov {
            let tx = x0 + i as f32 * step;
            if self.probe_a < 0.05 { self.probe_x = tx; }
            pm |= approach(&mut self.probe_x, tx, dt, 0.06);
        }
        pm |= approach(&mut self.probe_a, if hov.is_some() { 1.0 } else { 0.0 }, dt, 0.08);
        // Quieta bajo el ratón no pide fotogramas: solo mientras se desliza o aparece
        self.morph |= pm;
        if self.probe_a > 0.01 {
            let fi = ((self.probe_x - x0) / step).clamp(0.0, (n - 1) as f32);
            let (i0, fr) = (fi.floor() as usize, fi.fract());
            let i1 = (i0 + 1).min(n - 1);
            let yv = pts[i0].1 + (pts[i1].1 - pts[i0].1) * fr;
            let near = if fr < 0.5 { i0 } else { i1 };
            let a = self.probe_a;
            unsafe {
                ui.rt.FillRectangle(&rect(self.probe_x - 0.5, top - 6.0, self.probe_x + 0.5, bot), &ui.wa(0.35 * a));
                ui.rt.FillEllipse(&ellipse(self.probe_x, yv, 5.5), &ui.wa(a));
            }
            let label = format!("{}:00 · {:.0}°{}", f.hours[near].0, temps[near], f.rain.get(near).filter(|r| **r >= 5.0).map(|r| i18n::tf(" · {}% lluvia", &[&format!("{:.0}", r)])).unwrap_or_default());
            let lw = ui.p.width(&ui.f.small, &label) + 16.0;
            let lx = (self.probe_x - lw / 2.0).clamp(x0 - 4.0, x1 - lw + 4.0);
            popup::push_alpha(ui.rt, a);
            unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(lx, bot + 2.0, lx + lw, bot + 22.0), 10.0), &ui.chip()); }
            ui.text_c(&ui.f.small, &label, lx + lw / 2.0, bot + 4.0, 16.0, &ui.white);
            ui.rt_pop();
        }
    }
}

/// Próximas `n` horas en columnas: hora, icono, temperatura y, si puede llover, la probabilidad.
fn hour_cols(ui: &Ui, f: &weather::Forecast, x0: f32, x1: f32, y: f32, n: usize, t: f32) {
    let n = n.min(f.hours.len());
    if n == 0 { return; }
    let cw = (x1 - x0) / n as f32;
    let blue = ui.brush(rgba(0.55, 0.76, 1.0, 1.0));
    for i in 0..n {
        let (hr, tp, code, day) = f.hours[i];
        let k = ez(t, 0.25 + i as f32 * 0.03, 0.35);
        let cx = x0 + cw * (i as f32 + 0.5);
        let yy = y + 6.0 * (1.0 - k);
        popup::push_alpha(ui.rt, k);
        let label = if i == 0 { i18n::t("Ahora").to_string() } else { format!("{hr}") };
        ui.text_c(&ui.f.small, &label, cx, yy, 16.0, if i == 0 { &ui.white } else { &ui.dim });
        let b = ui.white.clone();
        weather::draw_icon(ui.rt, &ui.p, &ui.f.icon_l, cx - 14.0, yy + 20.0, 28.0, 28.0, code, day, &b);
        ui.text_c(&ui.f.lbl, &format!("{tp:.0}°"), cx, yy + 52.0, 18.0, &ui.white);
        if let Some(r) = f.rain.get(i).filter(|r| **r >= 20.0) { ui.text_c(&ui.f.small, &format!("{r:.0}%"), cx, yy + 70.0, 16.0, &blue); }
        ui.rt_pop();
    }
}

/// Próximos días en columnas (como el widget de siempre): día, icono, máxima y mínima.
fn day_cols(ui: &Ui, f: &weather::Forecast, x0: f32, x1: f32, y: f32, n: usize, t: f32) {
    let days: Vec<_> = f.days.iter().take(n).collect();
    let cw = (x1 - x0) / days.len().max(1) as f32;
    for (i, (wd, code, mn, mx)) in days.iter().enumerate() {
        let k = ez(t, 0.35 + i as f32 * 0.05, 0.35);
        let cx = x0 + cw * (i as f32 + 0.5);
        let yy = y + 8.0 * (1.0 - k);
        popup::push_alpha(ui.rt, k);
        ui.text_c(&ui.f.lbl, if i == 0 { i18n::t("Hoy") } else { i18n::weekday_short(*wd as usize) }, cx, yy, 18.0, if i == 0 { &ui.white } else { &ui.dim });
        let b = ui.white.clone();
        weather::draw_icon(ui.rt, &ui.p, &ui.f.icon_l, cx - 14.0, yy + 20.0, 28.0, 28.0, *code, true, &b);
        ui.text_c(&ui.f.lbl, &format!("{:.0}°", mx), cx, yy + 52.0, 18.0, &ui.white);
        ui.text_c(&ui.f.sub, &format!("{:.0}°", mn), cx, yy + 70.0, 16.0, &ui.dim);
        ui.rt_pop();
    }
}

/// Próximos días en filas: día, icono, mínima y máxima (con la barra de su margen si hay sitio).
fn day_rows(ui: &Ui, f: &weather::Forecast, x0: f32, x1: f32, y: f32, n: usize, rh: f32, t: f32) {
    let days: Vec<_> = f.days.iter().take(n).collect();
    let lo = days.iter().map(|d| d.2).fold(f32::MAX, f32::min);
    let hi = days.iter().map(|d| d.3).fold(f32::MIN, f32::max).max(lo + 1.0);
    let wide = x1 - x0 >= 220.0;
    for (i, (wd, code, mn, mx)) in days.iter().enumerate() {
        let k = ez(t, 0.3 + i as f32 * 0.05, 0.35);
        let yy = y + i as f32 * rh + 6.0 * (1.0 - k);
        let mid = yy + rh / 2.0;
        popup::push_alpha(ui.rt, k);
        ui.text(&ui.f.lbl, if i == 0 { i18n::t("Hoy") } else { i18n::weekday_short(*wd as usize) }, x0, mid - 9.0, 18.0, if i == 0 { &ui.white } else { &ui.dim });
        let b = ui.white.clone();
        weather::draw_icon(ui.rt, &ui.p, &ui.f.icon_l, x0 + 40.0, mid - 14.0, 28.0, 28.0, *code, true, &b);
        ui.text_r(&ui.f.lbl, &format!("{:.0}°", mx), x1, mid - 9.0, 18.0, &ui.white);
        if wide {
            // Barra del margen de cada día sobre el de toda la semana
            let (bx0, bx1) = (x0 + 116.0, x1 - 34.0);
            let bw = bx1 - bx0;
            let (a, b2) = (bx0 + (mn - lo) / (hi - lo) * bw, bx0 + (mx - lo) / (hi - lo) * bw);
            ui.text_r(&ui.f.sub, &format!("{:.0}°", mn), bx0 - 8.0, mid - 9.0, 18.0, &ui.dim);
            unsafe {
                ui.rt.FillRoundedRectangle(&rrect(rect(bx0, mid - 2.0, bx1, mid + 2.0), 2.0), &ui.track);
                let g = ui.p.dgrad(bx0, mid, bx1, mid, rgba(0.49, 0.72, 1.0, 1.0), rgba(1.0, 0.70, 0.36, 1.0));
                ui.rt.FillRoundedRectangle(&rrect(rect(a, mid - 2.0, b2.max(a + 4.0), mid + 2.0), 2.0), &g);
            }
        } else {
            ui.text_r(&ui.f.sub, &format!("{:.0}°", mn), x1 - 34.0, mid - 9.0, 18.0, &ui.dim);
        }
        ui.rt_pop();
    }
}

/// Detalles en recuadros: lluvia, sol, mañana y el margen de las próximas 24 h (los que haya datos).
fn weather_tiles(ui: &Ui, d: &DFonts, f: &weather::Forecast, rs: &[D2D_RECT_F], t: f32) {
    let mut tiles: Vec<(String, String, String)> = Vec::new();
    let rain_now = f.rain.first().copied().unwrap_or(0.0);
    let rain_max = f.rain.iter().take(12).cloned().fold(0.0f32, f32::max);
    tiles.push((i18n::t("LLUVIA").into(), format!("{rain_now:.0} %"), i18n::tf("Hasta {} % en 12 h", &[&format!("{rain_max:.0}")])));
    if let Some((rise, set)) = &f.sun { tiles.push((i18n::t("SOL").into(), rise.clone(), i18n::tf("Se pone a las {}", &[set]))); }
    if let Some(d1) = f.days.get(1) { tiles.push((i18n::t("MAÑANA").into(), format!("{:.0}° / {:.0}°", d1.3, d1.2), weather_desc(d1.1).to_string())); }
    let n = f.hours.len().min(25);
    if n >= 2 {
        let hs = &f.hours[..n];
        let lo = hs.iter().map(|h| h.1).fold(f32::MAX, f32::min);
        let (imax, hi) = hs.iter().enumerate().fold((0, f32::MIN), |b, (i, h)| if h.1 > b.1 { (i, h.1) } else { b });
        tiles.push((i18n::t("PRÓXIMAS 24 H").into(), format!("{lo:.0}° – {hi:.0}°"), i18n::tf("Máxima a las {}:00", &[&hs[imax].0])));
    }
    for (i, (r, (a, b, c))) in rs.iter().zip(tiles.iter()).enumerate() {
        tile(ui, d, *r, a, b, c, ez(t, 0.35 + i as f32 * 0.06, 0.4));
    }
}

impl Widget for WeatherW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let l = desk::lay(Kind::Weather);
        let (W, H) = (l.w, l.h);
        let city = sys::config_value("ciudad").unwrap_or_else(|| i18n::t("Tu ubicación").into());
        let cur = *WEATHER.lock().unwrap();
        let fc = weather::FORECAST.lock().unwrap().clone();
        // Todo el widget actualiza al pulsar (por debajo de las zonas de las horas)
        ui.hit(rect(0.0, 0.0, W, H), 1);
        let Some(cur) = cur else {
            ui.text(&d.title, &fit(ui, &d.title, &city, W - PAD * 2.0), PAD, PAD - 2.0, 22.0, &ui.white);
            let (a, b) = wrap2(ui, &ui.f.sub, i18n::t("Cargando el tiempo…"), W - PAD * 2.0);
            ui.text(&ui.f.sub, &a, PAD, PAD + 30.0, 20.0, &ui.dim);
            if !b.is_empty() { ui.text(&ui.f.sub, &b, PAD, PAD + 48.0, 20.0, &ui.dim); }
            return;
        };
        // Pequeño (o sin previsión aún): ciudad, temperatura con icono, cielo y máx/mín
        let Some(f) = fc.as_ref().filter(|_| l.sz != Sz::Pequeno) else {
            self.compact(ui, d, &cur, fc.as_ref(), &city, t, PAD, if l.sz == Sz::Pequeno { W - PAD } else { 150.0 });
            return;
        };
        match (l.sz, Some(f)) {
            // Lo mismo y, al lado, las próximas horas (4 en Mediano, 12 en Banda)
            (Sz::Mediano | Sz::Banda, Some(f)) => {
                self.compact(ui, d, &cur, Some(f), &city, t, PAD, 150.0);
                vline(ui, 170.0, PAD + 4.0, H - PAD - 4.0);
                let n = if l.sz == Sz::Mediano { 4 } else { 12 };
                hour_cols(ui, f, 180.0, W - PAD + 6.0, PAD + 26.0, n, t);
            }
            // Temperatura arriba y los próximos 5 días en filas
            (Sz::Alto, Some(f)) => {
                self.compact(ui, d, &cur, Some(f), &city, t, PAD, W - PAD);
                hline(ui, PAD - 4.0, W - PAD + 4.0, 160.0);
                day_rows(ui, f, PAD, W - PAD, 166.0, 5, 34.8, t);
            }
            // Cabecera, curva de 24 h y 5 días
            (Sz::Grande, Some(f)) => {
                self.top(ui, d, &cur, f, &city, t, PAD, W - PAD);
                self.curve(ui, f, t, dt, PAD, W - PAD, 112.0, 206.0);
                hline(ui, PAD - 4.0, W - PAD + 4.0, 232.0);
                day_cols(ui, f, PAD, W - PAD, 240.0, 5, t);
            }
            // Lo del Grande con todos los días y los detalles debajo
            (Sz::Columna, Some(f)) => {
                self.top(ui, d, &cur, f, &city, t, PAD, W - PAD);
                self.curve(ui, f, t, dt, PAD, W - PAD, 112.0, 206.0);
                hline(ui, PAD - 4.0, W - PAD + 4.0, 232.0);
                day_cols(ui, f, PAD, W - PAD, 240.0, 6, t);
                hline(ui, PAD - 4.0, W - PAD + 4.0, 344.0);
                let (cw, ch) = ((W - PAD * 2.0 - 10.0) / 2.0, (H - PAD - 358.0 - 10.0) / 2.0);
                let rs: Vec<D2D_RECT_F> = (0..4).map(|i| { let (c, r) = ((i % 2) as f32, (i / 2) as f32); rect(PAD + c * (cw + 10.0), 358.0 + r * (ch + 10.0), PAD + c * (cw + 10.0) + cw, 358.0 + r * (ch + 10.0) + ch) }).collect();
                weather_tiles(ui, d, f, &rs, t);
            }
            // Cabecera, curva y días a la izquierda; los detalles en columna a la derecha
            (Sz::Extragrande, Some(f)) => {
                let x1 = 460.0;
                self.top(ui, d, &cur, f, &city, t, PAD, x1);
                self.curve(ui, f, t, dt, PAD, x1, 112.0, 206.0);
                hline(ui, PAD - 4.0, x1 + 4.0, 232.0);
                day_cols(ui, f, PAD, x1, 240.0, 6, t);
                vline(ui, 490.0, PAD, H - PAD);
                let (x0, ch) = (510.0, (H - PAD * 2.0 - 30.0) / 4.0);
                let rs: Vec<D2D_RECT_F> = (0..4).map(|i| rect(x0, PAD + i as f32 * (ch + 10.0), W - PAD, PAD + i as f32 * (ch + 10.0) + ch)).collect();
                weather_tiles(ui, d, f, &rs, t);
            }
            _ => {}
        }
    }
    fn click(&mut self, id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        if id.is_some_and(|i| i == 1 || i >= 100) { WEATHER_NOW.store(true, Ordering::Relaxed); }
        None
    }
    fn tick(&mut self) -> bool { true }
    // El icono animado y el punto que late van al ritmo del tic de cada segundo (casi sin CPU); las
    // transiciones (curva nueva, sonda, textos que ruedan) piden fotogramas solo mientras duran
    fn animating(&self, t: f32) -> bool { t < 1.4 || self.morph || self.r_temp.busy() || self.r_desc.busy() }
    fn smooth(&self) -> bool { self.morph || self.r_temp.busy() || self.r_desc.busy() }
    fn layouts(&self) -> bool { true }
}

// ---------------------------------------------------------------- música con letra en vivo
#[derive(Default)]
struct MusicW {
    art: Option<(u64, ID2D1Bitmap)>,
    /// Carátula anterior mientras se funde con la nueva
    prev_art: Option<ID2D1Bitmap>,
    art_at: Option<Instant>,
    tint: [f32; 3],
    tint_to: [f32; 3],
    cur: Option<usize>,
    shift: f32,
    key: String,
    /// Cuándo cambió la canción (entran el título y la cola)
    title_at: Option<Instant>,
    /// Pulso del botón de reproducir al pulsarlo
    pulse_at: Option<Instant>,
    /// Paso de pausa a reproducir en el botón (1 = sonando)
    play_k: f32,
    /// Se está viendo algo que se mueve solo (el espectro), no la cola
    live_view: bool,
    /// El ratón está sobre la línea de tiempo (su marca sigue al cursor)
    seek_hover: bool,
    playing: bool,
    /// Dónde está la línea de tiempo (x de inicio y de fin), para saber a qué punto saltar al pulsarla
    tl: (f32, f32),
    /// Alturas del espectro que se ven (suben rápido y caen despacio)
    spec: Vec<f32>,
}

/// a · b (primero a, luego b) para transformaciones 2D de Direct2D.
fn mul(a: &Matrix3x2, b: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: a.M11 * b.M11 + a.M12 * b.M21, M12: a.M11 * b.M12 + a.M12 * b.M22,
        M21: a.M21 * b.M11 + a.M22 * b.M21, M22: a.M21 * b.M12 + a.M22 * b.M22,
        M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31, M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    }
}

/// Escala `s` con el punto (x, y) fijo.
fn scale_at(s: f32, x: f32, y: f32) -> Matrix3x2 { Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: x * (1.0 - s), M32: y * (1.0 - s) } }

impl MusicW {
    /// Carátula de lado `s`: cuando cambia, la nueva entra con un "pop" mientras la anterior se desvanece.
    fn cover(&mut self, ui: &mut Ui, d: &DFonts, x: f32, y: f32, s: f32) {
        let fade = self.art_at.map(|a| (a.elapsed().as_secs_f32() / 0.5).min(1.0)).unwrap_or(1.0);
        if fade >= 1.0 { self.prev_art = None; }
        unsafe {
            let base = cur_transform(ui);
            if let Some(p) = &self.prev_art {
                popup::push_alpha(ui.rt, 1.0 - popup::ease_out(fade));
                draw_art(ui.rt, p, ART_BIG_PX, x, y, s, 14.0);
                ui.rt.PopLayer();
            }
            let sc = 0.94 + 0.06 * crate::ease_back(fade);
            ui.rt.SetTransform(&mul(&scale_at(sc, x + s / 2.0, y + s / 2.0), &base));
            popup::push_alpha(ui.rt, popup::ease_out(fade));
            match &self.art {
                Some((_, bmp)) => draw_art(ui.rt, bmp, ART_BIG_PX, x, y, s, 14.0),
                None => {
                    ui.rt.FillRoundedRectangle(&rrect(rect(x, y, x + s, y + s), 14.0), &ui.wa(0.12));
                    ui.glyph(&d.glyph_l, G_MUSIC, x, y, s, s, &ui.dim);
                }
            }
            ui.rt.PopLayer();
            ui.rt.SetTransform(&base);
        }
        ui.hit(rect(x, y, x + s, y + s), 4);
    }

    /// Botón central de reproducir/pausa: late al pulsarlo y el icono gira al cambiar.
    fn play_btn(&mut self, ui: &mut Ui, d: &DFonts, dt: f32, cx: f32, cy: f32, r: f32) {
        let fm = &d.glyph_m.clone();
        let pulse = self.pulse_at.map(|a| a.elapsed().as_secs_f32()).filter(|s| *s < 0.35).map(|s| (s / 0.35 * std::f32::consts::PI).sin()).unwrap_or(0.0);
        approach(&mut self.play_k, if self.playing { 1.0 } else { 0.0 }, dt, 0.08);
        unsafe {
            let base = cur_transform(ui);
            ui.rt.SetTransform(&mul(&scale_at(1.0 - 0.12 * pulse, cx, cy), &base));
            round_btn(ui, fm, "", cx, cy, r, 2, true);
            let inner = cur_transform(ui);
            let dark = ui.brush(rgba(0.08, 0.08, 0.09, 1.0));
            let press = 1.0 - 0.14 * desk::press(2);
            for (g, k) in [(G_PAUSE, self.play_k), (G_PLAY, 1.0 - self.play_k)] {
                if k <= 0.01 { continue; }
                let ang = (1.0 - k) * 1.2;
                let s = (0.55 + 0.45 * k) * press;
                let (c, sn) = (ang.cos() * s, ang.sin() * s);
                let m2 = Matrix3x2 { M11: c, M12: sn, M21: -sn, M22: c, M31: cx - c * cx + sn * cy, M32: cy - sn * cx - c * cy };
                ui.rt.SetTransform(&mul(&m2, &inner));
                popup::push_alpha(ui.rt, k);
                ui.glyph(fm, g, cx - r, cy - r, r * 2.0, r * 2.0, &dark);
                ui.rt.PopLayer();
            }
            ui.rt.SetTransform(&base);
        }
    }

    /// Anterior, reproducir y siguiente, centrados en `cx`.
    fn controls(&mut self, ui: &mut Ui, d: &DFonts, dt: f32, cx: f32, cy: f32, big: bool) {
        let fm = &d.glyph_m.clone();
        let (gap, rs, rm) = if big { (64.0, 20.0, 22.0) } else { (48.0, 17.0, 19.0) };
        round_btn(ui, fm, G_PREV, cx - gap, cy, rs, 1, false);
        self.play_btn(ui, d, dt, cx, cy, rm);
        round_btn(ui, fm, G_NEXT, cx + gap, cy, rs, 3, false);
    }

    /// Línea de tiempo de `x0` a `x1`: los tiempos debajo o, con `inline`, a los lados.
    fn timeline(&mut self, ui: &mut Ui, secs: Option<(f32, f32)>, x0: f32, x1: f32, py: f32, inline: bool) {
        let Some((p, e)) = secs else { self.seek_hover = false; return };
        let (lt, rt) = (live::mmss(p as u32), live::mmss(e as u32));
        let (bx0, bx1) = if inline { (x0 + ui.p.width(&ui.f.small, &lt) + 10.0, x1 - ui.p.width(&ui.f.small, &rt) - 10.0) } else { (x0, x1) };
        self.tl = (bx0, bx1);
        let fr = (p / e.max(1.0)).clamp(0.0, 1.0);
        // Al pasar el ratón la línea engorda y aparece su cabeza
        let hk = ui.heat(6);
        let th = 4.0 + 2.0 * hk;
        bar(ui, bx0, py - th / 2.0 + 2.0, bx1 - bx0, th, fr, &ui.white.clone());
        let hx = bx0 + (bx1 - bx0) * fr;
        unsafe { ui.rt.FillEllipse(&ellipse(hx, py + 2.0, 2.0 + 5.0 * hk), &ui.white); }
        // Bajo el ratón: marca y minuto exacto al que se saltaría pulsando
        let (mx, _) = desk::mouse_at();
        if hk > 0.01 && mx >= bx0 && mx <= bx1 {
            let f2 = (mx - bx0) / (bx1 - bx0);
            let lab = live::mmss((f2 * e) as u32);
            let lw2 = ui.p.width(&ui.f.small, &lab) + 14.0;
            let bx = (mx - lw2 / 2.0).clamp(x0, x1 - lw2);
            popup::push_alpha(ui.rt, hk);
            unsafe {
                ui.rt.FillRectangle(&rect(mx - 0.75, py - 5.0, mx + 0.75, py + 9.0), &ui.wa(0.8));
                ui.rt.FillRoundedRectangle(&rrect(rect(bx, py - 30.0, bx + lw2, py - 10.0), 10.0), &ui.chip());
            }
            ui.text_c(&ui.f.small, &lab, bx + lw2 / 2.0, py - 28.0, 16.0, &ui.white);
            ui.rt_pop();
        }
        self.seek_hover = hk > 0.01;
        ui.hit(rect(bx0, py - 8.0, bx1, py + 12.0), 6);
        if inline {
            ui.text(&ui.f.small, &lt, x0, py - 6.0, 16.0, &ui.dim);
            ui.text_r(&ui.f.small, &rt, x1, py - 6.0, 16.0, &ui.dim);
        } else {
            ui.text(&ui.f.small, &lt, x0, py + 10.0, 16.0, &ui.dim);
            ui.text_r(&ui.f.small, &rt, x1, py + 10.0, 16.0, &ui.dim);
        }
    }

    /// Título y artista desde `y` (con `big`, el título en grande; si no, en dos líneas si hace falta).
    fn song(&self, ui: &mut Ui, d: &DFonts, m: &MediaInfo, since: f32, x: f32, w: f32, y: f32, big: bool, accent: &ID2D1Brush) -> f32 {
        let kt = ez(since, 0.0, 0.45);
        let ka = ez(since, 0.06, 0.45);
        let app = player_name(&m.app);
        ui.text(&ui.f.small, &fit(ui, &ui.f.small, &i18n::tf("Reproduciendo en {}", &[&app]), w), x, y - 4.0, 16.0, accent);
        popup::push_alpha(ui.rt, kt);
        let mut yy = y + 12.0;
        if big {
            ui.text(&d.big, &fit(ui, &d.big, &m.title, w), x, yy + 10.0 * (1.0 - kt), 32.0, &ui.white);
            yy += 32.0;
        } else {
            let (a, b) = wrap2(ui, &d.title, &m.title, w);
            ui.text(&d.title, &a, x, yy + 10.0 * (1.0 - kt), 22.0, &ui.white);
            yy += 22.0;
            if !b.is_empty() { ui.text(&d.title, &b, x, yy + 10.0 * (1.0 - kt), 22.0, &ui.white); yy += 22.0; }
        }
        ui.rt_pop();
        popup::push_alpha(ui.rt, ka);
        ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &m.artist, w), x, yy + 10.0 * (1.0 - ka), 18.0, &ui.dim);
        ui.rt_pop();
        ui.hit(rect(x, y - 4.0, x + w, yy + 18.0), 4);
        yy + 18.0
    }

    /// Próximos temas de `top` a `bot` (en cascada al cambiar de canción); sin cola, de qué álbum es y
    /// cuánto queda.
    fn queue(&mut self, ui: &mut Ui, d: &DFonts, m: &MediaInfo, secs: Option<(f32, f32)>, since: f32, lx: f32, x1: f32, top: f32, bot: f32) {
        let lw = x1 - lx;
        let next = upcoming(&m.title);
        if next.is_empty() {
            // Sin cola a la vista: de qué álbum es y cuánto queda, en grande (no un hueco vacío)
            let ka = ez(since, 0.1, 0.4);
            popup::push_alpha(ui.rt, ka);
            let mut y = top + 2.0 + 8.0 * (1.0 - ka);
            if !m.album.trim().is_empty() && m.album.trim() != m.title.trim() && y + 40.0 <= bot {
                caption(ui, i18n::t("DEL ÁLBUM"), lx, y);
                ui.text(&ui.f.name, &fit(ui, &ui.f.name, &m.album, lw), lx, y + 18.0, 20.0, &ui.white);
                y += 52.0;
            }
            if let Some((p, e)) = secs.filter(|_| y + 40.0 <= bot) {
                caption(ui, i18n::t("QUEDAN"), lx, y);
                ui.text(&d.title, &live::mmss((e - p).max(0.0) as u32), lx, y + 18.0, 22.0, &ui.white);
                y += 52.0;
            }
            if y + 18.0 <= bot + 4.0 {
                let msg = if deskdata::ytm_open() { i18n::t("Sin más temas en la cola") } else { i18n::t("La cola sale con YouTube Music") };
                ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, msg, lw), lx, y, 18.0, &ui.faint);
            }
            ui.rt_pop();
            return;
        }
        caption(ui, i18n::t("A CONTINUACIÓN"), lx, top + 2.0);
        let rh = 38.0;
        let fit_n = (((bot - top - 22.0) / rh) as usize).max(1);
        for (i, q) in next.iter().take(fit_n).enumerate() {
            let kq = ez(since, 0.12 + i as f32 * 0.07, 0.4);
            let y = top + 24.0 + i as f32 * rh + 10.0 * (1.0 - kq);
            // Al pasar el ratón la fila se ilumina, su texto se desplaza un poco y aparece el ▶; al
            // pulsarla, ese tema empieza a sonar
            let id = 200 + i as u32;
            let hk = ui.heat(id);
            let row = rect(lx - 8.0, y - 3.0, x1 + 8.0, y + rh - 3.0);
            popup::push_alpha(ui.rt, kq * if i == 0 { 1.0 } else { 0.85 + 0.15 * hk });
            ui.card_k(row, hk);
            let dw = ui.p.width(&ui.f.sub, &q.dur);
            let tx = lx + 14.0 * hk;
            if hk > 0.01 {
                popup::push_alpha(ui.rt, hk);
                ui.glyph(&ui.f.icon, G_PLAY, lx - 4.0, y, 14.0, 18.0, &ui.white);
                ui.rt_pop();
            }
            let tw = lw - dw - 12.0 - 14.0 * hk;
            ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &q.title, tw), tx, y, 18.0, &ui.white);
            ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &q.artists, tw), tx, y + 17.0, 16.0, &ui.dim);
            ui.text_r(&ui.f.sub, &q.dur, x1, y, 18.0, &ui.dim);
            ui.hit(row, id);
            ui.rt_pop();
        }
    }

    /// Espectro de lo que suena en barras redondeadas de `x0` a `x1`, apoyadas en `bot`.
    fn spectrum(&mut self, ui: &Ui, x0: f32, x1: f32, top: f32, bot: f32, t: f32, dt: f32) {
        crate::audioviz::want();
        let mut lv = *crate::audioviz::LEVELS.lock().unwrap();
        // En la demo, sin nada sonando de verdad, una forma que respira (para las capturas)
        if sys::demo() && lv.iter().sum::<f32>() < 0.2 {
            for (i, v) in lv.iter_mut().enumerate() { let x = i as f32; *v = (0.55 - x * 0.011 + 0.22 * (x * 0.7 + t * 3.1).sin() * (x * 0.23 + t * 1.3).cos()).clamp(0.04, 1.0); }
        }
        if !self.playing { for v in lv.iter_mut() { *v = 0.0; } }
        let n = lv.len();
        self.spec.resize(n, 0.0);
        let gap = 3.0;
        let bw = ((x1 - x0) - gap * (n - 1) as f32) / n as f32;
        for (i, v) in lv.iter().enumerate() {
            let s = &mut self.spec[i];
            if *v > *s { *s = *v; } else { approach(s, *v, dt, 0.18); }
            let h = (*s * (bot - top)).max(3.0);
            let x = x0 + i as f32 * (bw + gap);
            unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(x, bot - h, x + bw, bot), bw.min(3.0) / 2.0), &ui.wa(0.35 + 0.55 * s.min(1.0))); }
        }
    }
}

impl Widget for MusicW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let l = desk::lay(Kind::Music);
        let (W, H) = (l.w, l.h);
        let media = MEDIA.lock().unwrap().clone();
        self.live_view = false;
        let Some(m) = media else {
            self.playing = false;
            let k = ez(t, 0.1, 0.5);
            popup::push_alpha(ui.rt, k);
            let cy = H / 2.0 - if W >= 300.0 { 0.0 } else { 6.0 };
            ui.glyph(&d.glyph_l, G_MUSIC, 0.0, cy - 60.0 + 8.0 * (1.0 - k), W, 40.0, &ui.dim);
            ui.text_c(&d.title, &fit(ui, &d.title, i18n::t("Nada sonando"), W - PAD * 2.0), W / 2.0, cy - 8.0, 24.0, &ui.white);
            let (a, b) = wrap2(ui, &ui.f.sub, i18n::t("Pon música en Spotify, YouTube Music o el navegador"), W - PAD * 2.0);
            ui.text_c(&ui.f.sub, &a, W / 2.0, cy + 18.0, 18.0, &ui.dim);
            if !b.is_empty() { ui.text_c(&ui.f.sub, &b, W / 2.0, cy + 34.0, 18.0, &ui.dim); }
            ui.rt_pop();
            return;
        };
        self.playing = m.playing;
        let k = format!("{}\u{1}{}", m.title, m.artist);
        if k != self.key {
            self.key = k.clone();
            self.cur = None;
            self.shift = 0.0;
            self.title_at = Some(Instant::now());
        }
        let since = self.title_at.map(|a| a.elapsed().as_secs_f32()).unwrap_or(9.0).min(t + 0.0);

        // Carátula nueva: se carga y se toma su color
        if self.art.as_ref().map(|a| a.0) != Some(m.art_ver) {
            let new = ART_BIG.lock().unwrap().as_ref().filter(|a| a.0 == m.art_ver).and_then(|(v, px)| bitmap(ui.rt, px, ART_BIG_PX).map(|b| (*v, b)));
            if new.is_some() || ART_BIG.lock().unwrap().is_none() {
                self.prev_art = self.art.take().map(|a| a.1);
                self.art = new;
                self.art_at = Some(Instant::now());
                self.tint_to = ART.lock().unwrap().as_ref().filter(|a| a.0 == m.art_ver).map(|(_, px)| art_tint(px)).unwrap_or([0.55, 0.55, 0.6]);
                if self.tint == [0.0; 3] { self.tint = self.tint_to; }
            }
        }
        for i in 0..3 { approach(&mut self.tint[i], self.tint_to[i], dt, 0.6); }
        let [r, g, b] = self.tint;
        if !desk::floating() {
            unsafe { ui.rt.FillRectangle(&rect(0.0, 0.0, W, H), &ui.p.vgrad(0.0, H, rgba(r * 0.55, g * 0.55, b * 0.55, 0.55), rgba(r * 0.2, g * 0.2, b * 0.2, 0.15))); }
        }
        let accent = ui.brush(rgba((r * 0.6 + 0.4).min(1.0), (g * 0.6 + 0.4).min(1.0), (b * 0.6 + 0.4).min(1.0), 1.0));
        let secs = media_secs(m.playing);

        match l.sz {
            // Carátula entera y el botón de reproducir encima
            Sz::Pequeno => {
                let s = W - PAD * 2.0;
                self.cover(ui, d, PAD, PAD, s);
                unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(PAD, PAD + s * 0.55, PAD + s, PAD + s), 14.0), &ui.p.vgrad(PAD + s * 0.55, PAD + s, rgba(0.0, 0.0, 0.0, 0.0), rgba(0.0, 0.0, 0.0, 0.35))); }
                self.play_btn(ui, d, dt, PAD + s - 24.0, PAD + s - 24.0, 18.0);
            }
            // Carátula, canción y controles
            Sz::Mediano => {
                let s = H - PAD * 2.0;
                self.cover(ui, d, PAD, PAD, s);
                let lx = PAD + s + 20.0;
                self.song(ui, d, &m, since, lx, W - PAD - lx, PAD, false, &accent);
                self.controls(ui, d, dt, lx + (W - PAD - lx) / 2.0, H - PAD - 20.0, false);
            }
            // Carátula, canción con controles y progreso, y la cola a la derecha
            Sz::Banda => {
                let s = H - PAD * 2.0;
                self.cover(ui, d, PAD, PAD, s);
                let (lx, x1) = (PAD + s + 22.0, 440.0);
                self.song(ui, d, &m, since, lx, x1 - lx, PAD, true, &accent);
                self.controls(ui, d, dt, lx + (x1 - lx) / 2.0, 104.0, false);
                self.timeline(ui, secs, lx, x1, H - PAD - 12.0, true);
                vline(ui, 466.0, PAD, H - PAD);
                self.queue(ui, d, &m, secs, since, 490.0, W - PAD, PAD - 6.0, H - PAD + 4.0);
            }
            // Carátula arriba, canción, progreso y controles
            Sz::Alto => {
                let s = W - PAD * 2.0;
                self.cover(ui, d, PAD, PAD, s);
                self.song(ui, d, &m, since, PAD, s, PAD + s + 14.0, false, &accent);
                self.timeline(ui, secs, PAD, W - PAD, 262.0, false);
                self.controls(ui, d, dt, W / 2.0, H - PAD - 22.0, false);
            }
            // Carátula y canción, progreso, controles y el tema siguiente
            Sz::Grande | Sz::Columna => {
                let s = 150.0;
                self.cover(ui, d, PAD, PAD, s);
                let lx = PAD + s + 20.0;
                let y = self.song(ui, d, &m, since, lx, W - PAD - lx, PAD + 2.0, false, &accent);
                if !m.album.trim().is_empty() && m.album.trim() != m.title.trim() && y + 20.0 <= PAD + s {
                    ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &m.album, W - PAD - lx), lx, y + 2.0, 18.0, &ui.faint);
                }
                self.timeline(ui, secs, PAD, W - PAD, PAD + s + 24.0, false);
                self.controls(ui, d, dt, W / 2.0, PAD + s + 76.0, true);
                hline(ui, PAD - 4.0, W - PAD + 4.0, PAD + s + 106.0);
                self.queue(ui, d, &m, secs, since, PAD, W - PAD, PAD + s + 114.0, H - PAD + 4.0);
            }
            // Como el widget de siempre: carátula grande, canción y cola, el espectro, y debajo el progreso y los controles
            Sz::Extragrande => {
                let asz = H - PAD - 92.0;
                self.cover(ui, d, PAD, PAD, asz);
                let lx = PAD + asz + 24.0;
                self.song(ui, d, &m, since, lx, W - PAD - lx, PAD, true, &accent);
                let (top, bot) = (PAD + 74.0, PAD + asz);
                self.queue(ui, d, &m, secs, since, lx, W - PAD, top, bot - 58.0);
                self.spectrum(ui, lx, W - PAD, bot - 44.0, bot, t, dt);
                self.live_view = true;
                self.timeline(ui, secs, PAD, W - PAD, PAD + asz + 18.0, false);
                self.controls(ui, d, dt, W / 2.0, H - PAD - 18.0, true);
            }
        }
    }
    fn click(&mut self, id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        match id? {
            1 => Some(Box::new(|| media(Cmd::Prev))),
            // Línea de tiempo: salta al punto pulsado (y la barra se mueve ya, sin esperar al reproductor)
            6 => {
                let (x0, x1) = self.tl;
                let f = ((desk::click_at().0 - x0) / (x1 - x0).max(1.0)).clamp(0.0, 1.0);
                let mut tl = TIMELINE.lock().unwrap();
                let (_, end, _) = (*tl)?;
                let t = (end as f64 * f as f64) as i64;
                let now = unsafe { let ft = windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime(); ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64 };
                *tl = Some((t, end, now));
                Some(Box::new(move || media(Cmd::Seek(t))))
            }
            2 => { self.pulse_at = Some(Instant::now()); Some(Box::new(|| media(Cmd::Toggle))) }
            3 => Some(Box::new(|| media(Cmd::Next))),
            4 => { let app = MEDIA.lock().unwrap().as_ref().map(|m| m.app.clone()).unwrap_or_default(); Some(Box::new(move || open_player(&app))) }
            // Tema de la cola: que suene ese (lo pulsa el hilo de la cola en YouTube Music)
            i @ 200..=230 => {
                let title = MEDIA.lock().unwrap().as_ref().map(|m| m.title.clone()).unwrap_or_default();
                let q = upcoming(&title).get((i - 200) as usize).map(|q| q.title.clone())?;
                *PLAY_REQ.lock().unwrap() = Some(q);
                self.title_at = Some(Instant::now());
                None
            }
            _ => None,
        }
    }
    fn animating(&self, t: f32) -> bool {
        // Sonando, solo hace falta animar sin parar el espectro; la cola es quieta
        t < 1.3 || self.shift.abs() > 0.5 || (self.playing && self.live_view) || self.tint != self.tint_to
            || self.art_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.6)
            || self.title_at.is_some_and(|a| a.elapsed().as_secs_f32() < 1.2)
            || self.pulse_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.4)
            || (self.play_k > 0.005 && self.play_k < 0.995)
            || self.spec.iter().any(|s| *s > 0.01)
    }
    fn follows_mouse(&self) -> bool { self.seek_hover }
    // Cambio de canción o de pausa a reproducir: a 60 fotogramas mientras dura
    fn smooth(&self) -> bool {
        self.art_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.6)
            || self.title_at.is_some_and(|a| a.elapsed().as_secs_f32() < 1.0)
            || self.pulse_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.4)
            || self.shift.abs() > 1.0
            || (self.play_k > 0.005 && self.play_k < 0.995)
    }
    fn layouts(&self) -> bool { true }
}

/// Temas que vienen detrás del que suena (si se encuentra en la cola; si no, la cola tal cual).
fn upcoming(title: &str) -> Vec<QItem> {
    let q = QUEUE.lock().unwrap().clone();
    // Windows y YouTube Music no siempre escriben igual el título ("Otro levvel" frente a
    // "Otro levvel (con alejo)"): se comparan sin paréntesis, signos ni mayúsculas
    let norm = |s: &str| -> String {
        let mut out = String::new();
        let mut depth = 0;
        for c in s.chars() {
            match c { '(' | '[' => depth += 1, ')' | ']' => depth = (depth - 1).max(0), _ if depth == 0 && c.is_alphanumeric() => out.extend(c.to_lowercase()), _ => {} }
        }
        out
    };
    let t = norm(title);
    if t.is_empty() { return Vec::new(); }
    let hit = q.iter().position(|x| norm(&x.title) == t)
        .or_else(|| q.iter().position(|x| { let n = norm(&x.title); n.starts_with(&t) || t.starts_with(&n) && n.len() >= 4 }));
    match hit { Some(i) => q[i + 1..].to_vec(), None => Vec::new() }
}

/// "Spotify.exe" → "Spotify"; navegadores con YouTube Music → "YouTube Music".
fn player_name(app: &str) -> String {
    let a = app.to_lowercase();
    if a.contains("spotify") { return "Spotify".into(); }
    // La app de YouTube Music (instalada desde el navegador) se anuncia como el navegador
    if !QUEUE.lock().unwrap().is_empty() || deskdata::ytm_open() { return "YouTube Music".into(); }
    if ["brave", "chrome", "msedge", "firefox", "opera", "vivaldi"].iter().any(|b| a.contains(b)) { return i18n::t("Navegador").into(); }
    if a.contains("youtube") { return "YouTube Music".into(); }
    if a.contains("zune") || a.contains("media") { return i18n::t("Reproductor multimedia").into(); }
    let s = app.split(['!', '_']).next().unwrap_or(app).rsplit('.').next().unwrap_or(app).trim_end_matches(".exe");
    s.to_string()
}

// ---------------------------------------------------------------- sistema
/// `disp`: cifras que se ven (CPU, RAM, bajada, subida), que corren con suavidad hacia el valor real.
/// `apps`: fila animada de cada app (y, barra, opacidad, % actual).
/// `graph`: imagen con las curvas (lienzo con el que se creó, muestra que dibuja, la imagen).
/// `live`: medidas y último valor para pintar la gráfica encima de la capa fija; `layer_at`: muestra
/// con la que se pintó la capa. `rv`: lo que marcan los anillos y barras (CPU, memoria, disco,
/// batería, de 0 a 1), que corre hacia el valor real; `nums`: dónde van las cifras que cuentan en
/// `paint_live` (x, y, cuál de `disp`, grande).
#[derive(Default)]
struct SystemW {
    disp: [f32; 4], apps: HashMap<String, (f32, f32, f32, f32)>, moving: bool,
    graph: Option<(Option<windows::core::IUnknown>, Option<Instant>, ID2D1BitmapRenderTarget)>,
    live: Option<(f32, f32, f32, f32, f32, f32, f32, f32, Option<Instant>)>,
    layer_at: Option<Instant>,
    /// Sonda de la gráfica: opacidad, posición y si sigue al ratón
    probe_a: f32, probe_x: f32, probe_on: bool,
    rv: [f32; 4],
    nums: Vec<(f32, f32, usize, bool)>,
}

fn uptime_text(s: u64) -> String {
    let (d, h, m) = (s / 86400, s / 3600 % 24, s / 60 % 60);
    if d > 0 { i18n::tf("Encendido {} d {} h", &[&d, &h]) } else if h > 0 { i18n::tf("Encendido {} h {} min", &[&h, &m]) } else { i18n::tf("Encendido {} min", &[&m]) }
}

/// Batería del equipo: (porcentaje, enchufado). None si no tiene (en la demo, una inventada).
fn pc_battery() -> Option<(u8, bool)> { if sys::demo() { Some((72, true)) } else { crate::battery() } }

/// Disco principal (el primero): fracción usada, letra y espacio libre.
fn main_disk() -> Option<(f32, String, u64)> {
    let st = STORE.lock().unwrap();
    let dr = st.as_ref()?.drives.first()?.clone();
    Some(((dr.total - dr.free) as f32 / dr.total.max(1) as f32, dr.root.trim_end_matches('\\').to_string(), dr.free))
}

impl SystemW {
    /// Valores reales de los medidores (CPU, memoria, disco, batería; -1 si no hay batería).
    fn real(s: &SysData) -> [f32; 4] {
        [s.cpu.last().copied().unwrap_or(0.0) / 100.0, s.ram.last().copied().unwrap_or(0.0) / 100.0,
            main_disk().map(|d| d.0).unwrap_or(0.0), pc_battery().map(|b| b.0 as f32 / 100.0).unwrap_or(-1.0)]
    }

    /// Anillos en fila de `x0` a `x1` (CPU, memoria, disco y, si hay sitio y batería, batería).
    fn rings(&self, ui: &Ui, d: &DFonts, x0: f32, x1: f32, cy: f32, r: f32, n: usize) {
        let labels = [i18n::t("Procesador"), i18n::t("Memoria"), i18n::t("Disco"), i18n::t("Batería")];
        let n = if self.rv[3] < 0.0 { n.min(3) } else { n };
        let cw = (x1 - x0) / n as f32;
        for i in 0..n {
            let v = self.rv[i].max(0.0);
            let lab = if i == 2 { main_disk().map(|m| i18n::tf("Disco {}", &[&m.1])).unwrap_or_else(|| labels[2].into()) } else { labels[i].to_string() };
            ring_stat(ui, d, x0 + cw * (i as f32 + 0.5), cy, r, v, &format!("{:.0}%", v * 100.0), &lab);
        }
    }

    /// Red: bajada y subida en grande a la izquierda y las barras del último medio minuto a la derecha.
    fn net(&self, ui: &Ui, d: &DFonts, s: &SysData, x0: f32, x1: f32, y: f32, y1: f32, vals_w: f32) {
        ui.glyph(&ui.f.icon, G_DOWN, x0 - 2.0, y, 16.0, 16.0, &ui.faint);
        caption(ui, i18n::t("RED"), x0 + 16.0, y);
        let (down, up) = (s.down.last().copied().unwrap_or(0.0), s.up.last().copied().unwrap_or(0.0));
        ui.glyph(&ui.f.icon, G_DOWN, x0 - 2.0, y + 22.0, 16.0, 22.0, &ui.dim);
        ui.text(&d.title, &fmt_rate(down), x0 + 18.0, y + 22.0, 22.0, &ui.white);
        ui.glyph(&ui.f.icon, G_UP, x0 - 2.0, y + 46.0, 16.0, 22.0, &ui.dim);
        ui.text(&d.title, &fmt_rate(up), x0 + 18.0, y + 46.0, 22.0, &ui.dim);
        self.net_bars(ui, s, x0 + vals_w, x1, y + 4.0, y1);
    }

    fn net_bars(&self, ui: &Ui, s: &SysData, x0: f32, x1: f32, bt: f32, bb: f32) {
        // Una barra cada 7 DIP como poco (en los huecos estrechos, menos segundos)
        let nb = (((x1 - x0) / 7.0) as usize).clamp(8, 30);
        let vals: Vec<f32> = s.down.iter().rev().take(nb).rev().cloned().collect();
        let mx = vals.iter().cloned().fold(50_000.0f32, f32::max) * 1.15;
        let bw = (x1 - x0) / nb as f32;
        for (i, v) in vals.iter().enumerate() {
            let x = x0 + (i + nb - vals.len()) as f32 * bw;
            let h = (v / mx * (bb - bt)).max(2.0);
            let last = i == vals.len() - 1;
            unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(x + 1.0, bb - h, x + bw - 1.5, bb), 1.5), &ui.wa(if last { 0.95 } else { 0.42 })); }
        }
    }

    /// Las apps que más gastan (`n` filas desde `by`). Cada app conserva su fila animada: si cambia el
    /// orden, se deslizan a su nuevo puesto; las que entran aparecen y las que salen se desvanecen.
    fn top_apps(&mut self, ui: &Ui, s: &SysData, ax: f32, x1: f32, by: f32, n: usize, dt: f32) {
        caption(ui, i18n::t("LO QUE MÁS GASTA"), ax, by - 2.0);
        let rank: Vec<(String, f32)> = s.top.iter().take(n).cloned().collect();
        for (i, (name, pct)) in rank.iter().enumerate() {
            let ty = by + 20.0 + i as f32 * 22.0;
            let e = self.apps.entry(name.clone()).or_insert((ty + 10.0, 0.0, 0.0, 0.0));
            e.3 = *pct;
        }
        for (name, e) in self.apps.iter_mut() {
            let slot = rank.iter().position(|(n, _)| n == name);
            let (ty, ta, tb) = match slot { Some(i) => (by + 20.0 + i as f32 * 22.0, 1.0, (e.3 / 25.0).min(1.0)), None => (e.0 + 6.0, 0.0, 0.0) };
            self.moving |= approach(&mut e.0, ty, dt, 0.16);
            self.moving |= approach(&mut e.2, ta, dt, 0.14);
            self.moving |= approach(&mut e.1, tb, dt, 0.3);
        }
        self.apps.retain(|n, e| e.2 > 0.01 || rank.iter().any(|(r, _)| r == n));
        let names: Vec<String> = self.apps.keys().cloned().collect();
        let w = x1 - ax;
        let (nw, bw) = if w >= 260.0 { (112.0, w - 150.0) } else { (w - 44.0, 0.0) };
        for name in names {
            let (y, b, a, pct) = self.apps[&name];
            popup::push_alpha(ui.rt, a);
            ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &name, nw - 8.0), ax, y, 18.0, &ui.white);
            if bw > 0.0 { bar(ui, ax + nw, y + 7.0, bw, 4.0, b, &ui.wa(0.8)); }
            ui.text_r(&ui.f.lbl, &format!("{pct:.0}%"), x1, y, 18.0, &ui.dim);
            ui.rt_pop();
        }
    }

    /// Gráfica del último minuto de `x0` a `x1` (de `gt` a `gb`): las curvas se dibujan en una imagen
    /// solo cuando llega una muestra nueva (una vez por segundo); entre una y otra, `paint_live` la
    /// desliza entera, que a la GPU no le cuesta nada. Debajo, su leyenda.
    fn graph(&mut self, ui: &Ui, s: &SysData, t: f32, x0: f32, x1: f32, gt: f32, gb: f32) {
        for i in 0..4 {
            let yy = gt + (gb - gt) * i as f32 / 3.0;
            unsafe { ui.rt.FillRectangle(&rect(x0, yy, x1, yy + 1.0 / ui.s), &ui.wa(0.06)); }
        }
        let gw = x1 - x0;
        let step = gw / (HIST - 2) as f32;
        let k = ez(t, 0.1, 0.8);
        const M: f32 = 10.0;
        let (bw, bh) = (HIST as f32 * step + M * 2.0, gb - gt + M * 2.0);
        let rtk = { use windows::core::Interface; ui.rt.cast::<windows::core::IUnknown>().ok() };
        let stale = self.graph.as_ref().is_none_or(|g| g.0 != rtk || g.1 != s.at) || k < 1.0;
        if stale {
            let fresh = unsafe { ui.rt.CreateCompatibleRenderTarget(Some(&D2D_SIZE_F { width: bw, height: bh }), None, None, D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE).ok() };
            if let Some(bt) = fresh {
                let local = |v: &[f32]| -> Vec<(f32, f32)> {
                    let off = HIST - v.len();
                    v.iter().enumerate().map(|(i, x)| ((i + off) as f32 * step + M, gb - (x / 100.0).clamp(0.0, 1.0) * (gb - gt) * k - (gt - M))).collect()
                };
                unsafe {
                    let rtb: &ID2D1RenderTarget = &bt;
                    rtb.BeginDraw();
                    rtb.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
                    let base_y = gb - (gt - M);
                    let stroke = round_stroke(rtb);
                    let w45: Option<ID2D1Brush> = rtb.CreateSolidColorBrush(&ui.ic(0.45), None).ok().map(|b| b.into());
                    let w95: Option<ID2D1Brush> = rtb.CreateSolidColorBrush(&ui.ic(0.95), None).ok().map(|b| b.into());
                    let fill: Option<ID2D1Brush> = rtb.CreateGradientStopCollection(&[D2D1_GRADIENT_STOP { position: 0.0, color: ui.ic(0.20) }, D2D1_GRADIENT_STOP { position: 1.0, color: ui.ic(0.0) }], D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok()
                        .and_then(|c| rtb.CreateLinearGradientBrush(&D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: 0.0, Y: M }, endPoint: Vector2 { X: 0.0, Y: base_y } }, None, &c).ok()).map(|b| b.into());
                    if let (Some(l), Some(b)) = (smooth_path(rtb, &local(&s.ram), None), &w45) { rtb.DrawGeometry(&l, b, 2.0, stroke.as_ref()); }
                    let cp = local(&s.cpu);
                    if let (Some(a), Some(b)) = (smooth_path(rtb, &cp, Some(base_y)), &fill) { rtb.FillGeometry(&a, b, None); }
                    if let (Some(l), Some(b)) = (smooth_path(rtb, &cp, None), &w95) { rtb.DrawGeometry(&l, b, 2.5, stroke.as_ref()); }
                    let _ = rtb.EndDraw(None, None);
                }
                self.graph = Some((rtk, s.at, bt));
            }
        }
        // La imagen de las curvas y el punto vivo se pintan en `paint_live` (cada fotograma); aquí solo
        // se apuntan las medidas y el último valor
        self.live = Some((x0, x1, gt, gb, step, bw, bh, s.cpu.last().copied().unwrap_or(0.0) * k, s.at));
        // Leyenda bajo la gráfica
        unsafe {
            ui.rt.FillRoundedRectangle(&rrect(rect(x0, gb + 13.0, x0 + 12.0, gb + 16.0), 1.5), &ui.white);
            ui.rt.FillRoundedRectangle(&rrect(rect(x0 + 66.0, gb + 13.0, x0 + 78.0, gb + 16.0), 1.5), &ui.wa(0.45));
        }
        ui.text(&ui.f.small, "CPU", x0 + 18.0, gb + 6.0, 16.0, &ui.dim);
        ui.text(&ui.f.small, i18n::t("Memoria"), x0 + 84.0, gb + 6.0, 16.0, &ui.dim);
        ui.text_r(&ui.f.small, i18n::t("Último minuto"), x1, gb + 6.0, 16.0, &ui.faint);
    }
}

impl Widget for SystemW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let l = desk::lay(Kind::System);
        let (W, H) = (l.w, l.h);
        let s = SYS.lock().unwrap().clone().unwrap_or_default();
        self.moving = false;
        self.live = None;
        self.nums.clear();
        self.layer_at = s.at;
        // Los medidores corren hacia el valor real (al aparecer, desde cero)
        let real = Self::real(&s);
        for i in 0..4 {
            if real[i] < 0.0 { self.rv[i] = -1.0; continue; }
            if self.rv[i] < 0.0 { self.rv[i] = 0.0; }
            self.moving |= approach(&mut self.rv[i], real[i] * ez(t, 0.1, 0.6), dt, 0.3);
        }
        let up = if s.uptime > 0 { uptime_text(s.uptime) } else { String::new() };
        let bat = pc_battery();
        match l.sz {
            // El anillo de la CPU
            Sz::Pequeno => {
                let v = self.rv[0];
                ring_stat(ui, d, W / 2.0, PAD + 52.0, 46.0, v, &format!("{:.0}%", v * 100.0), "");
                ui.text_c(&ui.f.small, i18n::t("Procesador"), W / 2.0, H - PAD - 14.0, 16.0, &ui.dim);
            }
            // CPU, memoria y disco
            Sz::Mediano => self.rings(ui, d, PAD - 6.0, W - PAD + 6.0, PAD + 50.0, 38.0, 3),
            // Los anillos y la red
            Sz::Banda => {
                self.rings(ui, d, PAD - 6.0, 400.0, PAD + 50.0, 38.0, 4);
                vline(ui, 420.0, PAD, H - PAD);
                self.net(ui, d, &s, 444.0, W - PAD, PAD - 2.0, H - PAD - 22.0, 150.0);
                if !up.is_empty() { ui.text(&ui.f.small, &up, 444.0, H - PAD - 16.0, 16.0, &ui.faint); }
            }
            // Tres barras verticales: CPU, memoria y disco
            Sz::Alto => {
                header(ui, d, i18n::t("Sistema"), "", W);
                if !up.is_empty() { ui.text(&ui.f.small, &fit(ui, &ui.f.small, &up, W - PAD * 2.0), PAD, PAD + 22.0, 16.0, &ui.faint); }
                let labels = ["CPU", "RAM", i18n::t("Disco")];
                let cw = (W - PAD * 2.0) / 3.0;
                let (bt, bb) = (104.0, H - PAD - 44.0);
                for i in 0..3 {
                    let v = self.rv[i].max(0.0);
                    let cx = PAD + cw * (i as f32 + 0.5);
                    ui.text_c(&ui.f.lbl, &format!("{:.0}%", v * 100.0), cx, bt - 26.0, 18.0, &ui.white);
                    let bw = 22.0;
                    unsafe {
                        ui.rt.FillRoundedRectangle(&rrect(rect(cx - bw / 2.0, bt, cx + bw / 2.0, bb), 8.0), &ui.track);
                        let h = ((bb - bt) * v).max(if v > 0.0 { bw } else { 0.0 });
                        if h > 0.0 { ui.rt.FillRoundedRectangle(&rrect(rect(cx - bw / 2.0, bb - h, cx + bw / 2.0, bb), 8.0), &fill_brush(ui, v)); }
                    }
                    ui.text_c(&ui.f.small, labels[i], cx, bb + 8.0, 16.0, &ui.dim);
                }
                if let Some((p, plug)) = bat {
                    let s2 = if plug { i18n::tf("Batería {} % · enchufado", &[&p]) } else { i18n::tf("Batería {} %", &[&p]) };
                    ui.text_c(&ui.f.small, &fit(ui, &ui.f.small, &s2, W - PAD * 2.0), W / 2.0, H - PAD - 12.0, 16.0, &ui.faint);
                }
            }
            // Anillos (con batería), red y lo que más gasta
            Sz::Grande => {
                header(ui, d, i18n::t("Sistema"), &up, W);
                self.rings(ui, d, PAD - 6.0, W - PAD + 6.0, PAD + 76.0, 30.0, 4);
                hline(ui, PAD - 4.0, W - PAD + 4.0, 160.0);
                self.net(ui, d, &s, PAD, W - PAD, 168.0, 236.0, 150.0);
                hline(ui, PAD - 4.0, W - PAD + 4.0, 248.0);
                self.top_apps(ui, &s, PAD, W - PAD, 258.0, 3, dt);
            }
            // Lo del Grande con la gráfica del último minuto
            Sz::Columna => {
                header(ui, d, i18n::t("Sistema"), &up, W);
                self.rings(ui, d, PAD - 6.0, W - PAD + 6.0, PAD + 76.0, 30.0, 4);
                hline(ui, PAD - 4.0, W - PAD + 4.0, 160.0);
                self.graph(ui, &s, t, PAD, W - PAD, 184.0, 290.0);
                hline(ui, PAD - 4.0, W - PAD + 4.0, 326.0);
                self.net(ui, d, &s, PAD, W - PAD, 336.0, 404.0, 150.0);
                hline(ui, PAD - 4.0, W - PAD + 4.0, 416.0);
                self.top_apps(ui, &s, PAD, W - PAD, 426.0, 3, dt);
            }
            // Todo: cifras grandes, gráfica del último minuto, red y lo que más gasta
            Sz::Extragrande => {
                let right = match bat { Some((p, _)) if !up.is_empty() => format!("{up}  ·  {}", i18n::tf("Batería {} %", &[&p])), _ => up.clone() };
                header(ui, d, i18n::t("Sistema"), &right, W);
                // Cifras: los rótulos van en la capa fija y las cifras, que cuentan hacia el valor nuevo,
                // se pintan encima en cada fotograma (`paint_live`); el disco cambia poco y va aquí
                let y = PAD + 30.0;
                let iw = W - PAD * 2.0;
                let xs = [0.0, 0.15, 0.62, 0.81];
                let labels = [i18n::t("Procesador").to_string(), i18n::tf("Memoria · {} de {}", &[&fmt_bytes(s.ram_used), &fmt_bytes(s.ram_total)]), i18n::t("Bajada").into(), i18n::t("Subida").into()];
                for (i, lb) in labels.iter().enumerate() {
                    ui.text(&ui.f.small, &fit(ui, &ui.f.small, lb, iw * 0.25), PAD + iw * xs[i], y + 30.0, 16.0, &ui.dim);
                    self.nums.push((PAD + iw * xs[i], y, i, i < 2));
                }
                if let Some((frac, letter, free)) = main_disk() {
                    let dx = PAD + iw * 0.42;
                    ui.text(&d.big, &format!("{:.0}%", frac * 100.0), dx, y, 30.0, &ui.white);
                    ui.text(&ui.f.small, &fit(ui, &ui.f.small, &i18n::tf("Disco {} · {} libres", &[&letter, &fmt_bytes(free)]), iw * 0.19), dx, y + 30.0, 16.0, &ui.dim);
                }
                let (gt, gb) = (PAD + 96.0, PAD + 212.0);
                self.graph(ui, &s, t, PAD, W - PAD, gt, gb);
                let by = gb + 34.0;
                let half = (W - PAD * 2.0 - 40.0) / 2.0;
                ui.glyph(&ui.f.icon, G_DOWN, PAD - 2.0, by - 2.0, 16.0, 18.0, &ui.dim);
                ui.text(&ui.f.small, i18n::t("RED"), PAD + 16.0, by - 2.0, 18.0, &ui.faint);
                let rate = format!("↓ {}   ↑ {}", fmt_rate(s.down.last().copied().unwrap_or(0.0)), fmt_rate(s.up.last().copied().unwrap_or(0.0)));
                ui.text_r(&ui.f.small, &rate, PAD + half, by - 2.0, 18.0, &ui.dim);
                self.net_bars(ui, &s, PAD, PAD + half, by + 22.0, H - PAD - 4.0);
                self.top_apps(ui, &s, PAD + half + 40.0, W - PAD, by, 3, dt);
            }
        }
        ui.hit(rect(0.0, 0.0, W, H), 1);
    }
    fn click(&mut self, id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        // Abre el Administrador de tareas
        if id == Some(1) { return Some(Box::new(|| sys::open("taskmgr.exe"))); }
        None
    }
    // Con gráfica o cifras que cuentan, sin parar; si no, solo mientras los medidores se mueven (las
    // muestras llegan con el tic de cada segundo)
    fn animating(&self, t: f32) -> bool { t < 1.5 || self.moving || self.live.is_some() || !self.nums.is_empty() }
    // Con las curvas ya en una imagen, deslizarla cuesta poco: va a 60 fotogramas
    fn frame_ms(&self) -> u32 { if self.live.is_some() { 15 } else { 33 } }
    fn layered(&self) -> bool { true }
    // La capa fija (anillos, rótulos, red, apps) se rehace al llegar cada muestra o mientras algo se anima
    fn layer_stale(&self) -> bool { self.moving || SYS.lock().unwrap().as_ref().map(|s| s.at) != Some(self.layer_at) }
    fn paint_live(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        // Cifras grandes: cuentan hacia el valor nuevo en vez de saltar (al aparecer, desde cero)
        let real = SYS.lock().unwrap().as_ref().map(|s| [s.cpu.last().copied().unwrap_or(0.0), s.ram.last().copied().unwrap_or(0.0), s.down.last().copied().unwrap_or(0.0), s.up.last().copied().unwrap_or(0.0)]).unwrap_or([0.0; 4]);
        for i in 0..4 { approach(&mut self.disp[i], real[i], dt, if t < 1.0 { 0.25 } else { 0.35 }); }
        let [cpu, ram, down, up] = self.disp;
        let vals = [format!("{cpu:.0}%"), format!("{ram:.0}%"), fmt_rate(down), fmt_rate(up)];
        for (x, y, i, big) in self.nums.iter() {
            ui.text(if *big { &d.big } else { &d.title }, &vals[*i], *x, y + if *big { 0.0 } else { 4.0 }, 30.0, &ui.white);
        }
        let Some((x0, x1, gt, gb, step, bw, bh, last, at)) = self.live else { return };
        const M: f32 = 10.0;
        let frac = at.map(|a| a.elapsed().as_secs_f32().min(1.0)).unwrap_or(0.0);
        unsafe {
            ui.rt.PushAxisAlignedClip(&rect(x0, gt - 6.0, x1, gb + 2.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            if let Some((_, _, bt)) = &self.graph {
                if let Ok(bmp) = bt.GetBitmap() {
                    let bx = x0 - frac * step - step - M;
                    ui.rt.DrawBitmap(&bmp, Some(&rect(bx, gt - M, bx + bw, gt - M + bh)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                }
            }
            // Punto vivo en la cabeza de la CPU: con cada muestra nueva suelta una onda que se abre y se apaga
            let x = x0 + (HIST - 1) as f32 * step - frac * step - step;
            let y = gb - (last / 100.0).clamp(0.0, 1.0) * (gb - gt);
            ui.rt.FillEllipse(&ellipse(x, y, 3.5 + 9.0 * popup::ease_out(frac)), &ui.wa(0.30 * (1.0 - frac)));
            ui.rt.FillEllipse(&ellipse(x, y, 3.5), &ui.white);
            ui.rt.PopAxisAlignedClip();
        }
        // Sonda: con el ratón sobre la gráfica, una línea sigue al cursor con la CPU y la memoria de ese segundo
        let (mx, my) = desk::mouse_at();
        let inside = mx >= x0 && mx <= x1 && my >= gt - 6.0 && my <= gb + 2.0;
        approach(&mut self.probe_a, if inside { 1.0 } else { 0.0 }, dt, 0.08);
        self.probe_on = inside || self.probe_a > 0.01;
        if self.probe_a > 0.01 {
            let data = SYS.lock().unwrap().clone().unwrap_or_default();
            let n = data.cpu.len();
            if n >= 2 {
                if inside { self.probe_x = mx; }
                let off = HIST - n;
                let x_of = |i: usize| x0 + (i + off) as f32 * step - frac * step - step;
                let i = (0..n).min_by(|a, b| (x_of(*a) - self.probe_x).abs().partial_cmp(&(x_of(*b) - self.probe_x).abs()).unwrap()).unwrap_or(n - 1);
                let px = x_of(i);
                let cy = gb - (data.cpu[i] / 100.0).clamp(0.0, 1.0) * (gb - gt);
                let ry = gb - (data.ram.get(i).copied().unwrap_or(0.0) / 100.0).clamp(0.0, 1.0) * (gb - gt);
                let a = self.probe_a;
                unsafe {
                    ui.rt.FillRectangle(&rect(px - 0.5, gt - 4.0, px + 0.5, gb), &ui.wa(0.35 * a));
                    ui.rt.FillEllipse(&ellipse(px, ry, 3.5), &ui.wa(0.6 * a));
                    ui.rt.FillEllipse(&ellipse(px, cy, 4.5), &ui.wa(a));
                }
                let ago_s = n - 1 - i;
                let when = if ago_s == 0 { i18n::t("ahora").to_string() } else { i18n::tf("hace {} s", &[&ago_s]) };
                let lab = i18n::tf("CPU {}% · Memoria {}% · {}", &[&format!("{:.0}", data.cpu[i]), &format!("{:.0}", data.ram.get(i).copied().unwrap_or(0.0)), &when]);
                let lw = ui.p.width(&ui.f.small, &lab) + 16.0;
                let lx = (px - lw / 2.0).clamp(x0 - 4.0, x1 - lw + 4.0);
                popup::push_alpha(ui.rt, a);
                unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(lx, gt - 30.0, lx + lw, gt - 10.0), 10.0), &ui.chip()); }
                ui.text_c(&ui.f.small, &lab, lx + lw / 2.0, gt - 28.0, 16.0, &ui.white);
                ui.rt_pop();
            }
        }
    }
    fn follows_mouse(&self) -> bool { self.probe_on }
    fn layouts(&self) -> bool { true }
}

// ---------------------------------------------------------------- almacenamiento y papelera
/// `fills`: relleno que se ve de cada disco (va hacia el real); `shake`: sacudida de la papelera
/// cuando cambia lo que tiene.
#[derive(Default)]
struct StorageW { fills: Vec<f32>, r_bin: Roll, r_free: Vec<Roll>, last_items: Option<u64>, shake: Option<Instant>, moving: bool }

/// «Windows (C:)» / «Disco (D:)».
fn drive_name(dr: &Drive) -> String {
    let letter = dr.root.trim_end_matches('\\');
    if dr.label.is_empty() { i18n::tf("Disco ({})", &[&letter]) } else { format!("{} ({letter})", dr.label) }
}

impl StorageW {
    /// Disco en fila (el de siempre): icono, nombre, libre a la derecha y la barra; con `used`, una
    /// línea más con lo usado.
    fn drive_row(&mut self, ui: &mut Ui, i: usize, dr: &Drive, x0: f32, x1: f32, y: f32, used: bool) {
        let frac = (dr.total - dr.free) as f32 / dr.total.max(1) as f32;
        let id = 40 + i as u32;
        let hk = ui.heat(id);
        let h = if used { 62.0 } else { 44.0 };
        ui.card_k(rect(x0 - 8.0, y - 4.0, x1 + 8.0, y + h), hk);
        ui.glyph(&ui.f.icon_l, G_DRIVE, x0 + 3.0 * hk, y, 20.0, 20.0, &ui.wa(0.72 + 0.25 * hk));
        let dim = ui.dim.clone();
        let fsub = ui.f.sub.clone();
        let free = i18n::tf("{} libres de {}", &[&fmt_bytes(dr.free), &fmt_bytes(dr.total)]);
        let fw = ui.p.width(&fsub, &free);
        ui.text(&ui.f.name, &fit(ui, &ui.f.name, &drive_name(dr), x1 - x0 - 36.0 - fw - 10.0), x0 + 28.0 + 4.0 * hk, y, 20.0, &ui.white);
        self.r_free[i].draw(ui, &fsub, &free, x1, y, 20.0, &dim, true);
        let fill = fill_brush(ui, frac);
        let th = 8.0 + 3.0 * hk;
        bar(ui, x0, y + 32.0 - th / 2.0, x1 - x0, th, self.fills[i], &fill);
        if used {
            ui.text(&ui.f.sub, &i18n::tf("{} usados · {} %", &[&fmt_bytes(dr.total - dr.free), &format!("{:.0}", frac * 100.0)]), x0, y + 42.0, 16.0, &ui.faint);
        }
        if hk > 0.01 && !used {
            // Al pasar el ratón: cuánto está lleno, al final del relleno
            let lab = i18n::tf("{} % usado", &[&format!("{:.0}", frac * 100.0)]);
            let lw = ui.p.width(&ui.f.small, &lab) + 14.0;
            let ex = x0 + (x1 - x0) * self.fills[i];
            let lx = (ex - lw / 2.0).clamp(x0, x1 - lw);
            popup::push_alpha(ui.rt, hk);
            unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(lx, y + 38.0, lx + lw, y + 54.0), 8.0), &ui.chip()); }
            ui.text_c(&ui.f.small, &lab, lx + lw / 2.0, y + 38.0, 16.0, &ui.white);
            ui.rt_pop();
        }
        ui.hit(rect(x0 - 8.0, y - 4.0, x1 + 8.0, y + h), id);
    }

    /// Disco como anillo con el % usado dentro, el nombre y lo libre debajo.
    fn drive_ring(&self, ui: &mut Ui, d: &DFonts, i: usize, dr: &Drive, cx: f32, cy: f32, r: f32, w: f32) {
        let id = 40 + i as u32;
        let hk = ui.heat(id);
        let v = self.fills[i];
        let base = cur_transform(ui);
        unsafe { ui.rt.SetTransform(&mul(&scale_at(1.0 + 0.04 * hk, cx, cy), &base)); }
        ring_stat(ui, d, cx, cy, r, v, &format!("{:.0}%", v * 100.0), "");
        unsafe { ui.rt.SetTransform(&base); }
        let y = cy + r + 12.0;
        ui.text_c(&ui.f.lbl, &fit(ui, &ui.f.lbl, &drive_name(dr), w), cx, y, 18.0, &ui.white);
        ui.text_c(&ui.f.sub, &fit(ui, &ui.f.sub, &i18n::tf("{} libres", &[&fmt_bytes(dr.free)]), w), cx, y + 18.0, 16.0, &ui.dim);
        ui.hit(rect(cx - w / 2.0, cy - r - 8.0, cx + w / 2.0, y + 36.0), id);
    }

    /// Papelera: el cubo (se sacude cuando cambia lo que tiene), cuánto tiene y, con `buttons`,
    /// Abrir y Vaciar a la derecha.
    fn bin(&mut self, ui: &mut Ui, d: &DFonts, st: &StoreData, x0: f32, x1: f32, by: f32, buttons: bool) {
        let sh = self.shake.map(|s| s.elapsed().as_secs_f32()).filter(|s| *s < 0.7);
        unsafe {
            ui.rt.FillRoundedRectangle(&rrect(rect(x0, by + 4.0, x0 + 44.0, by + 48.0), 12.0), &ui.wa(0.09));
            let base = cur_transform(ui);
            if let Some(s) = sh {
                // Balanceo amortiguado alrededor de la base del cubo
                let ang = 0.28 * (s * 26.0).sin() * (1.0 - s / 0.7).powi(2);
                let (c, sn) = (ang.cos(), ang.sin());
                let (px, py) = (x0 + 22.0, by + 40.0);
                let rot = Matrix3x2 { M11: c, M12: sn, M21: -sn, M22: c, M31: px - c * px + sn * py, M32: py - sn * px - c * py };
                ui.rt.SetTransform(&mul(&rot, &base));
            }
            ui.glyph(&d.glyph_m, G_TRASH, x0, by + 4.0, 44.0, 44.0, &ui.white);
            ui.rt.SetTransform(&base);
        }
        let tx = x0 + 56.0;
        let bw = if buttons { 92.0 } else { 0.0 };
        ui.text(&ui.f.name, i18n::t("Papelera"), tx, by + 6.0, 20.0, &ui.white);
        let mut sub = if st.bin_items == 0 { i18n::t("Vacía").to_string() } else if st.bin_items == 1 { i18n::tf("1 elemento · {}", &[&fmt_bytes(st.bin_size)]) } else { i18n::tf("{} elementos · {}", &[&st.bin_items, &fmt_bytes(st.bin_size)]) };
        // Estrecho: solo cuánto ocupa
        if ui.p.width(&ui.f.sub, &sub) > x1 - tx - bw && st.bin_items > 0 { sub = fmt_bytes(st.bin_size); }
        let sub = fit(ui, &ui.f.sub, &sub, x1 - tx - bw);
        let dim = ui.dim.clone();
        let fsub = ui.f.sub.clone();
        self.r_bin.draw(ui, &fsub, &sub, tx, by + 28.0, 18.0, &dim, false);
        if buttons {
            // El cubo y su texto abren la papelera; a la derecha, vaciarla
            let r = rect(x0 - 8.0, by, x1 - bw, by + 52.0);
            let k = ui.heat(10);
            ui.card_k(r, k);
            ui.hit(r, 10);
            let vr = rect(x1 - 80.0, by + 12.0, x1, by + 42.0);
            let k = ui.heat(11);
            unsafe { ui.rt.FillRoundedRectangle(&rrect(vr, 10.0), &ui.brush(rgba(1.0, 0.35, 0.35, 0.12 + 0.10 * k))); }
            let red = ui.brush(rgba(1.0, 0.55, 0.55, if st.bin_items == 0 { 0.4 } else { 1.0 }));
            ui.text_c(&ui.f.lbl, i18n::t("Vaciar…"), (vr.left + vr.right) / 2.0, vr.top, vr.bottom - vr.top, &red);
            if st.bin_items > 0 { ui.hit(vr, 11); }
        }
    }

    /// Abrir y Vaciar (en rojo suave; Windows pide confirmación antes de borrar) de `x0` a `x1`.
    fn bin_buttons(&mut self, ui: &mut Ui, st: &StoreData, x0: f32, x1: f32, y: f32) {
        let mid = x0 + (x1 - x0) * 0.44;
        pbutton(ui, rect(x0, y, mid - 4.0, y + 30.0), None, i18n::t("Abrir"), 10, false);
        let r = rect(mid + 4.0, y, x1, y + 30.0);
        let k = ui.heat(11);
        unsafe { ui.rt.FillRoundedRectangle(&rrect(r, 10.0), &ui.brush(rgba(1.0, 0.35, 0.35, 0.12 + 0.10 * k))); }
        let red = ui.brush(rgba(1.0, 0.55, 0.55, if st.bin_items == 0 { 0.4 } else { 1.0 }));
        ui.text_c(&ui.f.lbl, i18n::t("Vaciar…"), (r.left + r.right) / 2.0, r.top, r.bottom - r.top, &red);
        if st.bin_items > 0 { ui.hit(r, 11); }
    }

    /// Lo más grande de la papelera (si tiene algo) desde `fy`; devuelve dónde acaba.
    fn biggest(&self, ui: &Ui, st: &StoreData, t: f32, x0: f32, x1: f32, fy: f32, bottom: f32) -> f32 {
        if st.bin_big.is_empty() { return fy; }
        caption(ui, i18n::t("LO MÁS GRANDE"), x0, fy);
        let mut end = fy + 20.0;
        for (i, (name, size)) in st.bin_big.iter().enumerate() {
            let yy = fy + 20.0 + i as f32 * 20.0;
            if yy > bottom - 16.0 { break; }
            let k = ez(t, 0.4 + i as f32 * 0.06, 0.35);
            popup::push_alpha(ui.rt, k);
            let sz = fmt_bytes(*size);
            let sw = ui.p.width(&ui.f.sub, &sz);
            ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, name, x1 - x0 - sw - 12.0), x0 + 6.0 * (1.0 - k), yy, 18.0, &ui.dim);
            ui.text_r(&ui.f.sub, &sz, x1, yy, 18.0, &ui.dim);
            ui.rt_pop();
            end = yy + 20.0;
        }
        end + 12.0
    }

    /// Liberar espacio (Sensor de almacenamiento de Windows) en `fy`, si cabe antes de `bottom`.
    fn free_up(&self, ui: &mut Ui, st: &StoreData, x0: f32, x1: f32, fy: f32, bottom: f32) {
        if fy + 34.0 > bottom + 4.0 { return; }
        let k = ui.heat(12);
        let r = rect(x0 - 8.0, fy - 4.0, x1 + 8.0, fy + 34.0);
        ui.card_k(r, k);
        ui.glyph(&ui.f.icon_l, "\u{E7F8}", x0, fy, 20.0, 30.0, &ui.dim);
        let free: u64 = st.drives.iter().map(|d| d.free).sum();
        ui.text(&ui.f.lbl, i18n::t("Liberar espacio"), x0 + 28.0, fy, 16.0, &ui.white);
        ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &i18n::tf("{} libres en total · Sensor de almacenamiento", &[&fmt_bytes(free)]), x1 - x0 - 50.0), x0 + 28.0, fy + 16.0, 16.0, &ui.dim);
        ui.glyph(&ui.f.icon, G_RIGHT, x1 - 16.0, fy, 16.0, 30.0, &ui.wa(0.35 + 0.6 * k));
        ui.hit(r, 12);
    }

    /// Todo el espacio junto: una barra con un tramo por disco y lo libre en total.
    fn total(&self, ui: &Ui, st: &StoreData, x0: f32, x1: f32, y: f32) {
        let (total, free): (u64, u64) = st.drives.iter().fold((0, 0), |a, d| (a.0 + d.total, a.1 + d.free));
        if total == 0 { return; }
        caption(ui, i18n::t("EN TOTAL"), x0, y);
        ui.text_r(&ui.f.sub, &i18n::tf("{} libres de {}", &[&fmt_bytes(free), &fmt_bytes(total)]), x1, y, 16.0, &ui.dim);
        // Lo usado de todos los discos (sigue a los rellenos animados de cada uno)
        let used: f32 = st.drives.iter().zip(self.fills.iter()).map(|(dr, f)| dr.total as f32 * f).sum();
        let frac = used / total as f32;
        bar(ui, x0, y + 26.0, x1 - x0, 8.0, frac, &fill_brush(ui, frac));
    }
}

impl Widget for StorageW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let l = desk::lay(Kind::Storage);
        let (W, H) = (l.w, l.h);
        let st = STORE.lock().unwrap().clone();
        let n = st.as_ref().map(|s| s.drives.len()).unwrap_or(0);
        let count = match n { 0 => String::new(), 1 => i18n::t("1 unidad").into(), n => i18n::tf("{} unidades", &[&n]) };
        let Some(st) = st else { header(ui, d, i18n::t("Almacenamiento"), "", W); return };
        // Al aparecer se llenan en cascada; después siguen al espacio real con suavidad
        let nd = st.drives.len().min(3);
        self.fills.resize(nd, 0.0);
        self.r_free.resize_with(nd, Roll::default);
        self.moving = false;
        for (i, dr) in st.drives.iter().take(nd).enumerate() {
            let frac = (dr.total - dr.free) as f32 / dr.total.max(1) as f32;
            let k = ez(t, 0.15 + i as f32 * 0.12, 0.9);
            if t < 1.1 { self.fills[i] = frac * k; } else { self.moving |= approach(&mut self.fills[i], frac, dt, 0.4); }
        }
        // La papelera se sacude cuando cambia lo que tiene (se tira algo o se vacía)
        if self.last_items.is_some_and(|l| l != st.bin_items) { self.shake = Some(Instant::now()); }
        self.last_items = Some(st.bin_items);
        let drives: Vec<Drive> = st.drives.iter().take(nd).cloned().collect();
        match l.sz {
            // El disco principal como anillo
            Sz::Pequeno => {
                if let Some(dr) = drives.first() { self.drive_ring(ui, d, 0, dr, W / 2.0, PAD + 42.0, 40.0, W - PAD * 2.0); }
            }
            // Dos discos (o un disco y la papelera)
            Sz::Mediano => {
                let cw = (W - PAD * 2.0) / 2.0;
                for (i, dr) in drives.iter().take(2).enumerate() {
                    self.drive_ring(ui, d, i, dr, PAD + cw * (i as f32 + 0.5), PAD + 38.0, 36.0, cw - 12.0);
                }
                if drives.len() == 1 { self.bin(ui, d, &st, PAD + cw + 10.0, W - PAD, H / 2.0 - 26.0, false); }
            }
            // Todos los discos en línea y la papelera con sus botones
            Sz::Banda => {
                let slots = drives.len() as f32 + 1.2;
                let cw = (W - PAD * 2.0) / slots;
                for (i, dr) in drives.iter().enumerate() {
                    let x = PAD + cw * i as f32;
                    let id = 40 + i as u32;
                    let hk = ui.heat(id);
                    ui.card_k(rect(x - 8.0, PAD - 6.0, x + cw - 12.0, H - PAD + 6.0), hk);
                    let (cx, cy, r) = (x + 42.0, H / 2.0, 38.0);
                    ring_stat(ui, d, cx, cy, r, self.fills[i], &format!("{:.0}%", self.fills[i] * 100.0), "");
                    let tx = x + 98.0;
                    let tw = x + cw - 20.0 - tx;
                    ui.text(&ui.f.name, &fit(ui, &ui.f.name, &drive_name(dr), tw), tx, cy - 32.0, 20.0, &ui.white);
                    ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &i18n::tf("{} libres", &[&fmt_bytes(dr.free)]), tw), tx, cy - 10.0, 18.0, &ui.dim);
                    ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &i18n::tf("de {}", &[&fmt_bytes(dr.total)]), tw), tx, cy + 8.0, 18.0, &ui.faint);
                    ui.hit(rect(x - 8.0, PAD - 6.0, x + cw - 12.0, H - PAD + 6.0), id);
                    vline(ui, x + cw - 4.0, PAD + 6.0, H - PAD - 6.0);
                }
                let bx = PAD + cw * drives.len() as f32 + 8.0;
                self.bin(ui, d, &st, bx, W - PAD, PAD + 4.0, false);
                self.bin_buttons(ui, &st, bx, W - PAD, H - PAD - 36.0);
            }
            // Todos los discos en columna y la papelera debajo
            Sz::Alto => {
                header(ui, d, i18n::t("Almacenamiento"), "", W);
                let mut y = PAD + 36.0;
                for (i, dr) in drives.iter().enumerate() {
                    let id = 40 + i as u32;
                    let hk = ui.heat(id);
                    let frac = self.fills[i];
                    ui.card_k(rect(PAD - 8.0, y - 4.0, W - PAD + 8.0, y + 54.0), hk);
                    ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &drive_name(dr), W - PAD * 2.0), PAD, y, 18.0, &ui.white);
                    ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &i18n::tf("{} libres", &[&fmt_bytes(dr.free)]), W - PAD * 2.0), PAD, y + 18.0, 16.0, &ui.dim);
                    bar(ui, PAD, y + 40.0, W - PAD * 2.0, 8.0, frac, &fill_brush(ui, frac));
                    ui.hit(rect(PAD - 8.0, y - 4.0, W - PAD + 8.0, y + 54.0), id);
                    y += 66.0;
                }
                let by = (y + 2.0).max(H - PAD - 98.0);
                hline(ui, PAD - 4.0, W - PAD + 4.0, by - 8.0);
                self.bin(ui, d, &st, PAD, W - PAD, by, false);
                self.bin_buttons(ui, &st, PAD, W - PAD, H - PAD - 30.0);
            }
            // Como el widget de siempre: discos con su tamaño, papelera, lo más grande y liberar espacio
            Sz::Grande | Sz::Columna => {
                header(ui, d, i18n::t("Almacenamiento"), &count, W);
                let used = l.sz == Sz::Columna;
                let mut y = PAD + 36.0;
                for (i, dr) in drives.iter().enumerate() {
                    self.drive_row(ui, i, dr, PAD, W - PAD, y, used);
                    y += if used { 76.0 } else { 56.0 };
                }
                let by = y + 6.0;
                hline(ui, PAD - 4.0, W - PAD + 4.0, by - 10.0);
                self.bin(ui, d, &st, PAD, W - PAD, by, true);
                let fy = self.biggest(ui, &st, t, PAD, W - PAD, by + 62.0, H - PAD);
                if used {
                    self.free_up(ui, &st, PAD, W - PAD, fy, H - PAD - 64.0);
                    hline(ui, PAD - 4.0, W - PAD + 4.0, H - PAD - 50.0);
                    self.total(ui, &st, PAD, W - PAD, H - PAD - 38.0);
                } else {
                    self.free_up(ui, &st, PAD, W - PAD, fy, H - PAD);
                }
            }
            // Discos como anillos y el total a la izquierda; papelera, lo más grande y liberar espacio a la derecha
            Sz::Extragrande => {
                header(ui, d, i18n::t("Almacenamiento"), &count, 360.0);
                let n = drives.len().max(1) as f32;
                let cw = (340.0 - PAD) / n;
                for (i, dr) in drives.iter().enumerate() {
                    self.drive_ring(ui, d, i, dr, PAD + cw * (i as f32 + 0.5), 128.0, 50.0, cw - 12.0);
                }
                hline(ui, PAD - 4.0, 344.0, 254.0);
                self.total(ui, &st, PAD, 340.0, 268.0);
                vline(ui, 370.0, PAD, H - PAD);
                let x0 = 400.0;
                self.bin(ui, d, &st, x0, W - PAD, PAD, true);
                let fy = self.biggest(ui, &st, t, x0, W - PAD, PAD + 64.0, H - PAD);
                self.free_up(ui, &st, x0, W - PAD, fy.max(H - PAD - 34.0), H - PAD);
            }
        }
    }
    fn click(&mut self, id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        match id? {
            10 => Some(Box::new(|| sys::open("shell:RecycleBinFolder"))),
            12 => Some(Box::new(|| sys::open("ms-settings:storagesense"))),
            11 => Some(Box::new(|| {
                std::thread::spawn(|| {
                    // Sin opciones: Windows pregunta antes de vaciar (no se puede deshacer)
                    unsafe { let _ = SHEmptyRecycleBinW(None, PCWSTR::null(), 0); }
                    STORE_NOW.store(true, Ordering::Relaxed);
                });
            })),
            i @ 40..=42 => {
                let root = STORE.lock().unwrap().as_ref().and_then(|s| s.drives.get((i - 40) as usize).map(|d| d.root.clone()))?;
                Some(Box::new(move || sys::open(&root)))
            }
            _ => None,
        }
    }
    fn animating(&self, t: f32) -> bool {
        t < 1.3 || self.moving || self.r_bin.busy() || self.r_free.iter().any(|r| r.busy()) || self.shake.is_some_and(|s| s.elapsed().as_secs_f32() < 0.7)
    }
    fn smooth(&self) -> bool { self.animating(9.0) }
    fn layouts(&self) -> bool { true }
}

// ---------------------------------------------------------------- proyectos
/// `tab_at`: cuándo se cambió de pestaña (las filas vuelven a entrar en cascada); `pill`: posición
/// animada del resaltado de la pestaña (0 = En local, 1 = GitHub).
#[derive(Default)]
struct ProjectsW { tab: u8, scroll: usize, tab_at: Option<Instant>, pill: f32 }

const ROW_H: f32 = 44.0;

/// Una fila de proyecto ya preparada: nombre, detalle, hace cuánto, color del lenguaje, privado.
type PRow = (String, String, String, [f32; 3], bool);

/// Identificadores: fila `ROW_LOCAL + i` / `ROW_GH + i`; sus acciones rápidas, `ACT_LOCAL + i·4 + j` /
/// `ACT_GH + i·4 + j` (así se sabe de qué lista es aunque se vean las dos a la vez).
const ROW_LOCAL: u32 = 100;
const ROW_GH: u32 = 300;
const ACT_LOCAL: u32 = 500;
const ACT_GH: u32 = 1000;

fn local_rows(p: &Projects) -> Vec<PRow> {
    p.local.iter().map(|r| {
        let mut sub = String::new();
        if !r.lang.is_empty() { sub += r.lang; sub += " · "; }
        sub += &r.branch;
        match r.dirty { Some(0) => sub += i18n::t(" · limpio"), Some(1) => sub += i18n::t(" · 1 cambio"), Some(n) => sub += &i18n::tf(" · {} cambios", &[&n]), None => {} }
        (r.name.clone(), sub, ago(r.modified), r.color, false)
    }).collect()
}

fn gh_rows(p: &Projects) -> Vec<PRow> {
    p.gh.iter().map(|g| {
        let mut sub = if g.private { i18n::t("Privado").to_string() } else { i18n::t("Público").to_string() };
        if !g.lang.is_empty() { sub += " · "; sub += &g.lang; }
        if g.stars > 0 { sub += &format!(" · {} ★", g.stars); }
        (g.name.clone(), sub, ago(g.pushed), g.color, g.private)
    }).collect()
}

/// Qué decir cuando una lista está vacía.
fn empty_msg(p: &Projects, gh: bool) -> &'static str {
    if !gh { i18n::t(if p.scanned { "No hay proyectos con git en el Escritorio ni en las carpetas habituales" } else { "Buscando proyectos…" }) }
    else { i18n::t(match p.gh_ok { None => "Cargando tus repos de GitHub…", Some(false) => "Instala GitHub CLI e inicia sesión: gh auth login", Some(true) => "No tienes repos en GitHub" }) }
}

fn spawn_detached(cmd: &str, args: &[&str], dir: Option<&str>, console: bool) -> bool {
    let mut c = std::process::Command::new(cmd);
    c.args(args);
    if let Some(d) = dir { c.current_dir(d); }
    c.creation_flags(if console { 0x10 /* CREATE_NEW_CONSOLE */ } else { 0x0800_0000 });
    c.spawn().is_ok()
}

/// Abre la carpeta en VS Code lanzando `Code.exe` directamente (nunca a través de `cmd`, que
/// interpretaría `&`, `|`… del nombre de la carpeta).
fn open_vscode(path: &str) {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let pf = std::env::var("ProgramFiles").unwrap_or_default();
    let exe = [format!("{local}\\Programs\\Microsoft VS Code\\Code.exe"), format!("{pf}\\Microsoft VS Code\\Code.exe")]
        .into_iter().find(|p| std::path::Path::new(p).exists());
    match exe {
        Some(e) => { let _ = std::process::Command::new(e).arg(path).spawn(); }
        None => sys::open_args("code", &format!("\"{}\"", path.replace('"', ""))),
    }
}

fn open_terminal(path: String) {
    // La carpeta va como directorio de trabajo, nunca en la línea de órdenes: `wt` trata `;` como
    // separador de comandos y una carpeta con ese carácter podría lanzar otra cosa
    if !spawn_detached("wt", &["-d", "."], Some(&path), false) { spawn_detached("powershell", &["-NoExit"], Some(&path), true); }
}

impl ProjectsW {
    /// Filas de una lista de `x0` a `x1` desde `top` (las `n` que empiezan en `skip`), con la fecha y,
    /// al pasar el ratón, las acciones rápidas.
    fn rows(&self, ui: &mut Ui, rows: &[PRow], gh: bool, skip: usize, n: usize, x0: f32, x1: f32, top: f32, rh: f32, t: f32) {
        let (rid, aid) = if gh { (ROW_GH, ACT_GH) } else { (ROW_LOCAL, ACT_LOCAL) };
        let narrow = x1 - x0 < 200.0;
        for (vi, (i, (name, sub, when, col, private))) in rows.iter().enumerate().skip(skip).take(n).enumerate() {
            let k = ez(t, 0.1 + vi as f32 * 0.05, 0.35);
            let y = top + vi as f32 * rh + 8.0 * (1.0 - k);
            let id = rid + i as u32;
            popup::push_alpha(ui.rt, k);
            // La fila sigue encendida mientras el ratón está en ella o en sus botones
            let qa = aid + i as u32 * 4;
            let hk = (0..3).map(|j| ui.heat(qa + j)).fold(ui.heat(id), f32::max);
            ui.card_k(rect(x0 - 8.0, y, x1 + 8.0, y + rh - 4.0), hk);
            // Al pasar el ratón el punto de color crece y el texto se desplaza un poco
            let tx = x0 + 18.0 + 4.0 * hk;
            // Estrecho: tres líneas (nombre; lenguaje y rama; cambios y fecha)
            let ny = y + (rh - 4.0) / 2.0 - if narrow { 26.0 } else { 17.0 };
            unsafe { ui.rt.FillEllipse(&ellipse(x0 + 4.0, ny + 13.0, 5.0 + 1.5 * hk), &ui.brush(rgba(col[0], col[1], col[2], 1.0))); }
            let ww = if narrow { 0.0 } else { ui.p.width(&ui.f.small, when) + 34.0 };
            let nm = fit(ui, &ui.f.name, name, x1 - tx - ww - if *private { 20.0 } else { 0.0 });
            ui.text(&ui.f.name, &nm, tx, ny + 3.0, 20.0, &ui.white);
            if *private { let nw = ui.p.width(&ui.f.name, &nm); ui.glyph(&ui.f.icon, G_LOCK, tx + 4.0 + nw, ny + 5.0, 14.0, 16.0, &ui.faint); }
            if narrow {
                let parts: Vec<&str> = sub.split(" · ").collect();
                let k2 = parts.len().min(2);
                let mut rest: Vec<&str> = parts[k2..].to_vec();
                rest.push(when);
                ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &parts[..k2].join(" · "), x1 - tx - 4.0), tx, ny + 21.0, 16.0, &ui.dim);
                ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &rest.join(" · "), x1 - tx - 4.0), tx, ny + 37.0, 16.0, &ui.faint);
            } else {
                ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, sub, x1 - tx - 4.0), tx, ny + 22.0, 16.0, &ui.dim);
            }
            // Fecha e icono se apartan para dejar sitio a las acciones rápidas
            if !narrow {
                popup::push_alpha(ui.rt, 1.0 - hk);
                ui.text_r(&ui.f.small, when, x1 - 26.0 + 10.0 * hk, ny + 4.0, 18.0, &ui.faint);
                ui.glyph(&ui.f.icon, if gh { G_OPEN } else { G_FOLDER }, x1 - 18.0, ny + 4.0, 18.0, 18.0, &ui.wa(0.35));
                ui.rt_pop();
            }
            ui.hit(rect(x0 - 8.0, y, x1 + 8.0, y + rh - 4.0), id);
            if hk > 0.01 {
                let acts: [&str; 3] = if gh { [G_OPEN, G_ISSUE, G_COPY] } else { [G_FOLDER, G_CODE, G_TERM] };
                let fi = ui.f.icon.clone();
                for (j, g) in acts.iter().enumerate() {
                    // Entran en cascada desde la derecha
                    let kj = ((hk - j as f32 * 0.12) / 0.76).clamp(0.0, 1.0);
                    let cx = x1 - 12.0 - (2 - j) as f32 * 30.0 + 8.0 * (1.0 - kj);
                    popup::push_alpha(ui.rt, kj);
                    round_btn(ui, &fi, g, cx, ny + 19.0, 13.0, qa + j as u32, false);
                    ui.rt_pop();
                }
            }
            ui.rt_pop();
        }
    }

    /// Un proyecto como tarjeta (Pequeño y Banda): nombre grande, lenguaje y rama, cambios y fecha.
    fn card(&self, ui: &mut Ui, d: &DFonts, r: &Repo, i: usize, b: D2D_RECT_F, t: f32, k0: f32) {
        let id = ROW_LOCAL + i as u32;
        let hk = ui.heat(id);
        let k = ez(t, k0, 0.4);
        popup::push_alpha(ui.rt, k);
        ui.card_k(b, 0.6 + 0.4 * hk);
        let (x, w) = (b.left + 12.0, b.right - b.left - 24.0);
        let y = b.top + 10.0 + 6.0 * (1.0 - k);
        unsafe { ui.rt.FillEllipse(&ellipse(x + 5.0, y + 11.0, 5.0 + 1.5 * hk), &ui.brush(rgba(r.color[0], r.color[1], r.color[2], 1.0))); }
        ui.text(&d.title, &fit(ui, &d.title, &r.name, w - 18.0), x + 18.0, y, 22.0, &ui.white);
        let lang = if r.lang.is_empty() { r.branch.clone() } else { format!("{} · {}", r.lang, r.branch) };
        ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &lang, w), x, y + 28.0, 18.0, &ui.dim);
        let ch = match r.dirty { Some(0) => i18n::t("Limpio").to_string(), Some(1) => i18n::t("1 cambio").into(), Some(n) => i18n::tf("{} cambios", &[&n]), None => String::new() };
        let bottom = b.bottom - 26.0;
        if !ch.is_empty() { ui.text(&ui.f.lbl, &ch, x, bottom - 18.0, 18.0, if r.dirty.unwrap_or(0) > 0 { &ui.white } else { &ui.dim }); }
        ui.text(&ui.f.small, &ago(r.modified), x, bottom, 16.0, &ui.faint);
        ui.rt_pop();
        ui.hit(b, id);
    }
}

impl Widget for ProjectsW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let l = desk::lay(Kind::Projects);
        let (W, H) = (l.w, l.h);
        let p = PROJ.lock().unwrap().clone();
        // Tras cambiar de pestaña, las filas vuelven a entrar en cascada
        let tt = self.tab_at.map(|a| a.elapsed().as_secs_f32()).map_or(t, |s| s.min(t));
        let (loc, gh) = (local_rows(&p), gh_rows(&p));
        match l.sz {
            // El último proyecto en el que se trabajó
            Sz::Pequeno => {
                caption(ui, i18n::t("PROYECTOS"), PAD, PAD - 4.0);
                match p.local.first() {
                    Some(r) => self.card(ui, d, r, 0, rect(PAD - 8.0, PAD + 14.0, W - PAD + 8.0, H - PAD + 10.0), t, 0.05),
                    None => { let (a, b) = wrap2(ui, &ui.f.sub, empty_msg(&p, false), W - PAD * 2.0); ui.text(&ui.f.sub, &a, PAD, PAD + 30.0, 18.0, &ui.dim); ui.text(&ui.f.sub, &b, PAD, PAD + 48.0, 18.0, &ui.dim); }
                }
            }
            // Los 3 más recientes
            Sz::Mediano => {
                header(ui, d, i18n::t("Proyectos"), i18n::t("En local"), W);
                if loc.is_empty() { ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, empty_msg(&p, false), W - PAD * 2.0), PAD, PAD + 50.0, 20.0, &ui.dim); }
                self.rows(ui, &loc, false, 0, 3, PAD, W - PAD, PAD + 22.0, 37.0, t);
            }
            // Los 4 más recientes, en tarjetas
            Sz::Banda => {
                header(ui, d, i18n::t("Proyectos"), i18n::t("En local"), W);
                if loc.is_empty() { ui.text(&ui.f.sub, empty_msg(&p, false), PAD, PAD + 50.0, 20.0, &ui.dim); }
                let gap = 14.0;
                let cw = (W - PAD * 2.0 - gap * 3.0) / 4.0;
                for (i, r) in p.local.iter().take(4).enumerate() {
                    let x = PAD + i as f32 * (cw + gap);
                    self.card(ui, d, r, i, rect(x - 4.0, PAD + 30.0, x + cw + 4.0, H - PAD + 6.0), t, 0.08 + i as f32 * 0.05);
                }
            }
            // Los 5 más recientes, en filas estrechas
            Sz::Alto => {
                header(ui, d, i18n::t("Proyectos"), "", W);
                if loc.is_empty() { let (a, b) = wrap2(ui, &ui.f.sub, empty_msg(&p, false), W - PAD * 2.0); ui.text(&ui.f.sub, &a, PAD, PAD + 40.0, 18.0, &ui.dim); ui.text(&ui.f.sub, &b, PAD, PAD + 58.0, 18.0, &ui.dim); }
                self.rows(ui, &loc, false, 0, 5, PAD, W - PAD, PAD + 32.0, 58.0, t);
            }
            // Como el widget de siempre: pestañas En local / GitHub con rama, cambios y estrellas
            Sz::Grande => {
                let ROWS = ((H - PAD - 34.0 - 40.0) / ROW_H) as usize;
                ui.text(&d.title, i18n::t("Proyectos"), PAD, PAD - 2.0, 22.0, &ui.white);
                // Pestañas: el resaltado se desliza de una a otra
                let tabs = [i18n::t("En local"), "GitHub"];
                let mut rs = [rect(0.0, 0.0, 0.0, 0.0); 2];
                let mut x = W - PAD;
                for i in (0..2).rev() {
                    let w = ui.p.width(&ui.f.lbl, tabs[i]) + 22.0;
                    rs[i] = rect(x - w, PAD - 3.0, x, PAD + 21.0);
                    x -= w + 6.0;
                }
                approach(&mut self.pill, self.tab as f32, dt, 0.09);
                let lerp = |a: f32, b: f32| a + (b - a) * self.pill;
                for (i, r) in rs.iter().enumerate() {
                    let k = ui.heat(20 + i as u32);
                    unsafe { ui.rt.FillRoundedRectangle(&rrect(*r, 12.0), &ui.wa(0.07 + 0.06 * k)); }
                }
                let pr = rect(lerp(rs[0].left, rs[1].left), rs[0].top, lerp(rs[0].right, rs[1].right), rs[0].bottom);
                unsafe { ui.rt.FillRoundedRectangle(&rrect(pr, 12.0), &ui.wa(0.18)); }
                for (i, r) in rs.iter().enumerate() {
                    let on = (self.pill - i as f32).abs() < 0.5;
                    ui.text_c(&ui.f.lbl, tabs[i], (r.left + r.right) / 2.0, r.top, r.bottom - r.top, if on { &ui.white } else { &ui.dim });
                    ui.hit(*r, 20 + i as u32);
                }
                let isgh = self.tab == 1;
                let rows = if isgh { &gh } else { &loc };
                self.scroll = self.scroll.min(rows.len().saturating_sub(ROWS));
                if rows.is_empty() { let (a, b) = wrap2(ui, &ui.f.sub, empty_msg(&p, isgh), W - PAD * 2.0); ui.text_c(&ui.f.sub, &a, W / 2.0, H / 2.0 - 10.0, 20.0, &ui.dim); ui.text_c(&ui.f.sub, &b, W / 2.0, H / 2.0 + 8.0, 20.0, &ui.dim); }
                self.rows(ui, rows, isgh, self.scroll, ROWS, PAD, W - PAD, PAD + 34.0, ROW_H, tt);
                // Pie: cuántos hay y cómo se usa
                if rows.len() > ROWS {
                    ui.text(&ui.f.small, &i18n::tf("{}–{} de {}", &[&(self.scroll + 1), &(self.scroll + ROWS).min(rows.len()), &rows.len()]), PAD, H - PAD - 14.0, 16.0, &ui.faint);
                }
                ui.text_r(&ui.f.small, i18n::t("Clic derecho: más opciones"), W - PAD, H - PAD - 14.0, 16.0, &ui.faint);
            }
            // Las dos listas a la vez: en local arriba y GitHub abajo (Columna) o una al lado de otra (Extragrande)
            Sz::Columna | Sz::Extragrande => {
                ui.text(&d.title, i18n::t("Proyectos"), PAD, PAD - 2.0, 22.0, &ui.white);
                ui.text_r(&ui.f.small, i18n::t("Clic derecho: más opciones"), W - PAD, PAD, 18.0, &ui.faint);
                let sections: [(f32, f32, f32, f32, bool); 2] = if l.sz == Sz::Columna {
                    [(PAD, W - PAD, PAD + 36.0, 300.0, false), (PAD, W - PAD, 312.0, H - PAD, true)]
                } else {
                    [(PAD, 350.0, PAD + 36.0, H - PAD, false), (390.0, W - PAD, PAD + 36.0, H - PAD, true)]
                };
                if l.sz == Sz::Columna { hline(ui, PAD - 4.0, W - PAD + 4.0, 302.0); } else { vline(ui, 370.0, PAD + 36.0, H - PAD); }
                for (x0, x1, y0, y1, isgh) in sections {
                    let rows = if isgh { &gh } else { &loc };
                    caption(ui, if isgh { "GITHUB" } else { i18n::t("EN LOCAL") }, x0, y0);
                    let n = ((y1 - y0 - 22.0) / ROW_H) as usize;
                    if rows.is_empty() { let (a, b) = wrap2(ui, &ui.f.sub, empty_msg(&p, isgh), x1 - x0); ui.text(&ui.f.sub, &a, x0, y0 + 26.0, 18.0, &ui.dim); ui.text(&ui.f.sub, &b, x0, y0 + 44.0, 18.0, &ui.dim); }
                    self.rows(ui, rows, isgh, 0, n, x0, x1, y0 + 22.0, ROW_H, t);
                    if rows.len() > n { ui.text_r(&ui.f.small, &i18n::tf("{} de {}", &[&n, &rows.len()]), x1, y0, 16.0, &ui.faint); }
                }
            }
        }
    }
    fn click(&mut self, id: Option<u32>, right: bool, pt: POINT) -> Option<Box<dyn FnOnce()>> {
        let id = id?;
        if id == 20 || id == 21 {
            let t = (id - 20) as u8;
            if self.tab == t { PROJ_NOW.store(true, Ordering::Relaxed); }
            if self.tab != t { self.tab_at = Some(Instant::now()); }
            self.tab = t;
            self.scroll = 0;
            return None;
        }
        if id < ROW_LOCAL { return None; }
        let p = PROJ.lock().unwrap().clone();
        // Acciones rápidas de una fila (base + fila·4 + botón)
        if id >= ACT_LOCAL {
            let gh = id >= ACT_GH;
            let rel = id - if gh { ACT_GH } else { ACT_LOCAL };
            let (i, j) = ((rel / 4) as usize, rel % 4);
            if !gh {
                let r = p.local.get(i)?.clone();
                return Some(Box::new(move || match j { 0 => sys::open(&r.path), 1 => open_vscode(&r.path), _ => open_terminal(r.path.clone()) }));
            }
            let g = p.gh.get(i)?.clone();
            return Some(Box::new(move || match j { 0 => sys::open(&g.url), 1 => sys::open(&format!("{}/issues", g.url)), _ => copy_text(&g.url) }));
        }
        if id < ROW_GH {
            let r = p.local.get((id - ROW_LOCAL) as usize)?.clone();
            if !right { return Some(Box::new(move || sys::open(&r.path))); }
            Some(Box::new(move || {
                let mut items = vec!["Abrir en el Explorador", "Abrir en VS Code", "Abrir terminal aquí"];
                if r.remote.is_some() { items.push("Ver en GitHub"); }
                items.push("");
                items.push("Copiar ruta");
                // Se enseña traducido; se compara con el texto original
                let shown: Vec<&str> = items.iter().map(|s| i18n::t(*s)).collect();
                match desk::context_menu(&shown, pt).map(|k| items[k]) {
                    Some("Abrir en el Explorador") => sys::open(&r.path),
                    Some("Abrir en VS Code") => open_vscode(&r.path),
                    Some("Abrir terminal aquí") => open_terminal(r.path.clone()),
                    Some("Ver en GitHub") => if let Some(u) = &r.remote { sys::open(u) },
                    Some("Copiar ruta") => copy_text(&r.path),
                    _ => {}
                }
            }))
        } else {
            let g = p.gh.get((id - ROW_GH) as usize)?.clone();
            if !right { return Some(Box::new(move || sys::open(&g.url))); }
            let local = p.local.iter().find(|r| r.name.eq_ignore_ascii_case(&g.name) || r.remote.as_deref().is_some_and(|u| u.eq_ignore_ascii_case(&g.url))).map(|r| r.path.clone());
            Some(Box::new(move || {
                let mut items = vec!["Abrir en GitHub", "Issues", "Pull requests"];
                if local.is_some() { items.push(""); items.push("Abrir la carpeta local"); }
                items.push("");
                items.push("Copiar URL");
                // Se enseña traducido; se compara con el texto original
                let shown: Vec<&str> = items.iter().map(|s| i18n::t(*s)).collect();
                match desk::context_menu(&shown, pt).map(|k| items[k]) {
                    Some("Abrir en GitHub") => sys::open(&g.url),
                    Some("Issues") => sys::open(&format!("{}/issues", g.url)),
                    Some("Pull requests") => sys::open(&format!("{}/pulls", g.url)),
                    Some("Abrir la carpeta local") => if let Some(l) = &local { sys::open(l) },
                    Some("Copiar URL") => copy_text(&g.url),
                    _ => {}
                }
            }))
        }
    }
    // La rueda solo desplaza la lista con pestañas (en los demás tamaños se ve todo lo que cabe)
    fn wheel(&mut self, d: i32) -> bool {
        if desk::lay(Kind::Projects).sz != Sz::Grande { return false; }
        if d < 0 { self.scroll += 1; } else { self.scroll = self.scroll.saturating_sub(1); }
        true
    }
    fn animating(&self, t: f32) -> bool {
        t < 1.0 || (self.pill - self.tab as f32).abs() > 0.002 || self.tab_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.9)
    }
    fn layouts(&self) -> bool { true }
}

/// Copia texto marcado como privado: no entra en el historial de Windows (Win+V), ni en el portapapeles
/// en la nube, ni lo leen los gestores de portapapeles que respetan la marca (el de macbar incluido).
pub(crate) fn copy_secret(s: &str) { copy_with(s, true) }

/// Copia texto al portapapeles (CF_UNICODETEXT).
pub(crate) fn copy_text(s: &str) { copy_with(s, false) }

fn copy_with(s: &str, private: bool) {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    let w: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(Some(bar_hwnd())).is_err() { return; }
        let _ = EmptyClipboard();
        if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2) {
            let p = GlobalLock(h) as *mut u16;
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                let _ = GlobalUnlock(h);
                let _ = SetClipboardData(13, Some(HANDLE(h.0)));
            }
        }
        if private {
            // Formatos de marca: DWORD 0 = no guardar ni subir; el de exclusión basta con que exista
            for name in [w!("CanIncludeInClipboardHistory"), w!("CanUploadToCloudClipboard"), w!("ExcludeClipboardContentFromMonitorProcessing")] {
                let fmt = RegisterClipboardFormatW(name);
                if fmt == 0 { continue; }
                if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, 4) {
                    let p = GlobalLock(h) as *mut u32;
                    if !p.is_null() { *p = 0; let _ = GlobalUnlock(h); let _ = SetClipboardData(fmt, Some(HANDLE(h.0))); }
                }
            }
        }
        let _ = CloseClipboard();
    }
}

// ---------------------------------------------------------------- móvil (Enlace Móvil)
/// `bmps`: miniaturas ya convertidas en imágenes; `sync_at`: cuándo se pulsó actualizar (el botón gira).
#[derive(Default)]
struct PhoneW { moving: bool, bmps: HashMap<String, ID2D1Bitmap>, bmp_rt: crate::RtKey, sync_at: Option<Instant> }

const G_PHONE: &str = "\u{E8EA}";
const G_CHAT: &str = "\u{E8BD}";
const G_CALL: &str = "\u{E717}";
const G_PHOTO: &str = "\u{E91B}";
const G_BELL: &str = "\u{EA8F}";

/// Pila dibujada de 24 × 10 (el relleno es la carga; roja con poca).
fn battery_icon(ui: &Ui, x: f32, y: f32, pct: u8) {
    unsafe {
        let r = rect(x, y, x + 22.0, y + 10.0);
        ui.rt.DrawRoundedRectangle(&rrect(r, 2.5), &ui.white, 1.4, None);
        ui.rt.FillRectangle(&rect(x + 22.5, y + 3.0, x + 24.5, y + 7.0), &ui.white);
        let fillc = if pct <= 15 { ui.brush(rgba(1.0, 0.45, 0.4, 1.0)) } else { ui.white.clone() };
        ui.rt.FillRoundedRectangle(&rrect(rect(x + 2.0, y + 2.0, x + 2.0 + 18.0 * pct as f32 / 100.0, y + 8.0), 1.2), &fillc);
    }
}

impl PhoneW {
    /// Miniatura como imagen de Direct2D (se rehace si cambia el lienzo).
    fn bmp(&mut self, rt: &ID2D1RenderTarget, key: &str, px: &Arc<Vec<u8>>) -> Option<ID2D1Bitmap> {
        if self.bmp_rt.fresh(rt) { self.bmps.clear(); }
        if self.bmps.len() > 12 { self.bmps.clear(); }
        if let Some(b) = self.bmps.get(key) { return Some(b.clone()); }
        let b = bitmap(rt, px, crate::phone::THUMB)?;
        self.bmps.insert(key.to_string(), b.clone());
        Some(b)
    }
    /// Igual, para imágenes que no son cuadradas (el fondo del móvil). La clave lleva la dirección de los
    /// datos: al elegir otro fondo cambia y se rehace.
    fn bmp_wh(&mut self, rt: &ID2D1RenderTarget, key: &str, px: &Arc<Vec<u8>>, w: u32, h: u32) -> Option<ID2D1Bitmap> {
        if self.bmp_rt.fresh(rt) { self.bmps.clear(); }
        let key = format!("{key}:{:x}", Arc::as_ptr(px) as usize);
        if let Some(b) = self.bmps.get(&key) { return Some(b.clone()); }
        let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
        let b = unsafe { rt.CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(px.as_ptr() as _), w * 4, &bp).ok()? };
        self.bmps.insert(key, b.clone());
        Some(b)
    }

    /// El móvil con su fondo (o un degradado y la pista de que se puede poner uno). Al pulsarlo se elige fondo.
    fn frame(&mut self, ui: &mut Ui, st: &crate::phone::PhoneState, fx: f32, fy: f32, fw: f32, fh: f32, t: f32) {
        let k0 = ez(t, 0.0, 0.5);
        let hk = ui.heat(14);
        let base = cur_transform(ui);
        unsafe { ui.rt.SetTransform(&mul(&scale_at((0.94 + 0.06 * k0) * (1.0 + 0.03 * hk), fx + fw / 2.0, fy + fh / 2.0), &base)); }
        popup::push_alpha(ui.rt, k0);
        let fr = rrect(rect(fx, fy, fx + fw, fy + fh), 5.0);
        match st.wall.as_ref().and_then(|px| self.bmp_wh(ui.rt, "wall", px, crate::phone::WALL_W, crate::phone::WALL_H)) {
            Some(b) => unsafe {
                if let Ok(br) = ui.rt.CreateBitmapBrush(&b, None, None) {
                    br.SetTransform(&Matrix3x2 { M11: fw / crate::phone::WALL_W as f32, M12: 0.0, M21: 0.0, M22: fh / crate::phone::WALL_H as f32, M31: fx, M32: fy });
                    ui.rt.FillRoundedRectangle(&fr, &br);
                }
            },
            None => unsafe {
                // Sin fondo elegido: un degradado suave y una pista de que se puede poner uno
                let g = ui.p.vgrad(fy, fy + fh, rgba(0.36, 0.45, 0.62, 0.55), rgba(0.10, 0.12, 0.18, 0.55));
                ui.rt.FillRoundedRectangle(&fr, &g);
                ui.glyph(&ui.f.icon_l, G_PHOTO, fx, fy + fh / 2.0 - 24.0, fw, 28.0, &ui.wa(0.55));
                ui.text_c(&ui.f.small, i18n::t("Fondo"), fx + fw / 2.0, fy + fh / 2.0 + 6.0, 16.0, &ui.wa(0.55));
            },
        }
        ui.rt_pop();
        unsafe { ui.rt.SetTransform(&base); }
        ui.hit(rect(fx, fy, fx + fw, fy + fh), 14);
    }

    /// Nombre, conexión y batería, el estado con el botón de actualizar y cuántos avisos tiene (desde `x0`).
    fn status(&mut self, ui: &mut Ui, d: &DFonts, st: &crate::phone::PhoneState, x0: f32, x1: f32, fy: f32, t: f32) {
        let k1 = ez(t, 0.08, 0.45);
        popup::push_alpha(ui.rt, k1);
        let name = if st.name.is_empty() { i18n::t("Tu móvil").to_string() } else { st.name.clone() };
        ui.text(&d.title, &fit(ui, &d.title, &name, x1 - x0), x0 + 8.0 * (1.0 - k1), fy + 2.0, 26.0, &ui.white);
        // Iconos de estado: conexión y batería (la batería solo si se conoce)
        let iy = fy + 30.0;
        let on = if st.running { ui.white.clone() } else { ui.wa(0.35) };
        ui.glyph(&ui.f.icon, "\u{E701}", x0, iy, 18.0, 22.0, &on);
        if let Some((pct, _)) = st.battery {
            battery_icon(ui, x0 + 26.0, iy + 6.0, pct);
            ui.text(&ui.f.name, &format!("{pct} %"), x0 + 58.0, iy, 22.0, &ui.white);
        }
        // Estado: "Conectado" con su marca y, a la derecha, actualizar
        let py = fy + 60.0;
        let pr = rect(x0, py, x1, py + 38.0);
        unsafe { ui.rt.FillRoundedRectangle(&rrect(pr, 10.0), &ui.wa(0.08)); }
        let okc = if st.running { rgba(0.33, 0.78, 0.42, 1.0) } else { ui.ic(0.3) };
        unsafe { ui.rt.FillEllipse(&ellipse(x0 + 17.0, py + 19.0, 3.5), &ui.brush(okc)); }
        ui.text(&ui.f.name, if st.running { i18n::t("Conectado") } else { i18n::t("Sin conectar") }, x0 + 29.0, py, 38.0, &ui.white);
        unsafe { ui.rt.FillRectangle(&rect(x1 - 44.0, py + 8.0, x1 - 43.0, py + 30.0), &ui.wa(0.15)); }
        // El botón de actualizar gira un poco mientras se busca de nuevo
        let base = cur_transform(ui);
        let spin = self.sync_at.map(|s| s.elapsed().as_secs_f32()).filter(|s| *s < 0.8);
        let (scx, scy) = (x1 - 22.0, py + 19.0);
        if let Some(s) = spin {
            self.moving = true;
            let a = popup::ease_out(s / 0.8) * std::f32::consts::TAU;
            let (c, sn) = (a.cos(), a.sin());
            unsafe { ui.rt.SetTransform(&mul(&Matrix3x2 { M11: c, M12: sn, M21: -sn, M22: c, M31: scx - c * scx + sn * scy, M32: scy - sn * scx - c * scy }, &base)); }
        }
        round_btn(ui, &ui.f.icon.clone(), "\u{E72C}", scx, scy, 16.0, 15, false);
        unsafe { ui.rt.SetTransform(&base); }
        // Debajo: cuántas notificaciones tiene el móvil (solo el número; el contenido se ve en Enlace Móvil)
        let nn = if st.notifs.is_empty() { st.unread as usize } else { st.notifs.len() };
        let mut sx = x0 + 2.0;
        let ny = py + 44.0;
        if nn > 0 {
            let s = nn.to_string();
            let lbl = if nn == 1 { i18n::t("notificación").to_string() } else { i18n::t("notificaciones").to_string() };
            let hk = ui.heat(16);
            ui.text(&ui.f.lbl, &s, sx, ny, 18.0, &ui.white);
            let nw = ui.p.width(&ui.f.lbl, &s);
            ui.text(&ui.f.small, &lbl, sx + nw + 5.0, ny, 18.0, &ui.wa(0.55 + 0.35 * hk));
            let end = sx + nw + 5.0 + ui.p.width(&ui.f.small, &lbl);
            ui.hit(rect(sx - 4.0, ny - 4.0, end + 4.0, ny + 20.0), 16);
            sx = end + 10.0;
        }
        let sub = match st.battery { Some((_, when)) if !st.running || deskdata::now_unix().saturating_sub(when) > 120 => i18n::tf("Batería vista {}", &[&ago(when)]), _ => String::new() };
        if !sub.is_empty() {
            let s = if nn > 0 { format!("· {sub}") } else { sub };
            ui.text(&ui.f.small, &fit(ui, &ui.f.small, &s, x1 - sx), sx, ny, 18.0, &ui.faint);
        }
        ui.rt_pop();
    }

    /// Accesos (mensajes, llamadas, fotos y Enlace Móvil) en `cols` columnas de botones de `bh` de alto.
    fn access(&self, ui: &mut Ui, x0: f32, x1: f32, ay: f32, cols: usize, bh: f32, t: f32) {
        let gap = 8.0;
        let bw = (x1 - x0 - gap * (cols - 1) as f32) / cols as f32;
        for (i, g) in [G_CHAT, G_CALL, G_PHOTO, G_OPEN].iter().enumerate() {
            let id = 10 + i as u32;
            let k = ez(t, 0.15 + i as f32 * 0.05, 0.4);
            let hk = ui.heat(id);
            let p = desk::press(id);
            let (c, r) = (i % cols, i / cols);
            let x = x0 + c as f32 * (bw + gap);
            let y = ay + r as f32 * (bh + gap);
            let rr = rect(x, y + 6.0 * (1.0 - k), x + bw, y + bh + 6.0 * (1.0 - k));
            let base = cur_transform(ui);
            if p > 0.0 { unsafe { ui.rt.SetTransform(&mul(&scale_at(1.0 - 0.06 * p, x + bw / 2.0, y + bh / 2.0), &base)); } }
            popup::push_alpha(ui.rt, k);
            unsafe {
                ui.rt.FillRoundedRectangle(&rrect(rr, 9.0), &ui.wa(0.08 + 0.08 * hk));
                ui.rt.DrawRoundedRectangle(&rrect(rr, 9.0), &ui.wa(0.10), 1.0, None);
            }
            ui.glyph(&ui.f.icon_l, g, rr.left, rr.top, bw, bh, &ui.white);
            ui.hit(rr, id);
            ui.rt_pop();
            if p > 0.0 { unsafe { ui.rt.SetTransform(&base); } }
        }
    }

    /// Fotos recientes (las últimas de la cámara, capturas e Imágenes del móvil) en fila de `x0` a `x1`.
    fn photos(&mut self, ui: &mut Ui, st: &crate::phone::PhoneState, x0: f32, x1: f32, py: f32, t: f32) -> f32 {
        let gap = 8.0;
        let side = (x1 - x0 - gap * 2.0) / 3.0;
        for (i, ph) in st.photos.iter().take(3).enumerate() {
            let id = 30 + i as u32;
            let k = ez(t, 0.22 + i as f32 * 0.07, 0.45);
            let hk = ui.heat(id);
            let x = x0 + i as f32 * (side + gap);
            // Entra creciendo un poco; al pasar el ratón se acerca
            let sc = (0.92 + 0.08 * k) * (1.0 + 0.04 * hk);
            let (cx, cy) = (x + side / 2.0, py + side / 2.0);
            let base = cur_transform(ui);
            unsafe { ui.rt.SetTransform(&mul(&scale_at(sc, cx, cy), &base)); }
            popup::push_alpha(ui.rt, k);
            let key = format!("ph:{}", ph.path);
            match ph.thumb.as_ref().and_then(|px| self.bmp(ui.rt, &key, px)) {
                Some(b) => draw_art(ui.rt, &b, crate::phone::THUMB, x, py, side, 12.0),
                None => unsafe {
                    ui.rt.FillRoundedRectangle(&rrect(rect(x, py, x + side, py + side), 12.0), &ui.wa(0.08));
                    ui.glyph(&ui.f.icon_l, G_PHOTO, x, py, side, side, &ui.wa(0.4));
                },
            }
            if ph.video {
                unsafe { ui.rt.FillEllipse(&ellipse(cx, cy, 15.0), &ui.brush(rgba(0.0, 0.0, 0.0, 0.45))); }
                ui.glyph(&ui.f.icon, G_PLAY, cx - 15.0, cy - 15.0, 30.0, 30.0, &ui.brush(rgba(1.0, 1.0, 1.0, 0.95)));
            }
            ui.rt_pop();
            unsafe { ui.rt.SetTransform(&base); }
            ui.hit(rect(x, py, x + side, py + side), id);
        }
        py + side
    }
}

impl Widget for PhoneW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, _dt: f32) {
        let l = desk::lay(Kind::Phone);
        let (W, H) = (l.w, l.h);
        let st = crate::phone::PHONE.lock().unwrap().clone();
        self.moving = false;
        match l.sz {
            // Batería y conexión
            Sz::Pequeno => {
                let k = ez(t, 0.05, 0.45);
                popup::push_alpha(ui.rt, k);
                let name = if st.name.is_empty() { i18n::t("Tu móvil").to_string() } else { st.name.clone() };
                ui.glyph(&ui.f.icon_l, G_PHONE, PAD - 2.0, PAD - 4.0, 18.0, 22.0, &ui.dim);
                ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &name, W - PAD * 2.0 - 22.0), PAD + 22.0, PAD - 4.0, 22.0, &ui.white);
                match st.battery {
                    Some((pct, _)) => {
                        ui.text(&d.huge, &format!("{pct}%"), PAD - 2.0, PAD + 26.0 + 6.0 * (1.0 - k), 50.0, &ui.white);
                        let b = if pct <= 15 { ui.brush(rgba(1.0, 0.45, 0.4, 1.0)) } else { ui.white.clone() };
                        bar(ui, PAD, PAD + 84.0, W - PAD * 2.0, 6.0, pct as f32 / 100.0 * ez(t, 0.15, 0.8), &b);
                    }
                    None => ui.glyph(&d.glyph_l, G_PHONE, PAD, PAD + 30.0, 40.0, 50.0, &ui.dim),
                }
                let okc = if st.running { rgba(0.33, 0.78, 0.42, 1.0) } else { ui.ic(0.3) };
                unsafe { ui.rt.FillEllipse(&ellipse(PAD + 4.0, H - PAD - 8.0, 3.5), &ui.brush(okc)); }
                ui.text(&ui.f.lbl, if st.running { i18n::t("Conectado") } else { i18n::t("Sin conectar") }, PAD + 14.0, H - PAD - 17.0, 18.0, &ui.white);
                ui.rt_pop();
                ui.hit(rect(0.0, 0.0, W, H), 12);
            }
            // El móvil con su fondo, el estado y cuántos avisos tiene
            Sz::Mediano => {
                let fh: f32 = H - PAD * 2.0;
                let fw = (fh * 0.48).round();
                self.frame(ui, &st, PAD, PAD, fw, fh, t);
                self.status(ui, d, &st, PAD + fw + 18.0, W - PAD, PAD, t);
            }
            // El móvil, el estado y los accesos en rejilla
            Sz::Alto => {
                let k1 = ez(t, 0.08, 0.45);
                popup::push_alpha(ui.rt, k1);
                let name = if st.name.is_empty() { i18n::t("Tu móvil").to_string() } else { st.name.clone() };
                ui.text(&d.title, &fit(ui, &d.title, &name, W - PAD * 2.0), PAD, PAD - 2.0, 22.0, &ui.white);
                let iy = PAD + 22.0;
                let on = if st.running { ui.white.clone() } else { ui.wa(0.35) };
                ui.glyph(&ui.f.icon, "\u{E701}", PAD, iy, 18.0, 22.0, &on);
                if let Some((pct, _)) = st.battery {
                    battery_icon(ui, PAD + 26.0, iy + 6.0, pct);
                    ui.text(&ui.f.name, &format!("{pct} %"), PAD + 58.0, iy, 22.0, &ui.white);
                }
                ui.rt_pop();
                let (fw, fh) = (62.0, 128.0);
                self.frame(ui, &st, (W - fw) / 2.0, PAD + 56.0, fw, fh, t);
                let okc = if st.running { rgba(0.33, 0.78, 0.42, 1.0) } else { ui.ic(0.3) };
                let sy = PAD + 56.0 + fh + 12.0;
                let s = if st.running { i18n::t("Conectado") } else { i18n::t("Sin conectar") };
                let sw = ui.p.width(&ui.f.lbl, s) + 12.0;
                unsafe { ui.rt.FillEllipse(&ellipse(W / 2.0 - sw / 2.0 + 3.0, sy + 9.0, 3.5), &ui.brush(okc)); }
                ui.text(&ui.f.lbl, s, W / 2.0 - sw / 2.0 + 12.0, sy, 18.0, &ui.white);
                self.access(ui, PAD, W - PAD, H - PAD - 84.0, 2, 38.0, t);
            }
            // Lo del Mediano, los accesos y las fotos recientes
            Sz::Grande | Sz::Columna => {
                let big = l.sz == Sz::Columna;
                let fh: f32 = if big { 138.0 } else { 124.0 };
                let fw = (fh * 0.48).round();
                self.frame(ui, &st, PAD, PAD, fw, fh, t);
                self.status(ui, d, &st, PAD + fw + 18.0, W - PAD, PAD + if big { 8.0 } else { 0.0 }, t);
                let ay = PAD + fh + 14.0;
                self.access(ui, PAD, W - PAD, ay, 4, 42.0, t);
                let mut y = ay + 42.0 + 14.0;
                if !st.photos.is_empty() {
                    hline(ui, PAD - 4.0, W - PAD + 4.0, y - 2.0);
                    caption(ui, i18n::t("FOTOS RECIENTES"), PAD, y + 6.0);
                    let side = ((W - PAD * 2.0 - 16.0) / 3.0).min(H - PAD - (y + 28.0));
                    y = self.photos(ui, &st, PAD, PAD + side * 3.0 + 16.0, y + 28.0, t);
                }
                // Columna: los avisos agrupados por app (cuántos de cada una y el último hace cuánto; el
                // contenido se ve en Enlace Móvil)
                if big {
                    y += 16.0;
                    hline(ui, PAD - 4.0, W - PAD + 4.0, y - 2.0);
                    caption(ui, i18n::t("AVISOS"), PAD, y + 6.0);
                    let mut apps: Vec<(String, usize, u64)> = Vec::new();
                    for n in st.notifs.iter() {
                        match apps.iter_mut().find(|a| a.0 == n.app) { Some(a) => { a.1 += 1; a.2 = a.2.max(n.when); } None => apps.push((n.app.clone(), 1, n.when)) }
                    }
                    apps.sort_by(|a, b| b.2.cmp(&a.2));
                    let mut ry = y + 28.0;
                    if apps.is_empty() {
                        let msg = if st.unread > 0 { i18n::tf("{} sin leer · ábrelos en Enlace Móvil", &[&st.unread]) } else { i18n::t("Sin avisos").to_string() };
                        ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &msg, W - PAD * 2.0), PAD, ry, 18.0, &ui.dim);
                    }
                    for (i, (app, n, when)) in apps.iter().enumerate() {
                        if ry + 30.0 > H - PAD + 6.0 { break; }
                        let id = 100 + i as u32;
                        let hk = ui.heat(id);
                        let k = ez(t, 0.3 + i as f32 * 0.06, 0.35);
                        let r = rect(PAD - 8.0, ry - 4.0, W - PAD + 8.0, ry + 28.0);
                        popup::push_alpha(ui.rt, k);
                        ui.card_k(r, hk);
                        ui.glyph(&ui.f.icon, G_BELL, PAD - 2.0, ry, 18.0, 22.0, &ui.dim);
                        let cnt = n.to_string();
                        let cw = ui.p.width(&ui.f.lbl, &cnt) + 16.0;
                        unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(PAD + 24.0, ry + 2.0, PAD + 24.0 + cw, ry + 20.0), 9.0), &ui.wa(0.12)); }
                        ui.text_c(&ui.f.lbl, &cnt, PAD + 24.0 + cw / 2.0, ry + 2.0, 18.0, &ui.white);
                        let ago_s = ago(*when);
                        let aw = ui.p.width(&ui.f.small, &ago_s);
                        ui.text(&ui.f.name, &fit(ui, &ui.f.name, app, W - PAD * 2.0 - 36.0 - cw - aw - 12.0), PAD + 32.0 + cw, ry, 22.0, &ui.white);
                        ui.text_r(&ui.f.small, &ago_s, W - PAD, ry + 2.0, 18.0, &ui.faint);
                        ui.rt_pop();
                        ui.hit(r, id);
                        ry += 34.0;
                    }
                }
            }
            Sz::Banda | Sz::Extragrande => {}
        }
    }
    fn click(&mut self, id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        match id? {
            10 => Some(Box::new(|| sys::open("sms:"))),
            11 => Some(Box::new(|| sys::open("tel:"))),
            12 | 13 | 16 | 100..=199 => Some(Box::new(|| sys::open("ms-phone:"))),
            14 => Some(Box::new(crate::phone::pick_wallpaper)),
            15 => { self.sync_at = Some(Instant::now()); crate::phone::PHONE_NOW.store(true, Ordering::Relaxed); None }
            20 => Some(Box::new(crate::phone::ask_access)),
            i @ 30..=32 => {
                let p = crate::phone::PHONE.lock().unwrap().photos.get((i - 30) as usize).map(|p| p.path.clone())?;
                Some(Box::new(move || sys::open(&p)))
            }
            i @ 300..=399 => {
                let nid = crate::phone::PHONE.lock().unwrap().notifs.get((i - 300) as usize).map(|n| n.id)?;
                Some(Box::new(move || crate::phone::dismiss(nid)))
            }
            _ => None,
        }
    }
    fn animating(&self, t: f32) -> bool { t < 1.3 || self.moving || crate::phone::PHONE.lock().unwrap().running }
    fn frame_ms(&self) -> u32 { 50 }
    fn smooth(&self) -> bool { self.moving }
    fn layouts(&self) -> bool { true }
}
