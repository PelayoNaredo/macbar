//! Segunda tanda de diseños alternativos de los widgets: vinilo, póster, polaroid, visualizador radial
//! y cuenta atrás de paletas. Como los de `deskv`, cada uno es otro `Widget` con su tamaño (`desk::size`).

use super::*;
use desk::{layout_size as size, DFonts, Kind}; // `size`: medidas de la maqueta (ver `desk::layout_size`)
use deskw::Widget;
use popup::Ui;
use std::f32::consts::TAU;
use std::rc::Rc;

// ---------------------------------------------------------------- utilidades comunes
fn ez(t: f32, delay: f32, dur: f32) -> f32 { popup::ease_out(((t - delay) / dur).clamp(0.0, 1.0)) }
fn ellipse(x: f32, y: f32, r: f32) -> D2D1_ELLIPSE { D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: r, radiusY: r } }
fn v2(x: f32, y: f32) -> Vector2 { Vector2 { X: x, Y: y } }
fn smoothstep(a: f32, b: f32, x: f32) -> f32 { let t = ((x - a) / (b - a)).clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) }

/// a · b (primero a, luego b) para transformaciones 2D de Direct2D.
fn mul(a: &Matrix3x2, b: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: a.M11 * b.M11 + a.M12 * b.M21, M12: a.M11 * b.M12 + a.M12 * b.M22,
        M21: a.M21 * b.M11 + a.M22 * b.M21, M22: a.M21 * b.M12 + a.M22 * b.M22,
        M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31, M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    }
}
/// Giro `a` (radianes, en el sentido del reloj) y luego traslación a (x, y).
fn rot(a: f32, x: f32, y: f32) -> Matrix3x2 { let (s, c) = a.sin_cos(); Matrix3x2 { M11: c, M12: s, M21: -s, M22: c, M31: x, M32: y } }
fn cur_transform(ui: &Ui) -> Matrix3x2 { let mut m = Matrix3x2::default(); unsafe { ui.rt.GetTransform(&mut m); } m }

/// Avance suave hacia `target`. true mientras se mueve.
fn approach(x: &mut f32, target: f32, dt: f32, tau: f32) -> bool {
    if (target - *x).abs() < 0.002 { *x = target; return false; }
    *x += (target - *x) * (1.0 - (-dt / tau).exp());
    true
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

/// Carátula grande de lo que suena (vuelve a crearla si cambia la canción o el lienzo).
fn load_art(slot: &mut Option<(u64, ID2D1Bitmap)>, rt: &ID2D1RenderTarget, ver: u64) -> bool {
    if slot.as_ref().map(|a| a.0) == Some(ver) { return false; }
    let new = ART_BIG.lock().unwrap().as_ref().filter(|a| a.0 == ver).and_then(|(v, px)| deskw::bitmap(rt, px, ART_BIG_PX).map(|b| (*v, b)));
    if new.is_none() && slot.is_none() { return false; }
    *slot = new;
    true
}

/// La carátula recortada en círculo, girada `ang` radianes alrededor de su centro.
fn art_circle(ui: &Ui, bmp: &ID2D1Bitmap, cx: f32, cy: f32, r: f32, ang: f32, alpha: f32) {
    let s = 2.0 * r / ART_BIG_PX as f32;
    let m = mul(&Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: -r, M32: -r }, &rot(ang, cx, cy));
    unsafe {
        let props = D2D1_BRUSH_PROPERTIES { opacity: alpha, transform: m };
        if let Ok(br) = ui.rt.CreateBitmapBrush(bmp, None, Some(&props)) { ui.rt.FillEllipse(&ellipse(cx, cy, r), &br); }
    }
}

// ---------------------------------------------------------------- fuentes propias de estos diseños
struct VFonts { day: IDWriteTextFormat, word: IDWriteTextFormat, spaced: IDWriteTextFormat, flip: IDWriteTextFormat, script: IDWriteTextFormat }
thread_local! { static VF: std::cell::OnceCell<Rc<VFonts>> = const { std::cell::OnceCell::new() }; }

/// Letra «a mano» para el pie de la polaroid: Segoe Print o Ink Free si están; si no, Segoe UI cursiva.
fn script_format(dw: &IDWriteFactory, size: f32) -> IDWriteTextFormat {
    unsafe {
        let mut coll: Option<IDWriteFontCollection> = None;
        let _ = dw.GetSystemFontCollection(&mut coll, false);
        let has = |name: &str| -> bool {
            let Some(c) = &coll else { return false };
            let (mut i, mut ex) = (0u32, windows::core::BOOL(0));
            c.FindFamilyName(&HSTRING::from(name), &mut i, &mut ex).is_ok() && ex.as_bool()
        };
        let mk = |fam: &str, style: DWRITE_FONT_STYLE| -> Option<IDWriteTextFormat> {
            let f = dw.CreateTextFormat(&HSTRING::from(fam), None, DWRITE_FONT_WEIGHT_NORMAL, style, DWRITE_FONT_STRETCH_NORMAL, size, w!("es-ES")).ok()?;
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            Some(f)
        };
        for fam in ["Segoe Print", "Ink Free"] {
            if has(fam) { if let Some(f) = mk(fam, DWRITE_FONT_STYLE_NORMAL) { return f; } }
        }
        mk("Segoe UI", DWRITE_FONT_STYLE_ITALIC).unwrap_or_else(|| make_format(dw, FONT, size, DWRITE_FONT_WEIGHT_NORMAL))
    }
}

