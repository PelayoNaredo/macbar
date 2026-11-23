//! Diseños alternativos de los widgets (se eligen por widget: clic derecho → Diseño, o Configuración).
//! Cada uno es otro `Widget` con su propio tamaño (ver `desk::size`).

use super::*;
use desk::{DFonts, Kind, Sz};
use deskw::Widget;
use popup::Ui;

fn ez(t: f32, delay: f32, dur: f32) -> f32 { popup::ease_out(((t - delay) / dur).clamp(0.0, 1.0)) }

/// Colores del cielo (arriba, abajo) según el tiempo y si es de día, como el widget del Tiempo de iOS.
fn sky(code: i32, day: bool) -> ([f32; 3], [f32; 3]) {
    let cloudy = matches!(code, 2 | 3 | 45 | 48);
    let wet = matches!(code, 51..=67 | 80..=82 | 95..=99);
    let snow = matches!(code, 71..=77 | 85 | 86);
    match (day, cloudy, wet, snow) {
        (true, _, _, true) => ([0.62, 0.70, 0.80], [0.42, 0.52, 0.64]),
        (true, _, true, _) => ([0.33, 0.42, 0.52], [0.20, 0.27, 0.36]),
        (true, true, _, _) => ([0.45, 0.55, 0.66], [0.27, 0.36, 0.47]),
        (true, ..) => ([0.20, 0.48, 0.86], [0.11, 0.32, 0.68]),
        (false, _, true, _) | (false, true, ..) => ([0.12, 0.15, 0.21], [0.05, 0.07, 0.11]),
        (false, ..) => ([0.06, 0.10, 0.25], [0.02, 0.03, 0.10]),
    }
}

fn card_grad(ui: &Ui, w: f32, h: f32, top: [f32; 3], bot: [f32; 3], a: f32) {
    let g = ui.p.vgrad(0.0, h, rgba(top[0], top[1], top[2], a), rgba(bot[0], bot[1], bot[2], a));
    unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(0.0, 0.0, w, h), 22.0), &g); }
}

// ---------------------------------------------------------------- Tiempo: «Cielo»
/// Tarjeta con el degradado del cielo de ahora, temperatura grande, máx./mín. y las próximas horas.
#[derive(Default)]
pub struct SkyW;

/// Fila de `n` horas (hora, icono y temperatura) entre `x0` y `x1` desde `y`; entran en cascada.
#[allow(clippy::too_many_arguments)]
fn sky_hours(ui: &Ui, f: &weather::Forecast, x0: f32, x1: f32, y: f32, n: usize, t: f32, white: &ID2D1Brush, soft: &ID2D1Brush) {
    let cw = (x1 - x0) / n as f32;
    for (i, hr) in f.hours.iter().take(n).enumerate() {
        let ki = ez(t, 0.25 + i as f32 * 0.04, 0.4);
        let cx = x0 + i as f32 * cw + cw / 2.0;
        let y = y + 6.0 * (1.0 - ki);
        popup::push_alpha(ui.rt, ki);
        let label = if i == 0 { i18n::t("Ahora").to_string() } else { format!("{}", hr.0) };
        ui.text_c(&ui.f.small, &label, cx, y, 16.0, soft);
        weather::draw_icon(ui.rt, &ui.p, &ui.f.icon, cx - 9.0, y + 18.0, 18.0, 18.0, hr.2, hr.3, white);
        ui.text_c(&ui.f.lbl, &format!("{:.0}°", hr.1), cx, y + 38.0, 18.0, white);
        unsafe { ui.rt.PopLayer(); }
    }
}

