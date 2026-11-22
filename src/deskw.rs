//! Los seis widgets grandes del escritorio: calendario, tiempo, música con letra en vivo, sistema,
//! almacenamiento con papelera y proyectos (en local y en GitHub).
//! Cada uno tiene su propio tamaño (`desk::size`); dentro de cada widget, `W` y `H` son los suyos.
#![allow(non_snake_case)]

use super::*;
use desk::{size, DFonts, Kind};
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
const G_STAR: &str = "\u{E734}";
const G_PIN: &str = "\u{E707}";
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
        Kind::Calendar => if desk::variant(k) == 1 { Box::new(crate::deskv::TodayW) } else { Box::new(CalendarW::default()) },
        Kind::Weather => match desk::variant(k) { 1 => Box::new(crate::deskv::SkyW), 2 => Box::new(crate::deskv::MinWeatherW::default()), _ => Box::new(WeatherW::default()) },
        Kind::Music => if desk::variant(k) == 1 { Box::new(crate::deskv::CoverW::default()) } else { Box::new(MusicW::default()) },
        Kind::System if desk::variant(k) == 1 => Box::new(crate::deskv::RingsW::default()),
        Kind::System => Box::new(SystemW { bars: [0.0; 3], disp: [0.0; 4], apps: HashMap::new(), moving: false, graph: None, live: None, layer_at: None, probe_a: 0.0, probe_x: 0.0, probe_on: false }),
        Kind::Storage => Box::new(StorageW::default()),
        Kind::Projects => Box::new(ProjectsW { tab: 0, scroll: 0, tab_at: None, pill: 0.0 }),
        Kind::Phone => { crate::phone::start(); Box::new(PhoneW::default()) }
        Kind::Visual => Box::new(crate::deskw2::VisualW::default()),
        Kind::Photos => Box::new(crate::deskw2::PhotosW::default()),
        Kind::Clocks => Box::new(crate::deskw3::ClocksW::default()),
        Kind::Todo => Box::new(crate::deskw3::TodoW::default()),
        Kind::Countdown => Box::new(crate::deskw3::CountdownW::default()),
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


// ---------------------------------------------------------------- calendario
/// `slide`: avance del cambio de mes (±1 → 0); `dir`: hacia dónde (para dibujar también el mes que se va);
/// `timer_on`: fundido entre los botones de enfoque y el temporizador en marcha.
#[derive(Default)]
struct CalendarW { month_off: i32, slide: f32, dir: f32, r_time: Roll, r_date: Roll, r_title: Roll, r_left: Roll, timer_on: f32, ring: f32 }

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

impl CalendarW {
    fn go(&mut self, d: i32) {
        self.month_off += d;
        // Si ya se estaba deslizando, sigue desde donde iba (no salta)
        self.slide = (self.slide + d as f32).clamp(-1.6, 1.6);
        self.dir = d as f32;
    }

    /// Rejilla de un mes desplazada `dx`, con opacidad `a`. `t` lleva la cascada de entrada.
    fn grid(&self, ui: &mut Ui, y: i32, m: u32, dx: f32, a: f32, t: f32, hy: f32, W: f32) {
        if a <= 0.01 { return; }
        let now = unsafe { GetLocalTime() };
        let (ty, tm, td) = (now.wYear as i32, now.wMonth as u32, now.wDay as u32);
        let cw = (W - PAD * 2.0) / 7.0;
        let first = (calendar::weekday(y, m, 1) + 6) % 7; // 0 = lunes
        let ch = 25.0;
        popup::push_alpha(ui.rt, a);
        for day in 1..=days_in(y, m) {
            let idx = first + day - 1;
            let (c, r) = (idx % 7, idx / 7);
            let k = ez(t, 0.05 + r as f32 * 0.04 + c as f32 * 0.008, 0.35);
            let cx = PAD + cw * (c as f32 + 0.5) + dx;
            let cy = hy + 30.0 + r as f32 * ch + 6.0 * (1.0 - k);
            let today = y == ty && m == tm && day == td;
            let past = (y, m, day) < (ty, tm, td);
            if today {
                // Hoy: el círculo azul entra con un pequeño rebote y late una vez
                let pop = crate::ease_back(((t - 0.35) / 0.4).clamp(0.0, 1.0));
                let beat = ((t - 0.75) / 0.5).clamp(0.0, 1.0);
                let halo = (beat * std::f32::consts::PI).sin();
                unsafe {
                    if halo > 0.01 { ui.rt.FillEllipse(&ellipse(cx, cy, 12.0 + 6.0 * beat), &ui.brush(crate::profiles::accent(0.35 * halo))); }
                    ui.rt.FillEllipse(&ellipse(cx, cy, 12.0 * pop), &ui.on);
                }
            }
            // Al pasar el ratón, un círculo suave tras el día (solo en el mes que está quieto)
            let id = 300 + day;
            let hk = if dx.abs() < 1.0 { ui.heat(id) } else { 0.0 };
            if hk > 0.01 && !today { unsafe { ui.rt.FillEllipse(&ellipse(cx, cy, 11.0 + 1.5 * hk), &ui.wa(0.12 * hk)); } }
            let b = if today { ui.white.clone() } else if c >= 5 { ui.wa((0.42 + 0.4 * hk) * k) } else if past { ui.wa((0.55 + 0.4 * hk) * k) } else { ui.wa(0.95 * k) };
            ui.text_c(&ui.f.lbl, &day.to_string(), cx, cy - 8.0, 16.0, &b);
            if dx.abs() < 1.0 { ui.hits.push((rect(cx - 13.0, cy - 12.0, cx + 13.0, cy + 12.0), id)); }
        }
        unsafe { ui.rt.PopLayer(); }
    }
}

impl Widget for CalendarW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (W, H) = size(Kind::Calendar);
        let now = unsafe { GetLocalTime() };
        let (ty, tm, td) = (now.wYear as i32, now.wMonth as u32, now.wDay as u32);
        approach(&mut self.slide, 0.0, dt, 0.13);
        let red = ui.brush(rgba(1.0, 0.42, 0.40, 1.0));
        ui.text(&ui.f.lbl, &i18n::weekday(now.wDayOfWeek as usize).to_uppercase(), PAD, PAD - 4.0, 18.0, &red);
        // Fecha y hora ruedan al cambiar (cada minuto, a medianoche…)
        let white = ui.white.clone();
        // "18 de noviembre" / "November 18"
        let mes = i18n::month((tm - 1) as usize);
        let fecha = if i18n::en() { format!("{mes} {td}") } else { format!("{td} de {mes}") };
        self.r_date.draw(ui, &d.big, &fecha, PAD, PAD + 16.0, 34.0, &white, false);
        self.r_time.draw(ui, &d.big, &format!("{:02}:{:02}", now.wHour, now.wMinute), W - PAD, PAD + 16.0, 34.0, &white, true);
        ui.text_r(&ui.f.sub, &i18n::tf("Semana {}", &[&iso_week(ty, tm, td)]), W - PAD, PAD - 4.0, 18.0, &ui.dim);

        // Mes que se ve (con flechas o rueda)
        let (y, m) = month_add(ty, tm, self.month_off);
        let gy = 92.0;
        let mn = i18n::month((m - 1) as usize);
        let title = format!("{}{} {}", mn[..1].to_uppercase(), &mn[1..], y);
        let fname = ui.f.name.clone();
        self.r_title.draw(ui, &fname, &title, PAD, gy, 22.0, &white, false);
        let fi = &ui.f.icon.clone();
        round_btn(ui, fi, G_LEFT, W - PAD - 46.0, gy + 11.0, 13.0, 1, false);
        round_btn(ui, fi, G_RIGHT, W - PAD - 13.0, gy + 11.0, 13.0, 2, false);

        let cw = (W - PAD * 2.0) / 7.0;
        let hy = gy + 30.0;
        for (i, l) in i18n::weekday_letters().iter().enumerate() {
            ui.text_c(&ui.f.small, l, PAD + cw * (i as f32 + 0.5), hy, 16.0, &ui.faint);
        }
        // Cambio de mes: el nuevo entra por un lado mientras el anterior sale por el otro
        let gw = W - PAD * 2.0;
        let s = self.slide;
        unsafe { ui.rt.PushAxisAlignedClip(&rect(PAD - 6.0, hy + 14.0, W - PAD + 6.0, hy + 30.0 + 6.0 * 25.0 + 14.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE); }
        if s.abs() > 0.002 && self.dir != 0.0 {
            let (py, pm) = month_add(ty, tm, self.month_off - self.dir as i32);
            let off = s - self.dir;
            self.grid(ui, py, pm, off * gw * 0.6, (1.0 - (s.abs() - 1.0).abs()).clamp(0.0, 1.0) * (1.0 - off.abs().min(1.0) * 0.0), 9.0, hy, W);
        }
        self.grid(ui, y, m, s * gw * 0.6, 1.0 - s.abs().min(1.0) * 0.85, t, hy, W);
        unsafe { ui.rt.PopAxisAlignedClip(); }

        // Temporizador rápido: los botones y la cuenta atrás se funden entre sí
        let by = H - PAD - 34.0;
        ui.sep(by - 12.0);
        let left = live::timer_left();
        approach(&mut self.timer_on, if left.is_some() { 1.0 } else { 0.0 }, dt, 0.12);
        if self.timer_on < 0.99 {
            let a = 1.0 - self.timer_on;
            popup::push_alpha(ui.rt, a);
            let dy = 8.0 * self.timer_on;
            ui.glyph(&ui.f.icon_l, G_TIMER, PAD, by + 2.0 + dy, 20.0, 30.0, &ui.dim);
            ui.text(&ui.f.lbl, i18n::t("Enfoque"), PAD + 26.0, by + 7.0 + dy, 20.0, &ui.dim);
            for (i, mins) in [5u32, 15, 25].iter().enumerate() {
                let x = W - PAD - (3 - i) as f32 * 62.0 + 6.0;
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
                ui.rt.DrawEllipse(&ellipse(PAD + 14.0, by + 17.0 + dy, 12.0), &ui.track, 3.0, None);
                if let Some(g) = arc_geo(ui.rt, PAD + 14.0, by + 17.0 + dy, 12.0, self.ring) { ui.rt.DrawGeometry(&g, &orange, 3.0, round_stroke(ui.rt).as_ref()); }
            }
            let ft = d.title.clone();
            self.r_left.draw(ui, &ft, &live::mmss(l), PAD + 36.0, by + 6.0 + dy, 22.0, &white, false);
            let r = rect(W - PAD - 96.0, by + 2.0 + dy, W - PAD, by + 32.0 + dy);
            if left.is_some() { pbutton(ui, r, Some(G_STOP), i18n::t("Parar"), 30, false); }
            ui.rt_pop();
        }
    }
    fn click(&mut self, id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        match id? {
            1 => { self.go(-1); None }
            2 => { self.go(1); None }
            30 => Some(Box::new(live::timer_stop)),
            i @ 31..=33 => { let m = [5u32, 15, 25][(i - 31) as usize]; Some(Box::new(move || live::timer_start(m * 60))) }
            _ => { if self.month_off != 0 { let d = -self.month_off; self.month_off = 0; self.slide = (d as f32).clamp(-1.0, 1.0); self.dir = d.signum() as f32; } None }
        }
    }
    fn wheel(&mut self, d: i32) -> bool { self.go(if d < 0 { 1 } else { -1 }); true }
    fn animating(&self, t: f32) -> bool {
        t < 1.4 || self.slide.abs() > 0.002 || self.r_time.busy() || self.r_date.busy() || self.r_title.busy() || self.r_left.busy()
            || (self.timer_on > 0.001 && self.timer_on < 0.999) || (self.timer_on > 0.5)
    }
    fn smooth(&self) -> bool { self.slide.abs() > 0.002 || self.r_title.busy() || self.r_time.busy() || (self.timer_on > 0.001 && self.timer_on < 0.999) }
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