/// Fuentes de estos diseños: se crean una vez por hilo (el de los widgets) con la fábrica del lienzo.
fn vfonts(ui: &Ui) -> Rc<VFonts> {
    VF.with(|c| c.get_or_init(|| {
        let dw = ui.p.dw;
        Rc::new(VFonts {
            day: make_format(dw, FONT, 124.0, DWRITE_FONT_WEIGHT_EXTRA_LIGHT),
            word: make_format(dw, FONT, 24.0, DWRITE_FONT_WEIGHT_LIGHT),
            spaced: make_format(dw, FONT, 13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            flip: make_format(dw, FONT, 46.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            script: script_format(dw, 15.0),
        })
    }).clone())
}

/// «LUNES» → «L U N E S» con espacios finos (mayúsculas espaciadas, como en un cartel).
fn spaced(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 { out.push('\u{2009}'); }
        out.push(c);
    }
    out
}

// ================================================================ Música: «Vinilo»
/// 33 ⅓ revoluciones por minuto, en radianes por segundo.
const RPM33: f32 = 33.333 / 60.0 * TAU;
/// Ángulo del brazo: en reposo (fuera del disco) y al empezar el disco (lo demás lo da el progreso).
const ARM_REST: f32 = 0.02;
const ARM_PLAY: f32 = 0.36;
const ARM_SWEEP: f32 = 0.11;

/// Disco negro con surcos que gira mientras suena, con la carátula por etiqueta y un brazo que baja
/// sobre él al reproducir; a la derecha, la canción, su progreso y los controles.
#[derive(Default)]
pub struct VinylW { art: Option<(u64, ID2D1Bitmap)>, rt: crate::RtKey, art_at: Option<Instant>, ang: f32, spin: f32, arm: f32, playing: bool, key: String, title_at: Option<Instant>, moving: bool }

/// Ángulo del brazo (0 = recto hacia abajo) para un avance `arm` (0 reposo … 1 sobre el disco) y el
/// progreso de la canción: va entrando hacia la etiqueta a medida que avanza.
fn arm_angle(arm: f32, progress: f32) -> f32 {
    let play = ARM_PLAY + ARM_SWEEP * progress.clamp(0.0, 1.0);
    ARM_REST + (play - ARM_REST) * smoothstep(0.0, 0.75, arm)
}

impl VinylW {
    /// El brazo: base, contrapeso, tubo y cápsula. `lift`: cuánto se levanta (sombra más lejos).
    fn tonearm(ui: &Ui, px: f32, py: f32, len: f32, ang: f32, lift: f32) {
        let base = cur_transform(ui);
        let st = round_stroke(ui.rt);
        let metal = ui.brush(rgba(0.82, 0.82, 0.85, 1.0));
        let dark = ui.brush(rgba(0.55, 0.55, 0.58, 1.0));
        let acc = crate::profiles::accent(1.0);
        unsafe {
            // Sombra: se separa del disco al levantarse
            let (ox, oy) = (1.5 + 3.0 * lift, 2.5 + 5.0 * lift);
            ui.rt.SetTransform(&mul(&rot(ang, px + ox, py + oy), &base));
            let sh = ui.brush(rgba(0.0, 0.0, 0.0, 0.32 - 0.12 * lift));
            ui.rt.DrawLine(v2(0.0, -20.0), v2(0.0, len - 8.0), &sh, 3.5, st.as_ref());
            ui.rt.FillRoundedRectangle(&rrect(rect(-5.5, len - 12.0, 5.5, len + 7.0), 2.0), &sh);
            // El brazo, con la cápsula algo más grande al estar levantada (más cerca de quien mira)
            ui.rt.SetTransform(&mul(&rot(ang, px, py), &base));
            ui.rt.FillRoundedRectangle(&rrect(rect(-5.0, -27.0, 5.0, -11.0), 2.5), &dark);
            ui.rt.DrawLine(v2(0.0, -12.0), v2(0.0, len - 10.0), &metal, 2.6, st.as_ref());
            let s = 1.0 + 0.10 * lift;
            ui.rt.SetTransform(&mul(&mul(&Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: 0.0, M32: len * (1.0 - s) }, &rot(ang, px, py)), &base));
            ui.rt.FillRoundedRectangle(&rrect(rect(-5.0, len - 12.0, 5.0, len + 6.0), 1.8), &ui.brush(rgba(0.17, 0.17, 0.19, 1.0)));
            ui.rt.FillRoundedRectangle(&rrect(rect(-5.0, len - 12.0, 5.0, len - 8.0), 1.5), &ui.brush(rgba(acc.r, acc.g, acc.b, 1.0)));
            ui.rt.DrawLine(v2(5.0, len - 5.0), v2(9.5, len - 1.0), &metal, 1.6, st.as_ref());
            ui.rt.SetTransform(&base);
            // Base del brazo (no gira)
            ui.rt.FillEllipse(&ellipse(px + 1.0, py + 2.0, 11.5), &ui.brush(rgba(0.0, 0.0, 0.0, 0.3)));
            ui.rt.FillEllipse(&ellipse(px, py, 11.0), &ui.brush(rgba(0.19, 0.19, 0.21, 1.0)));
            ui.rt.DrawEllipse(&ellipse(px, py, 10.5), &ui.brush(rgba(1.0, 1.0, 1.0, 0.10)), 1.0, None);
            ui.rt.FillEllipse(&ellipse(px, py, 4.5), &metal);
        }
    }

    /// El disco: cuerpo, surcos, brillo fijo (la luz no gira), etiqueta y eje.
    fn disc(&self, ui: &Ui, cx: f32, cy: f32, r: f32) {
        unsafe {
            ui.rt.FillEllipse(&ellipse(cx + 1.0, cy + 3.0, r + 1.0), &ui.brush(rgba(0.0, 0.0, 0.0, 0.35)));
            ui.rt.FillEllipse(&ellipse(cx, cy, r), &ui.brush(rgba(0.065, 0.065, 0.075, 1.0)));
            // Surcos: anillos muy tenues, con dos separaciones entre temas algo más marcadas
            let mut rr = r * 0.42;
            let mut i = 0;
            while rr < r - 3.0 {
                let a = if i % 3 == 0 { 0.07 } else { 0.035 };
                ui.rt.DrawEllipse(&ellipse(cx, cy, rr), &ui.brush(rgba(1.0, 1.0, 1.0, a)), 0.6, None);
                rr += 2.4;
                i += 1;
            }
            for f in [0.63, 0.81] { ui.rt.DrawEllipse(&ellipse(cx, cy, r * f), &ui.brush(rgba(0.0, 0.0, 0.0, 0.8)), 1.4, None); }
            ui.rt.DrawEllipse(&ellipse(cx, cy, r - 0.5), &ui.brush(rgba(1.0, 1.0, 1.0, 0.10)), 1.0, None);
            // Brillo: dos cuñas claras enfrentadas, quietas mientras el disco gira
            if let Ok(fac) = ui.rt.GetFactory() {
                let sheen = ui.brush(rgba(1.0, 1.0, 1.0, 0.05));
                for a0 in [-1.05f32, 2.09] {
                    let Ok(geo) = fac.CreatePathGeometry() else { break };
                    let Ok(sk) = geo.Open() else { break };
                    let a1 = a0 + 0.42;
                    let ri = r * 0.40;
                    sk.BeginFigure(v2(cx + ri * a0.sin(), cy - ri * a0.cos()), D2D1_FIGURE_BEGIN_FILLED);
                    sk.AddLine(v2(cx + (r - 1.0) * a0.sin(), cy - (r - 1.0) * a0.cos()));
                    sk.AddArc(&D2D1_ARC_SEGMENT { point: v2(cx + (r - 1.0) * a1.sin(), cy - (r - 1.0) * a1.cos()), size: D2D_SIZE_F { width: r - 1.0, height: r - 1.0 }, rotationAngle: 0.0, sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE, arcSize: D2D1_ARC_SIZE_SMALL });
                    sk.AddLine(v2(cx + ri * a1.sin(), cy - ri * a1.cos()));
                    sk.EndFigure(D2D1_FIGURE_END_CLOSED);
                    let _ = sk.Close();
                    ui.rt.FillGeometry(&geo, &sheen, None);
                }
            }
            // Etiqueta: la carátula en círculo, que gira con el disco
            let lr = r * 0.36;
            match &self.art {
                Some((_, b)) => {
                    let k = self.art_at.map(|a| popup::ease_out(a.elapsed().as_secs_f32() / 0.5)).unwrap_or(1.0);
                    let acc = crate::profiles::accent(1.0);
                    if k < 1.0 { ui.rt.FillEllipse(&ellipse(cx, cy, lr), &ui.brush(rgba(acc.r * 0.7, acc.g * 0.7, acc.b * 0.7, 1.0))); }
                    art_circle(ui, b, cx, cy, lr, self.ang, k);
                }
                None => {
                    let acc = crate::profiles::accent(1.0);
                    ui.rt.FillEllipse(&ellipse(cx, cy, lr), &ui.brush(rgba(acc.r * 0.8, acc.g * 0.8, acc.b * 0.8, 1.0)));
                    // Una marca en la etiqueta, para que se vea que gira
                    let (s, c) = self.ang.sin_cos();
                    ui.rt.FillEllipse(&ellipse(cx + s * lr * 0.62, cy - c * lr * 0.62, 2.2), &ui.brush(rgba(1.0, 1.0, 1.0, 0.75)));
                }
            }
            ui.rt.DrawEllipse(&ellipse(cx, cy, lr), &ui.brush(rgba(0.0, 0.0, 0.0, 0.35)), 1.0, None);
            ui.rt.FillEllipse(&ellipse(cx, cy, 3.0), &ui.brush(rgba(0.78, 0.78, 0.80, 1.0)));
            ui.rt.FillEllipse(&ellipse(cx, cy, 1.3), &ui.brush(rgba(0.2, 0.2, 0.22, 1.0)));
        }
    }
}