impl Widget for SkyW {
    fn layouts(&self) -> bool { true }
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, _dt: f32) {
        let l = desk::lay(Kind::Weather);
        let (w, h) = (l.w, l.h);
        let city = sys::config_value("ciudad").unwrap_or_else(|| i18n::t("Tu ubicación").into());
        let cur = *WEATHER.lock().unwrap();
        let fc = weather::FORECAST.lock().unwrap().clone();
        let (top, bot) = cur.map(|c| sky(c.code, c.is_day)).unwrap_or(([0.2, 0.25, 0.32], [0.1, 0.12, 0.16]));
        card_grad(ui, w, h, top, bot, 1.0);
        let white = ui.brush(rgba(1.0, 1.0, 1.0, 0.98));
        let soft = ui.brush(rgba(1.0, 1.0, 1.0, 0.78));
        // En el pequeño, la ciudad deja sitio al icono de la derecha
        let city_w = if l.sz == Sz::Pequeno { w - 36.0 - 30.0 } else if l.sz == Sz::Banda { 200.0 } else { w - 36.0 - 40.0 };
        ui.text(&ui.f.name, &fit(ui, &ui.f.name, &city, city_w), 18.0, 14.0, 20.0, &white);
        let Some(cur) = cur else { ui.text(&ui.f.sub, i18n::t("Cargando el tiempo…"), 18.0, 44.0, 20.0, &soft); return; };
        // La temperatura sube desde abajo al aparecer
        let k = ez(t, 0.05, 0.6);
        let temp = format!("{:.0}°", cur.temp);
        ui.text(&d.huge, &temp, 16.0, 36.0 + 8.0 * (1.0 - k), 50.0, &white);
        let desc = crate::weather_desc(cur.code);
        let mm = fc.as_ref().and_then(|f| f.days.first()).map(|d0| i18n::tf("Máx {}°  Mín {}°", &[&format!("{:.0}", d0.3), &format!("{:.0}", d0.2)]));
        match l.sz {
            // Ciudad, temperatura con el icono, y abajo el cielo y la máx./mín.
            Sz::Pequeno => {
                weather::draw_icon_anim(ui.rt, &ui.p, &ui.f.icon_l, w - 42.0, 12.0, 26.0, 26.0, cur.code, cur.is_day, &white, Some(t));
                ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, desc, w - 36.0), 18.0, h - 56.0, 18.0, &white);
                if let Some(mm) = &mm { ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, mm, w - 36.0), 18.0, h - 36.0, 18.0, &soft); }
            }
            // A la izquierda lo de ahora y, tras una raya finísima, doce horas en línea
            Sz::Banda => {
                let tw = ui.p.width(&d.huge, &temp);
                weather::draw_icon_anim(ui.rt, &ui.p, &ui.f.icon_l, 16.0 + tw + 12.0, 52.0, 26.0, 26.0, cur.code, cur.is_day, &white, Some(t));
                ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, desc, 200.0), 18.0, h - 58.0, 18.0, &white);
                if let Some(mm) = &mm { ui.text(&ui.f.sub, mm, 18.0, h - 38.0, 18.0, &soft); }
                unsafe { ui.rt.FillRectangle(&rect(236.0, 22.0, 237.0, h - 22.0), &ui.brush(rgba(1.0, 1.0, 1.0, 0.18))); }
                match &fc {
                    Some(f) => sky_hours(ui, f, 250.0, w - 18.0, h / 2.0 - 30.0, 12, t, &white, &soft),
                    None => ui.text_c(&ui.f.sub, i18n::t("Sin previsión por ahora"), (250.0 + w - 18.0) / 2.0, h / 2.0 - 9.0, 18.0, &soft),
                }
            }
            // Arriba como el mediano; debajo, seis horas y los próximos cinco días
            _ => {
                weather::draw_icon_anim(ui.rt, &ui.p, &ui.f.icon_l, w - 42.0, 12.0, 26.0, 26.0, cur.code, cur.is_day, &white, Some(t));
                ui.text_r(&ui.f.sub, desc, w - 18.0, 42.0, 18.0, &white);
                if let Some(mm) = &mm { ui.text_r(&ui.f.sub, mm, w - 18.0, 60.0, 18.0, &soft); }
                // Sin previsión: se dice abajo (si no, media tarjeta vacía)
                let Some(f) = &fc else { ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, i18n::t("Sin previsión por ahora"), w - 36.0), 18.0, h - 40.0, 18.0, &soft); return };
                if l.sz != Sz::Grande { sky_hours(ui, f, 18.0, w - 18.0, h - 66.0, 6, t, &white, &soft); return; }
                sky_hours(ui, f, 18.0, w - 18.0, 96.0, 6, t, &white, &soft);
                unsafe { ui.rt.FillRectangle(&rect(18.0, 166.0, w - 18.0, 167.0), &ui.brush(rgba(1.0, 1.0, 1.0, 0.18))); }
                let days: Vec<_> = f.days.iter().take(5).collect();
                let lo = days.iter().map(|d| d.2).fold(f32::MAX, f32::min);
                let hi = days.iter().map(|d| d.3).fold(f32::MIN, f32::max);
                let rh = (h - 14.0 - 176.0) / 5.0;
                for (i, dd) in days.iter().enumerate() {
                    let ki = ez(t, 0.4 + i as f32 * 0.05, 0.4);
                    let y = 176.0 + i as f32 * rh + 4.0 * (1.0 - ki);
                    popup::push_alpha(ui.rt, ki);
                    let name = if i == 0 { i18n::t("Hoy").to_string() } else { i18n::weekday_short(dd.0 as usize).to_string() };
                    ui.text(&ui.f.lbl, &name, 18.0, y, rh, &white);
                    weather::draw_icon(ui.rt, &ui.p, &ui.f.icon, 72.0, y + rh / 2.0 - 9.0, 18.0, 18.0, dd.1, true, &white);
                    // Mínima, la franja del día dentro de la semana y la máxima
                    let (bx0, bx1) = (150.0, w - 66.0);
                    ui.text_r(&ui.f.sub, &format!("{:.0}°", dd.2), bx0 - 10.0, y, rh, &soft);
                    ui.text_r(&ui.f.lbl, &format!("{:.0}°", dd.3), w - 18.0, y, rh, &white);
                    let span = (hi - lo).max(1.0);
                    let (a, b) = (bx0 + (bx1 - bx0) * (dd.2 - lo) / span, bx0 + (bx1 - bx0) * (dd.3 - lo) / span);
                    let cy = y + rh / 2.0;
                    unsafe {
                        ui.rt.FillRoundedRectangle(&rrect(rect(bx0, cy - 2.0, bx1, cy + 2.0), 2.0), &ui.brush(rgba(1.0, 1.0, 1.0, 0.20)));
                        ui.rt.FillRoundedRectangle(&rrect(rect(a, cy - 2.0, b.max(a + 4.0), cy + 2.0), 2.0), &ui.brush(rgba(1.0, 1.0, 1.0, 0.85)));
                        ui.rt.PopLayer();
                    }
                }
            }
        }
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn animating(&self, t: f32) -> bool { t < 1.2 }
}