impl Widget for WeatherW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (W, _) = size(Kind::Weather);
        let city = sys::config_value("ciudad").unwrap_or_else(|| i18n::t("Tu ubicación").into());
        let cur = *WEATHER.lock().unwrap();
        let fc = weather::FORECAST.lock().unwrap().clone();
        ui.text(&d.title, &city, PAD, PAD - 2.0, 22.0, &ui.white);
        let Some(cur) = cur else {
            ui.text(&ui.f.sub, i18n::t("Cargando el tiempo…"), PAD, PAD + 30.0, 20.0, &ui.dim);
            return;
        };
        // Ancho: temperatura e icono grandes a la izquierda, los detalles en columna a la derecha
        // Al aparecer, la temperatura cuenta desde unos grados menos; luego rueda cuando cambia
        let white = ui.white.clone();
        let temp = format!("{:.0}°", cur.temp - 6.0 * (1.0 - ez(t, 0.1, 0.8)));
        if t < 0.95 { ui.text(&d.huge, &temp, PAD - 2.0, PAD + 22.0, 50.0, &white); self.r_temp.cur = temp.clone(); }
        else { self.r_temp.draw(ui, &d.huge, &temp, PAD - 2.0, PAD + 22.0, 50.0, &white, false); }
        let tw = ui.p.width(&d.huge, &temp);
        // El icono entra con un pequeño salto
        let ip = crate::ease_back(ez(t, 0.2, 0.5));
        unsafe {
            let base = cur_transform(ui);
            ui.rt.SetTransform(&mul(&scale_at(0.6 + 0.4 * ip, PAD + tw + 37.0, PAD + 45.0), &base));
            weather::draw_icon_anim(ui.rt, &ui.p, &ui.f.icon_xl, PAD + tw + 8.0, PAD + 16.0, 58.0, 58.0, cur.code, cur.is_day, &white, Some(t));
            ui.rt.SetTransform(&base);
        }
        let ftitle = d.title.clone();
        self.r_desc.draw(ui, &ftitle, weather_desc(cur.code), W - PAD, PAD - 2.0, 22.0, &white, true);
        if let Some(f) = &fc {
            if let Some(d0) = f.days.first() {
                ui.text_r(&ui.f.sub, &i18n::tf("Máx {}°  ·  Mín {}°", &[&format!("{:.0}", d0.3), &format!("{:.0}", d0.2)]), W - PAD, PAD + 24.0, 18.0, &ui.dim);
            }
            let rain_now = f.rain.first().copied().unwrap_or(0.0);
            let rain_max = f.rain.iter().take(12).cloned().fold(0.0f32, f32::max);
            ui.text_r(&ui.f.sub, &i18n::tf("Lluvia ahora {}%  ·  próximas 12 h {}%", &[&format!("{:.0}", rain_now), &format!("{:.0}", rain_max)]), W - PAD, PAD + 42.0, 18.0, &ui.dim);
            if let Some(d1) = f.days.get(1) {
                ui.text_r(&ui.f.sub, &i18n::tf("Mañana {}° / {}° · {}", &[&format!("{:.0}", d1.3), &format!("{:.0}", d1.2), &weather_desc(d1.1).to_lowercase()]), W - PAD, PAD + 60.0, 18.0, &ui.dim);
            }
            // Amanecer y anochecer, bajo la temperatura
            if let Some((rise, set)) = &f.sun {
                let ks = ez(t, 0.3, 0.4);
                popup::push_alpha(ui.rt, ks);
                // Sol que sale y luna, cada uno con su hora
                let y = PAD + 76.0;
                ui.glyph(&ui.f.icon, "\u{E706}", PAD - 2.0, y, 16.0, 18.0, &ui.dim);
                ui.text(&ui.f.sub, rise, PAD + 18.0, y, 18.0, &ui.dim);
                let x2 = PAD + 18.0 + ui.p.width(&ui.f.sub, rise) + 16.0;
                ui.glyph(&ui.f.icon, "\u{E708}", x2, y, 16.0, 18.0, &ui.dim);
                ui.text(&ui.f.sub, set, x2 + 20.0, y, 18.0, &ui.dim);
                ui.rt_pop();
            }

            // ---- Curva de las próximas 24 h
            let (top, bot) = (112.0, 206.0);
            let n = f.hours.len().min(25);
            if n >= 2 {
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
                let step = (W - PAD * 2.0) / (n - 1) as f32;
                let pts: Vec<(f32, f32)> = self.curve.iter().enumerate().map(|(i, v)| (PAD + i as f32 * step, bot - 16.0 - (v - lo) / (hi - lo) * (bot - top - 32.0))).collect();
                // Zonas para el ratón: una por hora
                for i in 0..n {
                    let x = PAD + i as f32 * step;
                    ui.hit(rect(x - step / 2.0, top - 10.0, x + step / 2.0, bot + 20.0), 100 + i as u32);
                }
                // Barras de lluvia al fondo
                for (i, r) in f.rain.iter().take(n).enumerate() {
                    if *r < 5.0 { continue; }
                    let h = r / 100.0 * 26.0 * ez(t, 0.2 + i as f32 * 0.01, 0.4);
                    let x = PAD + i as f32 * step;
                    unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(x - 3.0, bot - h, x + 3.0, bot), 2.0), &ui.brush(rgba(0.48, 0.72, 1.0, 0.45))); }
                }
                // La curva se dibuja de izquierda a derecha al aparecer
                let k = ez(t, 0.15, 1.0);
                unsafe {
                    ui.rt.PushAxisAlignedClip(&rect(0.0, top - 10.0, PAD + (W - PAD * 2.0) * k + 8.0, bot + 2.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
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
                    if i == 0 || pts[i].0 > PAD + (W - PAD * 2.0) * k { continue; }
                    let (x, y) = pts[i];
                    let s = format!("{:.0}°", temps[i]);
                    let x = x.clamp(PAD + 10.0, W - PAD - 10.0);
                    ui.text_c(&ui.f.small, &s, x, if below { y + 6.0 } else { y - 22.0 }, 16.0, &ui.white);
                }
                // Horas
                for i in (0..n).step_by(3) {
                    let label = if i == 0 { i18n::t("Ahora").to_string() } else { format!("{}", f.hours[i].0) };
                    let x = (PAD + i as f32 * step).clamp(PAD + 14.0, W - PAD - 10.0);
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
                    let tx = PAD + i as f32 * step;
                    if self.probe_a < 0.05 { self.probe_x = tx; }
                    pm |= approach(&mut self.probe_x, tx, dt, 0.06);
                }
                pm |= approach(&mut self.probe_a, if hov.is_some() { 1.0 } else { 0.0 }, dt, 0.08);
                // Quieta bajo el ratón no pide fotogramas: solo mientras se desliza o aparece
                self.morph |= pm;
                if self.probe_a > 0.01 {
                    let fi = ((self.probe_x - PAD) / step).clamp(0.0, (n - 1) as f32);
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
                    let lx = (self.probe_x - lw / 2.0).clamp(PAD - 4.0, W - PAD - lw + 4.0);
                    popup::push_alpha(ui.rt, a);
                    unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(lx, bot + 2.0, lx + lw, bot + 22.0), 10.0), &ui.chip()); }
                    ui.text_c(&ui.f.small, &label, lx + lw / 2.0, bot + 4.0, 16.0, &ui.white);
                    ui.rt_pop();
                }
            }
            // ---- Próximos días
            let dy = 240.0;
            ui.sep(dy - 8.0);
            let days: Vec<_> = f.days.iter().take(6).collect();
            let cw = (W - PAD * 2.0) / days.len().max(1) as f32;
            for (i, (wd, code, mn, mx)) in days.iter().enumerate() {
                let k = ez(t, 0.35 + i as f32 * 0.05, 0.35);
                let cx = PAD + cw * (i as f32 + 0.5);
                let y = dy + 8.0 * (1.0 - k);
                popup::push_alpha(ui.rt, k);
                ui.text_c(&ui.f.lbl, if i == 0 { i18n::t("Hoy") } else { i18n::weekday_short(*wd as usize) }, cx, y, 18.0, if i == 0 { &ui.white } else { &ui.dim });
                let b = ui.white.clone();
                weather::draw_icon(ui.rt, &ui.p, &ui.f.icon_l, cx - 14.0, y + 20.0, 28.0, 28.0, *code, true, &b);
                ui.text_c(&ui.f.lbl, &format!("{:.0}°", mx), cx, y + 52.0, 18.0, &ui.white);
                ui.text_c(&ui.f.sub, &format!("{:.0}°", mn), cx, y + 70.0, 16.0, &ui.dim);
                ui.rt_pop();
            }
        }
        // Todo el widget actualiza al pulsar (por debajo de las zonas de las horas)
        ui.hits.insert(0, (rect(0.0, 0.0, W, size(Kind::Weather).1), 1));
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
    /// Se está viendo algo que se mueve solo (letra o ecualizador), no la cola
    live_view: bool,
    /// El ratón está sobre la línea de tiempo (su marca sigue al cursor)
    seek_hover: bool,
    playing: bool,
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