impl Widget for VinylW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (w, h) = size(Kind::Music);
        if self.rt.fresh(ui.rt) { self.art = None; }
        let m = MEDIA.lock().unwrap().clone();
        let playing = m.as_ref().is_some_and(|m| m.playing);
        self.playing = playing;
        // Tras un rato quieto, el primer fotograma no da un salto
        let dt = dt.min(0.1);
        let target = if playing { 1.0 } else { 0.0 };
        let mut moving = approach(&mut self.spin, target, dt, 0.45);
        self.ang = (self.ang + dt * RPM33 * self.spin) % TAU;
        moving |= approach(&mut self.arm, target, dt, 0.4);
        match &m {
            Some(m) => {
                if load_art(&mut self.art, ui.rt, m.art_ver) { self.art_at = Some(Instant::now()); }
                let key = format!("{}\u{1}{}", m.title, m.artist);
                if key != self.key { if !self.key.is_empty() { self.title_at = Some(Instant::now()); } self.key = key; }
            }
            None => { self.art = None; self.key.clear(); }
        }
        let secs = m.as_ref().and_then(|m| media_secs(m.playing));
        let prog = secs.map(|(p, e)| (p / e.max(1.0)).clamp(0.0, 1.0)).unwrap_or(0.0);

        // ---- Disco y brazo: entran desde la izquierda con un giro
        let k = ez(t, 0.0, 0.7);
        let (r, cy) = (h / 2.0 - 12.0, h / 2.0);
        let cx = 12.0 + r - 16.0 * (1.0 - k);
        let base = cur_transform(ui);
        popup::push_alpha(ui.rt, k);
        let spin_in = -0.9 * (1.0 - k);
        let a0 = self.ang;
        self.ang += spin_in;
        self.disc(ui, cx, cy, r);
        self.ang = a0;
        let drop = smoothstep(0.7, 1.0, self.arm);
        Self::tonearm(ui, cx + r + 6.0, cy - r + 14.0, 104.0, arm_angle(self.arm, prog), 1.0 - drop);
        unsafe { ui.rt.PopLayer(); ui.rt.SetTransform(&base); }
        ui.hit(rect(0.0, 0.0, cx + r + 18.0, h), 4);

        // ---- Canción
        let x0 = 2.0 * r + 44.0;
        let maxw = w - 18.0 - x0;
        let kt = ez(self.title_at.map(|a| a.elapsed().as_secs_f32()).unwrap_or(t).min(t), 0.12, 0.45);
        let ka = ez(self.title_at.map(|a| a.elapsed().as_secs_f32()).unwrap_or(t).min(t), 0.2, 0.45);
        let Some(m) = m else {
            popup::push_alpha(ui.rt, kt);
            ui.text(&d.title, i18n::t("Nada sonando"), x0, 30.0 + 8.0 * (1.0 - kt), 24.0, &ui.white);
            ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, i18n::t("Pon algo de música"), maxw), x0, 56.0, 18.0, &ui.dim);
            unsafe { ui.rt.PopLayer(); }
            self.moving = moving;
            return;
        };
        popup::push_alpha(ui.rt, kt);
        ui.text(&d.title, &fit(ui, &d.title, &m.title, maxw), x0, 22.0 + 8.0 * (1.0 - kt), 24.0, &ui.white);
        unsafe { ui.rt.PopLayer(); }
        popup::push_alpha(ui.rt, ka);
        ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &m.artist, maxw), x0, 48.0 + 8.0 * (1.0 - ka), 18.0, &ui.dim);
        unsafe { ui.rt.PopLayer(); }

        // ---- Progreso: línea fina con los tiempos debajo
        let kp = ez(t, 0.3, 0.6);
        popup::push_alpha(ui.rt, kp);
        let py = 88.0;
        unsafe {
            ui.rt.FillRoundedRectangle(&rrect(rect(x0, py, x0 + maxw, py + 3.0), 1.5), &ui.track);
            if secs.is_some() && prog > 0.0 { ui.rt.FillRoundedRectangle(&rrect(rect(x0, py, x0 + (maxw * prog).max(3.0), py + 3.0), 1.5), &ui.white); }
        }
        if let Some((p, e)) = secs {
            ui.text(&ui.f.small, &live::mmss(p as u32), x0, py + 8.0, 16.0, &ui.dim);
            ui.text_r(&ui.f.small, &live::mmss(e as u32), x0 + maxw, py + 8.0, 16.0, &ui.dim);
        }
        // ---- Controles: anterior, reproducir/pausa (en un círculo) y siguiente
        let cyb = h - 30.0;
        let mid = x0 + maxw / 2.0;
        for (i, (g, id)) in [("\u{E892}", 1u32), (if m.playing { "\u{E769}" } else { "\u{E768}" }, 2), ("\u{E893}", 3)].into_iter().enumerate() {
            let bx = mid + (i as f32 - 1.0) * 52.0;
            let hk = ui.heat(id);
            let pr = 1.0 - 0.12 * desk::press(id);
            unsafe {
                ui.rt.SetTransform(&mul(&Matrix3x2 { M11: pr, M12: 0.0, M21: 0.0, M22: pr, M31: bx * (1.0 - pr), M32: cyb * (1.0 - pr) }, &base));
                if id == 2 { ui.rt.FillEllipse(&ellipse(bx, cyb, 19.0), &ui.wa(0.14 + 0.10 * hk)); }
                else if hk > 0.01 { ui.rt.FillEllipse(&ellipse(bx, cyb, 17.0), &ui.wa(0.08 * hk)); }
            }
            ui.glyph(&d.glyph_m, g, bx - 15.0, cyb - 15.0, 30.0, 30.0, &ui.white);
            unsafe { ui.rt.SetTransform(&base); }
            ui.hit(rect(bx - 20.0, cyb - 20.0, bx + 20.0, cyb + 20.0), id);
        }
        unsafe { ui.rt.PopLayer(); }
        self.moving = moving;
    }
    fn click(&mut self, id: Option<u32>, right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        if right { return None; }
        match id? {
            1 => Some(Box::new(|| crate::media(crate::Cmd::Prev))),
            2 => Some(Box::new(|| crate::media(crate::Cmd::Toggle))),
            3 => Some(Box::new(|| crate::media(crate::Cmd::Next))),
            4 => { let app = MEDIA.lock().unwrap().as_ref().map(|m| m.app.clone())?; Some(Box::new(move || open_player(&app))) }
            _ => None,
        }
    }
    // A 30 fotogramas solo mientras suena o se mueve el brazo; parado no pinta
    fn animating(&self, t: f32) -> bool {
        t < 1.2 || self.playing || self.moving
            || self.art_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.6)
            || self.title_at.is_some_and(|a| a.elapsed().as_secs_f32() < 0.8)
    }
    fn frame_ms(&self) -> u32 { 33 }
}