// ---------------------------------------------------------------- Tiempo: «Mínimo»
/// Solo la temperatura gigante en fino, una palabra y la ciudad; sin fondo.
#[derive(Default)]
pub struct MinWeatherW { r: deskw::Roll }

impl Widget for MinWeatherW {
    fn layouts(&self) -> bool { true }
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, t: f32, _dt: f32) {
        let l = desk::lay(Kind::Weather);
        let (w, h) = (l.w, l.h);
        let Some(cur) = *WEATHER.lock().unwrap() else { return };
        let k = ez(t, 0.0, 0.7);
        let vf = deskv_fonts();
        let f = vf.thin.clone();
        let temp = format!("{:.0}°", cur.temp);
        popup::push_alpha(ui.rt, k);
        let white = ui.white.clone();
        // La temperatura fina siempre arriba a la izquierda, con la palabra y la ciudad debajo
        let y0 = (h - 146.0) / 2.0;
        self.r.draw(ui, &f, &temp, 8.0, y0 + 4.0 + 10.0 * (1.0 - k), 96.0, &white, false);
        let word = crate::weather_desc(cur.code).to_lowercase();
        let city = sys::config_value("ciudad").unwrap_or_default();
        let lw = if l.sz == Sz::Pequeno { w - 28.0 } else { 190.0 };
        ui.text(&vf.word, &fit(ui, &vf.word, &word, lw), 14.0, y0 + 104.0, 26.0, &ui.white);
        if !city.is_empty() { ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &city, lw), 14.0, y0 + 128.0, 18.0, &ui.dim); }
        unsafe { ui.rt.PopLayer(); }
        let fc = weather::FORECAST.lock().unwrap().clone();
        let Some(fc) = fc else { return };
        match l.sz {
            // Las próximas horas, a la derecha y en columna: la hora en gris y los grados en blanco
            Sz::Mediano => {
                for (i, hr) in fc.hours.iter().skip(1).step_by(2).take(4).enumerate() {
                    let ki = ez(t, 0.2 + 0.06 * i as f32, 0.45);
                    popup::push_alpha(ui.rt, ki);
                    let y = 24.0 + i as f32 * 32.0 + 6.0 * (1.0 - ki);
                    ui.text_r(&ui.f.sub, &format!("{:02}:00", hr.0), w - 70.0, y, 26.0, &ui.dim);
                    ui.text_r(&vf.word, &format!("{:.0}°", hr.1), w - 16.0, y - 2.0, 30.0, &ui.white);
                    unsafe { ui.rt.PopLayer(); }
                }
            }
            // Los días que vienen, en línea: el día en gris, la máxima fina y la mínima apagada
            Sz::Banda => {
                let days: Vec<_> = fc.days.iter().skip(1).take(6).collect();
                if days.is_empty() { return; }
                let (x0, x1) = (250.0, w - 16.0);
                let cw = (x1 - x0) / days.len() as f32;
                for (i, dd) in days.iter().enumerate() {
                    let ki = ez(t, 0.2 + 0.05 * i as f32, 0.45);
                    popup::push_alpha(ui.rt, ki);
                    let cx = x0 + cw * i as f32 + cw / 2.0;
                    let y = h / 2.0 - 40.0 + 6.0 * (1.0 - ki);
                    ui.text_c(&ui.f.sub, &i18n::weekday_short(dd.0 as usize).to_lowercase(), cx, y, 18.0, &ui.dim);
                    ui.text_c(&vf.word, &format!("{:.0}°", dd.3), cx, y + 22.0, 30.0, &ui.white);
                    ui.text_c(&ui.f.sub, &format!("{:.0}°", dd.2), cx, y + 54.0, 18.0, &ui.faint);
                    unsafe { ui.rt.PopLayer(); }
                }
            }
            _ => {}
        }
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn animating(&self, t: f32) -> bool { t < 1.0 || self.r.busy() }
}

