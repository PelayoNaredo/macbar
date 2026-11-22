//! Tercera tanda de diseños alternativos de los widgets, con el estilo de los demás (flotantes, la misma
//! letra, los mismos grises y el acento solo en detalles minúsculos) y cristal de verdad como MATERIAL
//! del propio objeto: las baldosas del logotipo de Windows, el cristal del reloj, la ventana y el cuerpo
//! ahumado del casete, las columnas del espectro, el tubo de cada unidad, el orbe y las burbujas. El
//! vidrio dobla en sus cantos lo que tiene detrás, que es el fondo de pantalla real (`fondo`), con una
//! franja de color finísima; textos y cifras nunca quedan debajo de un cristal que los deforme. Ficha
//! del equipo, espectro, juego de la vida, orbe, reloj, casete y unidades, con datos reales. Como los de
//! `deskv`/`deskv2`, cada uno es otro `Widget` con su propio tamaño (`desk::size`).

use super::*;
use desk::{size, DFonts, Kind};
use deskw::Widget;
use popup::Ui;
use std::f32::consts::{PI, TAU};
use std::rc::Rc;

// ================================================================ utilidades comunes
fn ez(t: f32, delay: f32, dur: f32) -> f32 { popup::ease_out(((t - delay) / dur).clamp(0.0, 1.0)) }
fn ellipse(x: f32, y: f32, r: f32) -> D2D1_ELLIPSE { D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: r, radiusY: r } }
fn ell(x: f32, y: f32, rx: f32, ry: f32) -> D2D1_ELLIPSE { D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: rx, radiusY: ry } }
fn v2(x: f32, y: f32) -> Vector2 { Vector2 { X: x, Y: y } }
fn lerp(a: f32, b: f32, k: f32) -> f32 { a + (b - a) * k }

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
    /// ¿Movimiento reducido? (se vuelve a mirar cada pocos segundos): (cuándo, valor)
    static REDUCED: std::cell::Cell<(f32, bool)> = const { std::cell::Cell::new((-100.0, false)) };
    /// Degradados ya creados, por lienzo (los más recientes al final) y por colores y geometría
    static GRADS: RefCell<Vec<(windows::core::IUnknown, HashMap<u64, ID2D1Brush>)>> = const { RefCell::new(Vec::new()) };
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

/// ¿Windows tiene las animaciones desactivadas? (Accesibilidad > Efectos visuales; MACBAR_REDUCED=1 lo simula).
/// Con movimiento reducido lo que gira se queda quieto y la vida avanza despacio.
fn reduced() -> bool {
    let now = deskw2::ui_time();
    let (at, v) = REDUCED.with(|c| c.get());
    if now - at < 5.0 { return v; }
    let v = std::env::var_os("MACBAR_REDUCED").is_some() || {
        let mut on = windows::core::BOOL(1);
        unsafe { let _ = SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
        !on.as_bool()
    };
    REDUCED.with(|c| c.set((now, v)));
    v
}

/// Color de acento con opacidad (solo para detalles minúsculos: un punto, una raya de 1-2 px).
fn acc(a: f32) -> D2D1_COLOR_F { crate::profiles::accent(a) }
/// Blanco con opacidad (brillos y reflejos: la luz es blanca también sobre fondo claro).
fn wh(a: f32) -> D2D1_COLOR_F { rgba(1.0, 1.0, 1.0, a) }
/// El mismo color con otra opacidad.
fn with_a(c: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F { D2D1_COLOR_F { a, ..c } }

/// Opacidad redondeada a pasos de 1/`n` (así los pinceles y degradados se reutilizan de la caché).
fn q(a: f32, n: f32) -> f32 { (a.clamp(0.0, 1.0) * n).round() / n }

/// Un círculo relleno.
fn dot(rt: &ID2D1RenderTarget, x: f32, y: f32, r: f32, b: &ID2D1Brush) {
    if r > 0.05 { unsafe { rt.FillEllipse(&ellipse(x, y, r), b); } }
}

/// Espectro de ahora (en modo demo, uno de muestra si aún no llega nada).
fn levels_now() -> [f32; audioviz::BANDS] {
    let src = *audioviz::LEVELS.lock().unwrap();
    if sys::demo() && src.iter().sum::<f32>() < 0.2 {
        let t = deskw2::ui_time();
        let mut l = [0.0; audioviz::BANDS];
        for (i, v) in l.iter_mut().enumerate() {
            let f = i as f32;
            *v = ((0.75 - 0.013 * f) * (0.55 + 0.45 * (t * 3.1 + f * 0.5).sin()) * (0.7 + 0.3 * (t * 1.7 + f * 0.23).sin())).clamp(0.0, 1.0);
        }
        return l;
    }
    src
}

/// Media de las bandas [a, b) del espectro.
fn band_mean(l: &[f32; audioviz::BANDS], a: usize, b: usize) -> f32 {
    let b = b.min(audioviz::BANDS);
    if b <= a { return 0.0; }
    l[a..b].iter().sum::<f32>() / (b - a) as f32
}

/// Nivel de la columna `c` de `cols` a partir de las bandas (interpolando entre bandas vecinas).
fn col_level(l: &[f32; audioviz::BANDS], c: usize, cols: usize) -> f32 {
    if cols < 2 { return l[0].clamp(0.0, 1.0); }
    let pos = c.min(cols - 1) as f32 * (audioviz::BANDS - 1) as f32 / (cols - 1) as f32;
    let (a, fr) = (pos.floor() as usize, pos.fract());
    let b = (a + 1).min(audioviz::BANDS - 1);
    (l[a.min(audioviz::BANDS - 1)] * (1.0 - fr) + l[b] * fr).clamp(0.0, 1.0)
}

/// Barra fina como las de los widgets clásicos: carril y relleno blanco.
fn bar(ui: &Ui, x: f32, y: f32, w: f32, h: f32, frac: f32) {
    unsafe {
        ui.rt.FillRoundedRectangle(&rrect(rect(x, y, x + w, y + h), h / 2.0), &ui.track);
        let fw = (w * frac.clamp(0.0, 1.0)).max(if frac > 0.0 { h } else { 0.0 });
        if fw > 0.0 { ui.rt.FillRoundedRectangle(&rrect(rect(x, y, x + fw, y + h), h / 2.0), &ui.white); }
    }
}

// ---------------------------------------------------------------- transformaciones
/// a · b (primero a, luego b) para transformaciones 2D de Direct2D.
fn mulm(a: &Matrix3x2, b: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: a.M11 * b.M11 + a.M12 * b.M21, M12: a.M11 * b.M12 + a.M12 * b.M22,
        M21: a.M21 * b.M11 + a.M22 * b.M21, M22: a.M21 * b.M12 + a.M22 * b.M22,
        M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31, M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    }
}
fn cur_xf(ui: &Ui) -> Matrix3x2 { let mut m = Matrix3x2::default(); unsafe { ui.rt.GetTransform(&mut m); } m }

/// Pone el origen en (cx, cy) con escala (sx, sy) encima de `base`: así se dibuja en «espacio unidad»
/// (un círculo de radio 1) y los degradados, definidos una vez en ese espacio, valen para cualquier
/// tamaño y sitio sin crear otros.
fn place(ui: &Ui, base: &Matrix3x2, cx: f32, cy: f32, sx: f32, sy: f32) {
    unsafe { ui.rt.SetTransform(&mulm(&Matrix3x2 { M11: sx, M12: 0.0, M21: 0.0, M22: sy, M31: cx, M32: cy }, base)); }
}
/// Escala `k` alrededor de (cx, cy), sin cambiar el sistema de coordenadas del widget.
fn scale_about(ui: &Ui, base: &Matrix3x2, cx: f32, cy: f32, k: f32) {
    unsafe { ui.rt.SetTransform(&mulm(&Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: cx * (1.0 - k), M32: cy * (1.0 - k) }, base)); }
}
fn reset(ui: &Ui, base: &Matrix3x2) { unsafe { ui.rt.SetTransform(base); } }

// ---------------------------------------------------------------- degradados cacheados
/// Huella de un degradado: tipo, paradas y geometría.
fn gkey(kind: u32, stops: &[(f32, D2D1_COLOR_F)], geo: [f32; 4]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ kind as u64;
    let mut eat = |v: u32| { h ^= v as u64; h = h.wrapping_mul(0x0100_0000_01b3); };
    for (p, c) in stops { eat(p.to_bits()); eat(pack(c)); }
    for g in geo { eat(g.to_bits()); }
    h
}

/// Degradado de la caché del lienzo actual (o recién creado con `make`).
fn grad_cached(ui: &Ui, key: u64, make: impl FnOnce(&ID2D1RenderTarget) -> Option<ID2D1Brush>) -> Option<ID2D1Brush> {
    use windows::core::Interface;
    let unk = ui.rt.cast::<windows::core::IUnknown>().ok()?;
    GRADS.with(|g| {
        let mut g = g.borrow_mut();
        let i = match g.iter().position(|e| e.0 == unk) {
            Some(i) => i,
            None => {
                // Pocos lienzos a la vez (uno por widget y su capa): los que ya no se usan se van solos
                if g.len() >= 16 { g.remove(0); }
                g.push((unk, HashMap::new()));
                g.len() - 1
            }
        };
        if let Some(b) = g[i].1.get(&key) { return Some(b.clone()); }
        let b = make(ui.rt)?;
        if g[i].1.len() > 320 { g[i].1.clear(); }
        g[i].1.insert(key, b.clone());
        Some(b)
    })
}

fn stop_coll(rt: &ID2D1RenderTarget, stops: &[(f32, D2D1_COLOR_F)]) -> Option<ID2D1GradientStopCollection> {
    let v: Vec<D2D1_GRADIENT_STOP> = stops.iter().map(|(p, c)| D2D1_GRADIENT_STOP { position: *p, color: *c }).collect();
    unsafe { rt.CreateGradientStopCollection(&v, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok() }
}

/// Degradado lineal de (x0, y0) a (x1, y1), de la caché.
fn lin(ui: &Ui, x0: f32, y0: f32, x1: f32, y1: f32, stops: &[(f32, D2D1_COLOR_F)]) -> ID2D1Brush {
    grad_cached(ui, gkey(1, stops, [x0, y0, x1, y1]), |rt| unsafe {
        let col = stop_coll(rt, stops)?;
        rt.CreateLinearGradientBrush(&D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: v2(x0, y0), endPoint: v2(x1, y1) }, None, &col).ok().map(Into::into)
    }).unwrap_or_else(|| ui.brush(stops[0].1))
}

/// Degradado radial en espacio unidad (centro en el origen, radio 1; ver `place`). (ox, oy) desplaza el
/// foco del degradado, para brillos descentrados.
fn rad(ui: &Ui, ox: f32, oy: f32, stops: &[(f32, D2D1_COLOR_F)]) -> ID2D1Brush {
    grad_cached(ui, gkey(2, stops, [ox, oy, 0.0, 0.0]), |rt| unsafe {
        let col = stop_coll(rt, stops)?;
        let p = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES { center: v2(0.0, 0.0), gradientOriginOffset: v2(ox, oy), radiusX: 1.0, radiusY: 1.0 };
        rt.CreateRadialGradientBrush(&p, None, &col).ok().map(Into::into)
    }).unwrap_or_else(|| ui.brush(stops[0].1))
}

/// Resplandor suave: un círculo de `r` cuyo color se desvanece hacia el borde.
fn glow(ui: &Ui, base: &Matrix3x2, x: f32, y: f32, r: f32, c: D2D1_COLOR_F, a: f32) {
    if a < 0.005 || r < 0.5 { return; }
    place(ui, base, x, y, r, r);
    let b = rad(ui, 0.0, 0.0, &[(0.0, with_a(c, q(a, 40.0))), (0.45, with_a(c, q(a * 0.4, 40.0))), (1.0, with_a(c, 0.0))]);
    unsafe { ui.rt.FillEllipse(&ellipse(0.0, 0.0, 1.0), &b); }
    reset(ui, base);
}

// ---------------------------------------------------------------- tipografía fina
/// Letras grandes y finas de estos diseños (como la del Mínimo y el Póster): se crean una vez por hilo.
struct GFonts { clock: IDWriteTextFormat, face: IDWriteTextFormat, mid: IDWriteTextFormat }
thread_local! { static GF: std::cell::OnceCell<Rc<GFonts>> = const { std::cell::OnceCell::new() }; }

fn gfonts(ui: &Ui) -> Rc<GFonts> {
    GF.with(|c| c.get_or_init(|| {
        let dw = ui.p.dw;
        Rc::new(GFonts {
            clock: make_format(dw, FONT, 54.0, DWRITE_FONT_WEIGHT_LIGHT),
            face: make_format(dw, FONT, 40.0, DWRITE_FONT_WEIGHT_LIGHT),
            mid: make_format(dw, FONT, 21.0, DWRITE_FONT_WEIGHT_LIGHT),
        })
    }).clone())
}

// ================================================================ cristal que refracta
// El cristal de estos diseños es siempre una PIEZA del objeto (las baldosas del logotipo, el cristal
// del reloj, la ventana del casete, las columnas del espectro, el tubo de cada unidad, el orbe), nunca
// una lupa encima de la información. Lo que se ve a través de él es lo que de verdad tiene detrás: el
// fondo de pantalla bajo el widget (`fondo::under`) y, si acaso, algo decorativo del propio objeto (las
// bobinas tras la ventana, las marcas de minutos bajo el cristal del reloj). Textos y cifras van siempre
// por encima, sin deformar.
//
// Lo de detrás se pinta en una imagen aparte y se dobla con un mapa de desplazamiento (efecto
// DisplacementMap de Direct2D): nada en la parte plana de la pieza y cada vez más en el bisel, como el
// canto de un vidrio macizo. Los canales rojo y azul se doblan algo más y algo menos que el verde, y eso
// da la franja cromática finísima de los cantos. El mapa se hace una vez por tamaño y piezas; lo
// refractado, solo cuando cambia lo de detrás (el fondo o el decorado); por fotograma solo se rellenan
// las formas con esa imagen.