// ================================================================ Calendario: «Póster»
/// Sin fondo: el día de la semana en mayúsculas espaciadas, el número enorme y fino, y el mes debajo.
#[derive(Default)]
pub struct PosterW;

impl Widget for PosterW {
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, t: f32, _dt: f32) {
        let (w, _h) = size(Kind::Calendar);
        let f = vfonts(ui);
        let now = unsafe { GetLocalTime() };
        let acc = crate::profiles::accent(1.0);
        let x = 14.0;
        let k0 = ez(t, 0.0, 0.5);
        popup::push_alpha(ui.rt, k0);
        let wd = spaced(&i18n::weekday(now.wDayOfWeek as usize).to_uppercase());
        ui.text(&f.spaced, &wd, x + 2.0, 12.0, 18.0, &ui.brush(acc));
        unsafe { ui.rt.PopLayer(); }
        // El número sube y aparece; el mes llega un poco después
        let k1 = ez(t, 0.08, 0.8);
        popup::push_alpha(ui.rt, k1);
        ui.text(&f.day, &now.wDay.to_string(), x - 4.0, 30.0 + 14.0 * (1.0 - k1), 124.0, &ui.white);
        unsafe { ui.rt.PopLayer(); }
        let k2 = ez(t, 0.25, 0.6);
        popup::push_alpha(ui.rt, k2);
        let month = i18n::month(now.wMonth as usize + 11);
        let my = 158.0 + 6.0 * (1.0 - k2);
        ui.text(&f.word, month, x + 1.0, my, 28.0, &ui.white);
        let mw = ui.p.width(&f.word, month);
        if x + mw + 70.0 < w { ui.text(&f.word, &now.wYear.to_string(), x + mw + 10.0, my, 28.0, &ui.dim); }
        unsafe { ui.rt.PopLayer(); }
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn animating(&self, t: f32) -> bool { t < 1.0 }
}

// ================================================================ Fotos: «Polaroid»
/// Lo que tarda en caer y asentarse una foto nueva.
const DROP_S: f32 = 1.1;
const TILT: f32 = -3.0;
const PC_W: f32 = 192.0;
const PC_H: f32 = 222.0;
const PC_B: f32 = 10.0;
const PC_STRIP: f32 = 40.0;

/// Pose de la polaroid que cae (`e` en segundos desde que llegó): (desplazamiento vertical, giro en
/// grados, opacidad, escala). Cae desde arriba, se balancea un poco y acaba quieta en `TILT`.
fn drop_pose(e: f32) -> (f32, f32, f32, f32) {
    if e >= DROP_S { return (0.0, TILT, 1.0, 1.0); }
    let e = e.max(0.0);
    let fall = popup::ease_out((e / 0.5).min(1.0));
    let dy = -42.0 * (1.0 - fall);
    // Balanceo amortiguado alrededor de la inclinación final (llega con 0 al terminar)
    let sway = 5.0 * (-4.2 * e).exp() * (e * 11.0).cos() * (1.0 - smoothstep(DROP_S * 0.7, DROP_S, e));
    let alpha = (e / 0.22).min(1.0);
    let sc = 1.05 - 0.05 * fall;
    (dy, TILT + sway, alpha, sc)
}

/// «14 marzo 2025» / «March 14, 2025».
fn fmt_photo_date(y: i32, m: u32, d: u32) -> String {
    let mes = i18n::month(m.saturating_sub(1) as usize);
    if i18n::en() { format!("{mes} {d}, {y}") } else { format!("{d} {mes} {y}") }
}

/// Minutos que la hora local va por delante de la UTC (redondeado al cuarto de hora).
fn local_offset_min() -> i64 {
    let (l, u) = unsafe { (GetLocalTime(), windows::Win32::System::SystemInformation::GetSystemTime()) };
    let f = |t: &SYSTEMTIME| deskw3::days_from_civil(t.wYear as i32, t.wMonth as u32, t.wDay as u32) * 1440 + t.wHour as i64 * 60 + t.wMinute as i64;
    ((f(&l) - f(&u)) as f64 / 15.0).round() as i64 * 15
}

/// (año, mes, día) locales de un instante en segundos desde 1970.
fn local_ymd(unix_s: i64, offset_min: i64) -> (i32, u32, u32) { deskw3::civil_from_days((unix_s + offset_min * 60).div_euclid(86_400)) }

/// Fecha de la foto (la de modificación del archivo), o vacía si no se puede leer.
fn photo_date(path: &str) -> String {
    let Some(s) = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) else { return String::new() };
    let (y, m, d) = local_ymd(s.as_secs() as i64, local_offset_min());
    fmt_photo_date(y, m, d)
}

struct Snap { ver: u64, bmp: ID2D1Bitmap, w: f32, h: f32, path: String, date: String, at: Instant }