impl Widget for MusicW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (W, H) = size(Kind::Music);
        let media = MEDIA.lock().unwrap().clone();
        let Some(m) = media else {
            self.playing = false;
            let k = ez(t, 0.1, 0.5);
            popup::push_alpha(ui.rt, k);
            ui.glyph(&d.glyph_l, G_MUSIC, 0.0, H / 2.0 - 60.0 + 8.0 * (1.0 - k), W, 40.0, &ui.dim);
            ui.text_c(&d.title, i18n::t("Nada sonando"), W / 2.0, H / 2.0 - 8.0, 24.0, &ui.white);
            ui.text_c(&ui.f.sub, i18n::t("Pon música en Spotify, YouTube Music o el navegador"), W / 2.0, H / 2.0 + 18.0, 18.0, &ui.dim);
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
        let since_title = self.title_at.map(|a| a.elapsed().as_secs_f32()).unwrap_or(9.0).min(t + 0.0);

        // Carátula: cuando cambia, la nueva entra con un "pop" mientras la anterior se desvanece
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

        // ---- Carátula grande a la izquierda (como en YouTube Music)
        let asz = H - PAD - 92.0;
        let fade = self.art_at.map(|a| (a.elapsed().as_secs_f32() / 0.5).min(1.0)).unwrap_or(1.0);
        if fade >= 1.0 { self.prev_art = None; }
        unsafe {
            let base = { let mut m0 = Matrix3x2::default(); ui.rt.GetTransform(&mut m0); m0 };
            if let Some(p) = &self.prev_art {
                popup::push_alpha(ui.rt, 1.0 - popup::ease_out(fade));
                draw_art(ui.rt, p, ART_BIG_PX, PAD, PAD, asz, 14.0);
                ui.rt.PopLayer();
            }
            let sc = 0.94 + 0.06 * crate::ease_back(fade);
            ui.rt.SetTransform(&mul(&scale_at(sc, PAD + asz / 2.0, PAD + asz / 2.0), &base));
            popup::push_alpha(ui.rt, popup::ease_out(fade));
            match &self.art {
                Some((_, bmp)) => draw_art(ui.rt, bmp, ART_BIG_PX, PAD, PAD, asz, 14.0),
                None => {
                    ui.rt.FillRoundedRectangle(&rrect(rect(PAD, PAD, PAD + asz, PAD + asz), 14.0), &ui.wa(0.12));
                    ui.glyph(&d.glyph_l, G_MUSIC, PAD, PAD, asz, asz, &ui.dim);
                }
            }
            ui.rt.PopLayer();
            ui.rt.SetTransform(&base);
        }
        ui.hit(rect(PAD, PAD, PAD + asz, PAD + asz), 4);

        // ---- Canción: entra deslizándose desde abajo al cambiar
        let lx = PAD + asz + 24.0;
        let lw = W - lx - PAD;
        let app = player_name(&m.app);
        let kt = ez(since_title, 0.0, 0.45);
        let ka = ez(since_title, 0.06, 0.45);
        ui.text(&ui.f.small, &i18n::tf("Reproduciendo en {}", &[&app]), lx, PAD - 4.0, 16.0, &accent);
        popup::push_alpha(ui.rt, kt);
        ui.text(&d.big, &trunc(&m.title, (lw / 12.5) as usize), lx, PAD + 12.0 + 10.0 * (1.0 - kt), 32.0, &ui.white);
        ui.rt_pop();
        popup::push_alpha(ui.rt, ka);
        ui.text(&ui.f.sub, &trunc(&m.artist, (lw / 6.8) as usize), lx, PAD + 44.0 + 10.0 * (1.0 - ka), 18.0, &ui.dim);
        ui.rt_pop();
        ui.hit(rect(lx, PAD - 4.0, W - PAD, PAD + 62.0), 4);

        // ---- Lista de reproducción: los próximos temas, en cascada al cambiar de canción
        let secs = media_secs(m.playing);
        let (top, bot) = (PAD + 74.0, PAD + asz);
        self.live_view = false;
        let _ = (&accent, &k);
        let next = upcoming(&m.title);
        if next.is_empty() {
            // Sin cola a la vista: de qué álbum es y cuánto queda, en grande (no un hueco vacío)
            let ka = ez(since_title, 0.1, 0.4);
            popup::push_alpha(ui.rt, ka);
            let mut y = top + 2.0 + 8.0 * (1.0 - ka);
            if !m.album.trim().is_empty() && m.album.trim() != m.title.trim() {
                ui.text(&ui.f.small, i18n::t("DEL ÁLBUM"), lx, y, 16.0, &ui.faint);
                ui.text(&ui.f.name, &trunc(&m.album, (lw / 7.5) as usize), lx, y + 18.0, 20.0, &ui.white);
                y += 52.0;
            }
            if let Some((p, e)) = secs {
                ui.text(&ui.f.small, i18n::t("QUEDAN"), lx, y, 16.0, &ui.faint);
                ui.text(&d.title, &live::mmss((e - p).max(0.0) as u32), lx, y + 18.0, 22.0, &ui.white);
                y += 52.0;
            }
            let msg = if deskdata::ytm_open() { i18n::t("Sin más temas en la cola") } else { i18n::t("La cola sale con YouTube Music") };
            ui.text(&ui.f.sub, msg, lx, y.min(bot - 18.0), 18.0, &ui.faint);
            ui.rt_pop();
        } else {
            ui.text(&ui.f.small, i18n::t("A CONTINUACIÓN"), lx, top + 2.0, 16.0, &ui.faint);
            let rh = 38.0;
            let fit = (((bot - top - 24.0) / rh) as usize).max(1);
            for (i, q) in next.iter().take(fit).enumerate() {
                let kq = ez(since_title, 0.12 + i as f32 * 0.07, 0.4);
                let y = top + 24.0 + i as f32 * rh + 10.0 * (1.0 - kq);
                // Al pasar el ratón la fila se ilumina, su texto se desplaza un poco y aparece el ▶; al
                // pulsarla, ese tema empieza a sonar
                let id = 200 + i as u32;
                let hk = ui.heat(id);
                let row = rect(lx - 8.0, y - 3.0, W - PAD + 8.0, y + rh - 3.0);
                popup::push_alpha(ui.rt, kq * if i == 0 { 1.0 } else { 0.85 + 0.15 * hk });
                ui.card_k(row, hk);
                let dw = ui.p.width(&ui.f.sub, &q.dur);
                let maxc = ((lw - dw - 16.0) / 7.2) as usize;
                let tx = lx + 14.0 * hk;
                if hk > 0.01 {
                    popup::push_alpha(ui.rt, hk);
                    ui.glyph(&ui.f.icon, G_PLAY, lx - 4.0, y, 14.0, 18.0, &ui.white);
                    ui.rt_pop();
                }
                ui.text(&ui.f.lbl, &trunc(&q.title, maxc), tx, y, 18.0, &ui.white);
                ui.text(&ui.f.sub, &trunc(&q.artists, maxc + 4), tx, y + 17.0, 16.0, &ui.dim);
                ui.text_r(&ui.f.sub, &q.dur, W - PAD, y, 18.0, &ui.dim);
                ui.hit(row, id);
                ui.rt_pop();
            }
        }

        // ---- Línea de tiempo y controles, a todo lo ancho
        let py = PAD + asz + 18.0;
        if let Some((p, e)) = secs {
            let fr = (p / e.max(1.0)).clamp(0.0, 1.0);
            // Al pasar el ratón la línea engorda y aparece su cabeza
            let hk = ui.heat(6);
            let th = 4.0 + 2.0 * hk;
            bar(ui, PAD, py - th / 2.0 + 2.0, W - PAD * 2.0, th, fr, &ui.white.clone());
            let hx = PAD + (W - PAD * 2.0) * fr;
            unsafe { ui.rt.FillEllipse(&ellipse(hx, py + 2.0, 2.0 + 5.0 * hk), &ui.white); }
            // Bajo el ratón: marca y minuto exacto al que se saltaría pulsando
            let (mx, _) = desk::mouse_at();
            if hk > 0.01 && mx >= PAD && mx <= W - PAD {
                let f2 = (mx - PAD) / (W - PAD * 2.0);
                let lab = live::mmss((f2 * e) as u32);
                let lw2 = ui.p.width(&ui.f.small, &lab) + 14.0;
                let bx = (mx - lw2 / 2.0).clamp(PAD, W - PAD - lw2);
                popup::push_alpha(ui.rt, hk);
                unsafe {
                    ui.rt.FillRectangle(&rect(mx - 0.75, py - 5.0, mx + 0.75, py + 9.0), &ui.wa(0.8));
                    ui.rt.FillRoundedRectangle(&rrect(rect(bx, py - 30.0, bx + lw2, py - 10.0), 10.0), &ui.chip());
                }
                ui.text_c(&ui.f.small, &lab, bx + lw2 / 2.0, py - 28.0, 16.0, &ui.white);
                ui.rt_pop();
            }
            self.seek_hover = hk > 0.01;
            ui.hit(rect(PAD, py - 8.0, W - PAD, py + 12.0), 6);
            ui.text(&ui.f.small, &live::mmss(p as u32), PAD, py + 10.0, 16.0, &ui.dim);
            ui.text_r(&ui.f.small, &live::mmss(e as u32), W - PAD, py + 10.0, 16.0, &ui.dim);
        }
        let cy = H - PAD - 18.0;
        let fm = &d.glyph_m.clone();
        round_btn(ui, fm, G_PREV, W / 2.0 - 64.0, cy, 20.0, 1, false);
        // El botón central late un instante al pulsarlo
        let pulse = self.pulse_at.map(|a| a.elapsed().as_secs_f32()).filter(|s| *s < 0.35).map(|s| (s / 0.35 * std::f32::consts::PI).sin()).unwrap_or(0.0);
        // Pausa ⇄ reproducir: un icono gira y se encoge mientras el otro entra
        approach(&mut self.play_k, if m.playing { 1.0 } else { 0.0 }, dt, 0.08);
        unsafe {
            let base = { let mut m0 = Matrix3x2::default(); ui.rt.GetTransform(&mut m0); m0 };
            ui.rt.SetTransform(&mul(&scale_at(1.0 - 0.12 * pulse, W / 2.0, cy), &base));
            round_btn(ui, fm, "", W / 2.0, cy, 22.0, 2, true);
            let inner = cur_transform(ui);
            let dark = ui.brush(rgba(0.08, 0.08, 0.09, 1.0));
            let press = 1.0 - 0.14 * desk::press(2);
            for (g, k) in [(G_PAUSE, self.play_k), (G_PLAY, 1.0 - self.play_k)] {
                if k <= 0.01 { continue; }
                let ang = (1.0 - k) * 1.2;
                let s = (0.55 + 0.45 * k) * press;
                let (c, sn) = (ang.cos() * s, ang.sin() * s);
                let (px, py2) = (W / 2.0, cy);
                let m2 = Matrix3x2 { M11: c, M12: sn, M21: -sn, M22: c, M31: px - c * px + sn * py2, M32: py2 - sn * px - c * py2 };
                ui.rt.SetTransform(&mul(&m2, &inner));
                popup::push_alpha(ui.rt, k);
                ui.glyph(fm, g, W / 2.0 - 22.0, cy - 22.0, 44.0, 44.0, &dark);
                ui.rt.PopLayer();
            }
            ui.rt.SetTransform(&base);
        }
        round_btn(ui, fm, G_NEXT, W / 2.0 + 64.0, cy, 20.0, 3, false);
    }
    fn click(&mut self, id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        match id? {
            1 => Some(Box::new(|| media(Cmd::Prev))),
            // Línea de tiempo: salta al punto pulsado (y la barra se mueve ya, sin esperar al reproductor)
            6 => {
                let (W, _) = size(Kind::Music);
                let f = ((desk::click_at().0 - PAD) / (W - PAD * 2.0)).clamp(0.0, 1.0);
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
        // Sonando, solo hace falta animar sin parar la letra (se ilumina) o el ecualizador; la cola es quieta
        t < 1.3 || self.shift.abs() > 0.5 || (self.playing && self.live_view) || self.tint != self.tint_to
            || self.art_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.6)
            || self.title_at.is_some_and(|a| a.elapsed().as_secs_f32() < 1.2)
            || self.pulse_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.4)
            || (self.play_k > 0.005 && self.play_k < 0.995)
    }
    fn follows_mouse(&self) -> bool { self.seek_hover }
    // Cambio de canción, de verso o de pausa a reproducir: a 60 fotogramas mientras dura
    fn smooth(&self) -> bool {
        self.art_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.6)
            || self.title_at.is_some_and(|a| a.elapsed().as_secs_f32() < 1.0)
            || self.pulse_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.4)
            || self.shift.abs() > 1.0
            || (self.play_k > 0.005 && self.play_k < 0.995)
    }
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
/// Posición (fracción del ancho útil) de cada una de las cuatro cifras del widget de sistema.
const SYS_XS: [f32; 4] = [0.0, 0.20, 0.58, 0.80];