/// Forma de una pieza: círculo (centro y radio) o rectángulo redondeado (x0, y0, x1, y1, radio).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Shape { Circle(f32, f32, f32), RRect(f32, f32, f32, f32, f32) }

/// Pieza de cristal: su forma, el ancho del bisel (la franja del canto que dobla; dentro es plana) y
/// la fuerza de la lente.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Piece { shape: Shape, bevel: f32, k: f32 }

impl Piece {
    fn disc(cx: f32, cy: f32, r: f32, bevel: f32, k: f32) -> Self { Piece { shape: Shape::Circle(cx, cy, r), bevel: bevel.max(0.5), k } }
    fn slab(x0: f32, y0: f32, x1: f32, y1: f32, rad: f32, bevel: f32, k: f32) -> Self {
        let rad = rad.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
        Piece { shape: Shape::RRect(x0, y0, x1, y1, rad), bevel: bevel.max(0.5), k }
    }
    /// Distancia con signo al borde (negativa dentro) y normal hacia fuera.
    fn sdf(&self, x: f32, y: f32) -> (f32, f32, f32) {
        match self.shape {
            Shape::Circle(cx, cy, r) => {
                let (dx, dy) = (x - cx, y - cy);
                let l = (dx * dx + dy * dy).sqrt();
                if l < 1e-5 { return (-r, 0.0, 0.0); }
                (l - r, dx / l, dy / l)
            }
            Shape::RRect(x0, y0, x1, y1, rad) => {
                let (cx, cy, hx, hy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0, (x1 - x0) / 2.0, (y1 - y0) / 2.0);
                let (px, py) = (x - cx, y - cy);
                let (qx, qy) = (px.abs() - (hx - rad), py.abs() - (hy - rad));
                if qx > 0.0 && qy > 0.0 {
                    let l = (qx * qx + qy * qy).sqrt();
                    (l - rad, px.signum() * qx / l, py.signum() * qy / l)
                } else if qx > qy {
                    (qx - rad, px.signum(), 0.0)
                } else {
                    (qy - rad, 0.0, py.signum())
                }
            }
        }
    }
    /// Desplazamiento (DIP) con el que se mira lo de detrás desde (x, y); None fuera del cristal.
    fn disp(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let (d, nx, ny) = self.sdf(x, y);
        if d > 0.0 { return None; }
        let m = 1.0 - (-d / self.bevel).min(1.0);
        if m <= 0.0 { return Some((0.0, 0.0)); }
        let b = bend(m) * self.bevel * self.k;
        Some((nx * b, ny * b))
    }
    fn bounds(&self) -> (f32, f32, f32, f32) {
        match self.shape { Shape::Circle(cx, cy, r) => (cx - r, cy - r, cx + r, cy + r), Shape::RRect(x0, y0, x1, y1, _) => (x0, y0, x1, y1) }
    }
    /// Lo más que se desplaza la mirada en esta pieza.
    fn max_disp(&self) -> f32 { -bend(1.0) * self.bevel * self.k }
    fn geometry(&self, fac: &ID2D1Factory) -> Option<ID2D1Geometry> {
        unsafe {
            Some(match self.shape {
                Shape::Circle(cx, cy, r) => fac.CreateEllipseGeometry(&ellipse(cx, cy, r)).ok()?.into(),
                Shape::RRect(x0, y0, x1, y1, rad) => fac.CreateRoundedRectangleGeometry(&rrect(rect(x0, y0, x1, y1), rad)).ok()?.into(),
            })
        }
    }
}

/// Cuánto se desvía la mirada a una altura `m` del bisel (0 donde empieza, 1 en el canto), en unidades
/// del bisel; negativo = hacia dentro. Casi nada al empezar y mucho pegado al canto.
fn bend(m: f32) -> f32 { -(0.2 * m + 1.05 * m * m * m) }

/// Color del mapa para un desplazamiento `d` (DIP) con `span` (px) de recorrido total y `s` px por DIP.
fn map_byte(d: f32, s: f32, span: f32) -> u8 { ((0.5 + d * s / span).clamp(0.0, 1.0) * 255.0).round() as u8 }

/// Color de un pincel sólido (para rehacer los de la interfaz en otro lienzo).
fn col_of(b: &ID2D1Brush) -> D2D1_COLOR_F {
    use windows::core::Interface;
    b.cast::<ID2D1SolidColorBrush>().map(|s| unsafe { s.GetColor() }).unwrap_or(rgba(1.0, 1.0, 1.0, 1.0))
}

/// Arco de circunferencia (ángulos en radianes, en el sentido del reloj desde las 3).
fn arc_geo(f: &ID2D1Factory, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> Option<ID2D1Geometry> {
    unsafe {
        let path = f.CreatePathGeometry().ok()?;
        let sink = path.Open().ok()?;
        sink.BeginFigure(v2(cx + r * a0.cos(), cy + r * a0.sin()), D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddArc(&D2D1_ARC_SEGMENT { point: v2(cx + r * a1.cos(), cy + r * a1.sin()), size: D2D_SIZE_F { width: r, height: r }, rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE, arcSize: if a1 - a0 > PI { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL } });
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close().ok()?;
        Some(path.into())
    }
}

/// Recursos de la refracción, rehechos solo si cambia el lienzo, el tamaño o las piezas.
struct LensRes {
    key: (windows::core::IUnknown, u32, u32),
    /// Lo de detrás (fondo + decorado) y lo de detrás ya doblado
    back: ID2D1BitmapRenderTarget,
    out: ID2D1BitmapRenderTarget,
    /// Pincel con lo doblado: rellenar cualquier forma con él es «poner cristal» ahí
    brush: ID2D1BitmapBrush,
    geos: Vec<ID2D1Geometry>,
    /// Desplazamiento del verde, del rojo (algo más) y del azul (algo menos), y su canal de cada uno
    disp: [ID2D1Effect; 3],
    chan: [ID2D1Effect; 3],
    /// Zona que ocupan las piezas (lo único que se procesa)
    clip: D2D_RECT_F,
    /// Arcos especulares de las piezas redondas: (geometría, grosor, opacidad)
    arcs: Vec<(ID2D1Geometry, f32, f32)>,
}

/// Una o varias piezas de cristal que doblan el fondo de pantalla (y lo que el objeto tenga detrás).
#[derive(Default)]
pub(crate) struct Lens {
    pieces: Vec<Piece>,
    res: Option<LensRes>,
    /// Huella de lo último refractado (None: hay que rehacerlo)
    last: Option<u64>,
    /// Caché de medidas y pinceles del lienzo de la imagen aparte
    cache: RefCell<Caches>,
}

impl Lens {
    /// Cambia las piezas (si son otras, se rehacen el mapa y las formas).
    fn set(&mut self, pieces: &[Piece]) {
        if self.pieces != pieces { self.pieces = pieces.to_vec(); self.res = None; self.last = None; }
    }

    /// Prepara los recursos. false si no se puede (sin contexto de dispositivo).
    fn prepare(&mut self, ui: &Ui, w: f32, h: f32) -> bool {
        use windows::core::Interface;
        if self.pieces.is_empty() { return false; }
        let Ok(unk) = ui.rt.cast::<windows::core::IUnknown>() else { return false };
        let s = ui.s.max(0.5);
        let (pw, ph) = ((w * s).ceil() as u32, (h * s).ceil() as u32);
        if self.res.as_ref().is_some_and(|r| r.key == (unk.clone(), pw, ph)) { return true; }
        self.last = None;
        self.res = self.build(ui, unk, w, h, pw, ph);
        self.res.is_some()
    }

    fn build(&self, ui: &Ui, unk: windows::core::IUnknown, w: f32, h: f32, pw: u32, ph: u32) -> Option<LensRes> {
        use windows::core::Interface;
        let s = ui.s.max(0.5);
        let fac = unsafe { ui.rt.GetFactory().ok()? };
        unsafe {
            let mk = || ui.rt.CreateCompatibleRenderTarget(Some(&D2D_SIZE_F { width: w, height: h }), None, None, D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE).ok();
            let (back, out) = (mk()?, mk()?);
            let odc: ID2D1DeviceContext = out.cast().ok()?;
            // Mapa de desplazamiento: rojo = x, verde = y (0,5 = quieto)
            let span = 2.0 * 1.06 * self.pieces.iter().map(|p| p.max_disp()).fold(1.0, f32::max) * s + 4.0;
            let bb: Vec<(f32, f32, f32, f32)> = self.pieces.iter().map(|p| p.bounds()).collect();
            let mut px = vec![0u8; (pw * ph * 4) as usize];
            for j in 0..ph {
                for i in 0..pw {
                    let (x, y) = ((i as f32 + 0.5) / s, (j as f32 + 0.5) / s);
                    let (dx, dy) = self.pieces.iter().zip(&bb)
                        .filter(|(_, b)| x >= b.0 && x <= b.2 && y >= b.1 && y <= b.3)
                        .find_map(|(p, _)| p.disp(x, y)).unwrap_or((0.0, 0.0));
                    let o = ((j * pw + i) * 4) as usize;
                    px[o] = 128;
                    px[o + 1] = map_byte(dy, s, span);
                    px[o + 2] = map_byte(dx, s, span);
                    px[o + 3] = 255;
                }
            }
            let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0 * s, dpiY: 96.0 * s };
            let map = ui.rt.CreateBitmap(D2D_SIZE_U { width: pw, height: ph }, Some(px.as_ptr() as _), pw * 4, &bp).ok()?;
            // Verde con el desplazamiento justo, rojo algo más y azul algo menos: cada uno, solo su canal
            let mut disp = Vec::new();
            let mut chan = Vec::new();
            for (k, c) in [(1.0f32, 1usize), (1.06, 0), (0.94, 2)] {
                let e = odc.CreateEffect(&CLSID_D2D1DisplacementMap).ok()?;
                e.SetValue(D2D1_DISPLACEMENTMAP_PROP_SCALE.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &(span * k).to_le_bytes()).ok()?;
                e.SetValue(D2D1_DISPLACEMENTMAP_PROP_X_CHANNEL_SELECT.0 as u32, D2D1_PROPERTY_TYPE_ENUM, &(D2D1_CHANNEL_SELECTOR_R.0 as u32).to_le_bytes()).ok()?;
                e.SetValue(D2D1_DISPLACEMENTMAP_PROP_Y_CHANNEL_SELECT.0 as u32, D2D1_PROPERTY_TYPE_ENUM, &(D2D1_CHANNEL_SELECTOR_G.0 as u32).to_le_bytes()).ok()?;
                e.SetInput(1, &map, false);
                let m = odc.CreateEffect(&CLSID_D2D1ColorMatrix).ok()?;
                let mut mx = [0.0f32; 20];
                mx[c * 5] = 1.0;
                mx[15] = 1.0;
                let bytes: Vec<u8> = mx.iter().flat_map(|v| v.to_le_bytes()).collect();
                m.SetValue(D2D1_COLORMATRIX_PROP_COLOR_MATRIX.0 as u32, D2D1_PROPERTY_TYPE_MATRIX_5X4, &bytes).ok()?;
                m.SetValue(D2D1_COLORMATRIX_PROP_ALPHA_MODE.0 as u32, D2D1_PROPERTY_TYPE_ENUM, &(D2D1_COLORMATRIX_ALPHA_MODE_PREMULTIPLIED.0 as u32).to_le_bytes()).ok()?;
                m.SetInput(0, &e.GetOutput().ok()?, false);
                disp.push(e);
                chan.push(m);
            }
            let bmp = out.GetBitmap().ok()?;
            let brush: ID2D1BitmapBrush = ui.rt.CreateBitmapBrush(&bmp, None, None).ok()?;
            let mut geos = Vec::new();
            let mut arcs = Vec::new();
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for p in &self.pieces {
                geos.push(p.geometry(&fac)?);
                let b = p.bounds();
                (x0, y0, x1, y1) = (x0.min(b.0), y0.min(b.1), x1.max(b.2), y1.max(b.3));
                if let Shape::Circle(cx, cy, r) = p.shape {
                    // Brillo curvo arriba a la izquierda (donde da la luz) y un reflejo tenue abajo a la derecha
                    arcs.push((arc_geo(&fac, cx, cy, r - p.bevel * 0.30, PI * 1.08, PI * 1.40)?, (r * 0.03).clamp(1.4, 2.6), 0.55));
                    arcs.push((arc_geo(&fac, cx, cy, r - p.bevel * 0.28, PI * 0.12, PI * 0.36)?, 1.1, 0.16));
                }
            }
            let clip = rect(x0 - 2.0, y0 - 2.0, x1 + 2.0, y1 + 2.0);
            Some(LensRes { key: (unk, pw, ph), back, out, brush, geos, disp: disp.try_into().ok()?, chan: chan.try_into().ok()?, clip, arcs })
        }
    }

    /// Rehace lo refractado si ha cambiado `key` (lo que el objeto tiene detrás) o el fondo de pantalla:
    /// en la imagen aparte va el fondo bajo el widget y encima `behind`. false si no hay cristal posible.
    fn update(&mut self, ui: &Ui, w: f32, h: f32, key: u64, behind: impl FnOnce(&Ui)) -> bool {
        use windows::core::Interface;
        if !self.prepare(ui, w, h) { return false; }
        let wall = fondo::under(ui);
        let full = key.rotate_left(7) ^ wall.as_ref().map(|b| b.1).unwrap_or(0x5eed) ^ (ui.ink * 1000.0) as u64;
        if self.last == Some(full) { return true; }
        let Some(r) = &self.res else { return false };
        let brt: &ID2D1RenderTarget = &r.back;
        unsafe {
            brt.BeginDraw();
            brt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
            brt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 });
            // Solo hace falta lo que cae bajo las piezas (y lo que el bisel trae de alrededor)
            let m = self.pieces.iter().map(|p| p.max_disp()).fold(0.0, f32::max) * 1.2 + 2.0;
            let c = r.clip;
            brt.PushAxisAlignedClip(&rect(c.left - m, c.top - m, c.right + m, c.bottom + m), D2D1_ANTIALIAS_MODE_ALIASED);
            if let Some((b, _)) = &wall { brt.DrawBitmap(b, Some(&rect(0.0, 0.0, w, h)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None); }
        }
        {
            let p = Painter { rt: brt, dw: ui.p.dw, cache: &self.cache, h: ui.p.h, s: ui.s };
            let c = |b: &ID2D1Brush| p.brush(col_of(b));
            let (white, dim, faint, card, card_edge, off, on, hov, track) = (c(&ui.white), c(&ui.dim), c(&ui.faint), c(&ui.card), c(&ui.card_edge), c(&ui.off), c(&ui.on), c(&ui.hov), c(&ui.track));
            let shadow = ui.shadow.as_ref().map(|b| p.brush(col_of(b)));
            let sub = Ui { p, rt: brt, f: ui.f, w: ui.w, s: ui.s, hover: None, shadow, glow: HashMap::new(), hits: Vec::new(),
                white, dim, faint, card, card_edge, off, on, hov, track, ink: ui.ink };
            behind(&sub);
        }
        unsafe { brt.PopAxisAlignedClip(); }
        if unsafe { brt.EndDraw(None, None) }.is_err() { self.res = None; return false; }
        let Ok(bmp) = (unsafe { r.back.GetBitmap() }) else { return false };
        let Ok(odc) = r.out.cast::<ID2D1DeviceContext>() else { return false };
        let ort: &ID2D1RenderTarget = &r.out;
        unsafe {
            ort.BeginDraw();
            ort.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
            ort.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 });
            ort.PushAxisAlignedClip(&r.clip, D2D1_ANTIALIAS_MODE_ALIASED);
            for e in &r.disp { e.SetInput(0, &bmp, true); }
            // Verde normal y encima, sumados, el rojo y el azul: en lo plano se recompone la imagen tal
            // cual; en el bisel los canales se separan un pelo (la franja de color del canto)
            for (i, m) in r.chan.iter().enumerate() {
                if let Ok(img) = m.GetOutput() {
                    odc.DrawImage(&img, None, None, D2D1_INTERPOLATION_MODE_LINEAR, if i == 0 { D2D1_COMPOSITE_MODE_SOURCE_OVER } else { D2D1_COMPOSITE_MODE_PLUS });
                }
            }
            ort.PopAxisAlignedClip();
        }
        if unsafe { ort.EndDraw(None, None) }.is_err() { self.res = None; return false; }
        self.last = Some(full);
        true
    }

    /// Pincel con lo de detrás ya doblado (en coordenadas del widget).
    fn brush(&self) -> Option<ID2D1Brush> { self.res.as_ref().map(|r| r.brush.clone().into()) }

    /// Rellena la pieza `i` con su cristal.
    fn fill(&self, ui: &Ui, i: usize) {
        if let (Some(r), Some(b)) = (&self.res, self.brush()) {
            if let Some(g) = r.geos.get(i) { unsafe { ui.rt.FillGeometry(g, &b, None); } }
        }
    }

    /// Sombra muy suave debajo de una pieza (antes de todo lo demás).
    fn shade(&self, ui: &Ui, base: &Matrix3x2, i: usize, a: f32) {
        let Some(p) = self.pieces.get(i) else { return };
        let a = a * (1.0 - 0.6 * ui.ink);
        match p.shape {
            Shape::Circle(cx, cy, r) => glow(ui, base, cx, cy + r * 0.10, r * 1.10, rgba(0.0, 0.0, 0.0, 1.0), 0.22 * a),
            Shape::RRect(x0, y0, x1, y1, rad) => {
                for (g, dy, al) in [(0.5f32, 2.0f32, 0.12f32), (3.0, 5.0, 0.06)] {
                    unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(x0 - g, y0 + dy - g, x1 + g, y1 + dy + g), rad + g), &ui.brush(rgba(0.0, 0.0, 0.0, q(al * a, 100.0)))); }
                }
            }
        }
    }

    /// Canto de la pieza `i`: un filo de luz más vivo arriba y un brillo especular (curvo en las
    /// redondas; una raya a lo largo en las alargadas). `a` = intensidad.
    fn rim(&self, ui: &Ui, i: usize, a: f32) {
        let Some(p) = self.pieces.get(i).copied() else { return };
        let st = round_stroke(ui.rt);
        let k = ui.ink;
        let (x0, y0, x1, y1) = p.bounds();
        let edge = lin(ui, 0.0, y0, 0.0, y1, &[(0.0, wh(q(lerp(0.50, 0.85, k) * a, 50.0))), (0.5, wh(q(lerp(0.08, 0.25, k) * a, 50.0))), (1.0, wh(q(lerp(0.22, 0.5, k) * a, 50.0)))]);
        unsafe {
            match p.shape {
                Shape::Circle(cx, cy, r) => {
                    ui.rt.DrawEllipse(&ellipse(cx, cy, r - 0.5), &edge, 1.0, None);
                    if let Some(res) = &self.res {
                        for (g, sw, al) in &res.arcs {
                            // Solo los arcos de esta pieza (están en el mismo orden, dos por pieza redonda)
                            if g.GetBounds(None).ok().filter(|bx| bx.left >= cx - r - 1.0 && bx.right <= cx + r + 1.0 && bx.top >= cy - r - 1.0 && bx.bottom <= cy + r + 1.0).is_some() {
                                ui.rt.DrawGeometry(g, &ui.brush(wh(q(al * a, 50.0))), *sw, st.as_ref());
                            }
                        }
                    }
                }
                Shape::RRect(_, _, _, _, rad) => {
                    // Junto al canto, el vidrio grueso se oscurece un poco (reflexión interna) y el filo brilla
                    let ins_d = (p.bevel * 0.22).clamp(1.5, 3.0);
                    ui.rt.DrawRoundedRectangle(&rrect(rect(x0 + ins_d, y0 + ins_d, x1 - ins_d, y1 - ins_d), (rad - ins_d).max(0.0)), &ui.brush(rgba(0.0, 0.0, 0.0, q(0.16 * a * (1.0 - 0.5 * k), 50.0))), 1.2, None);
                    ui.rt.DrawRoundedRectangle(&rrect(rect(x0 + 0.5, y0 + 0.5, x1 - 0.5, y1 - 0.5), (rad - 0.5).max(0.0)), &edge, 1.0, None);
                    let (w, h) = (x1 - x0, y1 - y0);
                    let ins = (p.bevel * 0.38).clamp(1.5, 5.0);
                    if w >= h {
                        // Alargada: brillo largo junto al canto de arriba, que se apaga hacia los extremos
                        let (a0, a1) = (x0 + rad.max(ins), x1 - rad.max(ins));
                        if a1 > a0 {
                            let fade = lin(ui, a0, 0.0, a1, 0.0, &[(0.0, wh(0.0)), (0.15, wh(q(0.50 * a, 50.0))), (0.7, wh(q(0.30 * a, 50.0))), (1.0, wh(0.0))]);
                            ui.rt.DrawLine(v2(a0, y0 + ins), v2(a1, y0 + ins), &fade, 1.3, st.as_ref());
                        }
                    } else {
                        // Columna: brillo a lo alto junto al canto izquierdo
                        let (b0, b1) = (y0 + rad.max(ins), y1 - rad.max(ins));
                        if b1 > b0 {
                            let fade = lin(ui, 0.0, b0, 0.0, b1, &[(0.0, wh(q(0.55 * a, 50.0))), (0.6, wh(q(0.25 * a, 50.0))), (1.0, wh(0.0))]);
                            ui.rt.DrawLine(v2(x0 + ins, b0), v2(x0 + ins, b1), &fade, 1.2, st.as_ref());
                        }
                    }
                }
            }
        }
    }
}

