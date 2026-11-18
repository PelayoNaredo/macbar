//! Panel del tiempo: ahora, próximas horas y próximos días (Open-Meteo), con iconos dibujados.

use super::*;
use popup::{Content, Re, Ui};

const W: f32 = 320.0;
const PAD: f32 = 14.0;
const ID_REFRESH: u32 = 1;
const ID_CITY: u32 = 2;

#[derive(Clone)]
pub struct Forecast {
    /// (hora, temperatura, código, de día)
    pub hours: Vec<(u32, f32, i32, bool)>,
    /// (día de la semana 0=domingo, código, mínima, máxima)
    pub days: Vec<(u32, i32, f32, f32)>,
    pub at: Instant,
}

pub static FORECAST: Mutex<Option<Forecast>> = Mutex::new(None);

fn section<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let k = format!("\"{key}\":{{");
    let s = &body[body.find(&k)? + k.len()..];
    Some(&s[..s.find('}')?])
}

fn arr<'a>(sec: &'a str, key: &str) -> Vec<&'a str> {
    let k = format!("\"{key}\":[");
    let Some(i) = sec.find(&k) else { return Vec::new() };
    let s = &sec[i + k.len()..];
    let Some(e) = s.find(']') else { return Vec::new() };
    s[..e].split(',').map(|x| x.trim().trim_matches('"')).collect()
}

fn nums(sec: &str, key: &str) -> Vec<f32> { arr(sec, key).iter().map(|x| x.parse().unwrap_or(f32::NAN)).collect() }

/// Lee la respuesta de Open-Meteo (con `timezone=auto`, las horas ya vienen en hora local).
pub fn parse(body: &str) -> Option<Forecast> {
    let cur_time = section(body, "current").map(|c| arr_str_field(c, "time")).unwrap_or_default();
    let h = section(body, "hourly")?;
    let times = arr(h, "time");
    let temps = nums(h, "temperature_2m");
    let codes = nums(h, "weather_code");
    let isday = nums(h, "is_day");
    // Desde la hora actual
    let now_prefix = cur_time.get(..13).unwrap_or("").to_string();
    let start = times.iter().position(|t| t.get(..13) == Some(now_prefix.as_str())).unwrap_or(0);
    let hours = (start..(start + 7).min(times.len())).filter_map(|i| {
        let hr: u32 = times[i].get(11..13)?.parse().ok()?;
        Some((hr, *temps.get(i)?, *codes.get(i)? as i32, isday.get(i).copied().unwrap_or(1.0) > 0.5))
    }).collect();
    let d = section(body, "daily")?;
    let dt = arr(d, "time");
    let dc = nums(d, "weather_code");
    let dmin = nums(d, "temperature_2m_min");
    let dmax = nums(d, "temperature_2m_max");
    let days = (0..dt.len().min(6)).filter_map(|i| {
        let t = dt[i];
        let (y, m, dd) = (t.get(0..4)?.parse().ok()?, t.get(5..7)?.parse().ok()?, t.get(8..10)?.parse().ok()?);
        Some((calendar::weekday(y, m, dd), *dc.get(i)? as i32, *dmin.get(i)?, *dmax.get(i)?))
    }).collect();
    Some(Forecast { hours, days, at: Instant::now() })
}

fn arr_str_field(sec: &str, key: &str) -> String {
    let k = format!("\"{key}\":\"");
    sec.find(&k).and_then(|i| { let s = &sec[i + k.len()..]; s.find('"').map(|e| s[..e].to_string()) }).unwrap_or_default()
}

/// Icono del tiempo: el glifo (sol, luna o nube) y, encima, lluvia, nieve, niebla o rayo dibujados.
pub fn draw_icon(rt: &ID2D1HwndRenderTarget, p: &Painter, f: &IDWriteTextFormat, x: f32, top: f32, bw: f32, bh: f32, code: i32, day: bool, b: &ID2D1Brush) {
    draw_icon_anim(rt, p, f, x, top, bw, bh, code, day, b, None);
}