/// `live`: medidas y último valor para pintar la gráfica encima de la capa fija; `layer_at`: muestra
/// con la que se pintó la capa.
struct SystemW {
    bars: [f32; 3], disp: [f32; 4], apps: HashMap<String, (f32, f32, f32, f32)>, moving: bool,
    graph: Option<(Option<windows::core::IUnknown>, Option<Instant>, ID2D1BitmapRenderTarget)>,
    live: Option<(f32, f32, f32, f32, f32, f32, Option<Instant>)>,
    layer_at: Option<Instant>,
    /// Sonda de la gráfica: opacidad, posición y si sigue al ratón
    probe_a: f32, probe_x: f32, probe_on: bool,
}

fn uptime_text(s: u64) -> String {
    let (d, h, m) = (s / 86400, s / 3600 % 24, s / 60 % 60);
    if d > 0 { i18n::tf("Encendido {} d {} h", &[&d, &h]) } else if h > 0 { i18n::tf("Encendido {} h {} min", &[&h, &m]) } else { i18n::tf("Encendido {} min", &[&m]) }
}

impl Widget for SystemW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (W, H) = size(Kind::System);
        let s = SYS.lock().unwrap().clone().unwrap_or_default();
        header(ui, d, i18n::t("Sistema"), &if s.uptime > 0 { uptime_text(s.uptime) } else { String::new() }, W);
        self.moving = false;
        // ---- Cuatro cifras grandes en fila: los rótulos van en la capa fija y las cifras, que cuentan
        // hacia el valor nuevo, se pintan encima en cada fotograma (`paint_live`)
        let y = PAD + 30.0;
        let iw = W - PAD * 2.0;
        let labels = [i18n::t("Procesador").to_string(), i18n::tf("Memoria · {} de {}", &[&fmt_bytes(s.ram_used), &fmt_bytes(s.ram_total)]), i18n::t("Bajada").into(), i18n::t("Subida").into()];
        for (i, l) in labels.iter().enumerate() {
            ui.text(&ui.f.small, &trunc(l, 32), PAD + iw * SYS_XS[i], y + 30.0, 16.0, &ui.dim);
        }
        let _ = (dt, d);

        // ---- Gráfica del último minuto (se desliza con suavidad entre muestras)
        let (gt, gb) = (PAD + 96.0, PAD + 212.0);
        let gw = W - PAD * 2.0;
        for i in 0..4 {
            let yy = gt + (gb - gt) * i as f32 / 3.0;
            unsafe { ui.rt.FillRectangle(&rect(PAD, yy, W - PAD, yy + 1.0 / ui.s), &ui.wa(0.06)); }
        }
        let frac = s.at.map(|a| (a.elapsed().as_secs_f32() / 1.0).min(1.0)).unwrap_or(0.0);
        let step = gw / (HIST - 2) as f32;
        let k = ez(t, 0.1, 0.8);
        let to_pts = |v: &[f32]| -> Vec<(f32, f32)> {
            let off = HIST - v.len();
            v.iter().enumerate().map(|(i, x)| (PAD + (i + off) as f32 * step - frac * step - step, gb - (x / 100.0).clamp(0.0, 1.0) * (gb - gt) * k)).collect()
        };
        // Las curvas se dibujan en una imagen solo cuando llega una muestra nueva (una vez por segundo);
        // entre una y otra, la imagen se desliza entera, que a la GPU no le cuesta nada
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
        let _ = (frac, &to_pts);
        self.live = Some((gt, gb, step, bw, bh, s.cpu.last().copied().unwrap_or(0.0) * k, s.at));
        self.layer_at = s.at;

        // Leyenda bajo la gráfica
        unsafe {
            ui.rt.FillRoundedRectangle(&rrect(rect(PAD, gb + 13.0, PAD + 12.0, gb + 16.0), 1.5), &ui.white);
            ui.rt.FillRoundedRectangle(&rrect(rect(PAD + 66.0, gb + 13.0, PAD + 78.0, gb + 16.0), 1.5), &ui.wa(0.45));
        }
        ui.text(&ui.f.small, "CPU", PAD + 18.0, gb + 6.0, 16.0, &ui.dim);
        ui.text(&ui.f.small, i18n::t("Memoria"), PAD + 84.0, gb + 6.0, 16.0, &ui.dim);
        ui.text_r(&ui.f.small, i18n::t("Último minuto"), W - PAD, gb + 6.0, 16.0, &ui.faint);

        // ---- Abajo: red a la izquierda, apps que más gastan a la derecha
        let by = gb + 34.0;
        let half = (W - PAD * 2.0 - 24.0) / 2.0;
        ui.glyph(&ui.f.icon, G_DOWN, PAD - 2.0, by - 2.0, 16.0, 18.0, &ui.dim);
        ui.text(&ui.f.small, i18n::t("RED"), PAD + 16.0, by - 2.0, 18.0, &ui.faint);
        let _ = G_UP;
        let nb = 30usize;
        let vals: Vec<f32> = s.down.iter().rev().take(nb).rev().cloned().collect();
        let mx = vals.iter().cloned().fold(50_000.0f32, f32::max) * 1.15;
        let (bt, bb) = (by + 22.0, H - PAD - 4.0);
        let bw = half / nb as f32;
        for (i, v) in vals.iter().enumerate() {
            let x = PAD + (i + nb - vals.len()) as f32 * bw;
            let h = (v / mx * (bb - bt)).max(2.0);
            let last = i == vals.len() - 1;
            unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(x + 1.0, bb - h, x + bw - 1.5, bb), 1.5), &ui.wa(if last { 0.95 } else { 0.42 })); }
        }
        let ax = PAD + half + 24.0;
        ui.text(&ui.f.small, i18n::t("LO QUE MÁS GASTA"), ax, by - 2.0, 18.0, &ui.faint);
        // Cada app conserva su fila animada: si cambia el orden, se deslizan a su nuevo puesto; las que
        // entran aparecen y las que salen se desvanecen
        let rank: Vec<(String, f32)> = s.top.iter().take(3).cloned().collect();
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
        for name in names {
            let (y, b, a, pct) = self.apps[&name];
            popup::push_alpha(ui.rt, a);
            ui.text(&ui.f.lbl, &trunc(&name, 14), ax, y, 18.0, &ui.white);
            bar(ui, ax + 112.0, y + 7.0, half - 150.0, 4.0, b, &ui.wa(0.8));
            ui.text_r(&ui.f.lbl, &format!("{pct:.0}%"), W - PAD, y, 18.0, &ui.dim);
            ui.rt_pop();
        }
        let _ = &self.bars;
        ui.hit(rect(0.0, 0.0, W, H), 1);
    }
    fn click(&mut self, id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        // Abre el Administrador de tareas
        if id == Some(1) { return Some(Box::new(|| sys::open("taskmgr.exe"))); }
        None
    }
    fn animating(&self, _t: f32) -> bool { true }
    // Con las curvas ya en una imagen, deslizarla cuesta poco: va a 60 fotogramas
    fn frame_ms(&self) -> u32 { 15 }
    fn layered(&self) -> bool { true }
    // La capa fija (cifras, rótulos, red, apps) se rehace al llegar cada muestra o mientras algo se anima
    fn layer_stale(&self) -> bool { self.moving || SYS.lock().unwrap().as_ref().map(|s| s.at) != Some(self.layer_at) }
    fn paint_live(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (W, _) = size(Kind::System);
        // Cifras grandes: cuentan hacia el valor nuevo en vez de saltar (al aparecer, desde cero)
        let real = SYS.lock().unwrap().as_ref().map(|s| [s.cpu.last().copied().unwrap_or(0.0), s.ram.last().copied().unwrap_or(0.0), s.down.last().copied().unwrap_or(0.0), s.up.last().copied().unwrap_or(0.0)]).unwrap_or([0.0; 4]);
        let mut counting = false;
        for i in 0..4 { counting |= approach(&mut self.disp[i], real[i], dt, if t < 1.0 { 0.25 } else { 0.35 }); }
        let [cpu, ram, down, up] = self.disp;
        let vals = [(format!("{cpu:.0}%"), true), (format!("{ram:.0}%"), true), (fmt_rate(down), false), (fmt_rate(up), false)];
        let iw = W - PAD * 2.0;
        for (i, (v, big)) in vals.iter().enumerate() {
            ui.text(if *big { &d.big } else { &d.title }, v, PAD + iw * SYS_XS[i], PAD + 30.0 + if *big { 0.0 } else { 4.0 }, 30.0, &ui.white);
        }
        let _ = counting;
        let Some((gt, gb, step, bw, bh, last, at)) = self.live else { return };
        const M: f32 = 10.0;
        let frac = at.map(|a| a.elapsed().as_secs_f32().min(1.0)).unwrap_or(0.0);
        unsafe {
            ui.rt.PushAxisAlignedClip(&rect(PAD, gt - 6.0, W - PAD, gb + 2.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            if let Some((_, _, bt)) = &self.graph {
                if let Ok(bmp) = bt.GetBitmap() {
                    let x0 = PAD - frac * step - step - M;
                    ui.rt.DrawBitmap(&bmp, Some(&rect(x0, gt - M, x0 + bw, gt - M + bh)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                }
            }
            // Punto vivo en la cabeza de la CPU: con cada muestra nueva suelta una onda que se abre y se apaga
            let x = PAD + (HIST - 1) as f32 * step - frac * step - step;
            let y = gb - (last / 100.0).clamp(0.0, 1.0) * (gb - gt);
            ui.rt.FillEllipse(&ellipse(x, y, 3.5 + 9.0 * popup::ease_out(frac)), &ui.wa(0.30 * (1.0 - frac)));
            ui.rt.FillEllipse(&ellipse(x, y, 3.5), &ui.white);
            ui.rt.PopAxisAlignedClip();
        }
        // Sonda: con el ratón sobre la gráfica, una línea sigue al cursor con la CPU y la memoria de ese segundo
        let (mx, my) = desk::mouse_at();
        let inside = mx >= PAD && mx <= W - PAD && my >= gt - 6.0 && my <= gb + 2.0;
        approach(&mut self.probe_a, if inside { 1.0 } else { 0.0 }, dt, 0.08);
        self.probe_on = inside || self.probe_a > 0.01;
        if self.probe_a > 0.01 {
            let data = SYS.lock().unwrap().clone().unwrap_or_default();
            let n = data.cpu.len();
            if n >= 2 {
                if inside { self.probe_x = mx; }
                let off = HIST - n;
                let x_of = |i: usize| PAD + (i + off) as f32 * step - frac * step - step;
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
                let lx = (px - lw / 2.0).clamp(PAD - 4.0, W - PAD - lw + 4.0);
                popup::push_alpha(ui.rt, a);
                unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(lx, gt - 30.0, lx + lw, gt - 10.0), 10.0), &ui.chip()); }
                ui.text_c(&ui.f.small, &lab, lx + lw / 2.0, gt - 28.0, 16.0, &ui.white);
                ui.rt_pop();
            }
        }
    }
    fn follows_mouse(&self) -> bool { self.probe_on }
}

