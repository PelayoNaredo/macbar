//! Más widgets del escritorio: relojes del mundo, tareas y cuenta atrás.

use super::*;
use desk::{DFonts, Kind}; // Todos responsive: maquetan con `desk::lay`
use deskw::{header, Widget};
use popup::Ui;
use std::f32::consts::{PI, TAU};

const PAD: f32 = 20.0;

// ---------------------------------------------------------------- utilidades comunes
fn ellipse(x: f32, y: f32, r: f32) -> D2D1_ELLIPSE { D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: r, radiusY: r } }
fn v2(x: f32, y: f32) -> Vector2 { Vector2 { X: x, Y: y } }

fn mul(a: &Matrix3x2, b: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: a.M11 * b.M11 + a.M12 * b.M21, M12: a.M11 * b.M12 + a.M12 * b.M22,
        M21: a.M21 * b.M11 + a.M22 * b.M21, M22: a.M21 * b.M12 + a.M22 * b.M22,
        M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31, M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    }
}
fn scale_at(s: f32, x: f32, y: f32) -> Matrix3x2 { Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: x * (1.0 - s), M32: y * (1.0 - s) } }
fn cur_transform(ui: &Ui) -> Matrix3x2 { let mut m = Matrix3x2::default(); unsafe { ui.rt.GetTransform(&mut m); } m }

/// Avance suave hacia `target`. true mientras se mueve.
fn approach(x: &mut f32, target: f32, dt: f32, tau: f32) -> bool {
    if (target - *x).abs() < 0.002 { *x = target; return false; }
    *x += (target - *x) * (1.0 - (-dt / tau).exp());
    true
}

/// Entrada escalonada: 0 hasta `delay`, luego sube en `dur` segundos.
fn ez(t: f32, delay: f32, dur: f32) -> f32 { popup::ease_out(((t - delay) / dur).clamp(0.0, 1.0)) }

fn smoothstep(a: f32, b: f32, x: f32) -> f32 { let t = ((x - a) / (b - a)).clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) }

thread_local! {
    /// Trazo con extremos redondeados, uno por fábrica de Direct2D
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

/// Texto recortado con «…» para que quepa en `maxw`.
fn fit(ui: &Ui, f: &IDWriteTextFormat, s: &str, maxw: f32) -> String {
    if ui.p.width(f, s) <= maxw { return s.to_string(); }
    let mut cs: Vec<char> = s.chars().collect();
    while !cs.is_empty() {
        cs.pop();
        let t = cs.iter().collect::<String>().trim_end().to_string() + "…";
        if ui.p.width(f, &t) <= maxw { return t; }
    }
    "…".into()
}

/// Abre widgets.ini en el Bloc de notas; si falta la clave, la escribe antes con un ejemplo.
fn edit_ini(key: &'static str, example: String) {
    if desk::ini_value(key).is_none() { desk::set_ini_value(key, &example); }
    let p = sys::config_dir() + "\\widgets.ini";
    sys::open_args("notepad.exe", &format!("\"{p}\""));
}

/// Texto que "rueda" al cambiar (como el de deskw): el viejo sube y se va mientras entra el nuevo.
#[derive(Default)]
struct Roll { cur: String, prev: String, at: Option<Instant> }

const ROLL_S: f32 = 0.42;

impl Roll {
    fn set(&mut self, s: &str) {
        if self.cur != s {
            if !self.cur.is_empty() { self.prev = std::mem::replace(&mut self.cur, s.to_string()); self.at = Some(Instant::now()); }
            else { self.cur = s.to_string(); }
        }
    }
    fn k(&self) -> f32 { self.at.map(|a| (a.elapsed().as_secs_f32() / ROLL_S).min(1.0)).unwrap_or(1.0) }
    fn busy(&self) -> bool { self.k() < 1.0 }
    /// `up`: el nuevo entra desde abajo (true) o desde arriba (cuando el número baja).
    fn draw(&mut self, ui: &Ui, f: &IDWriteTextFormat, s: &str, x: f32, top: f32, bh: f32, b: &ID2D1Brush, up: bool) {
        self.set(s);
        let k = self.k();
        let e = popup::ease_out(k);
        let dist = bh * 0.55 * if up { 1.0 } else { -1.0 };
        let put = |t: &str, dy: f32, a: f32| {
            if a <= 0.01 { return; }
            popup::push_alpha(ui.rt, a);
            ui.text(f, t, x, top + dy, bh, b);
            unsafe { ui.rt.PopLayer(); }
        };
        if k < 1.0 { put(&self.prev, -dist * e, 1.0 - popup::ease_out((k * 1.6).min(1.0))); }
        put(&self.cur.clone(), dist * (1.0 - e), if k < 1.0 { e } else { 1.0 });
    }
}

// ---------------------------------------------------------------- fechas (lógica pura)
/// Días desde el 1-1-1970 (calendario gregoriano).
pub(crate) fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = y as i64 - if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let (m, d) = (m as i64, d as i64);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Al revés: (año, mes, día) de un número de días desde el 1-1-1970.
pub(crate) fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    ((yoe + era * 400 + if m <= 2 { 1 } else { 0 }) as i32, m as u32, d as u32)
}

fn days_in(y: i32, m: u32) -> u32 {
    match m { 1 | 3 | 5 | 7 | 8 | 10 | 12 => 31, 4 | 6 | 9 | 11 => 30, _ => if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 } }
}

/// «2026-12-20» → días desde 1970 (None si no es una fecha válida).
fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.trim().split('-');
    let y: i32 = it.next()?.trim().parse().ok()?;
    let m: u32 = it.next()?.trim().parse().ok()?;
    let d: u32 = it.next()?.trim().parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) || d == 0 || d > days_in(y, m) || !(1..=9999).contains(&y) { return None; }
    Some(days_from_civil(y, m, d))
}

fn fmt_date_iso(days: i64) -> String { let (y, m, d) = civil_from_days(days); format!("{y:04}-{m:02}-{d:02}") }

fn local_now() -> SYSTEMTIME { unsafe { GetLocalTime() } }

pub(crate) fn today() -> i64 { let n = local_now(); days_from_civil(n.wYear as i32, n.wMonth as u32, n.wDay as u32) }

// ================================================================ relojes del mundo
/// Ciudades por defecto (en el idioma de la interfaz).
/// (Ocho: los tamaños grandes enseñan hasta ocho relojes; los pequeños, las primeras.)
fn default_zones() -> String {
    if i18n::en() { "New York|America/New_York, Tokyo|Asia/Tokyo, London|Europe/London, Sydney|Australia/Sydney, Los Angeles|America/Los_Angeles, Dubai|Asia/Dubai, Beijing|Asia/Shanghai, São Paulo|America/Sao_Paulo".into() }
    else { "Nueva York|America/New_York, Tokio|Asia/Tokyo, Londres|Europe/London, Sídney|Australia/Sydney, Los Ángeles|America/Los_Angeles, Dubái|Asia/Dubai, Pekín|Asia/Shanghai, São Paulo|America/Sao_Paulo".into() }
}

/// Relojes como mucho (los que caben en Extragrande).
const MAX_ZONES: usize = 8;

/// «Nueva York|America/New_York, Tokio|Asia/Tokyo» → [(ciudad, zona)] (ocho como mucho).
fn parse_zones(s: &str) -> Vec<(String, String)> {
    s.split(',').filter_map(|p| {
        let (name, zone) = p.split_once('|')?;
        let (name, zone) = (name.trim(), zone.trim());
        (!name.is_empty() && !zone.is_empty()).then(|| (name.to_string(), zone.to_string()))
    }).take(MAX_ZONES).collect()
}

/// Relojes que caben en cada tamaño, en (columnas, filas). Alto los pone en lista (esfera y texto al lado).
fn clock_grid(s: desk::Sz) -> (usize, usize) {
    use desk::Sz::*;
    match s { Pequeno => (1, 1), Mediano => (2, 1), Banda => (4, 1), Alto => (1, 3), Grande => (2, 2), Columna => (2, 3), Extragrande => (4, 2) }
}

/// Minutos de diferencia entre dos horas de pared [año, mes, día, hora, minuto, …].
fn offset_min(local: &[i32; 6], there: &[i32; 6]) -> i64 {
    let f = |t: &[i32; 6]| days_from_civil(t[0], t[1] as u32, t[2] as u32) * 1440 + t[3] as i64 * 60 + t[4] as i64;
    // Se leen en instantes algo distintos: se redondea al cuarto de hora (no hay zonas más finas)
    ((f(there) - f(local)) as f64 / 15.0).round() as i64 * 15
}

/// «+6 h», «−3:30 h» o «Misma hora».
fn fmt_offset(min: i64) -> String {
    if min == 0 { return i18n::t("Misma hora").into(); }
    let sign = if min > 0 { "+" } else { "−" };
    let (h, m) = (min.abs() / 60, min.abs() % 60);
    if m == 0 { format!("{sign}{h} h") } else { format!("{sign}{h}:{m:02} h") }
}

/// Cuánto es de día (0 = noche cerrada, 1 = pleno día) a una hora decimal: amanece hacia las 6:30 y
/// anochece hacia las 19:30, con una transición suave de dos horas.
fn daylight(h: f32) -> f32 { smoothstep(5.5, 7.5, h) * (1.0 - smoothstep(18.5, 20.5, h)) }

struct Zone { name: String, cal: Option<windows::Globalization::Calendar>, now: Option<[i32; 6]> }

/// Calendario gregoriano de 24 h en la zona IANA dada (Windows las entiende desde Windows 10).
fn make_cal(zone: &str) -> Option<windows::Globalization::Calendar> {
    let c = windows::Globalization::Calendar::new().ok()?;
    let _ = c.ChangeCalendarSystem(&HSTRING::from("GregorianCalendar"));
    c.ChangeClock(&HSTRING::from("24HourClock")).ok()?;
    c.ChangeTimeZone(&HSTRING::from(zone)).ok()?;
    Some(c)
}