#[derive(Default)]
pub struct PolaroidW { cur: Option<Snap>, prev: Option<Snap>, rt: crate::RtKey, asked: bool }

impl PolaroidW {
    /// Una polaroid centrada en (cx, cy) con su pose; `snap` = None la deja vacía (sin fotos).
    #[allow(clippy::too_many_arguments)]
    fn draw(ui: &mut Ui, snap: Option<&Snap>, cx: f32, cy: f32, ang: f32, alpha: f32, sc: f32, script: &IDWriteTextFormat) {
        let base = cur_transform(ui);
        let m = mul(&Matrix3x2 { M11: sc, M12: 0.0, M21: 0.0, M22: sc, M31: 0.0, M32: 0.0 }, &rot(ang.to_radians(), cx, cy));
        let (l, t, r, b) = (-PC_W / 2.0, -PC_H / 2.0, PC_W / 2.0, PC_H / 2.0);
        // El texto sobre el papel blanco va sin el halo oscuro de los widgets flotantes
        let halo = ui.shadow.take();
        unsafe {
            ui.rt.SetTransform(&mul(&m, &base));
            popup::push_alpha(ui.rt, alpha);
            // Sombra suave: varias capas muy tenues que se abren hacia abajo
            for i in 0..7 {
                let g = 1.0 + i as f32 * 1.7;
                ui.rt.FillRoundedRectangle(&rrect(rect(l - g + 1.0, t - g + 5.0, r + g + 1.0, b + g + 5.0), 3.0 + g), &ui.brush(rgba(0.0, 0.0, 0.0, 0.045)));
            }
            ui.rt.FillRoundedRectangle(&rrect(rect(l, t, r, b), 3.0), &ui.brush(rgba(0.975, 0.97, 0.955, 1.0)));
            ui.rt.DrawRoundedRectangle(&rrect(rect(l + 0.3, t + 0.3, r - 0.3, b - 0.3), 3.0), &ui.brush(rgba(0.0, 0.0, 0.0, 0.10)), 0.6, None);
            let area = rect(l + PC_B, t + PC_B, r - PC_B, b - PC_STRIP);
            let (aw, ah) = (area.right - area.left, area.bottom - area.top);
            match snap {
                Some(s) => {
                    // Cubre el hueco de la foto (recortando lo que sobra, centrado)
                    let k = (aw / s.w).max(ah / s.h);
                    let tm = Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: area.left - (s.w * k - aw) / 2.0, M32: area.top - (s.h * k - ah) / 2.0 };
                    let props = D2D1_BRUSH_PROPERTIES { opacity: 1.0, transform: tm };
                    if let Ok(br) = ui.rt.CreateBitmapBrush(&s.bmp, None, Some(&props)) { ui.rt.FillRectangle(&area, &br); }
                    ui.rt.DrawRectangle(&area, &ui.brush(rgba(0.0, 0.0, 0.0, 0.14)), 0.8, None);
                    if !s.date.is_empty() {
                        let txt = fit(ui, script, &s.date, PC_W - 2.0 * PC_B - 8.0);
                        ui.text_c(script, &txt, 0.0, b - PC_STRIP, PC_STRIP - 2.0, &ui.brush(rgba(0.20, 0.21, 0.26, 0.88)));
                    }
                }
                None => {
                    ui.rt.FillRectangle(&area, &ui.brush(rgba(0.16, 0.16, 0.18, 1.0)));
                    ui.glyph(&ui.f.icon_xl, "\u{EB9F}", l, area.top + ah / 2.0 - 22.0, PC_W, 44.0, &ui.brush(rgba(1.0, 1.0, 1.0, 0.45)));
                    let msg = fit(ui, &ui.f.small, i18n::t("Clic derecho para elegir una carpeta de fotos"), PC_W - 2.0 * PC_B - 4.0);
                    ui.text_c(&ui.f.small, &msg, 0.0, b - PC_STRIP, PC_STRIP - 2.0, &ui.brush(rgba(0.25, 0.25, 0.3, 0.85)));
                }
            }
            ui.rt.PopLayer();
            ui.rt.SetTransform(&base);
        }
        ui.shadow = halo;
    }
    fn dropping(&self) -> bool { self.cur.as_ref().is_some_and(|c| c.at.elapsed().as_secs_f32() < DROP_S) }
}

impl Widget for PolaroidW {
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, t: f32, _dt: f32) {
        deskw2::photo_start();
        let (w, h) = size(Kind::Photos);
        if self.rt.fresh(ui.rt) { self.cur = None; self.prev = None; }
        // ¿Foto nueva? Cae encima de la anterior
        if let Some((ver, px, pw, ph, path)) = deskw2::photo_latest() {
            if self.cur.as_ref().is_none_or(|c| c.ver != ver) {
                let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                if let Ok(bmp) = unsafe { ui.rt.CreateBitmap(D2D_SIZE_U { width: pw, height: ph }, Some(px.as_ptr() as _), pw * 4, &bp) } {
                    let date = photo_date(&path);
                    self.prev = self.cur.take();
                    self.cur = Some(Snap { ver, bmp, w: pw as f32, h: ph as f32, path, date, at: Instant::now() });
                    self.asked = false;
                }
            }
        }
        let f = vfonts(ui);
        let (cx, cy) = (w / 2.0, h / 2.0 + 2.0);
        match &self.cur {
            None => {
                let k = ez(t, 0.0, 0.6);
                Self::draw(ui, None, cx, cy - 12.0 * (1.0 - k), TILT, k, 1.0, &f.script);
            }
            Some(c) => {
                let e = c.at.elapsed().as_secs_f32();
                // La de debajo se queda quieta y se apaga cuando la nueva ya ha caído
                if let Some(p) = &self.prev {
                    let a = 1.0 - smoothstep(DROP_S * 0.6, DROP_S, e);
                    if a > 0.0 { Self::draw(ui, Some(p), cx, cy, TILT, a, 1.0, &f.script); }
                }
                let (dy, ang, a, sc) = drop_pose(e);
                Self::draw(ui, self.cur.as_ref(), cx, cy + dy, ang, a, sc, &f.script);
                if e >= DROP_S { self.prev = None; }
            }
        }
        ui.hit(rect(cx - PC_W / 2.0, cy - PC_H / 2.0, cx + PC_W / 2.0, cy + PC_H / 2.0), 1);
    }
    fn click(&mut self, id: Option<u32>, right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        if right { return Some(Box::new(deskw2::pick_folder)); }
        let p = self.cur.as_ref().map(|c| c.path.clone()).filter(|_| id.is_some())?;
        Some(Box::new(move || sys::open(&p)))
    }
    // Quieta no pinta: cada segundo mira si hay foto nueva y pide la siguiente cuando toca
    fn tick(&mut self) -> bool {
        let ver = deskw2::photo_latest().map(|p| p.0);
        if ver.is_some() && ver != self.cur.as_ref().map(|c| c.ver) { return true; }
        if let Some(c) = &self.cur {
            if !self.asked && c.at.elapsed().as_secs_f32() > deskw2::SHOW_S - 2.0 { deskw2::photo_next(); self.asked = true; }
        }
        false
    }
    fn animating(&self, t: f32) -> bool { t < 1.0 || self.dropping() }
    fn smooth(&self) -> bool { self.dropping() }
    fn frame_ms(&self) -> u32 { 16 }
}