// ---------------------------------------------------------------- almacenamiento y papelera
/// `fills`: relleno que se ve de cada disco (va hacia el real); `shake`: sacudida de la papelera
/// cuando cambia lo que tiene.
#[derive(Default)]
struct StorageW { fills: Vec<f32>, r_bin: Roll, r_free: Vec<Roll>, last_items: Option<u64>, shake: Option<Instant>, moving: bool }

impl Widget for StorageW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (W, H) = size(Kind::Storage);
        let st = STORE.lock().unwrap().clone();
        let n = st.as_ref().map(|s| s.drives.len()).unwrap_or(0);
        header(ui, d, i18n::t("Almacenamiento"), &match n { 0 => String::new(), 1 => i18n::t("1 unidad").into(), n => i18n::tf("{} unidades", &[&n]) }, W);
        let Some(st) = st else { return };
        let mut y = PAD + 36.0;
        self.fills.resize(st.drives.len().min(3), 0.0);
        self.r_free.resize_with(st.drives.len().min(3), Roll::default);
        self.moving = false;
        for (i, dr) in st.drives.iter().take(3).enumerate() {
            let used = dr.total - dr.free;
            let frac = used as f32 / dr.total as f32;
            let letter = dr.root.trim_end_matches('\\');
            let name = if dr.label.is_empty() { i18n::tf("Disco ({})", &[&letter]) } else { format!("{} ({letter})", dr.label) };
            let id = 40 + i as u32;
            ui.card_k(rect(PAD - 8.0, y - 4.0, W - PAD + 8.0, y + 44.0), ui.heat(id));
            let hk = ui.heat(id);
            ui.glyph(&ui.f.icon_l, G_DRIVE, PAD + 3.0 * hk, y, 20.0, 20.0, &ui.wa(0.72 + 0.25 * hk));
            ui.text(&ui.f.name, &trunc(&name, 22), PAD + 28.0 + 4.0 * hk, y, 20.0, &ui.white);
            let dim = ui.dim.clone();
            let fsub = ui.f.sub.clone();
            self.r_free[i].draw(ui, &fsub, &i18n::tf("{} libres de {}", &[&fmt_bytes(dr.free), &fmt_bytes(dr.total)]), W - PAD, y, 20.0, &dim, true);
            // Al aparecer se llena en cascada; después sigue al espacio real con suavidad
            let k = ez(t, 0.15 + i as f32 * 0.12, 0.9);
            if t < 1.1 { self.fills[i] = frac * k; } else { self.moving |= approach(&mut self.fills[i], frac, dt, 0.4); }
            let fill = if frac >= 0.9 { ui.brush(rgba(1.0, 0.62, 0.26, 1.0)) } else { ui.white.clone() };
            let th = 8.0 + 3.0 * hk;
            bar(ui, PAD, y + 32.0 - th / 2.0, W - PAD * 2.0, th, self.fills[i], &fill);
            if hk > 0.01 {
                // Al pasar el ratón: cuánto está lleno, al final del relleno
                let lab = i18n::tf("{} % usado", &[&format!("{:.0}", frac * 100.0)]);
                let lw = ui.p.width(&ui.f.small, &lab) + 14.0;
                let ex = PAD + (W - PAD * 2.0) * self.fills[i];
                let lx = (ex - lw / 2.0).clamp(PAD, W - PAD - lw);
                popup::push_alpha(ui.rt, hk);
                unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(lx, y + 38.0, lx + lw, y + 54.0), 8.0), &ui.chip()); }
                ui.text_c(&ui.f.small, &lab, lx + lw / 2.0, y + 38.0, 16.0, &ui.white);
                ui.rt_pop();
            }
            ui.hit(rect(PAD - 8.0, y - 4.0, W - PAD + 8.0, y + 44.0), id);
            y += 56.0;
        }
        // ---- Papelera
        let by = y + 6.0;
        ui.sep(by - 10.0);
        // La papelera se sacude cuando cambia lo que tiene (se tira algo o se vacía)
        if self.last_items.is_some_and(|l| l != st.bin_items) { self.shake = Some(Instant::now()); }
        self.last_items = Some(st.bin_items);
        let sh = self.shake.map(|s| s.elapsed().as_secs_f32()).filter(|s| *s < 0.7);
        unsafe {
            ui.rt.FillRoundedRectangle(&rrect(rect(PAD, by + 4.0, PAD + 44.0, by + 48.0), 12.0), &ui.wa(0.09));
            let base = cur_transform(ui);
            if let Some(s) = sh {
                // Balanceo amortiguado alrededor de la base del cubo
                let ang = 0.28 * (s * 26.0).sin() * (1.0 - s / 0.7).powi(2);
                let (c, sn) = (ang.cos(), ang.sin());
                let (px, py) = (PAD + 22.0, by + 40.0);
                let rot = Matrix3x2 { M11: c, M12: sn, M21: -sn, M22: c, M31: px - c * px + sn * py, M32: py - sn * px - c * py };
                ui.rt.SetTransform(&mul(&rot, &base));
            }
            ui.glyph(&d.glyph_m, G_TRASH, PAD, by + 4.0, 44.0, 44.0, &ui.white);
            ui.rt.SetTransform(&base);
        }
        ui.text(&ui.f.name, i18n::t("Papelera"), PAD + 56.0, by + 6.0, 20.0, &ui.white);
        let sub = if st.bin_items == 0 { i18n::t("Vacía").to_string() } else if st.bin_items == 1 { i18n::tf("1 elemento · {}", &[&fmt_bytes(st.bin_size)]) } else { i18n::tf("{} elementos · {}", &[&st.bin_items, &fmt_bytes(st.bin_size)]) };
        let dim = ui.dim.clone();
        let fsub = ui.f.sub.clone();
        self.r_bin.draw(ui, &fsub, &sub, PAD + 56.0, by + 28.0, 18.0, &dim, false);
        pbutton(ui, rect(W - PAD - 158.0, by + 12.0, W - PAD - 88.0, by + 42.0), None, i18n::t("Abrir"), 10, false);
        // Vaciar en rojo suave (Windows pide confirmación antes de borrar)
        let r = rect(W - PAD - 80.0, by + 12.0, W - PAD, by + 42.0);
        let k = ui.heat(11);
        unsafe { ui.rt.FillRoundedRectangle(&rrect(r, 10.0), &ui.brush(rgba(1.0, 0.35, 0.35, 0.12 + 0.10 * k))); }
        let red = ui.brush(rgba(1.0, 0.55, 0.55, if st.bin_items == 0 { 0.4 } else { 1.0 }));
        ui.text_c(&ui.f.lbl, i18n::t("Vaciar…"), (r.left + r.right) / 2.0, r.top, r.bottom - r.top, &red);
        if st.bin_items > 0 { ui.hit(r, 11); }
        // ---- Lo más grande de la papelera (si tiene algo)
        let mut fy = by + 62.0;
        if !st.bin_big.is_empty() {
            ui.text(&ui.f.small, i18n::t("LO MÁS GRANDE"), PAD, fy, 16.0, &ui.faint);
            for (i, (name, size)) in st.bin_big.iter().enumerate() {
                let yy = fy + 20.0 + i as f32 * 20.0;
                if yy > H - PAD - 16.0 { break; }
                let k = ez(t, 0.4 + i as f32 * 0.06, 0.35);
                popup::push_alpha(ui.rt, k);
                ui.text(&ui.f.sub, &trunc(name, 34), PAD + 6.0 * (1.0 - k), yy, 18.0, &ui.dim);
                ui.text_r(&ui.f.sub, &fmt_bytes(*size), W - PAD, yy, 18.0, &ui.dim);
                ui.rt_pop();
            }
            fy += 20.0 + st.bin_big.len().min(3) as f32 * 20.0 + 12.0;
        }
        // ---- Liberar espacio (Sensor de almacenamiento de Windows), justo debajo: sin huecos vacíos
        if fy + 34.0 <= H - PAD + 4.0 {
            let k = ui.heat(12);
            let r = rect(PAD - 8.0, fy - 4.0, W - PAD + 8.0, fy + 34.0);
            ui.card_k(r, k);
            ui.glyph(&ui.f.icon_l, "\u{E7F8}", PAD, fy, 20.0, 30.0, &ui.dim);
            let free: u64 = st.drives.iter().map(|d| d.free).sum();
            ui.text(&ui.f.lbl, i18n::t("Liberar espacio"), PAD + 28.0, fy, 16.0, &ui.white);
            ui.text(&ui.f.sub, &i18n::tf("{} libres en total · Sensor de almacenamiento", &[&fmt_bytes(free)]), PAD + 28.0, fy + 16.0, 16.0, &ui.dim);
            ui.glyph(&ui.f.icon, G_RIGHT, W - PAD - 16.0, fy, 16.0, 30.0, &ui.wa(0.35 + 0.6 * k));
            ui.hit(r, 12);
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
}