fn read_cal(c: &windows::Globalization::Calendar) -> Option<[i32; 6]> {
    c.SetToNow().ok()?;
    Some([c.Year().ok()?, c.Month().ok()?, c.Day().ok()?, c.Hour().ok()?, c.Minute().ok()?, c.Second().ok()?])
}

const TIC_S: f32 = 0.12;

#[derive(Default)]
pub struct ClocksW { cfg: Option<String>, zones: Vec<Zone>, checked: Option<Instant>, read_at: Option<Instant>, local: [i32; 6], sec: i32, sec_prev: i32, tic: Option<Instant>, seconds: bool }

impl ClocksW {
    /// Relee la lista de ciudades (cada pocos segundos: si se edita widgets.ini, cambia sola).
    fn refresh(&mut self) {
        if self.checked.is_some_and(|c| c.elapsed().as_secs_f32() < 3.0) { return; }
        self.checked = Some(Instant::now());
        let cfg = if sys::demo() { default_zones() } else { desk::ini_value("relojes").unwrap_or_else(default_zones) };
        self.seconds = sys::demo() || desk::ini_value("relojes_segundero").is_none_or(|v| v != "0");
        if self.cfg.as_deref() == Some(cfg.as_str()) { return; }
        self.zones = parse_zones(&cfg).into_iter().map(|(name, zone)| Zone { name, cal: make_cal(&zone), now: None }).collect();
        self.cfg = Some(cfg);
        self.read_at = None;
    }
    fn update(&mut self) {
        if self.read_at.is_some_and(|r| r.elapsed().as_secs_f32() < 0.2) { return; }
        let first = self.read_at.is_none();
        self.read_at = Some(Instant::now());
        let n = local_now();
        self.local = [n.wYear as i32, n.wMonth as i32, n.wDay as i32, n.wHour as i32, n.wMinute as i32, n.wSecond as i32];
        for z in &mut self.zones { z.now = z.cal.as_ref().and_then(read_cal); }
        if self.local[5] != self.sec {
            self.sec_prev = if first { self.local[5] } else { self.sec };
            self.sec = self.local[5];
            if !first { self.tic = Some(Instant::now()); }
        }
    }
    fn tic_k(&self) -> f32 { self.tic.map(|t| (t.elapsed().as_secs_f32() / TIC_S).min(1.0)).unwrap_or(1.0) }
}

/// Una aguja desde el centro: `a` en radianes (0 = las 12, en el sentido del reloj).
fn hand(ui: &Ui, cx: f32, cy: f32, a: f32, len: f32, tail: f32, w: f32, b: &ID2D1Brush, st: Option<&ID2D1StrokeStyle>) {
    let (s, c) = a.sin_cos();
    unsafe { ui.rt.DrawLine(v2(cx - s * tail, cy + c * tail), v2(cx + s * len, cy - c * len), b, w, st); }
}

/// Esfera: clara de día y oscura de noche (con transición), marcas de las horas y agujas.
/// `ang`: (hora, minuto, segundo) en radianes; `k`: entrada (0..1).
fn dial(ui: &Ui, cx: f32, cy: f32, r: f32, day: f32, ang: Option<(f32, f32, Option<f32>)>, k: f32) {
    let mix = |a: f32, b: f32| a + (b - a) * day;
    let c0 = rgba(mix(0.20, 0.99), mix(0.22, 0.99), mix(0.29, 1.0), 0.94);
    let c1 = rgba(mix(0.07, 0.86), mix(0.08, 0.88), mix(0.11, 0.92), 0.94);
    let ink = mix(0.94, 0.13);
    let st = round_stroke(ui.rt);
    unsafe {
        let face = ui.p.dgrad(cx - r, cy - r, cx + r, cy + r, c0, c1);
        ui.rt.FillEllipse(&ellipse(cx, cy, r), &face);
        ui.rt.DrawEllipse(&ellipse(cx, cy, r - 0.5), &ui.brush(rgba(ink, ink, ink, 0.10 + 0.06 * day)), 1.0, None);
        for i in 0..12 {
            let a = i as f32 / 12.0 * TAU;
            let (s, c) = a.sin_cos();
            let big = i % 3 == 0;
            let (r0, r1) = (r * if big { 0.78 } else { 0.83 }, r * 0.90);
            let b = ui.brush(rgba(ink, ink, ink, if big { 0.85 } else { 0.38 }));
            ui.rt.DrawLine(v2(cx + s * r0, cy - c * r0), v2(cx + s * r1, cy - c * r1), &b, if big { 1.6 } else { 1.0 }, st.as_ref());
        }
    }
    let Some((ah, am, asec)) = ang else {
        ui.text_c(&ui.f.name, "?", cx, cy - 11.0, 22.0, &ui.brush(rgba(ink, ink, ink, 0.6)));
        return;
    };
    // Al aparecer, las agujas barren desde las 12 hasta su sitio
    let hb = ui.brush(rgba(ink, ink, ink, 0.96));
    hand(ui, cx, cy, ah * k, r * 0.50, r * 0.08, 3.2, &hb, st.as_ref());
    hand(ui, cx, cy, am * k, r * 0.74, r * 0.08, 2.2, &hb, st.as_ref());
    let acc = ui.on.clone();
    if let Some(a) = asec { hand(ui, cx, cy, a * k, r * 0.80, r * 0.20, 1.1, &acc, st.as_ref()); }
    unsafe {
        ui.rt.FillEllipse(&ellipse(cx, cy, 2.8), &hb);
        if asec.is_some() { ui.rt.FillEllipse(&ellipse(cx, cy, 1.5), &acc); }
    }
}

impl Widget for ClocksW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, _dt: f32) {
        let l = desk::lay(Kind::Clocks);
        let (w, h) = (l.w, l.h);
        self.refresh();
        self.update();
        // Los que caben en este tamaño (si hay menos ciudades, se reparten el sitio)
        let (gc, gr) = clock_grid(l.sz);
        let list = l.sz == desk::Sz::Alto;
        let n = self.zones.len().min(gc * gr).max(1);
        let cols = if list { 1 } else { n.min(gc) };
        let rows = n.div_ceil(cols);
        let (aw, ah) = (w - PAD * 2.0, h - PAD * 2.0);
        let (cw, ch) = (aw / cols as f32, ah / rows as f32);
        let r = if list { (ch * 0.30).min(24.0) } else { (cw * 0.36).min((ch - 48.0) / 2.0).min(70.0) };
        let content = if list { 2.0 * r } else { 2.0 * r + 44.0 };
        // Segundero: salta cada segundo con un «tic» elástico corto
        let kt = self.tic_k();
        let jump = (self.sec - self.sec_prev).rem_euclid(60) as f32;
        let sec = self.sec_prev as f32 + jump * crate::ease_back(kt);
        if self.zones.is_empty() {
            let msg = if l.sz == desk::Sz::Pequeno || list { i18n::t("Clic derecho: ciudades") } else { i18n::t("Clic derecho para elegir ciudades") };
            ui.text_c(&ui.f.sub, msg, w / 2.0, h / 2.0 - 9.0, 18.0, &ui.dim);
        }
        let local = self.local;
        for (i, z) in self.zones.iter().take(n).enumerate() {
            let (c, rr) = (i % cols, i / cols);
            let (cell_x, cell_y) = (PAD + cw * c as f32, PAD + ch * rr as f32);
            let top = cell_y + (ch - content) / 2.0;
            let cx = if list { cell_x + r } else { cell_x + cw / 2.0 };
            let cy = top + r;
            let k = ez(t, 0.08 + 0.07 * i as f32, 0.8);
            let (day, ang, info) = match z.now {
                Some(nw) => {
                    let (hh, mm) = (nw[3] as f32, nw[4] as f32);
                    // Los segundos son los mismos en todas partes: los del reloj local
                    let ah = ((hh % 12.0) + mm / 60.0) / 12.0 * TAU;
                    let am = (mm + sec / 60.0) / 60.0 * TAU;
                    let asec = self.seconds.then(|| sec / 60.0 * TAU);
                    let info = format!("{:02}:{:02} · {}", nw[3], nw[4], fmt_offset(offset_min(&local, &nw)));
                    (daylight(hh + mm / 60.0), Some((ah, am, asec)), info)
                }
                None => (0.0, None, i18n::t("Zona desconocida").to_string()),
            };
            // Entrada: cada esfera crece un poco desde su centro, en cascada
            let base = cur_transform(ui);
            let sc = 0.86 + 0.14 * k;
            unsafe { ui.rt.SetTransform(&mul(&scale_at(sc, cx, cy), &base)); }
            popup::push_alpha(ui.rt, k);
            dial(ui, cx, cy, r, day, ang, popup::ease_out(((t - 0.15 - 0.07 * i as f32) / 0.9).clamp(0.0, 1.0)));
            unsafe { ui.rt.PopLayer(); ui.rt.SetTransform(&base); }
            if list {
                // En lista: ciudad, hora grande y diferencia, a la derecha de la esfera
                let tx = cx + r + 10.0;
                let tw = w - PAD - tx;
                let (hm, off) = match z.now { Some(nw) => (format!("{:02}:{:02}", nw[3], nw[4]), fmt_offset(offset_min(&local, &nw))), None => ("--:--".into(), i18n::t("Zona desconocida").to_string()) };
                ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &z.name, tw), tx, cy - 30.0, 18.0, &ui.white);
                ui.text(&d.big, &hm, tx, cy - 13.0, 28.0, &ui.white);
                ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &off, tw), tx, cy + 14.0, 16.0, &ui.dim);
                ui.hit(rect(cell_x - 6.0, top - 6.0, w - PAD + 6.0, top + content + 6.0), 1 + i as u32);
            } else {
                let name = fit(ui, &ui.f.lbl, &z.name, cw - 6.0);
                ui.text_c(&ui.f.lbl, &name, cx, cy + r + 8.0, 18.0, &ui.white);
                let info = fit(ui, &ui.f.sub, &info, cw - 4.0);
                ui.text_c(&ui.f.sub, &info, cx, cy + r + 25.0, 16.0, &ui.dim);
                ui.hit(rect(cx - cw / 2.0, top - 6.0, cx + cw / 2.0, top + content + 4.0), 1 + i as u32);
            }
        }
    }
    fn layouts(&self) -> bool { true }
    fn click(&mut self, id: Option<u32>, right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        // Clic derecho en una esfera: editar las ciudades (fuera de ellas, el menú de siempre)
        if right && id.is_some() { return Some(Box::new(|| edit_ini("relojes", default_zones()))); }
        None
    }
    fn tick(&mut self) -> bool { true }
    fn animating(&self, t: f32) -> bool { t < 1.4 || self.tic_k() < 1.0 }
    fn smooth(&self) -> bool { self.tic_k() < 1.0 }
    fn frame_ms(&self) -> u32 { 16 }
}