// ================================================================ Visualizador: «Radial»
const RAYS: usize = 64;
const INNER: f32 = 60.0;

/// Nivel de la barra `i` (de 64) a partir de las bandas: media vuelta de graves a agudos y la otra
/// media en espejo (los graves arriba, los agudos abajo), interpolando entre bandas.
fn ray_level(levels: &[f32; audioviz::BANDS], i: usize) -> f32 {
    let half = RAYS / 2;
    let j = if i < half { i } else { RAYS - 1 - i.min(RAYS - 1) };
    let pos = j as f32 * (audioviz::BANDS - 1) as f32 / (half - 1) as f32;
    let (a, fr) = (pos.floor() as usize, pos.fract());
    let b = (a + 1).min(audioviz::BANDS - 1);
    let a = a.min(audioviz::BANDS - 1);
    (levels[a] * (1.0 - fr) + levels[b] * fr).clamp(0.0, 1.0)
}

/// Espectro de muestra para el modo demo cuando no suena nada de verdad.
fn demo_levels(t: f32) -> [f32; audioviz::BANDS] {
    let mut l = [0.0; audioviz::BANDS];
    for (i, v) in l.iter_mut().enumerate() {
        let f = i as f32;
        *v = ((0.75 - 0.013 * f) * (0.55 + 0.45 * (t * 3.1 + f * 0.5).sin()) * (0.7 + 0.3 * (t * 1.7 + f * 0.23).sin())).clamp(0.0, 1.0);
    }
    l
}

/// 64 barras que salen de un círculo con la carátula dentro, del color de la carátula; todo gira despacio.
pub struct RadialW { levels: [f32; audioviz::BANDS], alive: f32, art: Option<(u64, ID2D1Bitmap)>, rt: crate::RtKey, art_at: Option<Instant> }

impl Default for RadialW { fn default() -> Self { RadialW { levels: [0.0; audioviz::BANDS], alive: 0.0, art: None, rt: Default::default(), art_at: None } } }

impl Widget for RadialW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (w, h) = size(Kind::Visual);
        if self.rt.fresh(ui.rt) { self.art = None; }
        let m = MEDIA.lock().unwrap().clone();
        match &m {
            Some(m) => { if load_art(&mut self.art, ui.rt, m.art_ver) { self.art_at = Some(Instant::now()); } }
            None => self.art = None,
        }
        let now = deskw2::ui_time();
        let mut src = *audioviz::LEVELS.lock().unwrap();
        if sys::demo() && src.iter().sum::<f32>() < 0.2 { src = demo_levels(now); }
        let k = 1.0 - (-dt / 0.05).exp();
        let mut energy = 0.0;
        for i in 0..audioviz::BANDS { self.levels[i] += (src[i] - self.levels[i]) * k; energy += self.levels[i]; }
        let on = energy / audioviz::BANDS as f32 > 0.02;
        self.alive += ((if on { 1.0 } else { 0.0 }) - self.alive) * (1.0 - (-dt / 0.3).exp());
        let tn = deskw2::tint();
        let (cx, cy) = (w / 2.0, h / 2.0);
        let ki = ez(t, 0.0, 0.8);
        let spin = now * 0.06 - 0.6 * (1.0 - ki);
        let r0 = INNER + 5.0;
        let maxl = w.min(h) / 2.0 - r0 - 6.0;
        let st = round_stroke(ui.rt);
        unsafe {
            // Halo tenue del color de la carátula alrededor del círculo
            ui.rt.FillEllipse(&ellipse(cx, cy, INNER + 2.0), &ui.brush(rgba(tn[0], tn[1], tn[2], 0.10 * ki)));
            for i in 0..RAYS {
                let v = ray_level(&self.levels, i);
                let a = i as f32 / RAYS as f32 * TAU + spin;
                let (s, c) = a.sin_cos();
                // En silencio quedan puntos que respiran despacio
                let idle = 1.5 + 1.5 * (0.5 + 0.5 * (now * 1.6 + i as f32 * 0.4).sin());
                let len = ((v * maxl).max(idle * (1.0 - self.alive) + 2.0 * self.alive)) * ki;
                let b = ui.brush(rgba(tn[0], tn[1], tn[2], (0.5 + 0.5 * v) * ki));
                ui.rt.DrawLine(v2(cx + s * r0, cy - c * r0), v2(cx + s * (r0 + len), cy - c * (r0 + len)), &b, 3.0, st.as_ref());
            }
            ui.rt.DrawEllipse(&ellipse(cx, cy, INNER), &ui.brush(rgba(tn[0], tn[1], tn[2], 0.45 * ki)), 1.2, None);
            let ar = INNER - 5.0;
            let sc = 0.85 + 0.15 * ki;
            match &self.art {
                Some((_, b)) => {
                    let ka = self.art_at.map(|a| popup::ease_out(a.elapsed().as_secs_f32() / 0.5)).unwrap_or(1.0);
                    art_circle(ui, b, cx, cy, ar * sc, spin, ki * ka);
                }
                None => {
                    ui.rt.FillEllipse(&ellipse(cx, cy, ar * sc), &ui.wa(0.07 * ki));
                    popup::push_alpha(ui.rt, ki);
                    ui.glyph(&d.glyph_l, "\u{E8D6}", cx - 20.0, cy - 20.0, 40.0, 40.0, &ui.dim);
                    ui.rt.PopLayer();
                }
            }
        }
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn tick(&mut self) -> bool {
        if deskdata::VISIBLE.load(Ordering::Relaxed) { audioviz::want(); }
        true
    }
    fn animating(&self, _t: f32) -> bool { true }
    fn smooth(&self) -> bool { self.alive > 0.01 }
    fn frame_ms(&self) -> u32 { if self.alive > 0.01 { 16 } else { 60 } }
}

// ================================================================ Cuenta atrás: «Paletas»
const FLIP_S: f32 = 0.3;

/// Días, horas y minutos que faltan desde (hoy, hh:mm) hasta el principio del día `date`.
fn remaining(date: i64, today: i64, hh: u16, mm: u16) -> (i64, i64, i64) {
    let total = (date - today) * 1440 - (hh as i64 * 60 + mm as i64);
    if total <= 0 { return (0, 0, 0); }
    (total / 1440, total % 1440 / 60, total % 60)
}