// ---------------------------------------------------------------- Calendario: «Hoy»
/// Cuadrado pequeño: el día de la semana en el color de acento y el número enorme (como el de iOS).
#[derive(Default)]
pub struct TodayW;

/// Próximas fechas de Cuenta atrás (nombre y cuándo) en filas de `rh` desde `y`, entre `x0` y `x1`.
/// Con `two` el cuándo va debajo del nombre (columnas estrechas); si no, a la derecha.
#[allow(clippy::too_many_arguments)]
fn next_dates(ui: &Ui, x0: f32, x1: f32, y: f32, rh: f32, n: usize, two: bool, t: f32) {
    let today = deskw3::today();
    let dates = deskw3::countdown_dates(today);
    ui.text(&ui.f.small, i18n::t("PRÓXIMAS FECHAS"), x0, y, 16.0, &ui.faint);
    let y = y + 24.0;
    if dates.is_empty() {
        let (a, b) = wrap2(ui, &ui.f.sub, i18n::t("Sin fechas a la vista"), x1 - x0);
        ui.text(&ui.f.sub, &a, x0, y, 18.0, &ui.dim);
        if !b.is_empty() { ui.text(&ui.f.sub, &b, x0, y + 18.0, 18.0, &ui.dim); }
        return;
    }
    for (i, (name, date)) in dates.iter().take(n).enumerate() {
        let ki = ez(t, 0.2 + 0.06 * i as f32, 0.4);
        popup::push_alpha(ui.rt, ki);
        let yy = y + i as f32 * rh + 4.0 * (1.0 - ki);
        let when = deskw::days_left(date - today);
        let wb = if *date == today { ui.brush(crate::profiles::accent(1.0)) } else { ui.dim.clone() };
        if two {
            ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, name, x1 - x0), x0, yy, 18.0, &ui.white);
            ui.text(&ui.f.sub, &when, x0, yy + 18.0, 18.0, &wb);
        } else {
            let ww = ui.p.width(&ui.f.sub, &when);
            ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, name, x1 - x0 - ww - 10.0), x0, yy, 18.0, &ui.white);
            ui.text_r(&ui.f.sub, &when, x1, yy, 18.0, &wb);
        }
        unsafe { ui.rt.PopLayer(); }
    }
}