// ================================================================ Sistema: «Ficha»
/// Datos fijos del equipo (se leen una vez).
struct SysInfo { user: String, pc: String, os: String, cpu: String, model: String }

/// Valor de texto del registro de Windows.
fn reg_sz(root: windows::Win32::System::Registry::HKEY, key: &str, val: &str) -> Option<String> {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut buf = vec![0u16; 256];
        let mut size = (buf.len() * 2) as u32;
        RegGetValueW(root, &HSTRING::from(key), &HSTRING::from(val), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut size)).ok().ok()?;
        let s = String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

/// «AMD Ryzen 7 5800H with Radeon Graphics» → «AMD Ryzen 7 5800H»; «Intel(R) Core(TM) i7-9750H CPU @ 2.60GHz» → «Intel Core i7-9750H».
fn short_cpu(s: &str) -> String {
    let mut s = s.replace("(R)", "").replace("(TM)", "").replace("(tm)", "");
    if let Some(i) = s.find(" @ ") { s.truncate(i); }
    for sep in [" with ", " w/ "] { if let Some(i) = s.find(sep) { s.truncate(i); } }
    let mut words: Vec<&str> = s.split_whitespace().collect();
    // «8-Core Processor», «Processor», «CPU» al final sobran
    if words.len() > 2 && words[words.len() - 1].eq_ignore_ascii_case("Processor") && words[words.len() - 2].to_ascii_lowercase().ends_with("-core") { words.truncate(words.len() - 2); }
    while words.len() > 1 && (words[words.len() - 1].eq_ignore_ascii_case("Processor") || words[words.len() - 1].eq_ignore_ascii_case("CPU")) { words.pop(); }
    words.join(" ")
}

/// «OMEN-PELAYO» → «Omen-Pelayo».
fn title_case(s: &str) -> String {
    let mut out = String::new();
    let mut up = true;
    for ch in s.chars() {
        if up { out.extend(ch.to_uppercase()); } else { out.extend(ch.to_lowercase()); }
        up = !ch.is_alphanumeric();
    }
    out
}

fn sysinfo() -> &'static SysInfo {
    static I: std::sync::OnceLock<SysInfo> = std::sync::OnceLock::new();
    I.get_or_init(|| {
        use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE as LM;
        let nt = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
        let build: u32 = reg_sz(LM, nt, "CurrentBuildNumber").and_then(|b| b.parse().ok()).unwrap_or(0);
        let name = if build >= 22000 { "Windows 11" } else { "Windows 10" };
        let dv = reg_sz(LM, nt, "DisplayVersion").unwrap_or_default();
        let os = format!("{name} {dv}{}", if build > 0 { format!(" ({build})") } else { String::new() }).split_whitespace().collect::<Vec<_>>().join(" ");
        let cpu = reg_sz(LM, "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0", "ProcessorNameString").map(|c| short_cpu(&c)).unwrap_or_default();
        let (user, pc, model) = if sys::demo() { ("alex".to_string(), "desktop".to_string(), String::new()) } else {
            let bios = "HARDWARE\\DESCRIPTION\\System\\BIOS";
            let junk = |s: &String| { let l = s.to_lowercase(); l.contains("o.e.m") || l.contains("system product") || l.contains("default string") || l.contains("system manufacturer") };
            let mk = reg_sz(LM, bios, "SystemManufacturer").filter(|s| !junk(s)).unwrap_or_default();
            let pr = reg_sz(LM, bios, "SystemProductName").filter(|s| !junk(s)).unwrap_or_default();
            let model = if pr.to_lowercase().starts_with(&mk.to_lowercase()) || mk.is_empty() { pr } else { format!("{mk} {pr}") };
            (std::env::var("USERNAME").unwrap_or_default().to_lowercase(), std::env::var("COMPUTERNAME").unwrap_or_default().to_lowercase(), model.trim().to_string())
        };
        SysInfo { user, pc, os, cpu, model }
    })
}

/// «3 d 4 h 12 min», «4 h 12 min» o «12 min».
fn uptime_words(s: u64) -> String {
    let (d, h, m) = (s / 86400, s / 3600 % 24, s / 60 % 60);
    if d > 0 { format!("{d} d {h} h {m} min") } else if h > 0 { format!("{h} h {m} min") } else { format!("{m} min") }
}

/// Tiempo encendido en grande: «3:25» (horas:minutos) o, pasado un día, «2d 3h».
fn uptime_big(s: u64) -> String {
    let (d, h, m) = (s / 86400, s / 3600 % 24, s / 60 % 60);
    if d > 0 { format!("{d}d {h}h") } else { format!("{h}:{m:02}") }
}

/// Ficha del equipo: a la izquierda, el logotipo de Windows hecho de cuatro baldosas de cristal que
/// doblan el fondo de pantalla y, debajo, el tiempo encendido grande y fino; a la derecha, claves en gris
/// y valores en blanco; abajo, procesador y memoria en barras finas.
#[derive(Default)]
pub struct TermW { cpu: f32, ram: f32, lens: Lens }