// ================================================================ tareas
/// Una línea de tareas.txt: «[x] texto» (hecha), «[ ] texto» o solo el texto (pendiente).
fn parse_tasks(s: &str) -> Vec<(bool, String)> {
    s.lines().filter_map(|l| {
        let l = l.trim();
        let (done, rest) = if let Some(r) = l.strip_prefix("[x]").or_else(|| l.strip_prefix("[X]")) { (true, r) }
            else if let Some(r) = l.strip_prefix("[ ]").or_else(|| l.strip_prefix("[]")) { (false, r) }
            else { (false, l) };
        let rest = rest.trim();
        (!rest.is_empty()).then(|| (done, rest.to_string()))
    }).collect()
}

fn serialize_tasks(t: &[(bool, String)]) -> String {
    t.iter().map(|(d, s)| format!("{} {}\n", if *d { "[x]" } else { "[ ]" }, s.replace(['\r', '\n'], " "))).collect()
}

fn tasks_path() -> String { sys::config_dir() + "\\tareas.txt" }

/// Tareas escritas en el cuadro de «+», a la espera de que el widget las recoja.
static ADDED: Mutex<Vec<String>> = Mutex::new(Vec::new());

const RH: f32 = 34.0;
const LIST_TOP: f32 = 58.0;
const CHECK_S: f32 = 0.62;
const GONE_S: f32 = 0.24;
const BORN_S: f32 = 0.3;

/// `pos`: sitio en la lista (x, y desde arriba), que se desliza cuando las hechas bajan al final.
struct Task { text: String, done: bool, k: f32, gone: Option<Instant>, born: Option<Instant>, fit: Option<String>, pos: Option<(f32, f32)> }

impl Task {
    fn new(done: bool, text: String, born: bool) -> Self {
        Task { text, done, k: if done { 1.0 } else { 0.0 }, gone: None, born: born.then(Instant::now), fit: None, pos: None }
    }
}

#[derive(Default)]
pub struct TodoW { tasks: Vec<Task>, loaded: bool, mtime: Option<std::time::SystemTime>, scroll: f32, scroll_to: f32, max_scroll: f32, r_count: Roll, prog: f32, moving: bool }

impl TodoW {
    fn load(&mut self) {
        let list = if sys::demo() {
            [(true, "Responder el correo de Marta"), (true, "Revisar la presentación"), (false, "Comprar pan y café"), (false, "Llamar al dentista"), (false, "Preparar la reunión del lunes"),
             (false, "Reservar mesa para el sábado"), (true, "Pagar el alquiler"), (false, "Devolver el libro a la biblioteca"), (false, "Enviar el informe trimestral"),
             (false, "Regar las plantas"), (false, "Comprar el regalo de Ana"), (true, "Renovar el abono de transporte"), (false, "Llamar a mamá"), (false, "Ordenar las fotos del viaje")]
                .iter().map(|(d, s)| (*d, i18n::t(s).to_string())).collect()
        } else {
            parse_tasks(&std::fs::read_to_string(tasks_path()).unwrap_or_default())
        };
        self.tasks = list.into_iter().map(|(d, s)| Task::new(d, s, false)).collect();
        self.mtime = self.file_mtime();
        self.loaded = true;
    }
    fn file_mtime(&self) -> Option<std::time::SystemTime> { std::fs::metadata(tasks_path()).and_then(|m| m.modified()).ok() }
    /// Guarda (sin las que se están borrando) de forma atómica: nunca queda un archivo a medias.
    fn save(&mut self) {
        if sys::demo() { return; }
        let list: Vec<(bool, String)> = self.tasks.iter().filter(|t| t.gone.is_none()).map(|t| (t.done, t.text.clone())).collect();
        let _ = std::fs::create_dir_all(sys::config_dir());
        let tmp = tasks_path() + ".tmp";
        if std::fs::write(&tmp, serialize_tasks(&list)).is_ok() { let _ = std::fs::rename(&tmp, tasks_path()); }
        self.mtime = self.file_mtime();
    }
}

/// Casilla redonda: se rellena con el acento y el check se dibuja de un trazo (`k` de 0 a 1).
fn checkbox(ui: &Ui, cx: f32, cy: f32, k: f32, hot: f32) {
    let r = 9.0;
    let fill = (k / 0.35).clamp(0.0, 1.0);
    let st = round_stroke(ui.rt);
    unsafe {
        if fill < 1.0 { ui.rt.DrawEllipse(&ellipse(cx, cy, r - 0.75), &ui.wa((0.42 + 0.3 * hot) * (1.0 - fill)), 1.5, None); }
        if fill > 0.0 { ui.rt.FillEllipse(&ellipse(cx, cy, r * crate::ease_back(fill).max(0.0)), &ui.on); }
        let c = popup::ease_out(((k - 0.2) / 0.4).clamp(0.0, 1.0));
        if c > 0.0 {
            let pts = [(cx - 4.2, cy + 0.2), (cx - 1.3, cy + 3.2), (cx + 4.4, cy - 3.4)];
            let l1 = ((pts[1].0 - pts[0].0).powi(2) + (pts[1].1 - pts[0].1).powi(2)).sqrt();
            let l2 = ((pts[2].0 - pts[1].0).powi(2) + (pts[2].1 - pts[1].1).powi(2)).sqrt();
            let d = c * (l1 + l2);
            let lerp = |a: (f32, f32), b: (f32, f32), u: f32| v2(a.0 + (b.0 - a.0) * u, a.1 + (b.1 - a.1) * u);
            let wb = ui.brush(rgba(1.0, 1.0, 1.0, 0.98));
            ui.rt.DrawLine(v2(pts[0].0, pts[0].1), lerp(pts[0], pts[1], (d / l1).min(1.0)), &wb, 1.8, st.as_ref());
            if d > l1 { ui.rt.DrawLine(v2(pts[1].0, pts[1].1), lerp(pts[1], pts[2], ((d - l1) / l2).min(1.0)), &wb, 1.8, st.as_ref()); }
        }
    }
}