/// Como `draw_icon`, pero con `anim = Some(segundos)` el icono se mueve: el sol gira despacio,
/// la lluvia y la nieve caen, la niebla se desliza y el rayo destella.
pub fn draw_icon_anim(rt: &ID2D1HwndRenderTarget, p: &Painter, f: &IDWriteTextFormat, x: f32, top: f32, bw: f32, bh: f32, code: i32, day: bool, b: &ID2D1Brush, anim: Option<f32>) {
    let size = unsafe { f.GetFontSize() };
    let extra = matches!(code, 45 | 48 | 51..=67 | 71..=86 | 95..=99);
    let base = match code { 0 | 1 => if day { "\u{E706}" } else { "\u{E708}" }, _ => "\u{E753}" };
    let [l, t, iw, ih] = p.ink(f, base);
    // Con algo debajo (gotas, copos…) la nube sube un poco
    let lift = if extra { size * 0.16 } else { 0.0 };
    let gy = p.snap(top + bh / 2.0 - ih / 2.0 - t - lift);
    let ph = anim.unwrap_or(0.0);
    let spin = anim.is_some() && matches!(code, 0 | 1) && day;
    let mut old = popup::IDENTITY;
    if spin {
        let (cx, cy) = (x + bw / 2.0, gy + t + ih / 2.0);
        unsafe {
            rt.GetTransform(&mut old);
            let a = ph * 0.35;
            let (s, c) = a.sin_cos();
            let rot = Matrix3x2 { M11: c, M12: s, M21: -s, M22: c, M31: cx - c * cx + s * cy, M32: cy - s * cx - c * cy };
            rt.SetTransform(&(rot * old));
        }
    }
    // Tormenta: la nube destella de vez en cuando
    let flash = if anim.is_some() && (95..=99).contains(&code) { let m = ph % 2.6; if m < 0.08 || (0.18..0.24).contains(&m) { 1.0 } else { 0.7 } } else { 1.0 };
    unsafe { b.SetOpacity(flash); }
    p.draw_at(f, base, x + (bw - iw) / 2.0 - l, gy, b, None);
    unsafe { b.SetOpacity(1.0); }
    if spin { unsafe { rt.SetTransform(&old); } }
    if code == 2 {
        // Parcialmente nuboso: un sol pequeño asomando arriba a la derecha
        let r = size * 0.13;
        let (cx, cy) = (x + bw / 2.0 + iw * 0.36, gy + t + ih * 0.12);
        unsafe { rt.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r }, b, size * 0.07, None); }
    }
    if !extra { return; }
    let cx = x + bw / 2.0;
    let by = gy + t + ih + size * 0.04; // justo bajo la nube
    let sw = (size * 0.075).max(1.0);
    unsafe {
        // Caída en bucle: cada gota/copo baja y se desvanece con su propio desfase
        let fall = |k: f32, speed: f32| -> (f32, f32) {
            match anim { Some(t) => { let u = (t * speed + (k + 1.0) * 0.37).fract(); (u, (1.0 - u) * (u * 6.0).min(1.0)) } None => (0.0, 1.0) }
        };
        match code {
            45 | 48 => for (i, wd) in [0.55f32, 0.4].iter().enumerate() {
                let yy = by + i as f32 * size * 0.14;
                let dx = anim.map(|t| (t * 1.4 + i as f32 * 2.0).sin() * size * 0.06).unwrap_or(0.0);
                rt.DrawLine(Vector2 { X: cx + dx - size * wd / 2.0, Y: yy }, Vector2 { X: cx + dx + size * wd / 2.0, Y: yy }, b, sw, None);
            },
            71..=77 | 85 | 86 => for k in [-1.0f32, 0.0, 1.0] {
                let (u, a) = fall(k, 0.45);
                let drift = anim.map(|t| (t * 2.0 + k * 1.7).sin() * size * 0.03).unwrap_or(0.0);
                b.SetOpacity(a);
                let e = D2D1_ELLIPSE { point: Vector2 { X: cx + k * size * 0.2 + drift, Y: by + size * 0.08 + (k.abs() * size * 0.06) + u * size * 0.16 }, radiusX: sw * 0.9, radiusY: sw * 0.9 };
                rt.FillEllipse(&e, b);
                b.SetOpacity(1.0);
            },
            95..=99 => {
                let pts = [(0.05, -0.02), (-0.08, 0.14), (0.04, 0.14), (-0.06, 0.3)];
                for w in pts.windows(2) {
                    rt.DrawLine(Vector2 { X: cx + w[0].0 * size, Y: by + w[0].1 * size }, Vector2 { X: cx + w[1].0 * size, Y: by + w[1].1 * size }, b, sw, None);
                }
            }
            _ => for k in [-1.0f32, 0.0, 1.0] {
                let x0 = cx + k * size * 0.2;
                let n = if (51..=57).contains(&code) { 0.1 } else { 0.18 };
                let (u, a) = fall(k, 1.1);
                let dy = u * size * 0.14;
                b.SetOpacity(a);
                rt.DrawLine(Vector2 { X: x0 + size * 0.04 - u * size * 0.03, Y: by + dy }, Vector2 { X: x0 - size * 0.03 - u * size * 0.03, Y: by + dy + size * n }, b, sw, None);
                b.SetOpacity(1.0);
            },
        }
    }
}