impl Widget for TermW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (w, h) = size(Kind::System);
        let info = sysinfo();
        let sd = deskdata::SYS.lock().unwrap().clone();
        let cpu_now = sd.as_ref().and_then(|s| s.cpu.last().copied()).unwrap_or(0.0);
        let ram_now = sd.as_ref().and_then(|s| s.ram.last().copied()).unwrap_or(0.0);
        let kin = ez(t, 0.1, 0.9);
        approach(&mut self.cpu, cpu_now * kin, dt.min(0.2), 0.3);
        approach(&mut self.ram, ram_now * kin, dt.min(0.2), 0.3);
        let rt = ui.rt;
        let base = cur_xf(ui);
        let gf = gfonts(ui);
        let pc = if info.pc.is_empty() { "equipo" } else { &info.pc };
        let user = if info.user.is_empty() { "usuario" } else { &info.user };
        deskw::header(ui, d, &title_case(pc), &format!("{user}@{pc}"), w);

        // ---- El logotipo de Windows en cuatro baldosas de cristal macizo que doblan el fondo de pantalla
        let kl = ez(t, 0.05, 0.6);
        let (lx, ly, ts, gap) = (22.0, 60.0, 38.0, 5.0);
        let tiles: Vec<Piece> = (0..4).map(|i| {
            let (x, y) = (lx + (i % 2) as f32 * (ts + gap), ly + (i / 2) as f32 * (ts + gap));
            Piece::slab(x, y, x + ts, y + ts, 6.0, 15.0, 1.5)
        }).collect();
        self.lens.set(&tiles);
        popup::push_alpha(rt, kl);
        for i in 0..4 { self.lens.shade(ui, &base, i, 1.0); }
        if self.lens.update(ui, w, h, 0, |_| {}) { for i in 0..4 { self.lens.fill(ui, i); } }
        for (i, p) in tiles.iter().enumerate() {
            let (x0, y0, x1, y1) = p.bounds();
            // Cuerpo del vidrio: un velo que se aclara arriba (donde da la luz)
            let body = lin(ui, 0.0, y0, 0.0, y1, &[(0.0, wh(q(lerp(0.07, 0.18, ui.ink), 50.0))), (0.6, wh(0.0)), (1.0, wh(q(lerp(0.03, 0.08, ui.ink), 50.0)))]);
            unsafe { rt.FillRoundedRectangle(&rrect(rect(x0, y0, x1, y1), 6.0), &body); }
            self.lens.rim(ui, i, 1.0);
        }
        unsafe { rt.PopLayer(); }

        // ---- Tiempo encendido, debajo y limpio
        let up = if sys::demo() { 3 * 3600 + 25 * 60 + 9 } else { (unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }) / 1000 };
        let ku = ez(t, 0.15, 0.6);
        popup::push_alpha(rt, ku);
        ui.text(&gf.clock, &uptime_big(up), 17.0, 145.0 + 6.0 * (1.0 - ku), 66.0, &ui.white);
        unsafe { rt.PopLayer(); }
        // Un punto de acento diminuto: el equipo sigue encendido
        dot(rt, 23.0, 222.0, 2.2, &ui.brush(acc(1.0)));
        ui.text(&ui.f.small, &format!("{} · {}", i18n::t("encendido"), uptime_words(up)), 31.0, 213.0, 18.0, &ui.dim);

        // ---- Ficha: claves en gris y valores en blanco
        let x0 = 236.0;
        let st = deskdata::STORE.lock().unwrap().clone();
        let c_drive = st.as_ref().and_then(|s| s.drives.iter().find(|d| d.root.to_ascii_uppercase().starts_with("C:")).or(s.drives.first()).cloned());
        let media = MEDIA.lock().unwrap().clone();
        let mut rows: Vec<(&str, String)> = vec![
            (i18n::t("SO"), info.os.clone()),
            ("CPU", if info.cpu.is_empty() { "—".into() } else { info.cpu.clone() }),
            ("RAM", sd.as_ref().filter(|s| s.ram_total > 0).map(|s| format!("{} / {}", deskdata::fmt_bytes(s.ram_used), deskdata::fmt_bytes(s.ram_total))).unwrap_or_else(|| "—".into())),
        ];
        if let Some(dr) = &c_drive {
            let used = dr.total.saturating_sub(dr.free);
            let letter = dr.root.trim_end_matches('\\').to_string();
            let key: &str = if letter.eq_ignore_ascii_case("C:") { i18n::t("Disco C:") } else { i18n::t("Disco") };
            rows.push((key, format!("{} / {} ({:.0} %)", deskdata::fmt_bytes(used), deskdata::fmt_bytes(dr.total), used as f32 * 100.0 / dr.total.max(1) as f32)));
        }
        if let Some((p, ch)) = crate::battery() { rows.push((i18n::t("Batería"), if ch { i18n::tf("{} % · cargando", &[&p]) } else { format!("{p} %") })); }
        if !info.model.is_empty() { rows.push((i18n::t("Modelo"), info.model.clone())); }
        rows.push((i18n::t("Sonando"), media.as_ref().map(|m| if m.artist.is_empty() { m.title.clone() } else { format!("{} — {}", m.title, m.artist) }).unwrap_or_else(|| "—".into())));
        let kx = x0 + 80.0;
        let maxw = w - 20.0 - kx;
        let mut y = 58.0;
        for (i, (k, v)) in rows.iter().take(7).enumerate() {
            let ki = ez(t, 0.12 + 0.04 * i as f32, 0.35);
            popup::push_alpha(rt, ki);
            let dx = 6.0 * (1.0 - ki);
            ui.text(&ui.f.sub, k, x0 + dx, y, 20.0, &ui.dim);
            ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, v, maxw), kx + dx, y, 20.0, &ui.white);
            unsafe { rt.PopLayer(); }
            y += 23.0;
        }

        // ---- Abajo: procesador y memoria en barras finas y la hora
        ui.sep(h - 58.0);
        let fy = h - 42.0;
        let mut x = 20.0;
        for (lab, v) in [("CPU", self.cpu), ("RAM", self.ram)] {
            ui.text(&ui.f.small, lab, x, fy, 22.0, &ui.dim);
            let bx = x + ui.p.width(&ui.f.small, "RAM") + 10.0;
            bar(ui, bx, fy + 9.0, 120.0, 4.0, v / 100.0);
            ui.text(&ui.f.small, &format!("{:.0} %", v), bx + 128.0, fy, 22.0, &ui.white);
            x = bx + 128.0 + ui.p.width(&ui.f.small, "100 %") + 26.0;
        }
        let now = unsafe { GetLocalTime() };
        ui.text_r(&gf.mid, &format!("{:02}:{:02}", now.wHour, now.wMinute), w - 20.0, fy - 1.0, 24.0, &ui.white);
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn animating(&self, t: f32) -> bool { t < 1.4 }
}

// ================================================================ Visualizador: «Cristal»
const DCOLS: usize = 20;

/// Espectro en columnas de cristal macizo que doblan el fondo de pantalla, con su altura al compás del
/// audio y el pico como una raya finísima (el más alto, en el acento).
pub struct SpecDotsW { lv: [f32; DCOLS], peak: [f32; DCOLS], hold: [f32; DCOLS], alive: f32, moving: bool, lens: Lens }

impl Default for SpecDotsW { fn default() -> Self { SpecDotsW { lv: [0.0; DCOLS], peak: [0.0; DCOLS], hold: [0.0; DCOLS], alive: 0.0, moving: false, lens: Lens::default() } } }

/// Altura de una columna para un nivel: algo comprimida (con música, el espectro casi nunca baja de la mitad
/// y las columnas irían siempre casi llenas).
fn shape(v: f32) -> f32 { v.clamp(0.0, 1.0).powf(1.5) }

/// Avanza el pico de una columna: sube con el nivel, se queda un momento y luego cae acelerando.
fn peak_step(peak: &mut f32, hold: &mut f32, lv: f32, dt: f32) {
    if lv >= *peak { *peak = lv; *hold = 0.0; return; }
    *hold += dt;
    if *hold > 0.35 { *peak = (*peak - dt * (0.15 + 0.9 * (*hold - 0.35))).max(lv).max(0.0); }
}

impl Widget for SpecDotsW {
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, t: f32, dt: f32) {
        let (w, h) = size(Kind::Visual);
        let dt = dt.min(0.1);
        let src = levels_now();
        let k = 1.0 - (-dt / 0.05).exp();
        let mut energy = 0.0;
        self.moving = false;
        for c in 0..DCOLS {
            let v = col_level(&src, c, DCOLS);
            self.lv[c] += (v - self.lv[c]) * k;
            energy += self.lv[c];
            let before = self.peak[c];
            peak_step(&mut self.peak[c], &mut self.hold[c], self.lv[c], dt);
            if self.peak[c] > 0.004 && (self.peak[c] - before).abs() > 1e-5 { self.moving = true; }
        }
        let on = energy / DCOLS as f32 > 0.02;
        self.alive += ((if on { 1.0 } else { 0.0 }) - self.alive) * (1.0 - (-dt / 0.3).exp());
        let rt = ui.rt;
        let base = cur_xf(ui);
        // Línea de estado
        let live = self.alive > 0.5;
        dot(rt, 21.0, 19.0, 2.6, &ui.brush(if live { acc(1.0) } else { ui.ic(0.3) }));
        ui.text(&ui.f.small, if live { i18n::t("en vivo") } else { i18n::t("en pausa") }, 30.0, 10.0, 18.0, &ui.dim);
        let title = MEDIA.lock().unwrap().as_ref().map(|m| m.title.clone());
        if let Some(tt) = title { let s = fit(ui, &ui.f.small, &tt, w * 0.55); ui.text_r(&ui.f.small, &s, w - 18.0, 10.0, 18.0, &ui.faint); }
        // Columnas de cristal macizo: el mapa es el de las columnas enteras (se hace una vez) y en cada
        // fotograma solo se rellena con el cristal la altura que toca
        let (x0, x1, top, floor) = (20.0, w - 20.0, 40.0, h - 16.0);
        let pitch = (x1 - x0) / DCOLS as f32;
        let bw = (pitch * 0.64).round().max(4.0);
        let span = floor - top;
        let top_c = (0..DCOLS).fold(0, |a, c| if self.peak[c] > self.peak[a] { c } else { a });
        let xs: Vec<f32> = (0..DCOLS).map(|c| (x0 + c as f32 * pitch + (pitch - bw) / 2.0).round()).collect();
        let cols: Vec<Piece> = xs.iter().map(|&x| Piece::slab(x, top, x + bw, floor, bw / 2.0, bw / 2.0, 1.0)).collect();
        self.lens.set(&cols);
        let glass = self.lens.update(ui, w, h, 0, |_| {});
        let st = round_stroke(rt);
        let body = lin(ui, 0.0, top, 0.0, floor, &[(0.0, wh(q(lerp(0.16, 0.30, ui.ink), 50.0))), (1.0, wh(q(lerp(0.04, 0.10, ui.ink), 50.0)))]);
        let edge = ui.wa(q(lerp(0.38, 0.6, ui.ink), 50.0));
        let shine = wh(0.5);
        let (b_peak, a_peak) = (ui.wa(0.6), ui.brush(acc(1.0)));
        let gb = self.lens.brush();
        for (c, &x) in xs.iter().enumerate() {
            // Al aparecer, las columnas suben de izquierda a derecha
            let kin = ez(t, 0.012 * c as f32, 0.5);
            let hh = (shape(self.lv[c]) * span * kin).max(bw);
            let r = rrect(rect(x, floor - hh, x + bw, floor), bw / 2.0);
            unsafe {
                if glass { if let Some(b) = &gb { rt.FillRoundedRectangle(&r, b); } }
                rt.FillRoundedRectangle(&r, &body);
                rt.DrawRoundedRectangle(&rrect(rect(x + 0.5, floor - hh + 0.5, x + bw - 0.5, floor - 0.5), bw / 2.0 - 0.5), &edge, 1.0, None);
                // Brillo a lo alto junto al canto izquierdo
                if hh > bw + 4.0 { rt.DrawLine(v2(x + 2.2, floor - hh + bw / 2.0), v2(x + 2.2, floor - bw / 2.0), &ui.brush(shine), 1.0, st.as_ref()); }
            }
            let ph = shape(self.peak[c]) * span * kin;
            if ph > hh + 3.0 {
                let y = floor - ph;
                let b = if c == top_c && self.peak[c] > 0.06 { &a_peak } else { &b_peak };
                unsafe { rt.FillRectangle(&rect(x, y - 0.5, x + bw, y + 0.5), b); }
            }
        }
        let _ = base;
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn tick(&mut self) -> bool {
        if deskdata::VISIBLE.load(Ordering::Relaxed) { audioviz::want(); }
        true
    }
    fn animating(&self, t: f32) -> bool { t < 1.2 || self.alive > 0.01 || self.moving }
    fn smooth(&self) -> bool { self.alive > 0.01 }
    fn frame_ms(&self) -> u32 { if self.alive > 0.01 { 16 } else { 50 } }
}

// ================================================================ Visualizador: «Vida»
const LC: usize = 24;
const LR: usize = 11;
/// Lo que tarda una célula en aparecer (crece con rebote) y en irse (se encoge y se desvanece).
const BORN_S: f32 = 0.28;
const DIE_S: f32 = 0.4;
/// Lo que dura el brillo extra de las células que siembra un golpe.
const TINT_S: f32 = 1.6;

/// Una generación del juego de la vida en una rejilla toroidal (`c` columnas × `r` filas). Devuelve la población.
fn life_step(cur: &[bool], next: &mut [bool], c: usize, r: usize) -> usize {
    let mut pop = 0;
    for y in 0..r {
        for x in 0..c {
            let mut n = 0;
            for dy in [r - 1, 0, 1] {
                for dx in [c - 1, 0, 1] {
                    if dx == 0 && dy == 0 { continue; }
                    if cur[((y + dy) % r) * c + (x + dx) % c] { n += 1; }
                }
            }
            let alive = cur[y * c + x];
            let v = n == 3 || (alive && n == 2);
            next[y * c + x] = v;
            pop += v as usize;
        }
    }
    pop
}

/// Número fijo 0..1 para la casilla `i` (variante `k`): desplaza y da tamaño a cada burbuja para que
/// no se vea la rejilla.
fn cell_rand(i: usize, k: u32) -> f32 {
    let mut x = (i as u32).wrapping_mul(0x9E37_79B9) ^ k.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

/// Huella de una rejilla (para ver si se ha quedado quieta o oscilando).
fn grid_hash(g: &[bool]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, v) in g.iter().enumerate() { if *v { h ^= i as u64 + 1; h = h.wrapping_mul(0x0100_0000_01b3); } }
    h
}