impl Widget for TodoW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let l = desk::lay(Kind::Todo);
        let (w, h) = (l.w, l.h);
        if !self.loaded { self.load(); }
        let mut moving = false;
        // Lo escrito en el cuadro de «+» entra por abajo y se va hasta ella
        let added: Vec<String> = std::mem::take(&mut *ADDED.lock().unwrap());
        if !added.is_empty() {
            for s in added { self.tasks.push(Task::new(false, s, true)); }
            self.save();
            self.scroll_to = f32::MAX;
        }
        for tk in &mut self.tasks {
            let to = if tk.done { 1.0 } else { 0.0 };
            if tk.k != to { tk.k = if to > tk.k { (tk.k + dt / CHECK_S).min(1.0) } else { (tk.k - dt / CHECK_S).max(0.0) }; moving = true; }
            if tk.born.is_some_and(|b| b.elapsed().as_secs_f32() < BORN_S) { moving = true; }
        }
        let before = self.tasks.len();
        self.tasks.retain(|tk| tk.gone.is_none_or(|g| g.elapsed().as_secs_f32() < GONE_S));
        if self.tasks.len() != before || self.tasks.iter().any(|tk| tk.gone.is_some()) { moving = true; }

        let live: Vec<&Task> = self.tasks.iter().filter(|tk| tk.gone.is_none()).collect();
        let (done, total) = (live.iter().filter(|tk| tk.done).count(), live.len());
        let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
        moving |= approach(&mut self.prog, frac, dt, 0.25);
        if l.sz == desk::Sz::Pequeno {
            // Pequeño: cuántas quedan, en grande, y la primera pendiente (se puede marcar desde aquí)
            let pending = total - done;
            ui.text(&d.title, i18n::t("Tareas"), PAD, PAD - 4.0, 22.0, &ui.white);
            let num = pending.to_string();
            let dim = ui.dim.clone();
            let huge = d.huge.clone();
            self.r_count.draw(ui, &huge, &num, PAD, 44.0, 44.0, &ui.white.clone(), true);
            moving |= self.r_count.busy();
            let lbl = if pending == 1 { i18n::t("pendiente") } else { i18n::t("pendientes") };
            ui.text(&ui.f.name, lbl, PAD + ui.p.width(&d.huge, &num) + 8.0, 58.0, 22.0, &dim);
            unsafe {
                let (x0, x1, y) = (PAD, w - PAD, 100.0);
                ui.rt.FillRoundedRectangle(&rrect(rect(x0, y, x1, y + 3.0), 1.5), &ui.track);
                if self.prog > 0.005 { ui.rt.FillRoundedRectangle(&rrect(rect(x0, y, x0 + ((x1 - x0) * self.prog).max(3.0), y + 3.0), 1.5), &ui.on); }
            }
            // La que se acaba de marcar sigue a la vista hasta que termina de tacharse
            let pick = self.tasks.iter().position(|tk| tk.gone.is_none() && (!tk.done || tk.k < 1.0));
            let y = h - PAD - RH + 4.0;
            match pick {
                Some(i) => {
                    let fname = ui.f.name.clone();
                    let tk = &mut self.tasks[i];
                    let id = 100 + i as u32;
                    let hot = ui.heat(id);
                    ui.card_k(rect(PAD - 8.0, y + 2.0, w - PAD + 8.0, y + RH - 2.0), hot);
                    let cy = y + RH / 2.0;
                    checkbox(ui, PAD + 10.0, cy, tk.k, hot);
                    let tx = PAD + 30.0;
                    let s = fit(ui, &fname, &tk.text, w - PAD - tx);
                    let strike = popup::ease_out(((tk.k - 0.3) / 0.4).clamp(0.0, 1.0));
                    let fade = popup::ease_out(((tk.k - 0.65) / 0.35).clamp(0.0, 1.0));
                    ui.text(&fname, &s, tx, y, RH, &ui.wa(0.95 - 0.5 * fade));
                    if strike > 0.0 {
                        let tw = ui.p.width(&fname, &s);
                        unsafe { ui.rt.FillRectangle(&rect(tx - 1.0, cy - 0.7, tx - 1.0 + (tw + 2.0) * strike, cy + 0.7), &ui.wa(0.8 - 0.4 * fade)); }
                    }
                    ui.hit(rect(PAD - 8.0, y, w - PAD + 8.0, y + RH), id);
                }
                None => ui.text(&ui.f.sub, if total == 0 { i18n::t("Nada pendiente") } else { i18n::t("Todo hecho") }, PAD, y, RH, &ui.faint),
            }
            self.max_scroll = 0.0;
            self.moving = moving;
            return;
        }

        // Cabecera: «Hoy · 2 de 5», una línea de progreso y el botón «+»
        let hoy = i18n::t("Hoy");
        ui.text(&d.title, hoy, PAD, PAD - 4.0, 22.0, &ui.white);
        let x = PAD + ui.p.width(&d.title, hoy) + 6.0;
        ui.text(&ui.f.sub, "·", x, PAD - 4.0, 22.0, &ui.dim);
        let dim = ui.dim.clone();
        let count = i18n::tf("{} de {}", &[&done, &total]);
        let sub = ui.f.sub.clone();
        self.r_count.draw(ui, &sub, &count, x + 10.0, PAD - 4.0, 22.0, &dim, true);
        moving |= self.r_count.busy();
        unsafe {
            let (x0, x1, y) = (PAD, w - PAD, 46.0);
            ui.rt.FillRoundedRectangle(&rrect(rect(x0, y, x1, y + 3.0), 1.5), &ui.track);
            if self.prog > 0.005 { ui.rt.FillRoundedRectangle(&rrect(rect(x0, y, x0 + ((x1 - x0) * self.prog).max(3.0), y + 3.0), 1.5), &ui.on); }
        }
        let (bx, by, br) = (w - PAD - 12.0, PAD + 7.0, 13.0);
        let hk = ui.heat(2);
        let base = cur_transform(ui);
        let sc = (1.0 + 0.06 * hk) * (1.0 - 0.14 * desk::press(2));
        unsafe {
            ui.rt.SetTransform(&mul(&scale_at(sc, bx, by), &base));
            ui.rt.FillEllipse(&ellipse(bx, by, br), &ui.wa(0.10 + 0.12 * hk));
        }
        ui.glyph(&ui.f.icon, "\u{E710}", bx - br, by - br, br * 2.0, br * 2.0, &ui.white);
        unsafe { ui.rt.SetTransform(&base); }
        ui.hit(rect(bx - br - 3.0, by - br - 3.0, bx + br + 3.0, by + br + 3.0), 2);

        // Lista (con rueda si no caben). Banda y Extragrande, en columnas (por filas: 1 2 / 3 4)
        let cols = match l.sz { desk::Sz::Banda => 3, desk::Sz::Extragrande => 2, _ => 1 };
        let colw = (w - 2.0 * PAD - (cols - 1) as f32 * 2.0 * PAD) / cols as f32;
        // El final de la lista cae entre filas (sin una a medias abajo)
        let (top, bottom) = (LIST_TOP, LIST_TOP + ((h - 10.0 - LIST_TOP) / RH).floor() * RH);
        // En columnas, las que se borran se desvanecen sin cerrar el hueco (no se descoloca la rejilla)
        let grow = |tk: &Task| if cols > 1 { 1.0 } else { tk.gone.map(|g| 1.0 - popup::ease_out(g.elapsed().as_secs_f32() / GONE_S)).unwrap_or(1.0) };
        let content: f32 = if cols > 1 { self.tasks.len().div_ceil(cols) as f32 * RH } else { self.tasks.iter().map(|tk| RH * grow(tk)).sum() };
        // Las pendientes primero: las hechas bajan al final (cuando terminan de tacharse), deslizándose
        let mut ord: Vec<usize> = (0..self.tasks.len()).collect();
        ord.sort_by_key(|&i| (self.tasks[i].done && self.tasks[i].k >= 1.0) as u8);
        let mut acc = 0.0;
        for (slot, &i) in ord.iter().enumerate() {
            let tk = &mut self.tasks[i];
            let to = if cols > 1 { ((slot % cols) as f32 * (colw + 2.0 * PAD), (slot / cols) as f32 * RH) } else { (0.0, acc) };
            acc += RH * grow(tk);
            match &mut tk.pos {
                None => tk.pos = Some(to),
                Some(p) => { moving |= approach(&mut p.0, to.0, dt, 0.12); moving |= approach(&mut p.1, to.1, dt, 0.12); }
            }
        }
        self.max_scroll = (content - (bottom - top)).max(0.0);
        self.scroll_to = self.scroll_to.clamp(0.0, self.max_scroll);
        moving |= approach(&mut self.scroll, self.scroll_to, dt, 0.09);
        if total == 0 && self.tasks.is_empty() {
            let k = ez(t, 0.1, 0.5);
            let cy = (top + bottom) / 2.0;
            popup::push_alpha(ui.rt, k);
            if bottom - top > 120.0 { ui.glyph(&d.glyph_l, "\u{E73E}", w / 2.0 - 20.0, cy - 50.0, 40.0, 40.0, &ui.faint); }
            ui.text_c(&ui.f.name, i18n::t("Nada pendiente"), w / 2.0, cy - 6.0, 20.0, &ui.dim);
            ui.text_c(&ui.f.sub, i18n::t("Pulsa + para añadir una tarea"), w / 2.0, cy + 16.0, 18.0, &ui.faint);
            unsafe { ui.rt.PopLayer(); }
        }
        unsafe { ui.rt.PushAxisAlignedClip(&rect(0.0, top - 4.0, w, bottom), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE); }
        let fname = ui.f.name.clone();
        for (i, tk) in self.tasks.iter_mut().enumerate() {
            let g = tk.gone.map(|g| 1.0 - popup::ease_out(g.elapsed().as_secs_f32() / GONE_S)).unwrap_or(1.0);
            let rh = if cols > 1 { RH } else { RH * g };
            let (px, py) = tk.pos.unwrap_or((0.0, 0.0));
            let x0 = PAD + px;
            let ry = top - self.scroll + py;
            if ry + rh < top - 4.0 || ry > bottom { continue; }
            let id = 100 + i as u32;
            let born = tk.born.map(|b| popup::ease_out(b.elapsed().as_secs_f32() / BORN_S)).unwrap_or(1.0);
            let a = g * born * ez(t, 0.1 + 0.05 * i as f32, 0.4);
            if a <= 0.01 { continue; }
            let dx = 10.0 * (1.0 - born);
            popup::push_alpha(ui.rt, a);
            let hot = if tk.gone.is_none() { ui.heat(id) } else { 0.0 };
            ui.card_k(rect(x0 - 8.0, ry + 2.0, x0 + colw + 8.0, ry + RH - 2.0), hot);
            let cy = ry + RH / 2.0;
            checkbox(ui, x0 + 10.0 + dx, cy, tk.k, hot);
            let tx = x0 + 30.0;
            let maxw = x0 + colw - tx;
            let s = tk.fit.get_or_insert_with(|| fit(ui, &fname, &tk.text, maxw)).clone();
            // Hecha: se tacha de izquierda a derecha y luego se apaga
            let strike = popup::ease_out(((tk.k - 0.3) / 0.4).clamp(0.0, 1.0));
            let fade = popup::ease_out(((tk.k - 0.65) / 0.35).clamp(0.0, 1.0));
            ui.text(&fname, &s, tx + dx, ry, RH, &ui.wa(0.95 - 0.5 * fade));
            if strike > 0.0 {
                let tw = ui.p.width(&fname, &s);
                unsafe { ui.rt.FillRectangle(&rect(tx + dx - 1.0, cy - 0.7, tx + dx - 1.0 + (tw + 2.0) * strike, cy + 0.7), &ui.wa(0.8 - 0.4 * fade)); }
            }
            unsafe { ui.rt.PopLayer(); }
            if tk.gone.is_none() { ui.hit(rect(x0 - 8.0, ry.max(top - 4.0), x0 + colw + 8.0, (ry + RH).min(bottom)), id); }
        }
        unsafe { ui.rt.PopAxisAlignedClip(); }
        self.moving = moving;
    }
    fn layouts(&self) -> bool { true }
    fn click(&mut self, id: Option<u32>, right: bool, pt: POINT) -> Option<Box<dyn FnOnce()>> {
        let id = id?;
        if id == 2 && !right {
            let target = unsafe { WindowFromPoint(pt) };
            return Some(Box::new(move || input_open(pt, target)));
        }
        let i = id.checked_sub(100)? as usize;
        let tk = self.tasks.get_mut(i).filter(|tk| tk.gone.is_none())?;
        // Clic derecho en una tarea: se borra (en el resto del widget, el menú de siempre)
        if right { tk.gone = Some(Instant::now()); } else { tk.done = !tk.done; }
        self.save();
        self.moving = true;
        Some(Box::new(|| {}))
    }
    fn wheel(&mut self, d: i32) -> bool {
        if self.max_scroll <= 0.0 { return false; }
        self.scroll_to = (self.scroll_to - d as f32 / 120.0 * RH * 1.5).clamp(0.0, self.max_scroll);
        self.moving = true;
        true
    }
    fn tick(&mut self) -> bool {
        // Si alguien edita tareas.txt a mano, se recarga (no a mitad de una animación)
        if sys::demo() || !self.loaded || self.moving { return false; }
        let m = self.file_mtime();
        if m != self.mtime { self.load(); return true; }
        false
    }
    fn animating(&self, t: f32) -> bool { t < 1.3 || self.moving }
    fn smooth(&self) -> bool { self.moving }
    fn frame_ms(&self) -> u32 { 16 }
}