/// `opened`: cuándo se abrió (para la entrada en cascada); `spin`: giro del botón de actualizar.
pub struct WeatherPanel { loading: bool, opened: Instant, spin: f32, last: Instant }

impl WeatherPanel { pub fn new() -> Self { WeatherPanel { loading: false, opened: Instant::now(), spin: 0.0, last: Instant::now() } } }

/// Progreso 0..1 de un elemento que entra con `delay` segundos de retraso.
fn stagger(since: f32, delay: f32, dur: f32) -> f32 { popup::ease_out(((since - delay) / dur).clamp(0.0, 1.0)) }

const DAY_ROW: f32 = 30.0;

impl Content for WeatherPanel {
    fn size(&self) -> (f32, f32) {
        let days = FORECAST.lock().unwrap().as_ref().map(|f| f.days.len()).unwrap_or(0) as f32;
        if days == 0.0 { return (W, PAD + 76.0 + 12.0 + 34.0 + PAD); }
        (W, PAD + 76.0 + 12.0 + 96.0 + 12.0 + days * DAY_ROW + 16.0 + 12.0 + 34.0 + PAD)
    }
    fn shown(&mut self) { self.loading = false; self.opened = Instant::now(); self.spin = 0.0; }
    fn data(&mut self) { if self.loading { self.opened = Instant::now(); } self.loading = false; }
    fn tick(&mut self) -> bool { true }
    /// Fotogramas durante la entrada, mientras gira el botón y si el icono grande tiene movimiento.
    fn animating(&self) -> bool {
        let moving_icon = WEATHER.lock().unwrap().is_some_and(|w| (matches!(w.code, 0 | 1) && w.is_day) || matches!(w.code, 45 | 48 | 51..=67 | 71..=86 | 95..=99));
        self.opened.elapsed().as_secs_f32() < 1.0 || self.loading || self.spin > 0.0 || moving_icon
    }