/// Una paleta: el número de ahora y, mientras gira, el anterior.
#[derive(Default)]
struct Flap { cur: String, prev: String, at: Option<Instant> }

impl Flap {
    fn set(&mut self, s: String, delay: f32) -> bool {
        if self.cur == s { return false; }
        if self.cur.is_empty() { self.cur = s; return false; }
        self.prev = std::mem::replace(&mut self.cur, s);
        self.at = Some(Instant::now() + Duration::from_secs_f32(delay));
        true
    }
    /// Avance del giro (0..1; 1 = quieta).
    fn k(&self) -> f32 {
        let Some(a) = self.at else { return 1.0 };
        let now = Instant::now();
        if now < a { return 0.0; }
        ((now - a).as_secs_f32() / FLIP_S).min(1.0)
    }
}

#[derive(Default)]
pub struct FlipW { flaps: [Flap; 3], dates: Vec<(String, i64)>, checked: Option<Instant>, today: i64, sel: usize }

impl FlipW {
    /// Relee las fechas cada pocos segundos y pone los números. true si alguna paleta empieza a girar.
    fn update(&mut self) -> bool {
        if self.checked.is_none_or(|c| c.elapsed().as_secs_f32() >= 3.0) {
            self.checked = Some(Instant::now());
            self.today = deskw3::today();
            self.dates = deskw3::countdown_dates(self.today);
        }
        let Some((_, date)) = self.dates.get(self.sel % self.dates.len().max(1)).cloned() else { return false };
        let n = unsafe { GetLocalTime() };
        let (dd, hh, mm) = remaining(date, self.today, n.wHour, n.wMinute);
        let mut changed = false;
        // Si cambian varias a la vez, giran en cascada (de los minutos a los días)
        for (i, v) in [dd, hh, mm].into_iter().enumerate() { changed |= self.flaps[i].set(format!("{v:02}"), (2 - i) as f32 * 0.06); }
        changed
    }
    fn flipping(&self) -> bool { self.flaps.iter().any(|f| f.at.is_some() && f.k() < 1.0) }
}

/// Media paleta (la de arriba o la de abajo) con `text`, plegada en vertical hacia la línea del medio
/// (`sy` = 1 abierta, 0 de canto) y oscurecida `shade`.
#[allow(clippy::too_many_arguments)]
fn half(ui: &Ui, font: &IDWriteTextFormat, x: f32, y: f32, cw: f32, ch: f32, text: &str, top: bool, sy: f32, shade: f32) {
    if sy < 0.02 { return; }
    let mid = y + ch / 2.0;
    let base = cur_transform(ui);
    unsafe {
        ui.rt.SetTransform(&mul(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: sy, M31: 0.0, M32: mid * (1.0 - sy) }, &base));
        let clip = if top { rect(x, y, x + cw, mid) } else { rect(x, mid, x + cw, y + ch) };
        ui.rt.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
        let g = ui.p.vgrad(y, y + ch, rgba(0.17, 0.17, 0.19, 1.0), rgba(0.10, 0.10, 0.115, 1.0));
        ui.rt.FillRoundedRectangle(&rrect(rect(x, y, x + cw, y + ch), 9.0), &g);
        ui.text_c(font, text, x + cw / 2.0, y, ch, &ui.brush(rgba(0.96, 0.96, 0.97, 1.0)));
        if shade > 0.01 { ui.rt.FillRectangle(&clip, &ui.brush(rgba(0.0, 0.0, 0.0, shade))); }
        ui.rt.PopAxisAlignedClip();
        ui.rt.SetTransform(&base);
    }
}

/// Una tarjeta de paletas: quieta, o girando (la mitad de arriba del número viejo cae hasta la línea
/// y luego la de abajo del nuevo se despliega desde ella).
fn flip_card(ui: &Ui, font: &IDWriteTextFormat, x: f32, y: f32, cw: f32, ch: f32, f: &Flap) {
    let k = f.k();
    unsafe {
        ui.rt.FillRoundedRectangle(&rrect(rect(x + 1.0, y + 3.0, x + cw + 1.0, y + ch + 4.0), 10.0), &ui.brush(rgba(0.0, 0.0, 0.0, 0.28)));
        // Fondo entero debajo de las dos mitades: sin rendija en la línea del medio
        ui.rt.FillRoundedRectangle(&rrect(rect(x, y, x + cw, y + ch), 9.0), &ui.brush(rgba(0.12, 0.12, 0.135, 1.0)));
    }
    if k >= 1.0 || f.at.is_none() {
        half(ui, font, x, y, cw, ch, &f.cur, true, 1.0, 0.0);
        half(ui, font, x, y, cw, ch, &f.cur, false, 1.0, 0.0);
    } else {
        half(ui, font, x, y, cw, ch, &f.cur, true, 1.0, 0.25 * (1.0 - k));
        half(ui, font, x, y, cw, ch, &f.prev, false, 1.0, 0.0);
        if k < 0.5 {
            let e = k / 0.5;
            let s = 1.0 - e * e;
            half(ui, font, x, y, cw, ch, &f.prev, true, s, 0.35 * e);
        } else {
            let s = popup::ease_out((k - 0.5) / 0.5);
            half(ui, font, x, y, cw, ch, &f.cur, false, s, 0.35 * (1.0 - s));
        }
    }
    let mid = y + ch / 2.0;
    unsafe {
        ui.rt.FillRectangle(&rect(x, mid - 0.8, x + cw, mid + 0.8), &ui.brush(rgba(0.0, 0.0, 0.0, 0.7)));
        // Los dos «ejes» de la bisagra a los lados
        for bx in [x + 2.5, x + cw - 2.5] { ui.rt.FillRoundedRectangle(&rrect(rect(bx - 1.5, mid - 4.0, bx + 1.5, mid + 4.0), 1.5), &ui.brush(rgba(0.0, 0.0, 0.0, 0.6))); }
    }
}