/// Juego de la vida con células como burbujas de cristal (aro de luz, brillo y halo) que aparecen
/// creciendo y se van desvaneciéndose; cada golpe de graves siembra un grupo que nace más brillante.
/// Si se extingue o se estanca, se vuelve a sembrar. Sin música avanza despacio solo.
pub struct LifeW {
    cells: Vec<bool>, next: Vec<bool>, born: Vec<f32>, died: Vec<f32>, seeded: Vec<bool>,
    gens: u64, pop: usize, acc: f32, rng: u64, hist: [u64; 2], stale: u32, bass_avg: f32, last_beat: f32, now: f32, alive: f32,
    /// Burbujas a dibujar en este fotograma (x, y, escala, opacidad, brillo extra), reutilizado
    buf: Vec<(f32, f32, f32, f32, f32)>,
    /// Pincel con el fondo de pantalla bajo el widget (huella del recorte, lienzo, pincel)
    wall: Option<(u64, windows::core::IUnknown, ID2D1BitmapBrush)>,
}

impl Default for LifeW {
    fn default() -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
        let n = LC * LR;
        let mut l = LifeW { cells: vec![false; n], next: vec![false; n], born: vec![-10.0; n], died: vec![-10.0; n], seeded: vec![false; n],
            gens: 0, pop: 0, acc: 0.0, rng: seed, hist: [0; 2], stale: 0, bass_avg: 0.0, last_beat: -10.0, now: 0.0, alive: 0.0, buf: Vec::with_capacity(n), wall: None };
        l.reseed();
        l
    }
}

impl LifeW {
    /// Pincel con el fondo de pantalla que hay detrás (rehecho solo si cambia el recorte).
    fn wall_brush(&mut self, ui: &Ui) -> Option<ID2D1BitmapBrush> {
        use windows::core::Interface;
        let (bmp, key) = fondo::under(ui)?;
        let unk = ui.rt.cast::<windows::core::IUnknown>().ok()?;
        if let Some((k, u, b)) = &self.wall { if *k == key && *u == unk { return Some(b.clone()); } }
        let b = unsafe { ui.rt.CreateBitmapBrush(&bmp, None, None).ok()? };
        self.wall = Some((key, unk, b.clone()));
        Some(b)
    }
    fn rand(&mut self) -> u64 { let mut x = self.rng; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.rng = x; x }
    /// Siembra al azar toda la rejilla (un 28 %).
    fn reseed(&mut self) {
        for i in 0..LC * LR {
            let v = self.rand() % 100 < 28;
            if v && !self.cells[i] { self.born[i] = self.now; self.seeded[i] = false; }
            if !v && self.cells[i] { self.died[i] = self.now; }
            self.cells[i] = v;
        }
        self.gens = 0;
        self.stale = 0;
        self.pop = self.cells.iter().filter(|c| **c).count();
    }
    /// Un golpe: un grupo de células nuevas en un sitio al azar (nacen más brillantes).
    fn burst(&mut self) {
        let (cx, cy) = ((self.rand() % LC as u64) as usize, (self.rand() % LR as u64) as usize);
        // Pentominó R (crece mucho) o una mancha al azar de 4×4
        const R_PENTO: [(usize, usize); 5] = [(1, 0), (2, 0), (0, 1), (1, 1), (1, 2)];
        let cells: Vec<(usize, usize)> = if self.rand() % 3 == 0 { R_PENTO.to_vec() } else { (0..16).filter(|_| self.rand() % 2 == 0).map(|i| (i % 4, i / 4)).collect() };
        for (dx, dy) in cells {
            let i = ((cy + dy) % LR) * LC + (cx + dx) % LC;
            if !self.cells[i] { self.cells[i] = true; self.born[i] = self.now; self.seeded[i] = true; }
        }
    }
    fn step(&mut self) {
        self.pop = life_step(&self.cells, &mut self.next, LC, LR);
        for i in 0..LC * LR {
            // Lo que nace por las reglas no lleva el brillo extra (es solo de lo que siembra un golpe)
            if self.next[i] && !self.cells[i] { self.born[i] = self.now; self.seeded[i] = false; }
            if !self.next[i] && self.cells[i] { self.died[i] = self.now; }
        }
        std::mem::swap(&mut self.cells, &mut self.next);
        self.gens += 1;
        let hsh = grid_hash(&self.cells);
        // Quieta (periodo 1) u oscilando (periodo 2): al rato se vuelve a sembrar
        if hsh == self.hist[0] || hsh == self.hist[1] { self.stale += 1; } else { self.stale = 0; }
        self.hist = [hsh, self.hist[0]];
        if self.pop == 0 || self.stale > 24 { self.reseed(); }
    }
}

impl Widget for LifeW {
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, t: f32, dt: f32) {
        let (w, _h) = size(Kind::Visual);
        let dt = dt.min(0.25);
        self.now += dt;
        let src = levels_now();
        let energy = band_mean(&src, 0, audioviz::BANDS);
        self.alive += ((if energy > 0.02 { 1.0 } else { 0.0 }) - self.alive) * (1.0 - (-dt / 0.3).exp());
        // Golpe de graves: la energía baja supera claramente su media reciente
        let bass = band_mean(&src, 0, 6);
        let beat = self.alive > 0.5 && bass > 0.3 && bass > self.bass_avg * 1.15 + 0.03 && self.now - self.last_beat > 0.28;
        self.bass_avg += (bass - self.bass_avg) * (1.0 - (-dt / 0.6).exp());
        if beat { self.last_beat = self.now; self.burst(); }
        let red = reduced();
        let every = if red { 1.0 } else if self.alive > 0.5 { 0.11 } else { 0.5 };
        self.acc += dt;
        let mut n = 0;
        while self.acc >= every && n < 2 { self.acc -= every; self.step(); n += 1; }
        if self.acc > every { self.acc = 0.0; }

        let rt = ui.rt;
        let base = cur_xf(ui);
        // Línea de estado: generación y población; un punto de acento diminuto que destella con cada golpe
        let flash = (1.0f32 - (self.now - self.last_beat) / 0.5).clamp(0.0, 1.0);
        dot(rt, 21.0, 19.0, 2.6, &ui.brush(acc(q(0.35 + 0.65 * flash, 10.0))));
        ui.text(&ui.f.small, &i18n::tf("gen {} · pob {}", &[&self.gens, &self.pop]), 30.0, 10.0, 18.0, &ui.dim);
        let right = if self.alive > 0.5 { i18n::t("al ritmo") } else { i18n::t("en pausa") };
        ui.text_r(&ui.f.small, right, w - 18.0, 10.0, 18.0, &ui.faint);

        // Burbujas: primero se reúnen (posición, escala, opacidad y brillo) y luego se pintan por capas
        let (x0, y0) = (26.0, 48.0);
        let pitch = (w - 52.0) / (LC - 1) as f32;
        let r0 = pitch * 0.44;
        let kin = ez(t, 0.0, 0.6);
        self.buf.clear();
        for y in 0..LR {
            for x in 0..LC {
                let i = y * LC + x;
                // Cada burbuja algo desplazada de su casilla, de su propio tamaño y flotando despacio
                let bob = if red { 0.0 } else { (self.now * 0.9 + cell_rand(i, 3) * TAU).sin() * 1.3 };
                let (px, py) = (x0 + (x as f32 + (cell_rand(i, 1) - 0.5) * 0.5) * pitch, y0 + (y as f32 + (cell_rand(i, 2) - 0.5) * 0.5) * pitch + bob);
                let sz = 0.8 + 0.35 * cell_rand(i, 4);
                if self.cells[i] {
                    let age = (self.now - self.born[i]).max(0.0);
                    let s = if red { 1.0 } else { crate::ease_back((age / BORN_S).min(1.0)) };
                    let a = (age / (BORN_S * 0.6)).min(1.0);
                    let extra = if self.seeded[i] { (1.0 - age / TINT_S).clamp(0.0, 1.0) } else { 0.0 };
                    self.buf.push((px, py, s * sz * kin, a * kin, extra));
                } else {
                    let gone = self.now - self.died[i];
                    if gone < DIE_S && !red {
                        let k = 1.0 - gone / DIE_S;
                        self.buf.push((px, py - 3.0 * (1.0 - k), (0.55 + 0.45 * k) * sz * kin, k * k * 0.8 * kin, 0.0));
                    }
                }
            }
        }
        let c = ui.ic(1.0);
        // Capa 1: halos (se suman en los grupos y dan un resplandor suave)
        for &(px, py, s, a, extra) in &self.buf {
            let (a, extra) = (q(a, 8.0), q(extra, 4.0));
            if a <= 0.0 || s <= 0.01 { continue; }
            place(ui, &base, px, py, r0 * s * 2.2, r0 * s * 2.2);
            let b = rad(ui, 0.0, 0.0, &[(0.0, with_a(c, 0.10 * a + 0.12 * extra * a)), (1.0, with_a(c, 0.0))]);
            unsafe { rt.FillEllipse(&ellipse(0.0, 0.0, 1.0), &b); }
        }
        // Capa 2: cada gota es una lente diminuta: dentro se ve el fondo de pantalla de alrededor, más
        // pequeño y del revés (como en una gota de agua de verdad)
        reset(ui, &base);
        if let Some(wb) = self.wall_brush(ui) {
            const K: f32 = 2.4;
            for &(px, py, s, a, _) in &self.buf {
                let a = q(a, 8.0);
                if a <= 0.0 || s <= 0.01 { continue; }
                unsafe {
                    wb.SetTransform(&Matrix3x2 { M11: -1.0 / K, M12: 0.0, M21: 0.0, M22: -1.0 / K, M31: px * (1.0 + 1.0 / K), M32: py * (1.0 + 1.0 / K) });
                    wb.SetOpacity(a);
                    rt.FillEllipse(&ellipse(px, py, r0 * s), &wb);
                }
            }
        }
        // …con un aro de luz junto al borde y el centro casi transparente
        for &(px, py, s, a, extra) in &self.buf {
            let (a, extra) = (q(a, 8.0), q(extra, 4.0));
            if a <= 0.0 || s <= 0.01 { continue; }
            let a2 = (a * (1.0 + 0.4 * extra)).min(1.0);
            place(ui, &base, px, py, r0 * s, r0 * s);
            let b = rad(ui, -0.3, -0.35, &[(0.0, with_a(c, 0.12 * a2)), (0.6, with_a(c, 0.06 * a2)), (0.9, with_a(c, 0.50 * a2)), (1.0, with_a(c, 0.10 * a2))]);
            unsafe { rt.FillEllipse(&ellipse(0.0, 0.0, 1.0), &b); }
        }
        reset(ui, &base);
        // Capa 3: el brillo especular
        for &(px, py, s, a, _) in &self.buf {
            let a = q(a, 8.0);
            if a <= 0.0 || s <= 0.01 { continue; }
            let r = r0 * s;
            unsafe { rt.FillEllipse(&ell(px - 0.32 * r, py - 0.42 * r, 0.30 * r, 0.17 * r), &ui.brush(wh(q(0.7 * a, 10.0)))); }
            dot(rt, px + 0.38 * r, py + 0.40 * r, 0.09 * r, &ui.brush(wh(q(0.35 * a, 10.0))));
        }
    }
    fn click(&mut self, _id: Option<u32>, right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        // Clic: siembra un grupo nuevo
        if right { return None; }
        self.burst();
        Some(Box::new(|| {}))
    }
    fn tick(&mut self) -> bool {
        if deskdata::VISIBLE.load(Ordering::Relaxed) { audioviz::want(); }
        true
    }
    fn animating(&self, _t: f32) -> bool { true }
    fn frame_ms(&self) -> u32 { if self.alive > 0.01 { 33 } else if reduced() { 250 } else { 50 } }
}

// ================================================================ Visualizador: «Órbita»
const ORB_N: usize = 70;
const RING_N: usize = 84;
/// Ángulo áureo (radianes): reparte los puntos de la esfera sin que se amontonen.
const GOLDEN: f32 = 2.399_963;
/// Anillo de partículas: semiejes respecto al radio del orbe.
const RING_RX: f32 = 1.55;
const RING_RY: f32 = 0.30;

/// Puntos repartidos por igual sobre una esfera de radio 1 (espiral de Fibonacci).
fn fib_sphere(n: usize) -> Vec<[f32; 3]> {
    (0..n).map(|i| {
        let y = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let (s, c) = (i as f32 * GOLDEN).sin_cos();
        [c * r, y, s * r]
    }).collect()
}

/// Gira un punto: primero alrededor del eje vertical (`yaw`) y luego del horizontal (`pitch`).
fn rotate(p: [f32; 3], yaw: f32, pitch: f32) -> [f32; 3] {
    let (sy, cy) = yaw.sin_cos();
    let (x, z) = (p[0] * cy + p[2] * sy, -p[0] * sy + p[2] * cy);
    let (sp, cp) = pitch.sin_cos();
    [x, p[1] * cp - z * sp, p[1] * sp + z * cp]
}

/// Orbe de cristal macizo: dentro, una nube de partículas que gira y un núcleo blanco que late con el
/// audio; alrededor, un anillo de partículas. Todo lo que queda detrás del orbe se ve a través de él,
/// doblado como por una lente, con una luna diminuta en el acento.
pub struct OrbW { pts: Vec<[f32; 3]>, yaw: f32, energy: f32, alive: f32, lens: Lens }

impl Default for OrbW { fn default() -> Self { OrbW { pts: fib_sphere(ORB_N), yaw: 3.77, energy: 0.0, alive: 0.0, lens: Lens::default() } } }