/// Parte un texto en dos líneas que quepan en `maxw` (la segunda, recortada con «…»).
fn wrap2(ui: &Ui, f: &IDWriteTextFormat, s: &str, maxw: f32) -> (String, String) {
    if ui.p.width(f, s) <= maxw { return (s.to_string(), String::new()); }
    let words: Vec<&str> = s.split(' ').collect();
    let mut a = String::new();
    for (i, wd) in words.iter().enumerate() {
        let next = if a.is_empty() { wd.to_string() } else { format!("{a} {wd}") };
        if ui.p.width(f, &next) > maxw && !a.is_empty() { return (a, fit(ui, f, &words[i..].join(" "), maxw)); }
        a = next;
    }
    (fit(ui, f, &a, maxw), String::new())
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

impl Widget for TodayW {
    fn layouts(&self) -> bool { true }
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, t: f32, _dt: f32) {
        let l = desk::lay(Kind::Calendar);
        let now = unsafe { GetLocalTime() };
        let acc = crate::profiles::accent(1.0);
        let k = ez(t, 0.0, 0.6);
        // El bloque de siempre (día de la semana, mes, número y progreso del día) en un cuadrado de 170
        let (w, h) = (desk::CELL, desk::CELL);
        let wd = i18n::weekday(now.wDayOfWeek as usize).to_uppercase();
        ui.text(&ui.f.lbl, &wd, 18.0, 16.0, 18.0, &ui.brush(acc));
        let month = i18n::month(now.wMonth as usize + 11);
        ui.text(&ui.f.sub, &format!("{} {}", month, now.wYear), 18.0, 36.0, 16.0, &ui.dim);
        // El número cae desde arriba con un pequeño rebote
        let kb = crate::ease_back(ez(t, 0.1, 0.55));
        popup::push_alpha(ui.rt, k);
        ui.text(&deskv_fonts().day, &now.wDay.to_string(), 14.0, 58.0 - 16.0 * (1.0 - kb), 100.0, &ui.white);
        unsafe { ui.rt.PopLayer(); }
        // Progreso del día: una línea fina abajo
        let frac = (now.wHour as f32 * 60.0 + now.wMinute as f32) / 1440.0;
        unsafe {
            ui.rt.FillRoundedRectangle(&rrect(rect(18.0, h - 18.0, w - 18.0, h - 15.0), 1.5), &ui.track);
            ui.rt.FillRoundedRectangle(&rrect(rect(18.0, h - 18.0, 18.0 + (w - 36.0) * frac * k, h - 15.0), 1.5), &ui.brush(acc));
        }
        // Al lado (mediano) o debajo (alto), las próximas fechas de la cuenta atrás
        match l.sz {
            Sz::Mediano => {
                unsafe { ui.rt.FillRectangle(&rect(w, 22.0, w + 1.0 / ui.s.max(0.5), l.h - 22.0), &ui.card_edge); }
                // En dos líneas (nombre y cuándo): en una, los nombres se cortaban
                next_dates(ui, w + 18.0, l.w - 18.0, 18.0, 40.0, 3, true, t);
            }
            // Las que caben enteras (con su segunda línea) hasta el borde de abajo
            Sz::Alto => next_dates(ui, 18.0, l.w - 18.0, h + 8.0, 44.0, ((l.h - 14.0 - h - 8.0) / 44.0).floor().max(1.0) as usize, true, t),
            _ => {}
        }
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn animating(&self, t: f32) -> bool { t < 1.0 }
}

// ---------------------------------------------------------------- Sistema: «Anillos»
/// Tres anillos (procesador, memoria y disco del sistema) con su número dentro, y la red debajo.
#[derive(Default)]
pub struct RingsW { disp: [f32; 4] }

impl Widget for RingsW {
    fn layouts(&self) -> bool { true }
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let l = desk::lay(Kind::System);
        let (w, h) = (l.w, l.h);
        let sys = deskdata::SYS.lock().unwrap().clone();
        let disk = deskdata::STORE.lock().unwrap().as_ref().and_then(|s| s.drives.first().map(|dv| 1.0 - dv.free as f32 / dv.total.max(1) as f32));
        let bat = deskw::pc_battery();
        let vals = [sys.as_ref().and_then(|s| s.cpu.last().copied()).unwrap_or(0.0) / 100.0, sys.as_ref().and_then(|s| s.ram.last().copied()).unwrap_or(0.0) / 100.0, disk.unwrap_or(0.0), bat.map(|b| b.0 as f32 / 100.0).unwrap_or(0.0)];
        // Los anillos se llenan desde cero al aparecer y luego siguen al dato con suavidad
        let kin = ez(t, 0.0, 0.9);
        for i in 0..4 { self.disp[i] += (vals[i] * kin - self.disp[i]) * (1.0 - (-dt / 0.25).exp()); }
        // La batería, solo si el equipo la tiene (en gris claro: el color es de los otros tres)
        let cols = [[0.36, 0.62, 1.0], [0.75, 0.45, 1.0], [0.30, 0.85, 0.55], [0.86, 0.86, 0.90]];
        let names = [i18n::t("Procesador"), i18n::t("Memoria"), i18n::t("Disco"), i18n::t("Batería")];
        let draw = |ui: &Ui, i: usize, cx: f32, cy: f32, r: f32, sw: f32, v: f32| {
            let c = cols[i];
            ring(ui, cx, cy, r, sw, v, rgba(c[0], c[1], c[2], 1.0), rgba(c[0], c[1], c[2], 0.18));
            ui.text_c(&d.title, &format!("{:.0}%", v * 100.0), cx, cy - 11.0, 22.0, &ui.white);
            ui.text_c(&ui.f.small, names[i], cx, cy + r + 10.0, 16.0, &ui.dim);
        };
        let net = sys.as_ref().map(|s| (deskdata::fmt_rate(s.down.last().copied().unwrap_or(0.0)), deskdata::fmt_rate(s.up.last().copied().unwrap_or(0.0))));
        let n = if bat.is_some() { 4 } else { 3 };
        match l.sz {
            // El procesador grande y, debajo, memoria y disco en una línea
            Sz::Pequeno => {
                draw(ui, 0, w / 2.0, 70.0, 46.0, 8.0, self.disp[0]);
                let line = format!("RAM {:.0}%  ·  {} {:.0}%", self.disp[1] * 100.0, i18n::t("Disco"), self.disp[2] * 100.0);
                ui.text_c(&ui.f.small, &line, w / 2.0, h - 26.0, 16.0, &ui.faint);
            }
            // Los anillos en fila y, a la derecha tras una raya, la red en grande
            Sz::Banda => {
                let x1 = w - 190.0;
                let cw = x1 / n as f32;
                for i in 0..n { draw(ui, i, cw * i as f32 + cw / 2.0, 70.0, 42.0, 7.0, self.disp[i]); }
                unsafe { ui.rt.FillRectangle(&rect(x1, 26.0, x1 + 1.0 / ui.s.max(0.5), h - 26.0), &ui.card_edge); }
                if let Some((down, up)) = &net {
                    ui.text(&ui.f.small, i18n::t("RED"), x1 + 24.0, 34.0, 16.0, &ui.faint);
                    ui.text(&d.title, &format!("↓ {down}"), x1 + 24.0, 58.0, 24.0, &ui.white);
                    ui.text(&d.title, &format!("↑ {up}"), x1 + 24.0, 92.0, 24.0, &ui.dim);
                }
            }
            // Rejilla de dos por dos (la cuarta, la batería si la hay) y la red abajo
            Sz::Grande => {
                for i in 0..n {
                    let (cx, cy) = (w / 4.0 + (i % 2) as f32 * w / 2.0, 92.0 + (i / 2) as f32 * 150.0);
                    draw(ui, i, cx, cy, 50.0, 8.0, self.disp[i]);
                }
                if n == 3 {
                    // Sin batería, la red ocupa la cuarta casilla
                    if let Some((down, up)) = &net {
                        let (cx, cy) = (3.0 * w / 4.0, 242.0);
                        ui.text_c(&ui.f.small, i18n::t("RED"), cx, cy - 44.0, 16.0, &ui.faint);
                        ui.text_c(&d.title, &format!("↓ {down}"), cx, cy - 20.0, 24.0, &ui.white);
                        ui.text_c(&d.title, &format!("↑ {up}"), cx, cy + 10.0, 24.0, &ui.dim);
                    }
                } else if let Some((down, up)) = &net {
                    ui.text_c(&ui.f.sub, &format!("↓ {down}   ↑ {up}"), w / 2.0, h - 28.0, 18.0, &ui.dim);
                }
            }
            // Los tres de siempre y la red debajo
            _ => {
                let cw = w / 3.0;
                for i in 0..3 { draw(ui, i, cw * i as f32 + cw / 2.0, 66.0, 40.0, 7.0, self.disp[i]); }
                if let Some((down, up)) = &net { ui.text_c(&ui.f.sub, &format!("↓ {down}   ↑ {up}"), w / 2.0, h - 26.0, 18.0, &ui.dim); }
            }
        }
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn animating(&self, _t: f32) -> bool { true }
    fn frame_ms(&self) -> u32 { 50 }
}

/// Anillo de progreso (arco que se llena en el sentido del reloj desde arriba).
#[allow(clippy::too_many_arguments)]
fn ring(ui: &Ui, cx: f32, cy: f32, r: f32, sw: f32, fr: f32, fg: D2D1_COLOR_F, bg: D2D1_COLOR_F) {
    unsafe {
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
        ui.rt.DrawEllipse(&e, &ui.brush(bg), sw, None);
        let fr = fr.clamp(0.0, 1.0);
        if fr >= 0.999 { ui.rt.DrawEllipse(&e, &ui.brush(fg), sw, None); return; }
        if fr <= 0.003 { return; }
        let Ok(fac) = ui.rt.GetFactory() else { return };
        let Ok(geo) = fac.CreatePathGeometry() else { return };
        let Ok(sk) = geo.Open() else { return };
        let a = fr * std::f32::consts::TAU;
        sk.BeginFigure(Vector2 { X: cx, Y: cy - r }, D2D1_FIGURE_BEGIN_HOLLOW);
        sk.AddArc(&D2D1_ARC_SEGMENT { point: Vector2 { X: cx + r * a.sin(), Y: cy - r * a.cos() }, size: D2D_SIZE_F { width: r, height: r }, rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE, arcSize: if fr > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL } });
        sk.EndFigure(D2D1_FIGURE_END_OPEN);
        let _ = sk.Close();
        let cap = fac.CreateStrokeStyle(&D2D1_STROKE_STYLE_PROPERTIES { startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, ..Default::default() }, None).ok();
        ui.rt.DrawGeometry(&geo, &ui.brush(fg), sw, cap.as_ref());
    }
}