// ---------------------------------------------------------------- proyectos
/// `tab_at`: cuándo se cambió de pestaña (las filas vuelven a entrar en cascada); `pill`: posición
/// animada del resaltado de la pestaña (0 = En local, 1 = GitHub).
struct ProjectsW { tab: u8, scroll: usize, tab_at: Option<Instant>, pill: f32 }

const ROW_H: f32 = 44.0;
/// Filas que caben en el widget de proyectos.
fn rows_fit() -> usize { ((size(Kind::Projects).1 - PAD - 34.0 - 40.0) / ROW_H) as usize }

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

impl Widget for ProjectsW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (W, H) = size(Kind::Projects);
        let ROWS = rows_fit();
        let p = PROJ.lock().unwrap().clone();
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
        // Tras cambiar de pestaña, las filas vuelven a entrar en cascada
        let t = self.tab_at.map(|a| a.elapsed().as_secs_f32()).map_or(t, |s| s.min(t));
        let top = PAD + 34.0;
        let rows: Vec<(String, String, String, [f32; 3], bool)> = if self.tab == 0 {
            p.local.iter().map(|r| {
                let mut sub = String::new();
                if !r.lang.is_empty() { sub += r.lang; sub += " · "; }
                sub += &r.branch;
                match r.dirty { Some(0) => sub += i18n::t(" · limpio"), Some(1) => sub += i18n::t(" · 1 cambio"), Some(n) => sub += &i18n::tf(" · {} cambios", &[&n]), None => {} }
                (r.name.clone(), sub, ago(r.modified), r.color, false)
            }).collect()
        } else {
            p.gh.iter().map(|g| {
                let mut sub = if g.private { i18n::t("Privado").to_string() } else { i18n::t("Público").to_string() };
                if !g.lang.is_empty() { sub += " · "; sub += &g.lang; }
                if g.stars > 0 { sub += &format!(" · {} ★", g.stars); }
                (g.name.clone(), sub, ago(g.pushed), g.color, g.private)
            }).collect()
        };
        self.scroll = self.scroll.min(rows.len().saturating_sub(ROWS));
        if rows.is_empty() {
            let msg = if self.tab == 0 {
                i18n::t(if p.scanned { "No hay proyectos con git en el Escritorio ni en las carpetas habituales" } else { "Buscando proyectos…" })
            } else {
                i18n::t(match p.gh_ok { None => "Cargando tus repos de GitHub…", Some(false) => "Instala GitHub CLI e inicia sesión: gh auth login", Some(true) => "No tienes repos en GitHub" })
            };
            ui.text_c(&ui.f.sub, msg, W / 2.0, H / 2.0 - 10.0, 20.0, &ui.dim);
        }
        for (vi, (i, (name, sub, when, col, private))) in rows.iter().enumerate().skip(self.scroll).take(ROWS).enumerate() {
            let k = ez(t, 0.1 + vi as f32 * 0.05, 0.35);
            let y = top + vi as f32 * ROW_H + 8.0 * (1.0 - k);
            let id = 100 + i as u32;
            popup::push_alpha(ui.rt, k);
            // La fila sigue encendida mientras el ratón está en ella o en sus botones
            let qa = 500 + i as u32 * 4;
            let hk = (0..3).map(|j| ui.heat(qa + j)).fold(ui.heat(id), f32::max);
            ui.card_k(rect(PAD - 8.0, y, W - PAD + 8.0, y + ROW_H - 4.0), hk);
            // Al pasar el ratón el punto de color crece y el texto se desplaza un poco
            let tx = PAD + 18.0 + 4.0 * hk;
            unsafe { ui.rt.FillEllipse(&ellipse(PAD + 4.0, y + 20.0, 5.0 + 1.5 * hk), &ui.brush(rgba(col[0], col[1], col[2], 1.0))); }
            ui.text(&ui.f.name, &trunc(name, 24), tx, y + 3.0, 20.0, &ui.white);
            if *private { let nw = ui.p.width(&ui.f.name, &trunc(name, 24)); ui.glyph(&ui.f.icon, G_LOCK, tx + 4.0 + nw, y + 5.0, 14.0, 16.0, &ui.faint); }
            ui.text(&ui.f.sub, &trunc(sub, ((W - PAD * 2.0 - 120.0) / 6.0) as usize), tx, y + 22.0, 16.0, &ui.dim);
            // Fecha e icono se apartan para dejar sitio a las acciones rápidas
            popup::push_alpha(ui.rt, 1.0 - hk);
            ui.text_r(&ui.f.small, when, W - PAD - 26.0 + 10.0 * hk, y + 4.0, 18.0, &ui.faint);
            ui.glyph(&ui.f.icon, if self.tab == 0 { G_FOLDER } else { G_OPEN }, W - PAD - 18.0, y + 4.0, 18.0, 18.0, &ui.wa(0.35));
            ui.rt_pop();
            ui.hit(rect(PAD - 8.0, y, W - PAD + 8.0, y + ROW_H - 4.0), id);
            if hk > 0.01 {
                let acts: [&str; 3] = if self.tab == 0 { [G_FOLDER, G_CODE, G_TERM] } else { [G_OPEN, G_ISSUE, G_COPY] };
                let fi = ui.f.icon.clone();
                for (j, g) in acts.iter().enumerate() {
                    // Entran en cascada desde la derecha
                    let kj = ((hk - j as f32 * 0.12) / 0.76).clamp(0.0, 1.0);
                    let cx = W - PAD - 12.0 - (2 - j) as f32 * 30.0 + 8.0 * (1.0 - kj);
                    popup::push_alpha(ui.rt, kj);
                    round_btn(ui, &fi, g, cx, y + 19.0, 13.0, qa + j as u32, false);
                    ui.rt_pop();
                }
            }
            ui.rt_pop();
        }
        // Pie: cuántos hay y cómo se usa
        if rows.len() > ROWS {
            ui.text(&ui.f.small, &i18n::tf("{}–{} de {}", &[&(self.scroll + 1), &(self.scroll + ROWS).min(rows.len()), &rows.len()]), PAD, H - PAD - 14.0, 16.0, &ui.faint);
        }
        ui.text_r(&ui.f.small, i18n::t("Clic derecho: más opciones"), W - PAD, H - PAD - 14.0, 16.0, &ui.faint);
        let _ = (G_STAR, G_PIN);
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
        if id < 100 { return None; }
        // Acciones rápidas de una fila (500 + fila·4 + botón)
        if id >= 500 {
            let (i, j) = (((id - 500) / 4) as usize, (id - 500) % 4);
            let p = PROJ.lock().unwrap().clone();
            if self.tab == 0 {
                let r = p.local.get(i)?.clone();
                return Some(Box::new(move || match j { 0 => sys::open(&r.path), 1 => open_vscode(&r.path), _ => open_terminal(r.path.clone()) }));
            }
            let g = p.gh.get(i)?.clone();
            return Some(Box::new(move || match j { 0 => sys::open(&g.url), 1 => sys::open(&format!("{}/issues", g.url)), _ => copy_text(&g.url) }));
        }
        let i = (id - 100) as usize;
        let p = PROJ.lock().unwrap().clone();
        if self.tab == 0 {
            let r = p.local.get(i)?.clone();
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
            let g = p.gh.get(i)?.clone();
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
    fn wheel(&mut self, d: i32) -> bool {
        if d < 0 { self.scroll += 1; } else { self.scroll = self.scroll.saturating_sub(1); }
        true
    }
    fn animating(&self, t: f32) -> bool {
        t < 1.0 || (self.pill - self.tab as f32).abs() > 0.002 || self.tab_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.9)
    }
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
/// `born`: cuándo apareció cada aviso (los nuevos entran deslizándose); `seen`: si ya se pintó una vez
/// (los que había al abrir entran en la cascada normal, no como "nuevos").
#[derive(Default)]
struct PhoneW { born: HashMap<u32, Instant>, seen: bool, moving: bool, bmps: HashMap<String, ID2D1Bitmap>, bmp_rt: crate::RtKey, sync_at: Option<Instant> }

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
}