// ---------------------------------------------------------------- cuadro para escribir una tarea
// Ventanita oscura con un EDIT nativo de una línea: Intro añade, Esc (o hacer clic fuera) cancela.
const IN_W: f32 = 320.0;
const IN_H: f32 = 80.0;
const IN_EDIT: usize = 101;
const IN_EM_SETSEL: u32 = 0x00B1;
const IN_EM_SETLIMITTEXT: u32 = 0x00C5;
const IN_EM_SETMARGINS: u32 = 0x00D3;
const IN_ES_AUTOHSCROLL: u32 = 0x0080;

const fn gdi_rgb(r: u8, g: u8, b: u8) -> COLORREF { COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16) }
const IN_BG: COLORREF = gdi_rgb(0x1C, 0x1C, 0x1E);
const IN_INK: COLORREF = gdi_rgb(0xF2, 0xF2, 0xF7);
const IN_TITLE: COLORREF = gdi_rgb(0x8E, 0x8E, 0x93);
const IN_DIM: COLORREF = gdi_rgb(0x63, 0x63, 0x66);
const IN_BORDER: COLORREF = gdi_rgb(0x3A, 0x3A, 0x3C);

#[derive(Default)]
struct InState { hwnd: HWND, edit: HWND, target: HWND, dpi: u32, bg: HBRUSH, f_text: HFONT, f_title: HFONT, f_small: HFONT }

thread_local! { static INPUT: RefCell<InState> = RefCell::new(InState::default()); }

fn in_font(tenths: i32, weight: i32, dpi: u32) -> HFONT {
    let mut lf = LOGFONTW { lfHeight: -((tenths as i64 * dpi as i64 + 360) / 720) as i32, lfWeight: weight, lfCharSet: DEFAULT_CHARSET, lfQuality: CLEARTYPE_QUALITY, ..Default::default() };
    for (d, c) in lf.lfFaceName.iter_mut().zip("Segoe UI".encode_utf16()) { *d = c; }
    unsafe { CreateFontIndirectW(&lf) }
}

fn in_px(v: f32, dpi: u32) -> i32 { (v * dpi as f32 / 96.0).round() as i32 }

/// Abre el cuadro junto al punto `pt` (pantalla); al aceptar, repinta `target` (el widget).
fn input_open(pt: POINT, target: HWND) {
    unsafe {
        let cur = INPUT.with(|s| s.borrow().hwnd);
        if !cur.is_invalid() && IsWindow(Some(cur)).as_bool() { let _ = SetForegroundWindow(cur); return; }
        let Ok(hm) = GetModuleHandleW(None) else { return };
        let hinst: HINSTANCE = hm.into();
        let class = w!("MacBarTaskInput");
        let wc = WNDCLASSW { lpfnWndProc: Some(input_proc), hInstance: hinst, lpszClassName: class, hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
        RegisterClassW(&wc);
        let dpi = match GetDpiForWindow(target) { 0 => 96, d => d };
        let (w, h) = (in_px(IN_W, dpi), in_px(IN_H, dpi));
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut mi);
        let wa = mi.rcWork;
        let x = (pt.x - w / 2).min(wa.right - w - 8).max(wa.left + 8);
        let y = (pt.y + in_px(16.0, dpi)).min(wa.bottom - h - 8).max(wa.top + 8);
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, class, w!("macbar"), WS_POPUP | WS_CLIPCHILDREN, x, y, w, h, None, None, Some(hinst), None) else { return };
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
        let round: i32 = 2;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as _, 4);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &IN_BORDER as *const _ as _, 4);
        let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(IN_ES_AUTOHSCROLL);
        let Ok(edit) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("EDIT"), PCWSTR::null(), style, in_px(16.0, dpi), in_px(32.0, dpi), w - in_px(32.0, dpi), in_px(22.0, dpi),
            Some(hwnd), Some(HMENU(IN_EDIT as _)), Some(hinst), None) else { let _ = DestroyWindow(hwnd); return };
        let st = InState { hwnd, edit, target, dpi, bg: CreateSolidBrush(IN_BG), f_text: in_font(110, 400, dpi), f_title: in_font(90, 600, dpi), f_small: in_font(80, 400, dpi) };
        SendMessageW(edit, WM_SETFONT, Some(WPARAM(st.f_text.0 as usize)), Some(LPARAM(1)));
        SendMessageW(edit, IN_EM_SETMARGINS, Some(WPARAM(3)), Some(LPARAM(0)));
        SendMessageW(edit, IN_EM_SETLIMITTEXT, Some(WPARAM(300)), Some(LPARAM(0)));
        let _ = SetWindowSubclass(edit, Some(input_edit_proc), 1, 0);
        INPUT.with(|s| *s.borrow_mut() = st);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(edit));
    }
}

/// Recoge lo escrito, lo deja para el widget y cierra.
fn input_submit() {
    let (hwnd, edit, target) = INPUT.with(|s| { let s = s.borrow(); (s.hwnd, s.edit, s.target) });
    unsafe {
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(edit, &mut buf).max(0) as usize;
        let txt = String::from_utf16_lossy(&buf[..n.min(buf.len())]).trim().to_string();
        if !txt.is_empty() {
            ADDED.lock().unwrap().push(txt);
            let _ = InvalidateRect(Some(target), None, false);
        }
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

fn input_text(dc: HDC, font: HFONT, color: COLORREF, s: &str, rc: &mut RECT, fmt: DRAW_TEXT_FORMAT) {
    let mut t: Vec<u16> = s.encode_utf16().collect();
    if t.is_empty() { return; }
    unsafe {
        let old = SelectObject(dc, font.into());
        SetTextColor(dc, color);
        DrawTextW(dc, &mut t, rc, fmt);
        SelectObject(dc, old);
    }
}

fn input_paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (dpi, bg, ft, fs) = INPUT.with(|s| { let s = s.borrow(); (s.dpi, s.bg, s.f_title, s.f_small) });
        FillRect(dc, &rc, bg);
        SetBkMode(dc, TRANSPARENT);
        let p = |v: f32| in_px(v, dpi);
        let mut r = RECT { left: p(16.0), top: p(9.0), right: rc.right - p(16.0), bottom: p(28.0) };
        input_text(dc, ft, IN_TITLE, i18n::t("Nueva tarea"), &mut r, DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);
        // Raya de acento bajo lo que se escribe
        let a = crate::profiles::accent(1.0);
        let acc = CreateSolidBrush(gdi_rgb((a.r * 255.0) as u8, (a.g * 255.0) as u8, (a.b * 255.0) as u8));
        let line = RECT { left: p(16.0), top: p(56.0), right: rc.right - p(16.0), bottom: p(56.0) + p(1.5).max(1) };
        FillRect(dc, &line, acc);
        let _ = DeleteObject(acc.into());
        let mut r = RECT { left: p(16.0), top: p(60.0), right: rc.right - p(16.0), bottom: p(76.0) };
        input_text(dc, fs, IN_DIM, i18n::t("Intro para añadir · Esc para cancelar"), &mut r, DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn input_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => { input_paint(hwnd); LRESULT(0) }
            WM_ERASEBKGND => LRESULT(1),
            WM_CTLCOLOREDIT => {
                let dc = HDC(wp.0 as _);
                SetTextColor(dc, IN_INK);
                SetBkColor(dc, IN_BG);
                LRESULT(INPUT.with(|s| s.borrow().bg).0 as isize)
            }
            // Se arrastra desde cualquier parte que no sea el texto
            WM_NCHITTEST => {
                let r = DefWindowProcW(hwnd, msg, wp, lp);
                if r.0 == HTCLIENT as isize { LRESULT(HTCAPTION as isize) } else { r }
            }
            WM_ACTIVATE => {
                if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE { let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)); }
                else { let _ = SetFocus(Some(INPUT.with(|s| s.borrow().edit))); }
                LRESULT(0)
            }
            WM_CLOSE => { let _ = DestroyWindow(hwnd); LRESULT(0) }
            WM_DESTROY => {
                let st = INPUT.with(|s| std::mem::take(&mut *s.borrow_mut()));
                if !st.edit.is_invalid() { let _ = RemoveWindowSubclass(st.edit, Some(input_edit_proc), 1); }
                for f in [st.f_text, st.f_title, st.f_small] { if !f.is_invalid() { let _ = DeleteObject(f.into()); } }
                if !st.bg.is_invalid() { let _ = DeleteObject(st.bg.into()); }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

unsafe extern "system" fn input_edit_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        match msg {
            WM_KEYDOWN => {
                let vk = wp.0 as u16;
                if vk == VK_RETURN.0 { input_submit(); return LRESULT(0); }
                if vk == VK_ESCAPE.0 { let _ = PostMessageW(Some(INPUT.with(|s| s.borrow().hwnd)), WM_CLOSE, WPARAM(0), LPARAM(0)); return LRESULT(0); }
                if vk == b'A' as u16 && GetKeyState(VK_CONTROL.0 as i32) < 0 { SendMessageW(hwnd, IN_EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1))); return LRESULT(0); }
            }
            // Sin pitido ni caracteres raros por Intro, Esc o Ctrl+A
            WM_CHAR if matches!(wp.0, 0x0D | 0x1B | 0x01) => return LRESULT(0),
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}

// ================================================================ cuenta atrás
#[derive(Clone, Debug, PartialEq)]
struct Cd { name: String, date: i64, from: Option<i64> }

/// «Vacaciones|2026-12-20, Navidad|2026-12-25|2026-11-01» → fechas (la tercera parte, opcional, es
/// desde cuándo se cuenta, para la barra de progreso).
fn parse_countdowns(s: &str) -> Vec<Cd> {
    s.split(',').filter_map(|p| {
        let mut it = p.split('|');
        let name = it.next()?.trim();
        let date = parse_date(it.next()?)?;
        let from = it.next().and_then(parse_date);
        (!name.is_empty()).then(|| Cd { name: name.to_string(), date, from })
    }).collect()
}