    fn paint(&mut self, ui: &mut Ui) {
        let dt = self.last.elapsed().as_secs_f32().min(1.0 / 30.0);
        self.last = Instant::now();
        let since = self.opened.elapsed().as_secs_f32();
        // El botón de actualizar gira mientras carga y, al terminar, frena hasta la vuelta completa
        let tau = std::f32::consts::TAU;
        if self.loading { self.spin += dt * 7.0; }
        else if self.spin > 0.0 {
            let rest = tau - self.spin % tau;
            let step = dt * (2.0 + 6.0 * rest / tau);
            if rest <= step { self.spin = 0.0; } else { self.spin += step; }
        }
        let wnow = *WEATHER.lock().unwrap();
        let fc = FORECAST.lock().unwrap().clone();
        let city = sys::config_value("ciudad").unwrap_or_else(|| "Tu ubicación".into());
        let mut y = PAD;
        // ---- Ahora
        match wnow {
            Some(wt) => {
                // El icono entra con un pequeño rebote de escala
                let pop = 0.7 + 0.3 * crate::ease_back((since / 0.45).min(1.0));
                let mut old = popup::IDENTITY;
                unsafe {
                    ui.rt.GetTransform(&mut old);
                    let (cx, cy) = (PAD + 24.0, y + 36.0);
                    let m = Matrix3x2 { M11: pop, M12: 0.0, M21: 0.0, M22: pop, M31: cx * (1.0 - pop), M32: cy * (1.0 - pop) };
                    ui.rt.SetTransform(&(m * old));
                }
                draw_icon_anim(ui.rt, &ui.p, &ui.f.icon_xl, PAD, y + 8.0, 48.0, 56.0, wt.code, wt.is_day, &ui.white, Some(since));
                unsafe { ui.rt.SetTransform(&old); }
                let temp = format!("{}°", wt.temp.round() as i32);
                ui.text(&ui.f.big, &temp, PAD + 58.0, y + 10.0, 52.0, &ui.white);
                ui.text_r(&ui.f.name, &city, W - PAD, y + 8.0, 22.0, &ui.white);
                ui.text_r(&ui.f.sub, weather_desc(wt.code), W - PAD, y + 30.0, 18.0, &ui.dim);
                if let Some(d0) = fc.as_ref().and_then(|f| f.days.first()) {
                    let mm = format!("Máx {}°  ·  Mín {}°", d0.3.round() as i32, d0.2.round() as i32);
                    ui.text_r(&ui.f.sub, &mm, W - PAD, y + 50.0, 18.0, &ui.dim);
                }
            }
            None => ui.text(&ui.f.name, "Cargando el tiempo…", PAD, y, 76.0, &ui.dim),
        }
        y += 76.0 + 12.0;

        if let Some(f) = &fc {
            if !f.days.is_empty() {
                // ---- Próximas horas
                ui.sep(y - 6.0);
                let n = f.hours.len().max(1) as f32;
                let cw = (W - PAD * 2.0 - 12.0) / n;
                for (i, (hr, t, code, day)) in f.hours.iter().enumerate() {
                    // Columnas en cascada de izquierda a derecha: suben 10 px con fundido
                    let k = stagger(since, 0.08 + i as f32 * 0.04, 0.32);
                    let y = y + 10.0 * (1.0 - k);
                    popup::push_alpha(ui.rt, k);
                    let cx = PAD + 6.0 + i as f32 * cw + cw / 2.0;
                    let lbl = if i == 0 { "Ahora".to_string() } else { format!("{hr}") };
                    ui.text_c(&ui.f.small, &lbl, cx, y + 8.0, 18.0, if i == 0 { &ui.white } else { &ui.dim });
                    draw_icon(ui.rt, &ui.p, &ui.f.icon_l, cx - 14.0, y + 30.0, 28.0, 30.0, *code, *day, &ui.white);
                    ui.text_c(&ui.f.lbl, &format!("{}°", t.round() as i32), cx, y + 66.0, 20.0, &ui.white);
                    unsafe { ui.rt.PopLayer(); }
                }
                y += 96.0 + 12.0;

                // ---- Próximos días, con barra de mínima a máxima sobre la escala de la semana
                let h = f.days.len() as f32 * DAY_ROW + 16.0;
                ui.sep(y - 6.0);
                let lo = f.days.iter().map(|d| d.2).fold(f32::MAX, f32::min);
                let hi = f.days.iter().map(|d| d.3).fold(f32::MIN, f32::max).max(lo + 1.0);
                let dias = ["Dom", "Lun", "Mar", "Mié", "Jue", "Vie", "Sáb"];
                let (bl, br) = (PAD + 150.0, W - PAD - 44.0);
                for (i, (wd, code, mn, mx)) in f.days.iter().enumerate() {
                    // Filas en cascada y la barra de temperaturas crece desde su centro
                    let k = stagger(since, 0.2 + i as f32 * 0.045, 0.34);
                    let grow = stagger(since, 0.3 + i as f32 * 0.045, 0.5);
                    popup::push_alpha(ui.rt, k);
                    let ry = y + 8.0 + i as f32 * DAY_ROW + 6.0 * (1.0 - k);
                    let name = if i == 0 { "Hoy" } else { dias[*wd as usize % 7] };
                    ui.text(&ui.f.lbl, name, PAD + 14.0, ry, DAY_ROW, &ui.white);
                    draw_icon(ui.rt, &ui.p, &ui.f.icon, PAD + 54.0, ry, 26.0, DAY_ROW, *code, true, &ui.white);
                    ui.text_r(&ui.f.sub, &format!("{}°", mn.round() as i32), bl - 8.0, ry, DAY_ROW, &ui.dim);
                    let cy = ry + DAY_ROW / 2.0;
                    unsafe {
                        ui.rt.FillRoundedRectangle(&rrect(rect(bl, cy - 2.5, br, cy + 2.5), 2.5), &ui.track);
                        let x0 = bl + (br - bl) * ((mn - lo) / (hi - lo));
                        let x1 = (bl + (br - bl) * ((mx - lo) / (hi - lo))).max(x0 + 6.0);
                        let (mid, half) = ((x0 + x1) / 2.0, ((x1 - x0) / 2.0 * grow).max(2.5));
                        let g = ui.p.gradient(Vector2 { X: bl, Y: 0.0 }, Vector2 { X: br, Y: 0.0 }, rgba(0.45, 0.75, 1.0, 1.0), rgba(1.0, 0.72, 0.35, 1.0));
                        ui.rt.FillRoundedRectangle(&rrect(rect(mid - half, cy - 2.5, mid + half, cy + 2.5), 2.5), &g);
                        // Hoy: un punto blanco marca la temperatura actual sobre la barra
                        if i == 0 {
                            if let Some(wt) = wnow {
                                let px = bl + (br - bl) * ((wt.temp - lo) / (hi - lo)).clamp(0.0, 1.0);
                                let r = 3.5 * grow;
                                if r > 0.3 {
                                    ui.rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: px, Y: cy }, radiusX: r + 1.5, radiusY: r + 1.5 }, &ui.brush(rgba(0.08, 0.08, 0.1, 0.9)));
                                    ui.rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: px, Y: cy }, radiusX: r, radiusY: r }, &ui.white);
                                }
                            }
                        }
                    }
                    ui.text_r(&ui.f.lbl, &format!("{}°", mx.round() as i32), W - PAD - 12.0, ry, DAY_ROW, &ui.white);
                    unsafe { ui.rt.PopLayer(); }
                }
                y += h + 12.0;
            }
        }

        // ---- Pie: cuándo se actualizó y acciones
        let when = if self.loading { "Actualizando…".to_string() } else {
            match fc.as_ref().map(|f| f.at.elapsed().as_secs() / 60) { Some(0) => "Actualizado ahora mismo".into(), Some(m) => format!("Actualizado hace {m} min"), None => String::new() }
        };
        ui.text(&ui.f.sub, &when, PAD + 2.0, y, 34.0, &ui.dim);
        let b2 = rect(W - PAD - 86.0, y, W - PAD, y + 34.0);
        ui.button(b2, None, "Ciudad…", ID_CITY, false);
        // Actualizar: botón redondo solo con icono (deja sitio al texto de la izquierda)
        let b1 = rect(b2.left - 8.0 - 34.0, y, b2.left - 8.0, y + 34.0);
        ui.button(b1, None, "", ID_REFRESH, self.loading);
        // El glifo se dibuja aparte para poder girarlo
        let (cx, cy) = ((b1.left + b1.right) / 2.0, (b1.top + b1.bottom) / 2.0);
        let mut old = popup::IDENTITY;
        unsafe {
            ui.rt.GetTransform(&mut old);
            let (s, c) = self.spin.sin_cos();
            let rot = Matrix3x2 { M11: c, M12: s, M21: -s, M22: c, M31: cx - c * cx + s * cy, M32: cy - s * cx - c * cy };
            ui.rt.SetTransform(&(rot * old));
        }
        ui.glyph(&ui.f.icon, "\u{E72C}", cx - 8.0, b1.top, 16.0, b1.bottom - b1.top, &ui.white);
        unsafe { ui.rt.SetTransform(&old); }
    }

    fn click(&mut self, id: u32, _right: bool) -> Re {
        match id {
            ID_REFRESH => { self.loading = true; WEATHER_NOW.store(true, Ordering::Relaxed); Re::Stay }
            ID_CITY => Re::CloseRun(Box::new(sys::edit_config)),
            _ => Re::Stay,
        }
    }
}