const G_PHONE: &str = "\u{E8EA}";
const G_CHAT: &str = "\u{E8BD}";
const G_CALL: &str = "\u{E717}";
const G_PHOTO: &str = "\u{E91B}";
const G_BELL: &str = "\u{EA8F}";
const G_CLOSE: &str = "\u{E711}";
const NROW: f32 = 66.0;

/// Corta un texto en dos líneas de `n` caracteres como mucho (por palabras).
fn two_lines(s: &str, n: usize) -> (String, String) {
    if s.chars().count() <= n { return (s.to_string(), String::new()); }
    let words: Vec<&str> = s.split_whitespace().collect();
    let (mut a, mut used) = (String::new(), 0);
    for w in &words {
        let extra = if a.is_empty() { 0 } else { 1 };
        if a.chars().count() + w.chars().count() + extra > n { break; }
        if extra == 1 { a.push(' '); }
        a.push_str(w);
        used += 1;
    }
    if used == 0 { return (trunc(s, n), String::new()); }
    (a, trunc(&words[used..].join(" "), n))
}
impl Widget for PhoneW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, _dt: f32) {
        let (W, H) = size(Kind::Phone);
        let st = crate::phone::PHONE.lock().unwrap().clone();
        self.moving = false;

        // ---- Cabecera como la de Enlace Móvil: el móvil con su fondo, nombre, estado y batería
        let (fx, fy, fw, fh) = (PAD, PAD, 66.0, 138.0);
        let k0 = ez(t, 0.0, 0.5);
        let hk = ui.heat(14);
        let base = cur_transform(ui);
        unsafe { ui.rt.SetTransform(&mul(&scale_at((0.94 + 0.06 * k0) * (1.0 + 0.03 * hk), fx + fw / 2.0, fy + fh / 2.0), &base)); }
        popup::push_alpha(ui.rt, k0);
        let fr = rrect(rect(fx, fy, fx + fw, fy + fh), 11.0);
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
        unsafe {
            ui.rt.DrawRoundedRectangle(&fr, &ui.wa(0.55), 2.0, None);
            // Cámara arriba, como en el dibujo de Enlace Móvil
            ui.rt.FillEllipse(&ellipse(fx + fw / 2.0, fy + 7.0, 2.2), &ui.brush(rgba(0.0, 0.0, 0.0, 0.7)));
        }
        ui.rt_pop();
        unsafe { ui.rt.SetTransform(&base); }
        ui.hit(rect(fx, fy, fx + fw, fy + fh), 14);

        let x0 = fx + fw + 18.0;
        let k1 = ez(t, 0.08, 0.45);
        popup::push_alpha(ui.rt, k1);
        let name = if st.name.is_empty() { i18n::t("Tu móvil").to_string() } else { st.name.clone() };
        ui.text(&d.title, &trunc(&name, 20), x0 + 8.0 * (1.0 - k1), fy + 4.0, 26.0, &ui.white);
        // Iconos de estado: conexión y batería (la batería solo si se conoce)
        let iy = fy + 36.0;
        let mut ix = x0;
        let on = if st.running { ui.white.clone() } else { ui.wa(0.35) };
        ui.glyph(&ui.f.icon, "\u{E701}", ix, iy, 18.0, 22.0, &on);
        ix += 26.0;
        if let Some((pct, _)) = st.battery {
            // Pila dibujada: el relleno es la carga
            unsafe {
                let r = rect(ix, iy + 6.0, ix + 22.0, iy + 16.0);
                ui.rt.DrawRoundedRectangle(&rrect(r, 2.5), &ui.white, 1.4, None);
                ui.rt.FillRectangle(&rect(ix + 22.5, iy + 9.0, ix + 24.5, iy + 13.0), &ui.white);
                let fillc = if pct <= 15 { ui.brush(rgba(1.0, 0.45, 0.4, 1.0)) } else { ui.white.clone() };
                ui.rt.FillRoundedRectangle(&rrect(rect(ix + 2.0, iy + 8.0, ix + 2.0 + 18.0 * pct as f32 / 100.0, iy + 14.0), 1.2), &fillc);
            }
            ui.text(&ui.f.name, &format!("{pct} %"), ix + 32.0, iy, 22.0, &ui.white);
        }
        // Estado: "Conectado" con su marca y, a la derecha, actualizar
        let py = fy + 72.0;
        let pr = rect(x0, py, W - PAD, py + 40.0);
        unsafe { ui.rt.FillRoundedRectangle(&rrect(pr, 10.0), &ui.wa(0.08)); }
        let (okc, okg) = if st.running { (rgba(0.33, 0.78, 0.42, 1.0), "\u{E73E}") } else { (ui.ic(0.3), "\u{E711}") };
        unsafe { ui.rt.FillEllipse(&ellipse(x0 + 22.0, py + 20.0, 10.0), &ui.brush(okc)); }
        ui.glyph(&ui.f.small, okg, x0 + 12.0, py + 10.0, 20.0, 20.0, &ui.brush(rgba(0.0, 0.0, 0.0, 0.85)));
        ui.text(&ui.f.name, if st.running { i18n::t("Conectado") } else { i18n::t("Sin conectar") }, x0 + 40.0, py, 40.0, &ui.white);
        unsafe { ui.rt.FillRectangle(&rect(W - PAD - 44.0, py + 8.0, W - PAD - 43.0, py + 32.0), &ui.wa(0.15)); }
        // El botón de actualizar gira un poco mientras se busca de nuevo
        let spin = self.sync_at.map(|s| s.elapsed().as_secs_f32()).filter(|s| *s < 0.8);
        let (scx, scy) = (W - PAD - 22.0, py + 20.0);
        if let Some(s) = spin {
            self.moving = true;
            let a = popup::ease_out(s / 0.8) * std::f32::consts::TAU;
            let (c, sn) = (a.cos(), a.sin());
            unsafe { ui.rt.SetTransform(&mul(&Matrix3x2 { M11: c, M12: sn, M21: -sn, M22: c, M31: scx - c * scx + sn * scy, M32: scy - sn * scx - c * scy }, &base)); }
        }
        round_btn(ui, &ui.f.icon.clone(), "\u{E72C}", scx, scy, 16.0, 15, false);
        unsafe { ui.rt.SetTransform(&base); }
        let sub = match st.battery { Some((_, when)) if !st.running || deskdata::now_unix().saturating_sub(when) > 120 => i18n::tf("Batería vista {}", &[&ago(when)]), _ => String::new() };
        if !sub.is_empty() { ui.text(&ui.f.small, &sub, x0 + 2.0, py + 44.0, 18.0, &ui.faint); }
        ui.rt_pop();