fn fmt_countdowns(l: &[Cd]) -> String {
    l.iter().map(|c| match c.from {
        Some(f) => format!("{}|{}|{}", c.name, fmt_date_iso(c.date), fmt_date_iso(f)),
        None => format!("{}|{}", c.name, fmt_date_iso(c.date)),
    }).collect::<Vec<_>>().join(", ")
}

/// Índices de las fechas que aún no han pasado, de la más cercana a la más lejana.
fn upcoming(l: &[Cd], today: i64) -> Vec<usize> {
    let mut v: Vec<usize> = (0..l.len()).filter(|&i| l[i].date >= today).collect();
    v.sort_by_key(|&i| l[i].date);
    v
}

/// Parte recorrida desde `from` hasta `date` (0..1).
fn progress(today: i64, from: i64, date: i64) -> f32 {
    if date <= from { return 1.0; }
    ((today - from) as f32 / (date - from) as f32).clamp(0.0, 1.0)
}

/// Fechas de muestra para el modo demo (relativas a hoy).
fn demo_countdowns(today: i64) -> String {
    let (y, _, _) = civil_from_days(today);
    let l = [
        Cd { name: i18n::t("Vacaciones").into(), date: today + 28, from: Some(today - 40) },
        Cd { name: i18n::t("Concierto").into(), date: today + 61, from: Some(today - 10) },
        Cd { name: i18n::t("Cumpleaños de Ana").into(), date: today + 12, from: Some(today - 353) },
        Cd { name: i18n::t("Viaje a Lisboa").into(), date: today + 95, from: Some(today - 20) },
        Cd { name: i18n::t("Media maratón").into(), date: today + 45, from: Some(today - 60) },
        Cd { name: i18n::t("Fin de curso").into(), date: today + 190, from: Some(today - 80) },
        Cd { name: i18n::t("Año nuevo").into(), date: days_from_civil(y + 1, 1, 1), from: Some(days_from_civil(y, 1, 1)) },
    ];
    fmt_countdowns(&l)
}

/// Para otros diseños de la cuenta atrás: las fechas que faltan (nombre y día), de la más cercana a
/// la más lejana. Lee lo mismo que el widget de siempre (o las de muestra en el modo demo).
pub(crate) fn countdown_dates(today: i64) -> Vec<(String, i64)> {
    let cfg = if sys::demo() { demo_countdowns(today) } else { desk::ini_value("cuenta_atras").unwrap_or_default() };
    let l = parse_countdowns(&cfg);
    upcoming(&l, today).into_iter().map(|i| (l[i].name.clone(), l[i].date)).collect()
}

/// Abre widgets.ini para editar las fechas (con un ejemplo si aún no hay ninguna).
pub(crate) fn countdown_edit() {
    let ex = format!("{}|{}", i18n::t("Vacaciones"), fmt_date_iso(today() + 30));
    edit_ini("cuenta_atras", ex);
}
/// «20 dic» (con el año si no es el actual).
fn fmt_date_short(days: i64, today: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    let mes = i18n::month_short((m - 1) as usize);
    let base = if i18n::en() { format!("{mes} {d}") } else { format!("{d} {mes}") };
    if y != civil_from_days(today).0 { format!("{base} {y}") } else { base }
}

const BIG: f32 = 1.6;
const NUM_TOP: f32 = 46.0;
const PARTY_S: f32 = 3.4;

#[derive(Default)]
pub struct CountdownW { cfg: Option<String>, items: Vec<Cd>, checked: Option<Instant>, today: i64, sel: usize, r_num: Roll, r_name: Roll, party: Option<Instant>, partied: Option<(String, i64)>, bar: f32, dot: f32, lab_x: f32, moving: bool, up: bool,
    /// Rejillas (Banda, Alto, Extragrande): página que se ve y cuándo se pasó; lista de debajo (Grande y Columna): qué cuenta es cada fila
    grid: bool, page: usize, page_at: Option<Instant>, list: Vec<usize> }

impl CountdownW {
    /// Relee las fechas cada pocos segundos (y al cambiar el día). true si algo cambió.
    fn refresh(&mut self) -> bool {
        if self.checked.is_some_and(|c| c.elapsed().as_secs_f32() < 3.0) { return false; }
        self.checked = Some(Instant::now());
        let td = today();
        let day_changed = td != self.today;
        self.today = td;
        let cfg = if sys::demo() { demo_countdowns(td) } else { desk::ini_value("cuenta_atras").unwrap_or_default() };
        if self.cfg.as_deref() == Some(cfg.as_str()) { return day_changed; }
        let mut items = parse_countdowns(&cfg);
        // Las que no dicen desde cuándo cuentan empiezan hoy (se apunta, para que la barra avance)
        if !sys::demo() && items.iter().any(|c| c.from.is_none()) {
            for c in &mut items { if c.from.is_none() { c.from = Some(td.min(c.date)); } }
            let s = fmt_countdowns(&items);
            desk::set_ini_value("cuenta_atras", &s);
            self.cfg = Some(s);
        } else {
            self.cfg = Some(cfg);
        }
        self.items = items;
        true
    }
    fn page_k(&self) -> f32 { self.page_at.map(|a| popup::ease_out((a.elapsed().as_secs_f32() / 0.4).min(1.0))).unwrap_or(1.0) }
    fn party_k(&self) -> Option<f32> { self.party.map(|p| p.elapsed().as_secs_f32()).filter(|e| *e < PARTY_S) }
}

/// Número pseudoaleatorio estable (0..1) para la partícula `i` y el dato `n`.
fn rnd(i: u32, n: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B1) ^ n.wrapping_mul(0x85EB_CA77).wrapping_add(0x27D4_EB2F);
    x ^= x >> 15; x = x.wrapping_mul(0x2C1B_3C6D); x ^= x >> 12; x = x.wrapping_mul(0x297A_2D39); x ^= x >> 15;
    (x & 0xFFFF) as f32 / 65535.0
}

/// Confeti que salta desde (ox, oy), revolotea y cae mientras se desvanece.
fn confetti(ui: &Ui, ox: f32, oy: f32, e: f32) {
    let a = crate::profiles::accent(1.0);
    let colors = [rgba(a.r, a.g, a.b, 1.0), rgba(1.0, 0.80, 0.30, 1.0), rgba(1.0, 0.45, 0.58, 1.0), rgba(0.35, 0.86, 0.80, 1.0), rgba(0.96, 0.96, 1.0, 1.0)];
    let base = cur_transform(ui);
    let fade = 1.0 - smoothstep(PARTY_S - 1.0, PARTY_S, e);
    for i in 0..42u32 {
        let delay = rnd(i, 7) * 0.25;
        let t = e - delay;
        if t <= 0.0 { continue; }
        let ang = -PI / 2.0 + (rnd(i, 0) - 0.5) * 2.4;
        let sp = 170.0 + 190.0 * rnd(i, 1);
        // Sale disparado y el aire lo frena; la gravedad lo baja y un vaivén lo mece
        let k = (1.0 - (-2.4 * t).exp()) / 2.4;
        let x = ox + ang.cos() * sp * k + (t * (2.0 + 3.0 * rnd(i, 5)) + rnd(i, 6) * 6.0).sin() * 6.0 * t;
        let y = oy + ang.sin() * sp * k + 0.5 * 150.0 * t * t;
        let rot = rnd(i, 2) * TAU + t * (rnd(i, 3) - 0.5) * 14.0;
        // Se da la vuelta al caer (se estrecha y se ensancha)
        let flip = (t * (6.0 + 6.0 * rnd(i, 4)) + rnd(i, 8) * 6.0).cos().abs().max(0.15);
        let (pw, ph) = (3.4 * (0.7 + 0.6 * rnd(i, 9)), 7.0 * (0.7 + 0.6 * rnd(i, 10)));
        let (s, co) = rot.sin_cos();
        let m = Matrix3x2 { M11: co * flip, M12: s * flip, M21: -s, M22: co, M31: x, M32: y };
        let mut col = colors[(i % 5) as usize];
        col.a = fade * (0.9 + 0.1 * rnd(i, 11));
        unsafe {
            ui.rt.SetTransform(&mul(&m, &base));
            ui.rt.FillRectangle(&rect(-pw / 2.0, -ph / 2.0, pw / 2.0, ph / 2.0), &ui.brush(col));
        }
    }
    unsafe { ui.rt.SetTransform(&base); }
}

/// Una cuenta en su caja `r` (rejillas y lista): nombre y fecha arriba, número grande con «días» y la
/// barra fina abajo. `sc` escala el número; `a` es la entrada (0..1).
fn cd_cell(ui: &Ui, d: &DFonts, r: D2D_RECT_F, c: &Cd, today: i64, sc: f32, a: f32) {
    if a <= 0.01 { return; }
    popup::push_alpha(ui.rt, a);
    let days = c.date - today;
    let date = fmt_date_short(c.date, today);
    let dw = ui.p.width(&ui.f.sub, &date) + 10.0;
    ui.text(&d.title, &fit(ui, &d.title, &c.name, r.right - r.left - dw), r.left, r.top, 22.0, &ui.white);
    ui.text_r(&ui.f.sub, &date, r.right, r.top, 22.0, &ui.dim);
    let num = if days == 0 { i18n::t("¡Hoy!").to_string() } else { days.to_string() };
    let lbl = if days == 1 { i18n::t("día") } else { i18n::t("días") };
    // Lo más grande que quepa (sin pasar de `sc`) con «días» al lado
    let lw = if days != 0 { ui.p.width(&ui.f.name, lbl) + 8.0 } else { 0.0 };
    let sc = sc.min((r.right - r.left - lw) / ui.p.width(&d.huge, &num).max(1.0));
    let top = r.top + 28.0;
    let base = cur_transform(ui);
    unsafe { ui.rt.SetTransform(&mul(&scale_at(sc, r.left, top), &base)); }
    ui.text(&d.huge, &num, r.left, top, 44.0, &ui.white);
    unsafe { ui.rt.SetTransform(&base); }
    if days != 0 { ui.text(&ui.f.name, lbl, r.left + ui.p.width(&d.huge, &num) * sc + 8.0, top + 44.0 * sc - 29.0, 22.0, &ui.dim); }
    let frac = progress(today, c.from.unwrap_or(today), c.date) * a;
    let by = r.bottom - 4.0;
    unsafe {
        ui.rt.FillRoundedRectangle(&rrect(rect(r.left, by, r.right, by + 4.0), 2.0), &ui.track);
        if frac > 0.003 { ui.rt.FillRoundedRectangle(&rrect(rect(r.left, by, r.left + ((r.right - r.left) * frac).max(4.0), by + 4.0), 2.0), &ui.on); }
        ui.rt.PopLayer();
    }
}