impl Widget for FlipW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, _dt: f32) {
        let (w, h) = size(Kind::Countdown);
        self.update();
        let f = vfonts(ui);
        if self.dates.is_empty() {
            deskw::header(ui, d, i18n::t("Cuenta atrás"), "", w);
            ui.glyph(&d.glyph_l, "\u{E787}", w / 2.0 - 20.0, h / 2.0 - 26.0, 40.0, 40.0, &ui.faint);
            ui.text_c(&ui.f.sub, i18n::t("Clic derecho para añadir una fecha"), w / 2.0, h / 2.0 + 18.0, 18.0, &ui.dim);
            ui.hit(rect(0.0, 0.0, w, h), 1);
            return;
        }
        self.sel %= self.dates.len();
        let (name, date) = self.dates[self.sel].clone();
        let x0 = 16.0;
        let k0 = ez(t, 0.0, 0.5);
        popup::push_alpha(ui.rt, k0);
        let (_, mo, dd) = deskw3::civil_from_days(date);
        let right = if date == self.today { i18n::t("¡Hoy!").to_string() } else { i18n::day_month_short(dd, (mo - 1) as usize) };
        let rb = if date == self.today { ui.brush(crate::profiles::accent(1.0)) } else { ui.dim.clone() };
        let rw = ui.p.width(&ui.f.sub, &right);
        ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &name, w - 2.0 * x0 - rw - 12.0), x0 + 1.0, 8.0, 20.0, &ui.white);
        ui.text_r(&ui.f.sub, &right, w - x0, 8.0, 20.0, &rb);
        unsafe { ui.rt.PopLayer(); }
        // Tres tarjetas en cascada
        let (cw, ch, gap, y0) = (90.0, 80.0, 9.0, 36.0);
        let labels = [i18n::t("días"), i18n::t("horas"), "min"];
        let halo = ui.shadow.take();
        for i in 0..3 {
            let k = ez(t, 0.08 + 0.08 * i as f32, 0.5);
            let x = x0 + i as f32 * (cw + gap);
            popup::push_alpha(ui.rt, k);
            flip_card(ui, &f.flip, x, y0 + 8.0 * (1.0 - k), cw, ch, &self.flaps[i]);
            unsafe { ui.rt.PopLayer(); }
        }
        ui.shadow = halo;
        for (i, l) in labels.iter().enumerate() {
            let k = ez(t, 0.2 + 0.08 * i as f32, 0.5);
            popup::push_alpha(ui.rt, k);
            ui.text_c(&ui.f.small, l, x0 + i as f32 * (cw + gap) + cw / 2.0, y0 + ch + 6.0, 16.0, &ui.dim);
            unsafe { ui.rt.PopLayer(); }
        }
        ui.hit(rect(0.0, 0.0, w, h), 1);
    }
    fn click(&mut self, id: Option<u32>, right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        id?;
        // Clic derecho: editar las fechas; clic: la fecha siguiente (las paletas giran hasta ella)
        if right { return Some(Box::new(deskw3::countdown_edit)); }
        if self.dates.len() > 1 { self.sel = (self.sel + 1) % self.dates.len(); self.update(); }
        Some(Box::new(|| {}))
    }
    fn tick(&mut self) -> bool { self.update() }
    fn animating(&self, t: f32) -> bool { t < 1.0 || self.flipping() }
    fn smooth(&self) -> bool { self.flipping() }
    fn frame_ms(&self) -> u32 { 16 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiempo_que_falta() {
        let hoy = deskw3::days_from_civil(2026, 11, 22);
        // A las 14:00, para dentro de 28 días: 27 días, 10 horas y 0 minutos
        assert_eq!(remaining(hoy + 28, hoy, 14, 0), (27, 10, 0));
        assert_eq!(remaining(hoy + 1, hoy, 23, 59), (0, 0, 1));
        assert_eq!(remaining(hoy + 1, hoy, 0, 0), (1, 0, 0));
        // Ya es el día (o pasó): todo a cero, nunca negativo
        assert_eq!(remaining(hoy, hoy, 9, 30), (0, 0, 0));
        assert_eq!(remaining(hoy - 3, hoy, 9, 30), (0, 0, 0));
    }

    #[test]
    fn barras_del_visualizador_radial() {
        let mut l = [0.0f32; audioviz::BANDS];
        for (i, v) in l.iter_mut().enumerate() { *v = i as f32 / (audioviz::BANDS - 1) as f32; }
        // Espejo: cada barra vale lo mismo que su simétrica, de graves (arriba) a agudos (abajo)
        for i in 0..RAYS / 2 { assert!((ray_level(&l, i) - ray_level(&l, RAYS - 1 - i)).abs() < 1e-6); }
        assert!(ray_level(&l, 0).abs() < 1e-6);
        assert!((ray_level(&l, RAYS / 2 - 1) - 1.0).abs() < 1e-6);
        for i in 1..RAYS / 2 { assert!(ray_level(&l, i) >= ray_level(&l, i - 1)); }
        // Nunca se sale de las bandas, ni con niveles fuera de rango
        let big = [3.0f32; audioviz::BANDS];
        for i in 0..RAYS { assert!(ray_level(&big, i) <= 1.0); }
        for t in [0.0, 1.3, 77.7] { assert!(demo_levels(t).iter().all(|v| (0.0..=1.0).contains(v))); }
    }

    #[test]
    fn caida_de_la_polaroid() {
        // Al llegar está arriba y transparente; al acabar, quieta en su sitio y con su inclinación
        let (dy, _, a, _) = drop_pose(0.0);
        assert!(dy < -30.0 && a == 0.0);
        assert_eq!(drop_pose(DROP_S), (0.0, TILT, 1.0, 1.0));
        assert_eq!(drop_pose(DROP_S + 5.0), (0.0, TILT, 1.0, 1.0));
        // Sin saltos al terminar el balanceo
        let (dy, ang, a, sc) = drop_pose(DROP_S - 0.001);
        assert!(dy.abs() < 0.01 && (ang - TILT).abs() < 0.05 && a == 1.0 && (sc - 1.0).abs() < 0.001);
    }

    #[test]
    fn fecha_local_de_la_foto() {
        // 2025-03-14 23:30 UTC es ya el 15 en Madrid (+60 min) y sigue siendo el 14 en Nueva York (−300)
        let s = (deskw3::days_from_civil(2025, 3, 14) * 86_400 + 23 * 3600 + 30 * 60) as i64;
        assert_eq!(local_ymd(s, 60), (2025, 3, 15));
        assert_eq!(local_ymd(s, -300), (2025, 3, 14));
        assert_eq!(local_ymd(0, 0), (1970, 1, 1));
    }

    #[test]
    fn brazo_del_tocadiscos() {
        // En reposo fuera del disco; sonando, entra hacia la etiqueta a medida que avanza la canción
        assert_eq!(arm_angle(0.0, 0.5), ARM_REST);
        assert!((arm_angle(1.0, 0.0) - ARM_PLAY).abs() < 1e-6);
        assert!(arm_angle(1.0, 1.0) > arm_angle(1.0, 0.0));
        assert!((arm_angle(1.0, 5.0) - (ARM_PLAY + ARM_SWEEP)).abs() < 1e-6);
    }

    #[test]
    fn mayusculas_espaciadas() {
        assert_eq!(spaced("LUNES"), "L\u{2009}U\u{2009}N\u{2009}E\u{2009}S");
        assert_eq!(spaced(""), "");
        assert_eq!(spaced("Á"), "Á");
    }
}
