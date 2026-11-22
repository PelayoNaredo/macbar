//! Diseños alternativos de los widgets (se eligen por widget: clic derecho → Diseño, o Configuración).
//! Cada uno es otro `Widget` con su propio tamaño (ver `desk::size`).

use super::*;
use desk::{layout_size as size, DFonts, Kind}; // `size`: medidas de la maqueta (ver `desk::layout_size`)
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

impl Widget for SkyW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, _dt: f32) {
        let (w, h) = size(Kind::Weather);
        let city = sys::config_value("ciudad").unwrap_or_else(|| i18n::t("Tu ubicación").into());
        let cur = *WEATHER.lock().unwrap();
        let fc = weather::FORECAST.lock().unwrap().clone();
        let (top, bot) = cur.map(|c| sky(c.code, c.is_day)).unwrap_or(([0.2, 0.25, 0.32], [0.1, 0.12, 0.16]));
        card_grad(ui, w, h, top, bot, 1.0);
        let white = ui.brush(rgba(1.0, 1.0, 1.0, 0.98));
        let soft = ui.brush(rgba(1.0, 1.0, 1.0, 0.78));
        ui.text(&ui.f.name, &trunc(&city, 22), 18.0, 14.0, 20.0, &white);
        let Some(cur) = cur else { ui.text(&ui.f.sub, i18n::t("Cargando el tiempo…"), 18.0, 44.0, 20.0, &soft); return; };
        // La temperatura sube desde abajo al aparecer
        let k = ez(t, 0.05, 0.6);
        let temp = format!("{:.0}°", cur.temp);
        ui.text(&d.huge, &temp, 16.0, 36.0 + 8.0 * (1.0 - k), 50.0, &white);
        // Arriba a la derecha: icono, estado y máx./mín.
        weather::draw_icon_anim(ui.rt, &ui.p, &ui.f.icon_l, w - 42.0, 12.0, 26.0, 26.0, cur.code, cur.is_day, &white, Some(t));
        ui.text_r(&ui.f.sub, crate::weather_desc(cur.code), w - 18.0, 42.0, 18.0, &white);
        if let Some(d0) = fc.as_ref().and_then(|f| f.days.first()) {
            ui.text_r(&ui.f.sub, &format!("Máx {:.0}°  Mín {:.0}°", d0.3, d0.2), w - 18.0, 60.0, 18.0, &soft);
        }
        // Abajo: seis horas (hora, icono y temperatura), entran en cascada
        if let Some(f) = &fc {
            let n = 6usize;
            let cw = (w - 36.0) / n as f32;
            for (i, hr) in f.hours.iter().take(n).enumerate() {
                let ki = ez(t, 0.25 + i as f32 * 0.05, 0.4);
                let x = 18.0 + i as f32 * cw;
                let y = h - 66.0 + 6.0 * (1.0 - ki);
                popup::push_alpha(ui.rt, ki);
                let label = if i == 0 { i18n::t("Ahora").to_string() } else { format!("{}", hr.0) };
                ui.text_c(&ui.f.small, &label, x + cw / 2.0, y, 16.0, &soft);
                weather::draw_icon(ui.rt, &ui.p, &ui.f.icon, x + cw / 2.0 - 9.0, y + 18.0, 18.0, 18.0, hr.2, hr.3, &white);
                ui.text_c(&ui.f.lbl, &format!("{:.0}°", hr.1), x + cw / 2.0, y + 38.0, 18.0, &white);
                unsafe { ui.rt.PopLayer(); }
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
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, t: f32, _dt: f32) {
        let (_w, _h) = size(Kind::Weather);
        let Some(cur) = *WEATHER.lock().unwrap() else { return };
        let k = ez(t, 0.0, 0.7);
        let f = deskv_fonts().thin.clone();
        let temp = format!("{:.0}°", cur.temp);
        popup::push_alpha(ui.rt, k);
        let white = ui.white.clone();
        self.r.draw(ui, &f, &temp, 8.0, 4.0 + 10.0 * (1.0 - k), 96.0, &white, false);
        let word = crate::weather_desc(cur.code).to_lowercase();
        let city = sys::config_value("ciudad").unwrap_or_default();
        ui.text(&deskv_fonts().word, &word, 14.0, 104.0, 26.0, &ui.white);
        if !city.is_empty() { ui.text(&ui.f.sub, &city, 14.0, 128.0, 18.0, &ui.dim); }
        unsafe { ui.rt.PopLayer(); }
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn animating(&self, t: f32) -> bool { t < 1.0 || self.r.busy() }
}

// ---------------------------------------------------------------- Calendario: «Hoy»
/// Cuadrado pequeño: el día de la semana en el color de acento y el número enorme (como el de iOS).
#[derive(Default)]
pub struct TodayW;

impl Widget for TodayW {
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, t: f32, _dt: f32) {
        let (w, h) = size(Kind::Calendar);
        let now = unsafe { GetLocalTime() };
        let acc = crate::profiles::accent(1.0);
        let k = ez(t, 0.0, 0.6);
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
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn animating(&self, t: f32) -> bool { t < 1.0 }
}

// ---------------------------------------------------------------- Sistema: «Anillos»
/// Tres anillos (procesador, memoria y disco del sistema) con su número dentro, y la red debajo.
#[derive(Default)]
pub struct RingsW { disp: [f32; 3] }

impl Widget for RingsW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (w, h) = size(Kind::System);
        let sys = deskdata::SYS.lock().unwrap().clone();
        let disk = deskdata::STORE.lock().unwrap().as_ref().and_then(|s| s.drives.first().map(|dv| 1.0 - dv.free as f32 / dv.total.max(1) as f32));
        let vals = [sys.as_ref().and_then(|s| s.cpu.last().copied()).unwrap_or(0.0) / 100.0, sys.as_ref().and_then(|s| s.ram.last().copied()).unwrap_or(0.0) / 100.0, disk.unwrap_or(0.0)];
        // Los anillos se llenan desde cero al aparecer y luego siguen al dato con suavidad
        let kin = ez(t, 0.0, 0.9);
        for i in 0..3 { self.disp[i] += (vals[i] * kin - self.disp[i]) * (1.0 - (-dt / 0.25).exp()); }
        let cols = [[0.36, 0.62, 1.0], [0.75, 0.45, 1.0], [0.30, 0.85, 0.55]];
        let names = [i18n::t("Procesador"), i18n::t("Memoria"), i18n::t("Disco")];
        let cw = w / 3.0;
        for i in 0..3 {
            let (cx, cy, r) = (cw * i as f32 + cw / 2.0, 66.0, 40.0);
            let c = cols[i];
            ring(ui, cx, cy, r, 7.0, self.disp[i], rgba(c[0], c[1], c[2], 1.0), rgba(c[0], c[1], c[2], 0.18));
            ui.text_c(&d.title, &format!("{:.0}%", self.disp[i] * 100.0), cx, cy - 11.0, 22.0, &ui.white);
            ui.text_c(&ui.f.small, names[i], cx, cy + r + 10.0, 16.0, &ui.dim);
        }
        if let Some(s) = &sys {
            let down = s.down.last().copied().unwrap_or(0.0);
            let up = s.up.last().copied().unwrap_or(0.0);
            ui.text_c(&ui.f.sub, &format!("↓ {}   ↑ {}", deskdata::fmt_rate(down), deskdata::fmt_rate(up)), w / 2.0, h - 26.0, 18.0, &ui.dim);
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
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, _t: f32, _dt: f32) {
        let (w, h) = size(Kind::Music);
        if self.rt.fresh(ui.rt) { self.art = None; }
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
        ui.text(&d.title, &trunc(&m.title, 26), 18.0, h - 92.0, 22.0, &white);
        ui.text(&ui.f.sub, &trunc(&m.artist, 34), 18.0, h - 70.0, 18.0, &soft);
        // Controles
        let y = h - 34.0;
        for (i, (g, id)) in [("\u{E892}", 1u32), (if m.playing { "\u{E769}" } else { "\u{E768}" }, 2), ("\u{E893}", 3)].into_iter().enumerate() {
            let cx = w / 2.0 + (i as f32 - 1.0) * 56.0;
            ui.glyph(&d.glyph_m, g, cx - 16.0, y - 16.0, 32.0, 32.0, &white);
            ui.hit(rect(cx - 20.0, y - 20.0, cx + 20.0, y + 20.0), id);
        }
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