/// Partículas del anillo de un lado (`front`: las de delante del orbe), con desenfoque fingido.
#[allow(clippy::too_many_arguments)]
fn orbit_ring(u: &Ui, base: &Matrix3x2, cx: f32, cy: f32, r: f32, yaw: f32, ki: f32, front: bool) {
    for i in 0..RING_N {
        let a = i as f32 / RING_N as f32 * TAU + yaw * 0.6;
        let (s, c) = a.sin_cos();
        if (s >= 0.0) != front { continue; }
        let (x, y) = (cx + c * RING_RX * r * ki, cy + s * RING_RY * r * ki);
        let d = (s + 1.0) / 2.0;
        let hr = 2.0 + 2.5 * (1.0 - d);
        place(u, base, x, y, hr, hr);
        let b = rad(u, 0.0, 0.0, &[(0.0, u.ic(q((0.08 + 0.18 * d) * ki, 10.0))), (1.0, u.ic(0.0))]);
        unsafe { u.rt.FillEllipse(&ellipse(0.0, 0.0, 1.0), &b); }
        reset(u, base);
        dot(u.rt, x, y, 0.5 + 0.8 * d, &u.wa(q((0.30 + 0.6 * d) * ki, 10.0)));
    }
}

impl Widget for OrbW {
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, t: f32, dt: f32) {
        let (w, h) = size(Kind::Visual);
        let dt = dt.min(0.1);
        let src = levels_now();
        let e = band_mean(&src, 0, audioviz::BANDS);
        self.alive += ((if e > 0.02 { 1.0 } else { 0.0 }) - self.alive) * (1.0 - (-dt / 0.3).exp());
        let target = (band_mean(&src, 0, 8) * 0.7 + e * 0.6).min(1.0);
        self.energy += (target - self.energy) * (1.0 - (-dt / (if target > self.energy { 0.05 } else { 0.25 })).exp());
        if !reduced() { self.yaw = (self.yaw + dt * 0.42) % TAU; }
        let rt = ui.rt;
        let base = cur_xf(ui);
        let live = self.alive > 0.5;
        dot(rt, 21.0, 19.0, 2.6, &ui.brush(if live { acc(1.0) } else { ui.ic(0.3) }));
        ui.text(&ui.f.small, if live { i18n::t("en vivo") } else { i18n::t("en pausa") }, 30.0, 10.0, 18.0, &ui.dim);
        let ki = ez(t, 0.0, 0.8);
        let en = q(self.energy, 16.0);
        let (cx, cy) = (w / 2.0, h / 2.0 + 10.0);
        let r = (w.min(h) * 0.27).round();
        let (yaw, pts) = (self.yaw, &self.pts);
        self.lens.set(&[Piece::disc(cx, cy, r, r, 0.55)]);
        // Lo de detrás: el fondo de pantalla (lo pone la lente), la mitad de atrás del anillo, la nube de
        // dentro y el núcleo que late
        let back = |u: &Ui| {
            // En la imagen aparte no hay transformación: cada lienzo usa la suya
            let b0 = cur_xf(u);
            orbit_ring(u, &b0, cx, cy, r, yaw, ki, false);
            glow(u, &b0, cx, cy, r * (0.16 + 0.14 * en) * ki, u.ic(1.0), (0.35 + 0.35 * en) * ki);
            for p in pts {
                let p = rotate(*p, yaw, -0.36);
                let d = (p[2] + 1.0) / 2.0;
                dot(u.rt, cx + p[0] * r * 0.55, cy - p[1] * r * 0.55, 0.5 + 0.9 * d, &u.wa(q((0.15 + 0.6 * d) * ki, 10.0)));
            }
            dot(u.rt, cx, cy, 1.8 + 1.4 * en, &u.wa(0.9 * ki));
        };
        // Solo se vuelve a refractar si algo de dentro se ha movido
        let key = ((yaw.to_bits() as u64) << 32) | (en.to_bits() ^ ki.to_bits().rotate_left(11)) as u64;
        self.lens.shade(ui, &base, 0, ki);
        if self.lens.update(ui, w, h, key, back) {
            popup::push_alpha(rt, ki);
            self.lens.fill(ui, 0);
            unsafe { rt.FillEllipse(&ellipse(cx, cy, r), &ui.brush(wh(0.04))); }
            self.lens.rim(ui, 0, 1.0);
            unsafe { rt.PopLayer(); }
        }
        // La mitad de delante del anillo, por encima del cristal
        orbit_ring(ui, &base, cx, cy, r, yaw, ki, true);
        // La luna: recorre el anillo; detrás del orbe se ve apagada
        let a = yaw * 1.3;
        let (mx, my) = (cx + a.cos() * RING_RX * r * ki, cy + a.sin() * RING_RY * r * ki);
        let behind = a.sin() < 0.0 && ((mx - cx).powi(2) + (my - cy).powi(2)).sqrt() < r;
        dot(rt, mx, my, 2.4, &ui.brush(acc(if behind { 0.35 } else { 1.0 } * ki)));
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn tick(&mut self) -> bool {
        if deskdata::VISIBLE.load(Ordering::Relaxed) { audioviz::want(); }
        true
    }
    fn animating(&self, t: f32) -> bool { t < 1.2 || !reduced() || self.alive > 0.01 }
    fn smooth(&self) -> bool { self.alive > 0.01 }
    fn frame_ms(&self) -> u32 { if self.alive > 0.01 { 16 } else { 33 } }
}

// ================================================================ Relojes: «Ojo»
/// Fracción del minuto (0..1) que marca el arco de los segundos.
fn sec_frac(s: u16) -> f32 { (s.min(59) as f32) / 60.0 }

/// Reloj redondo con su cristal: una cúpula de vidrio que cubre toda la esfera, plana en el centro (la
/// hora y el día se leen tal cual) y que solo dobla en el canto el fondo de pantalla, las marcas de los
/// minutos y el arco finísimo de los segundos (con la punta en el acento), que van debajo.
#[derive(Default)]
pub struct EyeW { sec: f32, moving: bool, lens: Lens }

impl Widget for EyeW {
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, t: f32, dt: f32) {
        let (w, h) = size(Kind::Clocks);
        let rt = ui.rt;
        let base = cur_xf(ui);
        let gf = gfonts(ui);
        let (cx, cy) = (w / 2.0, h / 2.0);
        let now = unsafe { GetLocalTime() };
        let kt = ez(t, 0.1, 0.5);
        let hm = format!("{:02}:{:02}", now.wHour, now.wMinute);
        let date = format!("{} {}", i18n::weekday(now.wDayOfWeek as usize), now.wDay);
        // Segundero (va bajo el cristal)
        let target = sec_frac(now.wSecond);
        if target < self.sec - 0.01 || reduced() { self.sec = target; self.moving = false; }
        else { self.moving = approach(&mut self.sec, target, dt.min(0.1), 0.09); }
        let ka = ez(t, 0.25, 0.5);
        let frac = self.sec * ka;
        // El cristal del reloj: cubre toda la esfera; plano en el centro (la hora se lee tal cual) y solo
        // dobla en el canto, donde están las marcas de los minutos y el fondo de pantalla
        let cr = (w.min(h) / 2.0 - 4.0).round();
        // El vidrio abomba el canto y lo de debajo se ve ampliado hacia el borde: las marcas van algo
        // hacia dentro (hasta 0,87 del radio) y el cristal las lleva hasta el filo; el segundero queda en
        // la parte plana
        let mk = cr * 0.87;
        let ar = cr * 0.66;
        self.lens.set(&[Piece::disc(cx, cy, cr, cr * 0.28, 0.4)]);
        let dial = |u: &Ui| {
            // Marcas de los minutos (las de las horas, más largas)
            for i in 0..60 {
                let a = i as f32 / 60.0 * TAU;
                let (s, c) = a.sin_cos();
                let (r0, sw, al) = if i % 5 == 0 { (mk - 9.0, 1.4, 0.62) } else { (mk - 5.0, 1.0, 0.30) };
                unsafe { u.rt.DrawLine(v2(cx + r0 * s, cy - r0 * c), v2(cx + mk * s, cy - mk * c), &u.wa(q(al * ka, 50.0)), sw, None); }
            }
            // Segundero: carril finísimo, arco blanco y la punta en el acento
            unsafe { u.rt.DrawEllipse(&ellipse(cx, cy, ar), &u.wa(q(0.10 * ka, 100.0)), 1.0, None); }
            let pt = |a: f32| v2(cx + ar * a.sin(), cy - ar * a.cos());
            const SEG: f32 = 120.0;
            let n = (frac * SEG).ceil() as usize;
            let arc_b = u.wa(0.55);
            for i in 0..n {
                let (a0, a1) = (i as f32 / SEG * TAU, ((i + 1) as f32 / SEG).min(frac) * TAU);
                unsafe { u.rt.DrawLine(pt(a0), pt(a1), &arc_b, 1.5, None); }
            }
            let head = pt(frac * TAU);
            dot(u.rt, head.X, head.Y, 2.4 * ka, &u.brush(acc(1.0)));
        };
        let kc = ez(t, 0.0, 0.5);
        popup::push_alpha(rt, kc);
        self.lens.shade(ui, &base, 0, 0.8);
        let key = ((frac * 4000.0) as u64) << 8 | (ka * 100.0) as u64;
        if self.lens.update(ui, w, h, key, dial) { self.lens.fill(ui, 0); }
        // Un velo muy leve dentro del cristal para que la hora se lea sobre cualquier fondo
        let veil = rgba(lerp(0.0, 1.0, ui.ink), lerp(0.0, 1.0, ui.ink), lerp(0.0, 1.0, ui.ink), q(lerp(0.16, 0.22, ui.ink), 50.0));
        unsafe { rt.FillEllipse(&ellipse(cx, cy, cr), &ui.brush(veil)); }
        unsafe { rt.PopLayer(); }

        // La hora y el día, encima y sin deformar (caen en la parte plana del cristal)
        popup::push_alpha(rt, kt);
        ui.text_c(&gf.face, &hm, cx, cy - 32.0 + 6.0 * (1.0 - kt), 50.0, &ui.white);
        ui.text_c(&ui.f.sub, &date, cx, cy + 15.0, 18.0, &ui.dim);
        unsafe { rt.PopLayer(); }
        // El brillo del vidrio: filo fino y un reflejo curvo arriba a la izquierda
        popup::push_alpha(rt, kc);
        self.lens.rim(ui, 0, 1.0);
        unsafe { rt.PopLayer(); }
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn animating(&self, t: f32) -> bool { t < 1.2 || self.moving }
}

// ================================================================ Música: «Casete»
const WN: usize = 72;
const HUB: f32 = 11.0;
const PACK: f32 = 27.0;

/// Radio de la cinta en cada bobina (izquierda, derecha) para un progreso 0..1: la cinta que pasa es
/// superficie, así que el radio va con la raíz (la de la izquierda se vacía y la de la derecha se llena).
fn reel_radii(prog: f32) -> (f32, f32) {
    let p = prog.clamp(0.0, 1.0);
    let span = PACK * PACK - HUB * HUB;
    ((HUB * HUB + span * (1.0 - p)).sqrt(), (HUB * HUB + span * p).sqrt())
}

/// «1:14» (o «61:05» si pasa de la hora).
fn mmss(s: u32) -> String { format!("{}:{:02}", s / 60, s % 60) }

/// Casete de plástico ahumado a la izquierda (deja ver, oscurecido y doblado en los cantos, el fondo de
/// pantalla), con su ventana transparente por la que se ven dos bobinas que pasan la cinta según el
/// progreso real y giran solo mientras suena; a la derecha, la canción, una onda en línea fina, el
/// progreso y los controles.
pub struct TapeW { ang: [f32; 2], spin: f32, phase: [f32; 6], lv: [f32; audioviz::BANDS], wave: [f32; WN], alive: f32, playing: bool, moving: bool, body: Lens, win: Lens }

impl Default for TapeW {
    fn default() -> Self { TapeW { ang: [0.0, 1.1], spin: 0.0, phase: [0.0; 6], lv: [0.0; audioviz::BANDS], wave: [0.0; WN], alive: 0.0, playing: false, moving: false, body: Lens::default(), win: Lens::default() } }
}

impl TapeW {
    /// Una bobina: la cinta enrollada (casi negra, con un brillo en diagonal como el de la cinta de
    /// verdad), el buje blanco con sus seis dientes por dentro, que giran, y el hueco del eje.
    fn reel(ui: &Ui, base: &Matrix3x2, cx: f32, cy: f32, r: f32, ang: f32) {
        let rt = ui.rt;
        let st = round_stroke(rt);
        place(ui, base, cx, cy, r, r);
        let tape = rad(ui, 0.0, 0.0, &[(0.0, rgba(0.09, 0.075, 0.07, 1.0)), (0.85, rgba(0.15, 0.12, 0.11, 1.0)), (1.0, rgba(0.24, 0.20, 0.18, 1.0))]);
        let sheen = lin(ui, -0.8, -0.8, 0.8, 0.8, &[(0.0, wh(0.13)), (0.42, wh(0.0)), (0.58, wh(0.0)), (1.0, wh(0.07))]);
        unsafe {
            rt.FillEllipse(&ellipse(0.0, 0.0, 1.0), &tape);
            rt.FillEllipse(&ellipse(0.0, 0.0, 1.0), &sheen);
        }
        reset(ui, base);
        let hub = ui.brush(rgba(0.90, 0.90, 0.92, 1.0));
        unsafe {
            rt.FillEllipse(&ellipse(cx, cy, HUB - 1.0), &hub);
            rt.FillEllipse(&ellipse(cx, cy, HUB - 4.5), &ui.brush(rgba(0.10, 0.10, 0.11, 1.0)));
        }
        for i in 0..6 {
            let a = ang + i as f32 / 6.0 * TAU;
            let (s, c) = a.sin_cos();
            unsafe { rt.DrawLine(v2(cx + c * (HUB - 5.0), cy + s * (HUB - 5.0)), v2(cx + c * (HUB - 7.2), cy + s * (HUB - 7.2)), &hub, 1.6, st.as_ref()); }
        }
    }
}