// ---------------------------------------------------------------- Música: «Carátula»
/// La carátula llena el widget; abajo, un velo oscuro con el título, el artista y los controles.
#[derive(Default)]
pub struct CoverW { art: Option<(u64, ID2D1Bitmap)>, rt: crate::RtKey, art_at: Option<Instant> }

impl Widget for CoverW {
    fn layouts(&self) -> bool { true }
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, _dt: f32) {
        let l = desk::lay(Kind::Music);
        let (w, h) = (l.w, l.h);
        if self.rt.fresh(ui.rt) { self.art = None; }
        // En los altos la carátula es el cuadrado de arriba y debajo va la canción sobre un fondo oscuro
        if l.is_tall() { return self.paint_tall(ui, d, t, w, h); }
        let Some(m) = MEDIA.lock().unwrap().clone() else {
            unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(0.0, 0.0, w, h), 22.0), &ui.wa(0.06)); }
            ui.glyph(&d.glyph_l, "\u{E8D6}", 0.0, h / 2.0 - 30.0, w, 40.0, &ui.dim);
            ui.text_c(&ui.f.sub, i18n::t("Nada sonando"), w / 2.0, h / 2.0 + 16.0, 18.0, &ui.dim);
            return;
        };
        if self.art.as_ref().map(|a| a.0) != Some(m.art_ver) {
            self.art = ART_BIG.lock().unwrap().as_ref().filter(|a| a.0 == m.art_ver).and_then(|(v, px)| deskw::bitmap(ui.rt, px, ART_BIG_PX).map(|b| (*v, b)));
            self.art_at = Some(Instant::now());
        }
        unsafe {
            match &self.art {
                Some((_, b)) => {
                    // Al cambiar de canción, la carátula nueva entra con un zoom suave
                    let k = self.art_at.map(|a| popup::ease_out(a.elapsed().as_secs_f32() / 0.5)).unwrap_or(1.0);
                    let sc = 1.06 - 0.06 * k;
                    let side = w * sc;
                    let props = D2D1_BRUSH_PROPERTIES { opacity: 0.4 + 0.6 * k, transform: Matrix3x2 { M11: side / ART_BIG_PX as f32, M12: 0.0, M21: 0.0, M22: side / ART_BIG_PX as f32, M31: (w - side) / 2.0, M32: (h - side) / 2.0 } };
                    if let Ok(br) = ui.rt.CreateBitmapBrush(b, None, Some(&props)) { ui.rt.FillRoundedRectangle(&rrect(rect(0.0, 0.0, w, h), 22.0), &br); }
                }
                None => ui.rt.FillRoundedRectangle(&rrect(rect(0.0, 0.0, w, h), 22.0), &ui.wa(0.08)),
            }
            // Velo de abajo para que el texto se lea sobre cualquier carátula
            let veil = ui.p.vgrad(h * 0.45, h, rgba(0.0, 0.0, 0.0, 0.0), rgba(0.0, 0.0, 0.0, 0.78));
            ui.rt.FillRoundedRectangle(&rrect(rect(0.0, 0.0, w, h), 22.0), &veil);
        }
        let white = ui.brush(rgba(1.0, 1.0, 1.0, 0.98));
        let soft = ui.brush(rgba(1.0, 1.0, 1.0, 0.72));
        if l.sz == Sz::Pequeno {
            // Pequeño: la canción en dos líneas y solo reproducir/pausa, abajo a la derecha
            let tw = w - 18.0 - 52.0;
            ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &m.title, tw), 14.0, h - 48.0, 18.0, &white);
            ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &m.artist, tw), 14.0, h - 30.0, 18.0, &soft);
            let (cx, cy) = (w - 32.0, h - 32.0);
            unsafe { ui.rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: 18.0, radiusY: 18.0 }, &ui.brush(rgba(1.0, 1.0, 1.0, 0.16 + 0.10 * ui.heat(2)))); }
            ui.glyph(&d.glyph_m, if m.playing { "\u{E769}" } else { "\u{E768}" }, cx - 15.0, cy - 15.0, 30.0, 30.0, &white);
            ui.hit(rect(cx - 20.0, cy - 20.0, cx + 20.0, cy + 20.0), 2);
            return;
        }
        ui.text(&d.title, &fit(ui, &d.title, &m.title, w - 36.0), 18.0, h - 92.0, 22.0, &white);
        ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &m.artist, w - 36.0), 18.0, h - 70.0, 18.0, &soft);
        cover_controls(ui, d, &m, w / 2.0, h - 34.0, &white);
    }
    fn click(&mut self, id: Option<u32>, right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        if right { return None; }
        match id {
            Some(1) => Some(Box::new(|| crate::media(crate::Cmd::Prev))),
            Some(2) => Some(Box::new(|| crate::media(crate::Cmd::Toggle))),
            Some(3) => Some(Box::new(|| crate::media(crate::Cmd::Next))),
            _ => None,
        }
    }
    fn animating(&self, _t: f32) -> bool { self.art_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.6) }
}