/// Una cuenta en una fila de la lista (Grande y Columna): nombre y fecha a la izquierda, días a la derecha.
fn cd_row(ui: &Ui, d: &DFonts, x0: f32, x1: f32, y: f32, c: &Cd, today: i64, a: f32, hot: f32) {
    if a <= 0.01 { return; }
    popup::push_alpha(ui.rt, a);
    ui.card_k(rect(x0 - 10.0, y - 4.0, x1 + 10.0, y + CD_ROW - 6.0), hot);
    let days = c.date - today;
    let (num, lbl) = if days == 0 { (i18n::t("¡Hoy!").to_string(), "") } else { (days.to_string(), if days == 1 { i18n::t("día") } else { i18n::t("días") }) };
    let lw = if lbl.is_empty() { 0.0 } else { ui.p.width(&ui.f.sub, lbl) + 6.0 };
    let nw = ui.p.width(&d.big, &num);
    ui.text_r(&ui.f.sub, lbl, x1, y + 8.0, 22.0, &ui.dim);
    ui.text_r(&d.big, &num, x1 - lw, y + 2.0, 28.0, &ui.white);
    let tw = x1 - x0 - nw - lw - 14.0;
    ui.text(&ui.f.name, &fit(ui, &ui.f.name, &c.name, tw), x0, y + 2.0, 20.0, &ui.white);
    ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &fmt_date_short(c.date, today), tw), x0, y + 21.0, 18.0, &ui.dim);
    let frac = progress(today, c.from.unwrap_or(today), c.date) * a;
    let by = y + CD_ROW - 14.0;
    unsafe {
        ui.rt.FillRoundedRectangle(&rrect(rect(x0, by, x1, by + 3.0), 1.5), &ui.track);
        if frac > 0.003 { ui.rt.FillRoundedRectangle(&rrect(rect(x0, by, x0 + ((x1 - x0) * frac).max(3.0), by + 3.0), 1.5), &ui.on); }
        ui.rt.PopLayer();
    }
}

/// Alto de cada fila de la lista (Grande y Columna).
const CD_ROW: f32 = 58.0;
/// Alto de la cuenta destacada (la de arriba en Pequeño, Mediano, Grande y Columna).
const CD_FEAT: f32 = 170.0;

/// Cajas de las rejillas (Banda: 4 en línea; Alto: 2 apiladas; Extragrande: 2×2). None = destacada (+ lista).
fn cd_grid(l: desk::Lay) -> Option<Vec<D2D_RECT_F>> {
    let (w, h) = (l.w, l.h);
    match l.sz {
        desk::Sz::Banda => { let cw = (w - 2.0 * PAD - 3.0 * 28.0) / 4.0; Some((0..4).map(|i| { let x = PAD + i as f32 * (cw + 28.0); rect(x, PAD - 4.0, x + cw, h - 34.0) }).collect()) }
        desk::Sz::Alto => { let ch = (h - 2.0 * PAD - 30.0 - 24.0) / 2.0; Some((0..2).map(|i| { let y = PAD - 4.0 + i as f32 * (ch + 24.0); rect(PAD, y, w - PAD, y + ch) }).collect()) }
        desk::Sz::Extragrande => {
            let (cw, ch) = ((w - 2.0 * PAD - 40.0) / 2.0, (h - 2.0 * PAD - 30.0 - 28.0) / 2.0);
            Some((0..4).map(|i| { let (x, y) = (PAD + (i % 2) as f32 * (cw + 40.0), PAD - 4.0 + (i / 2) as f32 * (ch + 28.0)); rect(x, y, x + cw, y + ch) }).collect())
        }
        _ => None,
    }
}