impl Widget for TapeW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (w, h) = size(Kind::Music);
        let dt = dt.min(0.1);
        let m = MEDIA.lock().unwrap().clone();
        let playing = m.as_ref().is_some_and(|m| m.playing);
        self.playing = playing;
        let secs = m.as_ref().and_then(|m| media_secs(m.playing));
        let prog = secs.map(|(p, e)| (p / e.max(1.0)).clamp(0.0, 1.0)).unwrap_or(0.0);
        let mut moving = approach(&mut self.spin, if playing { 1.0 } else { 0.0 }, dt, 0.4);
        let (rl, rr) = reel_radii(prog);
        // La cinta corre a velocidad constante: la bobina más pequeña gira más deprisa
        if !reduced() {
            let v = 26.0 * self.spin;
            self.ang[0] = (self.ang[0] + v / rl * dt) % TAU;
            self.ang[1] = (self.ang[1] + v / rr * dt) % TAU;
        }
        // Espectro (solo se usa sonando)
        let src = if playing { levels_now() } else { [0.0; audioviz::BANDS] };
        let k = 1.0 - (-dt / 0.06).exp();
        let mut energy = 0.0;
        for i in 0..audioviz::BANDS { self.lv[i] += (src[i] - self.lv[i]) * k; energy += self.lv[i]; }
        self.alive += ((if energy / audioviz::BANDS as f32 > 0.02 { 1.0 } else { 0.0 }) - self.alive) * (1.0 - (-dt / 0.3).exp());
        moving |= self.alive > 0.01 && self.alive < 0.99;

        let rt = ui.rt;
        let base = cur_xf(ui);
        let st = round_stroke(rt);

        // ---- La casete, de plástico ahumado translúcido: el cuerpo es cristal que deja ver (oscurecido) el
        // fondo de pantalla, doblado en sus cantos; por la ventana transparente se ven las bobinas
        let kc = ez(t, 0.0, 0.6);
        let (bx0, by0, bx1, by1) = (14.0, 26.0, 240.0, h - 26.0);
        popup::push_alpha(rt, kc);
        ui.hit(rect(bx0, by0, bx1, by1), 4);
        let hk = ui.heat(4);
        self.body.set(&[Piece::slab(bx0, by0, bx1, by1, 12.0, 14.0, 0.9)]);
        self.body.shade(ui, &base, 0, 1.0);
        if self.body.update(ui, w, h, 0, |_| {}) { self.body.fill(ui, 0); }
        unsafe {
            // Ahumado: oscurece lo de detrás sin taparlo
            rt.FillRoundedRectangle(&rrect(rect(bx0, by0, bx1, by1), 12.0), &ui.brush(rgba(0.04 + 0.02 * hk, 0.04 + 0.02 * hk, 0.05 + 0.02 * hk, 0.66)));
            let sheen = lin(ui, 0.0, by0, 0.0, by1, &[(0.0, wh(0.07)), (0.5, wh(0.0)), (1.0, wh(0.02))]);
            rt.FillRoundedRectangle(&rrect(rect(bx0, by0, bx1, by1), 12.0), &sheen);
        }
        self.body.rim(ui, 0, 0.8);
        for (sx, sy) in [(bx0 + 8.0, by0 + 8.0), (bx1 - 8.0, by0 + 8.0), (bx0 + 8.0, by1 - 8.0), (bx1 - 8.0, by1 - 8.0)] {
            dot(rt, sx, sy, 2.0, &ui.brush(wh(0.14)));
        }
        // Etiqueta: una franja clara con la cara «A»
        unsafe { rt.FillRoundedRectangle(&rrect(rect(bx0 + 18.0, by0 + 10.0, bx1 - 18.0, by0 + 28.0), 4.0), &ui.brush(wh(0.08))); }
        ui.text(&ui.f.small, "A", bx0 + 25.0, by0 + 10.0, 18.0, &ui.brush(wh(0.55)));
        ui.text_r(&ui.f.small, "C60", bx1 - 25.0, by0 + 10.0, 18.0, &ui.brush(wh(0.30)));
        // La cinta que baja de las bobinas a las guías de abajo
        let (wx0, wy0, wx1, wy1) = (bx0 + 26.0, by0 + 36.0, bx1 - 26.0, by0 + 100.0);
        let rcy = (wy0 + wy1) / 2.0;
        let (c1, c2) = (bx0 + 70.0, bx1 - 70.0);
        let (g1, g2, gy) = (wx0 + 8.0, wx1 - 8.0, wy1 + 8.0);
        let tape_path = |u: &Ui| {
            let tape = u.brush(rgba(0.36, 0.30, 0.26, 0.9));
            let st = round_stroke(u.rt);
            unsafe {
                u.rt.DrawLine(v2(c1 - rl * 0.75, rcy + rl * 0.66), v2(g1, gy), &tape, 1.2, st.as_ref());
                u.rt.DrawLine(v2(g1, gy), v2(g2, gy), &tape, 1.2, st.as_ref());
                u.rt.DrawLine(v2(g2, gy), v2(c2 + rr * 0.75, rcy + rr * 0.66), &tape, 1.2, st.as_ref());
            }
        };
        tape_path(ui);
        // Ventana de plástico transparente: detrás, el fondo de pantalla, las bobinas y la cinta; el
        // plástico los dobla un poco en sus cantos
        let (a0, a1) = (self.ang[0], self.ang[1]);
        let reels = |u: &Ui| {
            let b0 = cur_xf(u);
            tape_path(u);
            Self::reel(u, &b0, c1, rcy, rl, a0);
            Self::reel(u, &b0, c2, rcy, rr, a1);
        };
        self.win.set(&[Piece::slab(wx0, wy0, wx1, wy1, 10.0, 8.0, 0.8)]);
        let key = ((a0 * 512.0) as u64) << 40 ^ ((a1 * 512.0) as u64) << 20 ^ ((rl * 64.0) as u64);
        if self.win.update(ui, w, h, key, reels) {
            self.win.fill(ui, 0);
        } else {
            Self::reel(ui, &base, c1, rcy, rl, a0);
            Self::reel(ui, &base, c2, rcy, rr, a1);
        }
        unsafe {
            rt.FillRoundedRectangle(&rrect(rect(wx0, wy0, wx1, wy1), 10.0), &ui.brush(rgba(0.0, 0.0, 0.0, 0.16)));
            rt.FillRoundedRectangle(&rrect(rect(wx0, wy0, wx1, wy1), 10.0), &lin(ui, 0.0, wy0, 0.0, wy1, &[(0.0, wh(0.08)), (0.45, wh(0.0)), (1.0, wh(0.0))]));
        }
        self.win.rim(ui, 0, 0.9);
        for gx in [g1, g2] { dot(rt, gx, gy, 2.2, &ui.brush(wh(0.35))); }
        let hx = (bx0 + bx1) / 2.0;
        unsafe { rt.FillRoundedRectangle(&rrect(rect(hx - 16.0, gy - 1.0, hx + 16.0, gy + 6.0), 2.5), &ui.brush(wh(0.16))); }
        unsafe { rt.PopLayer(); }

        // ---- A la derecha: la canción
        let x0 = 262.0;
        let xr = w - 18.0;
        let (title, artist) = match &m { Some(m) => (m.title.clone(), m.artist.clone()), None => (i18n::t("Nada sonando").to_string(), i18n::t("Pon algo de música").to_string()) };
        ui.text(&d.title, &fit(ui, &d.title, &title, xr - x0), x0, 16.0, 22.0, &ui.white);
        ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &artist, xr - x0), x0, 38.0, 18.0, &ui.dim);

        // ---- Onda: una línea fina, suma de senos con las bandas como amplitudes
        let groups = [band_mean(&self.lv, 0, 4), band_mean(&self.lv, 4, 9), band_mean(&self.lv, 9, 15), band_mean(&self.lv, 15, 22), band_mean(&self.lv, 22, 29), band_mean(&self.lv, 29, 36)];
        const FREQ: [f32; 6] = [1.0, 2.0, 3.5, 5.0, 8.0, 13.0];
        const SPEED: [f32; 6] = [1.3, 2.1, 3.4, 4.6, 6.1, 8.3];
        if !reduced() { for i in 0..6 { self.phase[i] = (self.phase[i] + dt * SPEED[i]) % TAU; } }
        let kw = 1.0 - (-dt / 0.05).exp();
        for x in 0..WN {
            let u = x as f32 / (WN - 1) as f32;
            let mut v = 0.0;
            for i in 0..6 { v += groups[i] * (TAU * FREQ[i] * u + self.phase[i] + i as f32).sin() / (1.0 + i as f32 * 0.35); }
            // Se apaga suave hacia los bordes
            let edge = (u * (1.0 - u) * 4.0).min(1.0).sqrt();
            let target = (v * 0.55).clamp(-1.0, 1.0) * edge;
            self.wave[x] += (target - self.wave[x]) * kw;
        }
        let mid = 92.0;
        let amp = 22.0;
        let sw = (xr - x0) / (WN - 1) as f32;
        let ks = ez(t, 0.15, 0.6);
        let line = ui.wa(q(0.9 * ks, 20.0));
        unsafe { rt.FillRectangle(&rect(x0, mid, xr, mid + 1.0 / ui.s.max(0.5)), &ui.wa(q(0.10 * ks, 50.0))); }
        for x in 1..WN {
            let (xa, xb) = (x0 + (x - 1) as f32 * sw, x0 + x as f32 * sw);
            unsafe { rt.DrawLine(v2(xa, mid - self.wave[x - 1] * amp), v2(xb, mid - self.wave[x] * amp), &line, 1.4, st.as_ref()); }
        }

        // ---- Progreso (la punta, un punto diminuto en el acento) y tiempos
        let py = 132.0;
        bar(ui, x0, py, xr - x0, 3.0, prog);
        if secs.is_some() { dot(rt, x0 + (xr - x0) * prog, py + 1.5, 2.6, &ui.brush(acc(1.0))); }
        if let Some((p, e)) = secs {
            ui.text(&ui.f.small, &mmss(p as u32), x0, py + 6.0, 16.0, &ui.faint);
            ui.text_r(&ui.f.small, &mmss(e as u32), xr, py + 6.0, 16.0, &ui.faint);
        }
        // ---- Controles
        let cyb = h - 26.0;
        let ccx = (x0 + xr) / 2.0;
        for (i, (g, id)) in [("\u{E892}", 1u32), (if playing { "\u{E769}" } else { "\u{E768}" }, 2), ("\u{E893}", 3)].into_iter().enumerate() {
            let bx = ccx + (i as f32 - 1.0) * 56.0;
            let hk = ui.heat(id);
            let pr = 1.0 - 0.12 * desk::press(id);
            scale_about(ui, &base, bx, cyb, pr);
            if hk > 0.01 || id == 2 { unsafe { rt.FillEllipse(&ellipse(bx, cyb, 17.0), &ui.wa(if id == 2 { 0.10 + 0.08 * hk } else { 0.08 * hk })); } }
            ui.glyph(&d.glyph_m, g, bx - 14.0, cyb - 14.0, 28.0, 28.0, &ui.white);
            reset(ui, &base);
            ui.hit(rect(bx - 18.0, cyb - 18.0, bx + 18.0, cyb + 18.0), id);
        }
        self.moving = moving;
    }
    fn click(&mut self, id: Option<u32>, right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        if right { return None; }
        match id? {
            1 => Some(Box::new(|| crate::media(crate::Cmd::Prev))),
            2 | 4 => Some(Box::new(|| crate::media(crate::Cmd::Toggle))),
            3 => Some(Box::new(|| crate::media(crate::Cmd::Next))),
            _ => None,
        }
    }
    fn tick(&mut self) -> bool {
        if self.playing && deskdata::VISIBLE.load(Ordering::Relaxed) { audioviz::want(); }
        true
    }
    // Solo pinta seguido mientras suena (o las bobinas frenan); parado, una vez por segundo
    fn animating(&self, t: f32) -> bool { t < 1.2 || self.playing || self.moving || self.alive > 0.01 }
    fn frame_ms(&self) -> u32 { 33 }
}

// ================================================================ Almacenamiento: «Tubos»
/// Como el clásico (icono, nombre y «libres de», fuera del vidrio), pero lo usado de cada unidad es una
/// barra blanca que va dentro de un tubo de cristal: el tubo dobla en sus cantos la barra y el fondo de
/// pantalla que tiene detrás.
#[derive(Default)]
pub struct DriveDotsW { fills: Vec<f32>, moving: bool, lens: Lens }