/// Anterior, reproducir/pausa y siguiente, centrados en `cx`.
fn cover_controls(ui: &mut Ui, d: &DFonts, m: &crate::MediaInfo, cx: f32, y: f32, white: &ID2D1Brush) {
    for (i, (g, id)) in [("\u{E892}", 1u32), (if m.playing { "\u{E769}" } else { "\u{E768}" }, 2), ("\u{E893}", 3)].into_iter().enumerate() {
        let bx = cx + (i as f32 - 1.0) * 56.0;
        ui.glyph(&d.glyph_m, g, bx - 16.0, y - 16.0, 32.0, 32.0, white);
        ui.hit(rect(bx - 20.0, y - 20.0, bx + 20.0, y + 20.0), id);
    }
}

impl CoverW {
    /// Alto y Columna: la carátula llena el cuadrado de arriba y se funde con el fondo oscuro de abajo,
    /// donde van la canción, su progreso y los controles.
    fn paint_tall(&mut self, ui: &mut Ui, d: &DFonts, t: f32, w: f32, h: f32) {
        let base = rgba(0.075, 0.075, 0.085, 1.0);
        unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(0.0, 0.0, w, h), 22.0), &ui.brush(base)); }
        let Some(m) = MEDIA.lock().unwrap().clone() else {
            ui.glyph(&d.glyph_l, "\u{E8D6}", 0.0, w / 2.0 - 30.0, w, 40.0, &ui.dim);
            ui.text_c(&ui.f.sub, i18n::t("Nada sonando"), w / 2.0, w / 2.0 + 16.0, 18.0, &ui.dim);
            return;
        };
        if self.art.as_ref().map(|a| a.0) != Some(m.art_ver) {
            self.art = ART_BIG.lock().unwrap().as_ref().filter(|a| a.0 == m.art_ver).and_then(|(v, px)| deskw::bitmap(ui.rt, px, ART_BIG_PX).map(|b| (*v, b)));
            self.art_at = Some(Instant::now());
        }
        unsafe {
            ui.rt.PushAxisAlignedClip(&rect(0.0, 0.0, w, w), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let card = rrect(rect(0.0, 0.0, w, w + 22.0), 22.0);
            match &self.art {
                Some((_, b)) => {
                    let k = self.art_at.map(|a| popup::ease_out(a.elapsed().as_secs_f32() / 0.5)).unwrap_or(1.0);
                    let side = w * (1.06 - 0.06 * k);
                    let props = D2D1_BRUSH_PROPERTIES { opacity: 0.4 + 0.6 * k, transform: Matrix3x2 { M11: side / ART_BIG_PX as f32, M12: 0.0, M21: 0.0, M22: side / ART_BIG_PX as f32, M31: (w - side) / 2.0, M32: (w - side) / 2.0 } };
                    if let Ok(br) = ui.rt.CreateBitmapBrush(b, None, Some(&props)) { ui.rt.FillRoundedRectangle(&card, &br); }
                }
                None => ui.rt.FillRoundedRectangle(&card, &ui.wa(0.08)),
            }
            // La carátula se funde con el fondo de abajo
            let veil = ui.p.vgrad(w * 0.55, w, rgba(base.r, base.g, base.b, 0.0), base);
            ui.rt.FillRectangle(&rect(0.0, w * 0.55, w, w + 1.0), &veil);
            ui.rt.PopAxisAlignedClip();
        }
        let white = ui.brush(rgba(1.0, 1.0, 1.0, 0.98));
        let soft = ui.brush(rgba(1.0, 1.0, 1.0, 0.72));
        let x = 18.0;
        let tw = w - 2.0 * x;
        let k = ez(t, 0.1, 0.5);
        popup::push_alpha(ui.rt, k);
        let mut y = w + 8.0;
        ui.text(&d.title, &fit(ui, &d.title, &m.title, tw), x, y, 22.0, &white);
        ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &m.artist, tw), x, y + 24.0, 18.0, &soft);
        y += 46.0;
        if w > 300.0 && !m.album.trim().is_empty() && m.album.trim() != m.title.trim() {
            ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &m.album, tw), x, y, 18.0, &ui.faint);
            y += 24.0;
        }
        // Progreso: línea fina con los tiempos debajo
        let secs = media_secs(m.playing);
        let prog = secs.map(|(p, e)| (p / e.max(1.0)).clamp(0.0, 1.0)).unwrap_or(0.0);
        let py = y + 10.0;
        unsafe {
            ui.rt.FillRoundedRectangle(&rrect(rect(x, py, x + tw, py + 3.0), 1.5), &ui.brush(rgba(1.0, 1.0, 1.0, 0.22)));
            if prog > 0.0 { ui.rt.FillRoundedRectangle(&rrect(rect(x, py, x + (tw * prog).max(3.0), py + 3.0), 1.5), &white); }
        }
        if let Some((p, e)) = secs {
            ui.text(&ui.f.small, &live::mmss(p as u32), x, py + 8.0, 16.0, &soft);
            ui.text_r(&ui.f.small, &live::mmss(e as u32), x + tw, py + 8.0, 16.0, &soft);
        }
        unsafe { ui.rt.PopLayer(); }
        cover_controls(ui, d, &m, w / 2.0, h - 36.0, &white);
    }
}

// ---------------------------------------------------------------- fuentes propias de estos diseños
pub struct VFonts { pub thin: IDWriteTextFormat, pub word: IDWriteTextFormat, pub day: IDWriteTextFormat }
thread_local! { static VF: std::cell::OnceCell<VFonts> = const { std::cell::OnceCell::new() }; }

/// Fuentes grandes (temperatura fina, número del día): se crean una vez por hilo.
fn deskv_fonts() -> &'static VFonts {
    VF.with(|c| {
        let f = c.get_or_init(|| {
            let dw: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap() };
            VFonts { thin: make_format(&dw, FONT, 88.0, DWRITE_FONT_WEIGHT_EXTRA_LIGHT), word: make_format(&dw, FONT, 22.0, DWRITE_FONT_WEIGHT_LIGHT), day: make_format(&dw, FONT, 84.0, DWRITE_FONT_WEIGHT_SEMI_LIGHT) }
        });
        // Vive lo mismo que el hilo de la interfaz (el de los widgets)
        unsafe { &*(f as *const VFonts) }
    })
}