impl Widget for CountdownW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let l = desk::lay(Kind::Countdown);
        let (w, h) = (l.w, l.h);
        self.refresh();
        let mut moving = false;
        let up = upcoming(&self.items, self.today);
        if up.is_empty() {
            header(ui, d, i18n::t("Cuenta atrás"), "", w);
            let small = l.sz == desk::Sz::Pequeno || l.sz == desk::Sz::Alto;
            let msg = if self.items.is_empty() { if small { i18n::t("Clic derecho: añadir") } else { i18n::t("Clic derecho para añadir una fecha") } } else { i18n::t("No quedan fechas por llegar") };
            ui.glyph(&d.glyph_l, "\u{E787}", w / 2.0 - 20.0, h / 2.0 - 30.0, 40.0, 40.0, &ui.faint);
            ui.text_c(&ui.f.sub, msg, w / 2.0, h / 2.0 + 14.0, 18.0, &ui.dim);
            ui.hit(rect(0.0, 0.0, w, h), 1);
            self.moving = false;
            return;
        }
        // Rejillas: todas a la vista (por páginas si hay más que cajas)
        if let Some(cells) = cd_grid(l) {
            self.grid = true;
            let per = cells.len();
            let pages = up.len().div_ceil(per);
            self.page %= pages.max(1);
            let sc = BIG;
            for (j, r) in cells.iter().enumerate() {
                let Some(&ix) = up.get(self.page * per + j) else { break };
                let a = ez(t, 0.08 + 0.07 * j as f32, 0.5) * self.page_k();
                cd_cell(ui, d, *r, &self.items[ix], self.today, sc, a);
                ui.hit(*r, 1);
            }
            if pages > 1 {
                moving |= approach(&mut self.dot, self.page as f32, dt, 0.09);
                crate::news::page_dots(ui, pages, self.dot, w / 2.0, h - 16.0);
            }
            moving |= self.page_k() < 1.0;
            self.moving = moving || t < 1.0;
            return;
        }
        self.grid = false;
        let fh = CD_FEAT;
        self.sel %= up.len();
        let c = self.items[up[self.sel]].clone();
        let days = c.date - self.today;
        // Es hoy: confeti una vez (al verlo por primera vez o al volver a él)
        let key = (c.name.clone(), c.date);
        if days == 0 && self.partied.as_ref() != Some(&key) { self.party = Some(Instant::now()); self.partied = Some(key); }
        let white = ui.white.clone();
        let dim = ui.dim.clone();
        let title = d.title.clone();
        let date = fmt_date_short(c.date, self.today);
        ui.text_r(&ui.f.sub, &date, w - PAD, PAD - 4.0, 22.0, &dim);
        // El nombre, recortado para no pisar la fecha
        let name = fit(ui, &title, &c.name, w - 2.0 * PAD - ui.p.width(&ui.f.sub, &date) - 10.0);
        self.r_name.draw(ui, &title, &name, PAD, PAD - 4.0, 22.0, &white, self.up);
        let num = if days == 0 { i18n::t("¡Hoy!").to_string() } else { days.to_string() };
        let lbl = if days == 1 { i18n::t("día") } else { i18n::t("días") };
        // Número grande (escalado: más grande que cualquier fuente de los widgets); en Pequeño, lo que quepa
        let lw = if days != 0 { ui.p.width(&ui.f.name, lbl) + 10.0 } else { 0.0 };
        let big = BIG.min((w - 2.0 * PAD - lw) / ui.p.width(&d.huge, &num).max(1.0));
        let k = ez(t, 0.1, 0.6);
        let base = cur_transform(ui);
        unsafe { ui.rt.SetTransform(&mul(&scale_at(big * (0.94 + 0.06 * k), PAD, NUM_TOP), &base)); }
        let huge = d.huge.clone();
        self.r_num.draw(ui, &huge, &num, PAD, NUM_TOP, 44.0, &white, self.up);
        unsafe { ui.rt.SetTransform(&base); }
        moving |= self.r_num.busy() || self.r_name.busy();
        if days != 0 {
            let lx = PAD + ui.p.width(&d.huge, &num) * big + 10.0;
            if self.lab_x == 0.0 { self.lab_x = lx; }
            moving |= approach(&mut self.lab_x, lx, dt, 0.08);
            ui.text(&ui.f.name, lbl, self.lab_x, NUM_TOP + 44.0 * big - 29.0, 22.0, &dim);
        }
        // Barra fina: lo que va desde que se creó la cuenta
        let frac = progress(self.today, c.from.unwrap_or(self.today), c.date) * ez(t, 0.2, 0.8);
        moving |= approach(&mut self.bar, frac, dt, 0.2);
        let (x0, x1, by) = (PAD, w - PAD, fh - 46.0);
        unsafe {
            ui.rt.FillRoundedRectangle(&rrect(rect(x0, by, x1, by + 4.0), 2.0), &ui.track);
            if self.bar > 0.003 { ui.rt.FillRoundedRectangle(&rrect(rect(x0, by, x0 + ((x1 - x0) * self.bar).max(4.0), by + 4.0), 2.0), &ui.on); }
        }
        ui.hit(rect(0.0, 0.0, w, by - 4.0), 1);
        // Grande y Columna: debajo, las siguientes en lista (un clic la pasa arriba)
        self.list.clear();
        if h > fh + CD_ROW {
            let rows = ((h - fh - 6.0) / CD_ROW).floor() as usize;
            let others: Vec<usize> = (1..up.len()).map(|o| (self.sel + o) % up.len()).take(rows).collect();
            unsafe { ui.rt.FillRectangle(&rect(PAD, fh - 10.0, w - PAD, fh - 10.0 + 1.0 / ui.s), &ui.wa(0.08)); }
            for (j, &si) in others.iter().enumerate() {
                let id = 100 + j as u32;
                let y = fh + 4.0 + j as f32 * CD_ROW;
                let a = ez(t, 0.2 + 0.07 * j as f32, 0.5);
                cd_row(ui, d, PAD, w - PAD, y, &self.items[up[si]], self.today, a, ui.heat(id));
                ui.hit(rect(PAD - 10.0, y - 4.0, w - PAD + 10.0, y + CD_ROW - 6.0), id);
                self.list.push(si);
            }
            if others.is_empty() {
                ui.text_c(&ui.f.sub, i18n::t("Clic derecho en el número para añadir más fechas"), w / 2.0, fh + 10.0, 18.0, &ui.faint);
            }
        }
        // Puntos para pasar a las demás fechas (el de ahora es una píldora); con lista no hacen falta
        let n = up.len();
        if n > 1 && self.list.is_empty() {
            let gap = 12.0;
            let dy = fh - 22.0;
            let x_of = |i: f32| w / 2.0 - (n as f32 - 1.0) * gap / 2.0 + i * gap;
            moving |= approach(&mut self.dot, self.sel as f32, dt, 0.09);
            unsafe {
                for i in 0..n {
                    let id = 10 + i as u32;
                    let x = x_of(i as f32);
                    ui.rt.FillEllipse(&ellipse(x, dy, 3.0), &ui.wa(0.32 + 0.3 * ui.heat(id)));
                    ui.hit(rect(x - gap / 2.0, dy - 9.0, x + gap / 2.0, dy + 9.0), id);
                }
                let px = x_of(self.dot);
                ui.rt.FillRoundedRectangle(&rrect(rect(px - 7.0, dy - 3.0, px + 7.0, dy + 3.0), 3.0), &ui.white);
            }
        }
        if let Some(e) = self.party_k() {
            confetti(ui, PAD + ui.p.width(&d.huge, &num) * big / 2.0, NUM_TOP + 34.0, e);
            moving = true;
        }
        self.moving = moving || t < 1.0;
    }
    fn layouts(&self) -> bool { true }
    fn click(&mut self, id: Option<u32>, right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        let id = id?;
        if right {
            // Clic derecho en el número o el nombre: editar las fechas (en la barra y los puntos, el menú de siempre)
            if id != 1 { return None; }
            let td = today();
            let ex = format!("{}|{}", i18n::t("Vacaciones"), fmt_date_iso(td + 30));
            return Some(Box::new(move || edit_ini("cuenta_atras", ex)));
        }
        let n = upcoming(&self.items, self.today).len().max(1);
        if self.grid {
            // Rejillas: los puntos pasan de página
            if (10..100).contains(&id) && (id - 10) as usize != self.page { self.page = (id - 10) as usize; self.page_at = Some(Instant::now()); self.moving = true; }
            return None;
        }
        let old = self.sel;
        match id {
            1 => self.sel = (self.sel + 1) % n,
            i if i >= 100 => { if let Some(&s) = self.list.get((i - 100) as usize) { self.sel = s; } }
            i if i >= 10 => self.sel = ((i - 10) as usize).min(n - 1),
            _ => {}
        }
        // Al pasar a una fecha más lejana el número entra por abajo; a una más cercana, por arriba
        self.up = self.sel >= old;
        // Volver a pulsar en «¡Hoy!» repite el confeti
        if self.sel == old { self.partied = None; }
        self.moving = true;
        None
    }
    fn tick(&mut self) -> bool { self.refresh() }
    fn animating(&self, t: f32) -> bool { t < 1.4 || self.moving }
    fn smooth(&self) -> bool { self.moving }
    fn frame_ms(&self) -> u32 { 16 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fechas_de_ida_y_vuelta() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        for z in [-800_000i64, -1, 0, 59, 60, 11016, 20_000, 2_000_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(parse_date("2026-12-20"), Some(days_from_civil(2026, 12, 20)));
        assert_eq!(parse_date(" 2024-02-29 "), Some(days_from_civil(2024, 2, 29)));
        assert_eq!(parse_date("2025-02-29"), None);
        assert_eq!(parse_date("2026-13-01"), None);
        assert_eq!(parse_date("2026-12"), None);
        assert_eq!(parse_date("hola"), None);
        assert_eq!(fmt_date_iso(days_from_civil(2026, 1, 5)), "2026-01-05");
    }

    #[test]
    fn dias_que_faltan_y_progreso() {
        let hoy = days_from_civil(2026, 11, 22);
        assert_eq!(days_from_civil(2026, 12, 20) - hoy, 28);
        assert_eq!(days_from_civil(2027, 1, 1) - hoy, 40);
        assert_eq!(progress(hoy, hoy, hoy + 10), 0.0);
        assert_eq!(progress(hoy, hoy - 5, hoy + 5), 0.5);
        assert_eq!(progress(hoy, hoy - 50, hoy - 1), 1.0);
        // Fecha de inicio posterior o igual a la final: completa (sin dividir por cero)
        assert_eq!(progress(hoy, hoy, hoy), 1.0);
    }

    #[test]
    fn cadena_de_cuentas_atras() {
        let l = parse_countdowns("Vacaciones|2026-12-20, Navidad | 2026-12-25 | 2026-11-01 ,malo|2026-02-30, |2026-01-01, sinfecha");
        assert_eq!(l.len(), 2);
        assert_eq!(l[0], Cd { name: "Vacaciones".into(), date: days_from_civil(2026, 12, 20), from: None });
        assert_eq!(l[1].name, "Navidad");
        assert_eq!(l[1].from, Some(days_from_civil(2026, 11, 1)));
        assert_eq!(fmt_countdowns(&l), "Vacaciones|2026-12-20, Navidad|2026-12-25|2026-11-01");
        assert_eq!(parse_countdowns(&fmt_countdowns(&l)), l);
        // Las pasadas no cuentan; las demás, de la más cercana a la más lejana (hoy incluido)
        let hoy = days_from_civil(2026, 12, 21);
        let l = parse_countdowns("A|2026-12-20, B|2027-03-01, C|2026-12-21, D|2026-12-30");
        assert_eq!(upcoming(&l, hoy), vec![2, 3, 1]);
    }

    #[test]
    fn cajas_de_la_cuenta_atras() {
        use desk::{Lay, Sz};
        for sz in Sz::ALL {
            let (w, h) = sz.dims();
            match cd_grid(Lay { w, h, sz }) {
                Some(v) => {
                    assert_eq!(v.len(), match sz { Sz::Alto => 2, _ => 4 }, "{sz:?}");
                    for a in &v { assert!(a.left >= PAD - 0.01 && a.right <= w - PAD + 0.01 && a.top >= 0.0 && a.bottom <= h - 20.0 && a.bottom - a.top >= 110.0, "{sz:?}"); }
                }
                // Destacada arriba (y la lista debajo si cabe)
                None => assert!(matches!(sz, Sz::Pequeno | Sz::Mediano | Sz::Grande | Sz::Columna), "{sz:?}"),
            }
        }
    }

    #[test]
    fn archivo_de_tareas() {
        let t = parse_tasks("[x] Hecha\n[ ] Pendiente\r\n\n   \nSin prefijo\n[X]   Mayúscula  \n[ ]\n[]Pegada");
        assert_eq!(t, vec![(true, "Hecha".into()), (false, "Pendiente".into()), (false, "Sin prefijo".into()), (true, "Mayúscula".into()), (false, "Pegada".into())]);
        let s = serialize_tasks(&t);
        assert_eq!(s, "[x] Hecha\n[ ] Pendiente\n[ ] Sin prefijo\n[x] Mayúscula\n[ ] Pegada\n");
        assert_eq!(parse_tasks(&s), t);
        // Un salto de línea dentro de una tarea no parte el archivo
        assert_eq!(serialize_tasks(&[(false, "a\nb".into())]), "[ ] a b\n");
        assert!(parse_tasks("").is_empty());
    }

    #[test]
    fn cadena_de_relojes() {
        let z = parse_zones("Nueva York|America/New_York, Tokio|Asia/Tokyo ,roto, |Europe/Paris, Londres|Europe/London, A|B, C|D");
        assert_eq!(z.len(), 5);
        // Ocho como mucho (los que caben en Extragrande); las de por defecto, justo ocho
        assert_eq!(parse_zones(&"A|B, ".repeat(12)).len(), MAX_ZONES);
        assert_eq!(parse_zones(&default_zones()).len(), MAX_ZONES);
        for s in desk::Sz::ALL { let (c, r) = clock_grid(s); assert!(c * r <= MAX_ZONES); }
        assert_eq!(z[0], ("Nueva York".into(), "America/New_York".into()));
        assert_eq!(z[2], ("Londres".into(), "Europe/London".into()));
        assert!(parse_zones("").is_empty());
    }

    #[test]
    fn diferencia_horaria() {
        let madrid = [2026, 11, 22, 13, 5, 0];
        assert_eq!(offset_min(&madrid, &[2026, 11, 22, 7, 5, 0]), -360);
        assert_eq!(offset_min(&madrid, &[2026, 11, 22, 21, 5, 0]), 480);
        // Al otro lado de la medianoche (y de fin de año)
        assert_eq!(offset_min(&[2026, 12, 31, 23, 30, 0], &[2027, 1, 1, 8, 30, 0]), 540);
        assert_eq!(offset_min(&madrid, &[2026, 11, 22, 17, 35, 0]), 270);
        // Leídas con un instante de diferencia (cambia el minuto entre medias): se redondea
        assert_eq!(offset_min(&madrid, &[2026, 11, 22, 19, 6, 0]), 360);
        assert_eq!(offset_min(&madrid, &madrid), 0);
        assert_eq!(fmt_offset(360), "+6 h");
        assert_eq!(fmt_offset(-300), "−5 h");
        assert_eq!(fmt_offset(330), "+5:30 h");
        assert_eq!(fmt_offset(-570), "−9:30 h");
    }

    #[test]
    fn dia_y_noche() {
        assert_eq!(daylight(12.0), 1.0);
        assert_eq!(daylight(0.0), 0.0);
        assert_eq!(daylight(23.5), 0.0);
        assert!(daylight(6.5) > 0.2 && daylight(6.5) < 0.8);
        assert!(daylight(19.5) > 0.2 && daylight(19.5) < 0.8);
    }
}