impl Widget for DriveDotsW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (w, h) = size(Kind::Storage);
        let st = deskdata::STORE.lock().unwrap().clone();
        let n = st.as_ref().map(|s| s.drives.len()).unwrap_or(0);
        deskw::header(ui, d, i18n::t("Almacenamiento"), &match n { 0 => String::new(), 1 => i18n::t("1 unidad").into(), n => i18n::tf("{} unidades", &[&n]) }, w);
        let Some(st) = st else { return };
        let base = cur_xf(ui);
        let pad = 20.0;
        let tr = 9.0;
        let shown = st.drives.len().min(3);
        self.fills.resize(shown, 0.0);
        self.moving = false;
        let mut pieces = Vec::new();
        let mut rows = Vec::new();
        for (i, dr) in st.drives.iter().take(3).enumerate() {
            let y = 56.0 + i as f32 * 54.0;
            let used = dr.total.saturating_sub(dr.free);
            let frac = used as f32 / dr.total.max(1) as f32;
            let letter = dr.root.trim_end_matches('\\');
            let name = if dr.label.is_empty() { i18n::tf("Disco ({})", &[&letter]) } else { format!("{} ({letter})", dr.label) };
            let id = 40 + i as u32;
            let hk = ui.heat(id);
            ui.card_k(rect(pad - 8.0, y - 4.0, w - pad + 8.0, y + 46.0), hk);
            ui.glyph(&ui.f.icon_l, "\u{EDA2}", pad + 3.0 * hk, y, 20.0, 20.0, &ui.wa(0.72 + 0.25 * hk));
            let free = i18n::tf("{} libres de {}", &[&deskdata::fmt_bytes(dr.free), &deskdata::fmt_bytes(dr.total)]);
            let fw = ui.p.width(&ui.f.sub, &free);
            ui.text(&ui.f.name, &fit(ui, &ui.f.name, &name, w - 2.0 * pad - fw - 40.0), pad + 28.0 + 4.0 * hk, y, 20.0, &ui.white);
            ui.text_r(&ui.f.sub, &free, w - pad, y, 20.0, &ui.dim);
            // Se llena en cascada al aparecer; después sigue al dato con suavidad
            let k = ez(t, 0.15 + i as f32 * 0.12, 0.9);
            if t < 1.1 { self.fills[i] = frac * k; } else { self.moving |= approach(&mut self.fills[i], frac, dt, 0.4); }
            let cy = y + 33.0;
            pieces.push(Piece::slab(pad, cy - tr, w - pad, cy + tr, tr, tr, 0.85));
            rows.push((cy, self.fills[i], frac >= 0.9));
            ui.hit(rect(pad - 8.0, y - 4.0, w - pad + 8.0, y + 46.0), id);
        }
        if pieces.is_empty() { return; }
        self.lens.set(&pieces);
        let draw_fill = |u: &Ui| {
            for &(cy, f, full) in &rows {
                let (x0, x1) = (pad + 3.0, w - pad - 3.0);
                let xe = x0 + ((x1 - x0) * f.clamp(0.0, 1.0)).max(if f > 0.0 { 7.0 } else { 0.0 });
                if xe > x0 { unsafe { u.rt.FillRoundedRectangle(&rrect(rect(x0, cy - 2.0, xe, cy + 2.0), 2.0), &u.wa(0.85)); } }
                // Casi llena: una raya diminuta en el acento en la punta
                if full && xe > x0 + 4.0 { unsafe { u.rt.FillRectangle(&rect(xe - 3.0, cy - 2.5, xe - 1.5, cy + 2.5), &u.brush(acc(1.0))); } }
            }
        };
        // Solo se vuelve a refractar si cambia el llenado
        let key = rows.iter().fold(0u64, |k, r| k.rotate_left(13) ^ (r.1 * 4000.0) as u64 ^ r.2 as u64);
        let glass = self.lens.update(ui, w, h, key, &draw_fill);
        for i in 0..rows.len() {
            self.lens.shade(ui, &base, i, 0.8);
            if glass { self.lens.fill(ui, i); } else { draw_fill(ui); }
            let (x0, y0, x1, y1) = pieces[i].bounds();
            unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(x0, y0, x1, y1), tr), &ui.brush(wh(q(lerp(0.05, 0.14, ui.ink), 50.0)))); }
            self.lens.rim(ui, i, 1.0);
        }
    }
    fn click(&mut self, id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        match id? {
            i @ 40..=42 => {
                let root = deskdata::STORE.lock().unwrap().as_ref().and_then(|s| s.drives.get((i - 40) as usize).map(|d| d.root.clone()))?;
                Some(Box::new(move || sys::open(&root)))
            }
            _ => None,
        }
    }
    fn animating(&self, t: f32) -> bool { t < 1.4 || self.moving }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn juego_de_la_vida() {
        // Parpadeador: una línea de 3 se vuelve vertical y vuelve (periodo 2), con la misma población
        let (c, r) = (6, 6);
        let mut g = vec![false; c * r];
        for x in 1..4 { g[2 * c + x] = true; }
        let mut n = vec![false; c * r];
        assert_eq!(life_step(&g, &mut n, c, r), 3);
        assert!(n[c + 2] && n[2 * c + 2] && n[3 * c + 2] && !n[2 * c + 1]);
        let mut n2 = vec![false; c * r];
        life_step(&n, &mut n2, c, r);
        assert_eq!(n2, g);
        assert_eq!(grid_hash(&n2), grid_hash(&g));
        assert_ne!(grid_hash(&n), grid_hash(&g));
        // Toroidal: un bloque partido por las esquinas sigue siendo un bloque quieto
        let mut b = vec![false; c * r];
        for (x, y) in [(0, 0), (c - 1, 0), (0, r - 1), (c - 1, r - 1)] { b[y * c + x] = true; }
        let mut bn = vec![false; c * r];
        assert_eq!(life_step(&b, &mut bn, c, r), 4);
        assert_eq!(bn, b);
        // Una célula sola muere
        let mut s = vec![false; c * r];
        s[7] = true;
        assert_eq!(life_step(&s, &mut bn, c, r), 0);
    }

    #[test]
    fn vida_se_resiembra_y_anota_muertes() {
        let mut l = LifeW::default();
        assert!(l.pop > 0);
        // Vacía: al dar un paso se vuelve a sembrar
        l.cells.iter_mut().for_each(|c| *c = false);
        l.step();
        assert!(l.pop > 0 && l.gens == 0);
        // Una célula sola muere en el paso siguiente y queda anotado cuándo (para desvanecerla)
        l.cells.iter_mut().for_each(|c| *c = false);
        l.cells[LC + 1] = true;
        // Un bloque quieto lejos: así la rejilla no se vacía y no se vuelve a sembrar al azar (que podía
        // resucitar justo esa casilla y hacer fallar la prueba de vez en cuando)
        for (x, y) in [(10, 5), (11, 5), (10, 6), (11, 6)] { l.cells[y * LC + x] = true; }
        l.now = 5.0;
        l.step();
        assert!(!l.cells[LC + 1] && l.died[LC + 1] == 5.0);
        // Lo que siembra un golpe queda marcado
        l.cells.iter_mut().for_each(|c| *c = false);
        l.burst();
        assert!((0..LC * LR).any(|i| l.cells[i] && l.seeded[i]));
        for i in 0..50 { let v = cell_rand(i, 1); assert!((0.0..=1.0).contains(&v)); }
    }

    #[test]
    fn esfera_de_fibonacci() {
        let p = fib_sphere(ORB_N);
        assert_eq!(p.len(), ORB_N);
        // Todos sobre la esfera y repartidos (el centro de masas cerca del centro)
        for v in &p { assert!(((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() - 1.0).abs() < 1e-4); }
        let c: [f32; 3] = p.iter().fold([0.0; 3], |a, v| [a[0] + v[0], a[1] + v[1], a[2] + v[2]]);
        for k in c { assert!((k / ORB_N as f32).abs() < 0.03); }
        // Girar no cambia la distancia al centro
        let r = rotate([0.3, 0.4, 0.866], 1.2, -0.36);
        assert!(((r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn refraccion_del_cristal() {
        // Nada donde empieza el bisel, mucho en el canto; crece hacia fuera
        assert_eq!(bend(0.0), 0.0);
        assert!(bend(1.0) < -1.0);
        for i in 1..20 { assert!(bend(i as f32 / 20.0) < bend((i - 1) as f32 / 20.0)); }
        // Orbe (bisel = radio): fuera no hay desplazamiento; dentro apunta hacia el centro
        let o = Piece::disc(100.0, 100.0, 40.0, 40.0, 0.55);
        assert!(o.disp(150.0, 100.0).is_none());
        let (dx, dy) = o.disp(130.0, 100.0).unwrap();
        assert!(dx < 0.0 && dy.abs() < 1e-4);
        // Cristal del reloj (bisel corto): el centro, donde va la hora, queda intacto
        let c = Piece::disc(0.0, 0.0, 80.0, 27.0, 0.6);
        assert_eq!(c.disp(30.0, 20.0), Some((0.0, 0.0)));
        assert!(c.disp(75.0, 0.0).unwrap().0 < -1.0);
        // Baldosa: plana dentro, dobla junto a cada canto hacia dentro, y nada fuera
        let b = Piece::slab(0.0, 0.0, 46.0, 46.0, 7.0, 10.0, 0.9);
        assert_eq!(b.disp(23.0, 23.0), Some((0.0, 0.0)));
        let (dx, dy) = b.disp(44.0, 23.0).unwrap();
        assert!(dx < 0.0 && dy.abs() < 1e-4);
        let (dx, dy) = b.disp(23.0, 1.0).unwrap();
        assert!(dy > 0.0 && dx.abs() < 1e-4);
        assert!(b.disp(47.0, 23.0).is_none());
        // Esquina redondeada: fuera del redondeo no es cristal
        assert!(b.disp(0.5, 0.5).is_none());
        // Columna fina (bisel = medio ancho): solo se desplaza en horizontal en la parte recta
        let col = Piece::slab(0.0, 0.0, 11.0, 150.0, 5.5, 5.5, 1.0);
        let (dx, dy) = col.disp(9.0, 75.0).unwrap();
        assert!(dx < 0.0 && dy.abs() < 1e-4);
        // Mapa: 0,5 es quieto y se satura en los extremos
        assert_eq!(map_byte(0.0, 1.5, 100.0), 128);
        assert_eq!(map_byte(1000.0, 1.5, 100.0), 255);
        assert_eq!(map_byte(-1000.0, 1.5, 100.0), 0);
        assert!((Piece::disc(0.0, 0.0, 10.0, 10.0, 1.0).max_disp() - 12.5).abs() < 1e-4);
    }

    #[test]
    fn reloj_y_tiempos() {
        assert_eq!(sec_frac(0), 0.0);
        assert!((sec_frac(30) - 0.5).abs() < 1e-6);
        assert!(sec_frac(59) < 1.0);
        assert!(sec_frac(61) < 1.0);
        assert_eq!(mmss(74), "1:14");
        assert_eq!(mmss(3665), "61:05");
    }

    #[test]
    fn bobinas_del_casete() {
        // Al empezar toda la cinta a la izquierda; al acabar, a la derecha; la superficie se conserva
        let (l, r) = reel_radii(0.0);
        assert!((l - PACK).abs() < 1e-4 && (r - HUB).abs() < 1e-4);
        let (l, r) = reel_radii(1.0);
        assert!((l - HUB).abs() < 1e-4 && (r - PACK).abs() < 1e-4);
        for p in [0.1, 0.37, 0.5, 0.9] {
            let (l, r) = reel_radii(p);
            assert!((l * l + r * r - (PACK * PACK + HUB * HUB)).abs() < 1e-2);
        }
        assert_eq!(reel_radii(-3.0), reel_radii(0.0));
    }

    #[test]
    fn picos_y_columnas() {
        // El pico sube con el nivel, aguanta un momento y luego cae sin bajar del nivel
        let (mut p, mut h) = (0.0, 0.0);
        peak_step(&mut p, &mut h, 0.8, 0.016);
        assert_eq!(p, 0.8);
        peak_step(&mut p, &mut h, 0.2, 0.2);
        assert_eq!(p, 0.8);
        for _ in 0..200 { peak_step(&mut p, &mut h, 0.2, 0.05); }
        assert!((p - 0.2).abs() < 1e-6);
        // Columnas: los extremos son la primera y la última banda
        let mut l = [0.0f32; audioviz::BANDS];
        for (i, v) in l.iter_mut().enumerate() { *v = i as f32 / (audioviz::BANDS - 1) as f32; }
        assert!(col_level(&l, 0, DCOLS).abs() < 1e-6);
        assert!((col_level(&l, DCOLS - 1, DCOLS) - 1.0).abs() < 1e-6);
        for c in 1..DCOLS { assert!(col_level(&l, c, DCOLS) >= col_level(&l, c - 1, DCOLS)); }
        assert!((band_mean(&l, 0, audioviz::BANDS) - 0.5).abs() < 1e-5);
        assert_eq!(band_mean(&l, 5, 5), 0.0);
    }

    #[test]
    fn huella_de_degradados() {
        // Mismos colores y geometría: misma huella; cualquier cambio, otra
        let s = [(0.0, rgba(1.0, 1.0, 1.0, 0.5)), (1.0, rgba(1.0, 1.0, 1.0, 0.0))];
        assert_eq!(gkey(1, &s, [0.0, 0.0, 0.0, 1.0]), gkey(1, &s, [0.0, 0.0, 0.0, 1.0]));
        assert_ne!(gkey(1, &s, [0.0, 0.0, 0.0, 1.0]), gkey(2, &s, [0.0, 0.0, 0.0, 1.0]));
        assert_ne!(gkey(1, &s, [0.0, 0.0, 0.0, 1.0]), gkey(1, &s, [0.0, 0.0, 0.0, 2.0]));
        let s2 = [(0.0, rgba(1.0, 1.0, 1.0, 0.6)), (1.0, rgba(1.0, 1.0, 1.0, 0.0))];
        assert_ne!(gkey(1, &s, [0.0; 4]), gkey(1, &s2, [0.0; 4]));
        assert_eq!(q(0.53, 10.0), 0.5);
    }

    #[test]
    fn textos_del_equipo() {
        assert_eq!(short_cpu("AMD Ryzen 7 5800H with Radeon Graphics"), "AMD Ryzen 7 5800H");
        assert_eq!(short_cpu("Intel(R) Core(TM) i7-9750H CPU @ 2.60GHz"), "Intel Core i7-9750H");
        assert_eq!(short_cpu("AMD Ryzen 9 5900X 12-Core Processor"), "AMD Ryzen 9 5900X");
        assert_eq!(short_cpu("AMD Ryzen AI 7 350 w/ Radeon 860M"), "AMD Ryzen AI 7 350");
        assert_eq!(title_case("OMEN-PELAYO"), "Omen-Pelayo");
        assert_eq!(uptime_big(3 * 3600 + 25 * 60 + 9), "3:25");
        assert_eq!(uptime_big(2 * 86400 + 3 * 3600), "2d 3h");
        assert_eq!(uptime_words(90), "1 min");
        assert_eq!(uptime_words(2 * 86400 + 3 * 3600 + 4 * 60), "2 d 3 h 4 min");
    }
}