        // ---- Accesos: botones anchos como los de Enlace Móvil
        let ay = fy + fh + 16.0;
        let gap = 8.0;
        let bw = (W - PAD * 2.0 - gap * 3.0) / 4.0;
        for (i, g) in [G_CHAT, G_CALL, G_PHOTO, G_OPEN].iter().enumerate() {
            let id = 10 + i as u32;
            let k = ez(t, 0.15 + i as f32 * 0.05, 0.4);
            let hk = ui.heat(id);
            let p = desk::press(id);
            let x = PAD + i as f32 * (bw + gap);
            let r = rect(x, ay + 6.0 * (1.0 - k), x + bw, ay + 44.0 + 6.0 * (1.0 - k));
            let base = cur_transform(ui);
            if p > 0.0 { unsafe { ui.rt.SetTransform(&mul(&scale_at(1.0 - 0.06 * p, x + bw / 2.0, ay + 22.0), &base)); } }
            popup::push_alpha(ui.rt, k);
            unsafe {
                ui.rt.FillRoundedRectangle(&rrect(r, 9.0), &ui.wa(0.08 + 0.08 * hk));
                ui.rt.DrawRoundedRectangle(&rrect(r, 9.0), &ui.wa(0.10), 1.0, None);
            }
            ui.glyph(&ui.f.icon_l, g, r.left, r.top, bw, 44.0, &ui.white);
            ui.hit(r, id);
            ui.rt_pop();
            if p > 0.0 { unsafe { ui.rt.SetTransform(&base); } }
        }

        // ---- Fotos recientes (las 3 últimas de la cámara, capturas e Imágenes del móvil)
        let mut ny = ay + 44.0 + 24.0;
        if !st.photos.is_empty() {
            ui.sep(ny - 8.0);
            ui.text(&ui.f.small, i18n::t("FOTOS RECIENTES"), PAD, ny, 18.0, &ui.faint);
            let gap = 8.0;
            let side = (W - PAD * 2.0 - gap * 2.0) / 3.0;
            let py = ny + 24.0;
            for (i, ph) in st.photos.iter().enumerate() {
                let id = 30 + i as u32;
                let k = ez(t, 0.22 + i as f32 * 0.07, 0.45);
                let hk = ui.heat(id);
                let x = PAD + i as f32 * (side + gap);
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
            ny = py + side + 22.0;
        }

        // ---- Notificaciones del móvil
        ui.sep(ny - 8.0);
        ui.text(&ui.f.small, i18n::t("NOTIFICACIONES"), PAD, ny, 18.0, &ui.faint);
        if !st.notifs.is_empty() { ui.text_r(&ui.f.small, &st.notifs.len().to_string(), W - PAD, ny, 18.0, &ui.faint); }
        let ly = ny + 26.0;
        if st.ready && !st.access {
            ui.glyph(&d.glyph_m, G_BELL, 0.0, ly + 30.0, W, 50.0, &ui.wa(0.4));
            ui.text_c(&ui.f.sub, i18n::t("Para ver aquí los avisos del móvil,"), W / 2.0, ly + 88.0, 18.0, &ui.dim);
            ui.text_c(&ui.f.sub, i18n::t("macbar necesita leer las notificaciones"), W / 2.0, ly + 106.0, 18.0, &ui.dim);
            pbutton(ui, rect(W / 2.0 - 70.0, ly + 136.0, W / 2.0 + 70.0, ly + 168.0), None, i18n::t("Dar permiso"), 20, false);
            return;
        }
        if st.ready && st.notifs.is_empty() {
            let k = ez(t, 0.3, 0.5);
            popup::push_alpha(ui.rt, k);
            if st.unread > 0 {
                // Enlace Móvil cuenta avisos pero no los pasa a Windows (banners apagados): al menos el número
                let r = rect(PAD - 8.0, ly + 20.0, W - PAD + 8.0, ly + 150.0);
                ui.card_k(r, ui.heat(16));
                ui.glyph(&d.glyph_m, G_BELL, 0.0, ly + 30.0, W, 50.0, &ui.wa(0.75));
                unsafe { ui.rt.FillEllipse(&ellipse(W / 2.0 + 16.0, ly + 38.0, 11.0), &ui.on); }
                ui.text_c(&ui.f.small, &st.unread.to_string(), W / 2.0 + 16.0, ly + 29.0, 18.0, &ui.brush(rgba(1.0, 1.0, 1.0, 1.0)));
                let n = st.unread.to_string();
                ui.text_c(&ui.f.lbl, &if st.unread == 1 { i18n::t("1 notificación en Enlace Móvil").to_string() } else { i18n::tf("{} notificaciones en Enlace Móvil", &[&n]) }, W / 2.0, ly + 88.0, 20.0, &ui.white);
                ui.text_c(&ui.f.small, i18n::t("Activa sus banners para verlas aquí"), W / 2.0, ly + 110.0, 18.0, &ui.faint);
                ui.hit(r, 16);
            } else {
                ui.glyph(&d.glyph_m, G_BELL, 0.0, ly + 40.0, W, 50.0, &ui.wa(0.3));
                ui.text_c(&ui.f.sub, i18n::t("Sin notificaciones del móvil"), W / 2.0, ly + 98.0, 18.0, &ui.dim);
            }
            ui.rt_pop();
            return;
        }
        // Los avisos que llegan con el widget abierto entran desde la derecha
        let now = Instant::now();
        for n in &st.notifs {
            self.born.entry(n.id).or_insert(if self.seen { now } else { now - Duration::from_secs(5) });
        }
        self.born.retain(|id, _| st.notifs.iter().any(|n| n.id == *id));
        self.seen = st.ready;
        let mut yy = ly;
        for (i, n) in st.notifs.iter().enumerate() {
            if yy + NROW > H - PAD + 6.0 { break; }
            let fresh = self.born.get(&n.id).map(|b| b.elapsed().as_secs_f32()).unwrap_or(9.0);
            let kn = if fresh < 1.0 { self.moving = true; popup::ease_out(fresh / 0.45) } else { 1.0 };
            let k = ez(t, 0.3 + i as f32 * 0.06, 0.4).min(kn);
            let dx = if fresh < 1.0 { 40.0 * (1.0 - kn) } else { 0.0 };
            let dy = 8.0 * (1.0 - ez(t, 0.3 + i as f32 * 0.06, 0.4));
            let id = 100 + i as u32;
            let hk = ui.heat(id).max(ui.heat(300 + i as u32));
            popup::push_alpha(ui.rt, k);
            let r = rect(PAD - 8.0 + dx, yy - 4.0 + dy, W - PAD + 8.0 + dx, yy + NROW - 8.0 + dy);
            unsafe { ui.rt.FillRoundedRectangle(&rrect(r, 12.0), &ui.wa(0.05 + 0.05 * hk)); }
            let x = PAD + 4.0 + dx;
            ui.text(&ui.f.lbl, &trunc(&n.title, 26), x, yy + 2.0 + dy, 20.0, &ui.white);
            let (l1, l2) = two_lines(&n.body, 38);
            ui.text(&ui.f.sub, &l1, x, yy + 21.0 + dy, 17.0, &ui.dim);
            if !l2.is_empty() { ui.text(&ui.f.sub, &l2, x, yy + 37.0 + dy, 17.0, &ui.dim); }
            ui.hit(r, id);
            if hk > 0.05 {
                // Al pasar el ratón: una cruz para quitarlo (también de Windows)
                round_btn(ui, &ui.f.icon.clone(), G_CLOSE, W - PAD - 6.0 + dx, yy + 12.0 + dy, 12.0, 300 + i as u32, false);
            } else {
                ui.text_r(&ui.f.small, &if n.app.is_empty() { ago(n.when) } else { format!("{} · {}", n.app, ago(n.when)) }, W - PAD + dx, yy + 2.0 + dy, 20.0, &ui.faint);
            }
            ui.rt_pop();
            yy += NROW;
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
}