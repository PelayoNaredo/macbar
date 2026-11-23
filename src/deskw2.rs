//! Widgets nuevos del escritorio: visualizador de audio y marco de fotos.

use super::*;
use desk::{layout_size as size, DFonts, Kind}; // `size`: medidas de la maqueta (ver `desk::layout_size`)
use deskw::{header, Widget};
use popup::Ui;

const PAD: f32 = 20.0;

/// Color de la carátula que suena (el mismo que tiñe la isla), o el acento si no hay música.
pub(crate) fn tint() -> [f32; 3] {
    let t = GFX.with(|g| g.try_borrow().ok().and_then(|g| g.as_ref().map(|g| g.tint_now)));
    match t {
        Some(t) if MEDIA.lock().unwrap().is_some() && t != [1.0; 3] => t,
        _ => { let a = crate::profiles::accent(1.0); [a.r, a.g, a.b] }
    }
}

// ---------------------------------------------------------------- visualizador de audio
/// Barras del espectro de lo que suena, con el color de la carátula y su reflejo debajo. Responsive:
/// el espectro ocupa lo que deja cada tamaño (en Alto, de lado y en espejo), y los grandes añaden la
/// canción (Grande y Extragrande) y la carátula (Columna).
pub struct VisualW { levels: [f32; audioviz::BANDS], alive: f32, art: Option<(u64, ID2D1Bitmap)>, rt: crate::RtKey }

impl Default for VisualW { fn default() -> Self { VisualW { levels: [0.0; audioviz::BANDS], alive: 0.0, art: None, rt: crate::RtKey::default() } } }

impl VisualW {
    /// Nivel de la barra `j` de `n`: con menos barras que bandas, cada una junta las suyas; con más (los
    /// anchos), se interpola entre bandas vecinas para que las barras no salgan gordas.
    fn level(&self, j: usize, n: usize) -> f32 {
        let b = audioviz::BANDS;
        if n > b {
            let x = j as f32 * (b - 1) as f32 / (n - 1).max(1) as f32;
            let (i, f) = (x.floor() as usize, x.fract());
            return self.levels[i.min(b - 1)] * (1.0 - f) + self.levels[(i + 1).min(b - 1)] * f;
        }
        let (a, z) = (j * b / n, ((j + 1) * b / n).max(j * b / n + 1));
        self.levels[a..z.min(b)].iter().copied().fold(0.0, f32::max)
    }
    /// Barras verticales en `r`: crecen desde una base con su reflejo debajo (el reflejo cabe en `r`).
    fn bars(&self, ui: &Ui, r: D2D_RECT_F, tn: [f32; 3]) {
        let gap = 3.0;
        // Barras de unos 6,5 DIP con su hueco, las que quepan en el ancho
        let n = (((r.right - r.left + gap) / 9.5).round() as usize).clamp(8, 96);
        let bw = ((r.right - r.left) - gap * (n as f32 - 1.0)) / n as f32;
        let base = r.bottom - (r.bottom - r.top) * 0.2;
        let maxh = base - r.top;
        unsafe {
            for j in 0..n {
                let v = self.level(j, n);
                let x = r.left + j as f32 * (bw + gap);
                // En silencio quedan puntos que respiran despacio, como un ecualizador dormido
                let idle = 2.5 + 1.5 * (0.5 + 0.5 * (ui_time() * 1.6 + j as f32 * 0.35 * 36.0 / n as f32).sin());
                let bh = (v * maxh).max(idle * (1.0 - self.alive) + 3.0 * self.alive);
                let fill = ui.brush(rgba(tn[0], tn[1], tn[2], 0.55 + 0.45 * v));
                ui.rt.FillRoundedRectangle(&rrect(rect(x, base - bh, x + bw, base), bw / 2.0), &fill);
                // Reflejo tenue bajo la línea base
                let refl = ui.brush(rgba(tn[0], tn[1], tn[2], 0.12 * v));
                let rh = (bh * 0.28).min(r.bottom - base - 4.0).max(0.0);
                ui.rt.FillRoundedRectangle(&rrect(rect(x, base + 4.0, x + bw, base + 4.0 + rh), bw / 2.0), &refl);
            }
        }
    }
    /// Espectro de lado (Alto): una barra por fila, en espejo desde el eje del centro; los graves arriba.
    fn bars_side(&self, ui: &Ui, r: D2D_RECT_F, n: usize, tn: [f32; 3]) {
        let gap = 3.0;
        let bh = ((r.bottom - r.top) - gap * (n as f32 - 1.0)) / n as f32;
        let cx = (r.left + r.right) / 2.0;
        let half = (r.right - r.left) / 2.0;
        unsafe {
            for j in 0..n {
                let v = self.level(j, n);
                let y = r.top + j as f32 * (bh + gap);
                let idle = 2.5 + 1.5 * (0.5 + 0.5 * (ui_time() * 1.6 + j as f32 * 0.35 * 36.0 / n as f32).sin());
                let len = (v * half).max(idle * (1.0 - self.alive) + 3.0 * self.alive);
                let fill = ui.brush(rgba(tn[0], tn[1], tn[2], 0.55 + 0.45 * v));
                ui.rt.FillRoundedRectangle(&rrect(rect(cx - len, y, cx + len, y + bh), bh / 2.0), &fill);
            }
        }
    }
    /// Título y artista (o la invitación a poner música) con la primera línea en `f`.
    fn song(ui: &Ui, f: &IDWriteTextFormat, m: &Option<MediaInfo>, x: f32, y: f32, maxw: f32, lines: bool) {
        match m {
            Some(m) => {
                ui.text(f, &crate::news::fit(ui, f, &m.title, maxw), x, y, 24.0, &ui.white);
                let sub = if lines && !m.album.is_empty() { format!("{} · {}", m.artist, m.album) } else { m.artist.clone() };
                ui.text(&ui.f.sub, &crate::news::fit(ui, &ui.f.sub, &sub, maxw), x, y + 24.0, 18.0, &ui.dim);
            }
            None => {
                ui.text(f, i18n::t("Nada sonando"), x, y, 24.0, &ui.dim);
                ui.text(&ui.f.sub, i18n::t("Pon algo de música"), x, y + 24.0, 18.0, &ui.faint);
            }
        }
    }
}

impl Widget for VisualW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, _t: f32, dt: f32) {
        let l = desk::lay(Kind::Visual);
        let (w, h) = (l.w, l.h);
        if self.rt.fresh(ui.rt) { self.art = None; }
        let m = MEDIA.lock().unwrap().clone();
        let src = *audioviz::LEVELS.lock().unwrap();
        // Lo que llega del hilo se suaviza aún un poco al pintar (sin saltos entre fotogramas)
        let k = 1.0 - (-dt / 0.05).exp();
        let mut energy = 0.0;
        for i in 0..audioviz::BANDS { self.levels[i] += (src[i] - self.levels[i]) * k; energy += self.levels[i]; }
        let on = energy / audioviz::BANDS as f32 > 0.02;
        self.alive += ((if on { 1.0 } else { 0.0 }) - self.alive) * (1.0 - (-dt / 0.3).exp());
        let tn = tint();
        let idle_msg = m.is_none() && !on;
        // La canción, a la derecha de la cabecera (en los que no la enseñan aparte)
        let head = |ui: &Ui, maxc: usize| {
            let right = m.as_ref().map(|m| trunc(&m.title, maxc)).unwrap_or_default();
            header(ui, d, i18n::t("Sonido"), &right, w);
        };
        use desk::Sz::*;
        match l.sz {
            Pequeno => {
                self.bars(ui, rect(PAD, PAD + 8.0, w - PAD, h - PAD + 4.0), tn);
                if idle_msg { ui.text_c(&ui.f.sub, i18n::t("Sin sonido"), w / 2.0, h - 38.0, 18.0, &ui.dim); }
            }
            Mediano | Banda => {
                if l.sz == Banda {
                    let right = m.as_ref().map(|m| trunc(&format!("{} · {}", m.title, m.artist), 70)).unwrap_or_default();
                    header(ui, d, i18n::t("Sonido"), &right, w);
                } else { head(ui, 30); }
                self.bars(ui, rect(PAD, 58.0, w - PAD, h - 14.0), tn);
                if idle_msg { ui.text_c(&ui.f.sub, i18n::t("Pon algo de música"), w / 2.0, h - 36.0, 18.0, &ui.dim); }
            }
            Alto => {
                header(ui, d, i18n::t("Sonido"), "", w);
                self.bars_side(ui, rect(PAD, 58.0, w - PAD, h - 62.0), 24, tn);
                // La canción, abajo y en dos líneas cortas
                Self::song(ui, &ui.f.name, &m, PAD, h - PAD - 40.0, w - 2.0 * PAD, false);
            }
            Grande => {
                header(ui, d, i18n::t("Sonido"), "", w);
                self.bars(ui, rect(PAD, 62.0, w - PAD, h - 82.0), tn);
                Self::song(ui, &d.title, &m, PAD, h - PAD - 44.0, w - 2.0 * PAD, false);
            }
            Columna => {
                header(ui, d, i18n::t("Sonido"), "", w);
                // Carátula grande arriba, la canción debajo y el espectro hasta el final
                let side = w - 2.0 * PAD;
                let (ax, ay) = (PAD, 58.0);
                let ver = m.as_ref().map(|m| m.art_ver);
                if self.art.as_ref().map(|a| a.0) != ver {
                    self.art = ver.and_then(|v| ART_BIG.lock().unwrap().as_ref().filter(|a| a.0 == v).and_then(|(v, px)| deskw::bitmap(ui.rt, px, ART_BIG_PX).map(|b| (*v, b))));
                }
                let side = side.min(h - ay - 190.0);
                let ax = ax + (w - 2.0 * PAD - side) / 2.0;
                unsafe {
                    match &self.art {
                        Some((_, b)) => {
                            let s = side / ART_BIG_PX as f32;
                            let props = D2D1_BRUSH_PROPERTIES { opacity: 1.0, transform: Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: ax, M32: ay } };
                            if let Ok(br) = ui.rt.CreateBitmapBrush(b, None, Some(&props)) { ui.rt.FillRoundedRectangle(&rrect(rect(ax, ay, ax + side, ay + side), 14.0), &br); }
                            ui.rt.DrawRoundedRectangle(&rrect(rect(ax + 0.5, ay + 0.5, ax + side - 0.5, ay + side - 0.5), 13.5), &ui.wa(0.12), 1.0, None);
                        }
                        None => {
                            ui.rt.FillRoundedRectangle(&rrect(rect(ax, ay, ax + side, ay + side), 14.0), &ui.wa(0.06));
                            ui.glyph(&d.glyph_l, "\u{E8D6}", ax, ay, side, side, &ui.faint);
                        }
                    }
                }
                let ty = ay + side + 14.0;
                Self::song(ui, &d.title, &m, PAD, ty, w - 2.0 * PAD, true);
                self.bars(ui, rect(PAD, ty + 54.0, w - PAD, h - 8.0), tn);
            }
            Extragrande => {
                header(ui, d, i18n::t("Sonido"), m.as_ref().map(|m| m.app.trim_end_matches(".exe")).unwrap_or(""), w);
                self.bars(ui, rect(PAD, 62.0, w - PAD, h - 84.0), tn);
                Self::song(ui, &d.big, &m, PAD, h - PAD - 46.0, w - 2.0 * PAD, true);
            }
        }
    }
    fn layouts(&self) -> bool { true }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn tick(&mut self) -> bool {
        // Solo captura mientras se ve (tapado por ventanas no gasta nada)
        if deskdata::VISIBLE.load(Ordering::Relaxed) { audioviz::want(); }
        true
    }
    fn animating(&self, _t: f32) -> bool { true }
    fn smooth(&self) -> bool { self.alive > 0.01 }
    fn frame_ms(&self) -> u32 { if self.alive > 0.01 { 16 } else { 60 } }
}

/// Segundos desde que arrancó macbar (para animaciones lentas y continuas).
pub(crate) fn ui_time() -> f32 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f32()
}

// ---------------------------------------------------------------- marco de fotos
/// Foto lista para pintar: versión, píxeles BGRA premultiplicados, ancho, alto y ruta.
pub(crate) type Photo = (u64, Arc<Vec<u8>>, u32, u32, String);
static PHOTO: Mutex<Option<Photo>> = Mutex::new(None);
static NEXT_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PHOTO_STARTED: std::sync::Once = std::sync::Once::new();
pub(crate) const SHOW_S: f32 = 14.0;
const FADE_S: f32 = 1.4;

/// Carpeta de las fotos: la elegida (widgets.ini «fotos_carpeta») o Imágenes.
fn photo_dir() -> String {
    // Modo demo: las capturas del propio repositorio (nunca fotos personales en capturas de muestra)
    if sys::demo() { if let Some(d) = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.parent()?.parent()?.join("docs").to_string_lossy().to_string())) { return d; } }
    desk::ini_value("fotos_carpeta").unwrap_or_else(|| std::env::var("USERPROFILE").unwrap_or_default() + "\\Pictures")
}

/// Imágenes de la carpeta y de sus subcarpetas (dos niveles, como mucho 3000).
fn list_photos(dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![(std::path::PathBuf::from(dir), 0)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { if depth < 2 { stack.push((p, depth + 1)); } continue; }
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "heic" | "bmp") { out.push(p.to_string_lossy().to_string()); }
            if out.len() >= 3000 { return out; }
        }
    }
    out
}

/// Decodifica una foto girada según su EXIF y reducida para que cubra `tw` × `th` sin perder calidad.
fn load_photo(path: &str, tw: u32, th: u32) -> Option<(Vec<u8>, u32, u32)> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let dec = wic.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnLoad).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        // Orientación EXIF (las fotos del móvil vienen «tumbadas» y la cámara lo apunta aquí)
        let orient = frame.GetMetadataQueryReader().ok().and_then(|q| {
            let mut v = windows::Win32::System::Com::StructuredStorage::PROPVARIANT::default();
            q.GetMetadataByName(w!("/app1/ifd/{ushort=274}"), &mut v).ok()?;
            u16::try_from(&v).ok()
        }).unwrap_or(1);
        let mut src: IWICBitmapSource = frame.into();
        let rot = match orient { 3 => Some(WICBitmapTransformRotate180), 6 => Some(WICBitmapTransformRotate90), 8 => Some(WICBitmapTransformRotate270), _ => None };
        if let Some(r) = rot {
            let fr = wic.CreateBitmapFlipRotator().ok()?;
            fr.Initialize(&src, r).ok()?;
            src = fr.into();
        }
        let (mut w, mut h) = (0u32, 0u32);
        src.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 { return None; }
        let k = (tw as f32 / w as f32).max(th as f32 / h as f32).min(1.0);
        let (nw, nh) = (((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1));
        let sc = wic.CreateBitmapScaler().ok()?;
        sc.Initialize(&src, nw, nh, WICBitmapInterpolationModeHighQualityCubic).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&sc, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let mut px = vec![0u8; (nw * nh * 4) as usize];
        conv.CopyPixels(std::ptr::null(), nw * 4, &mut px).ok()?;
        Some((px, nw, nh))
    }
}

/// Hilo que prepara la foto siguiente cuando se le pide (al azar, sin repetir las últimas).
fn photo_thread() {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let mut dir = String::new();
    let mut all: Vec<String> = Vec::new();
    let mut recent: std::collections::VecDeque<String> = Default::default();
    let mut ver = 0u64;
    loop {
        if !NEXT_ASKED.swap(false, Ordering::Relaxed) { std::thread::sleep(Duration::from_millis(250)); continue; }
        let d = photo_dir();
        if d != dir || all.is_empty() { all = list_photos(&d); dir = d; recent.clear(); }
        if all.is_empty() { *PHOTO.lock().unwrap() = None; continue; }
        // (sin barra, como en el render fuera de pantalla, la escala del 150 %)
        let s = if desk::shot() { 1.5 } else { scale_of(bar_hwnd()).max(1.0) } * desk::ZOOM;
        let (w, h) = size(Kind::Photos);
        // Algo más grande que el widget: el zoom lento no pierde nitidez
        let (tw, th) = ((w * s * 1.15) as u32, (h * s * 1.15) as u32);
        for _ in 0..8 {
            let i = (deskdata_rand() % all.len() as u64) as usize;
            let p = all[i].clone();
            if recent.contains(&p) && all.len() > 12 { continue; }
            if let Some((px, pw, ph)) = load_photo(&p, tw, th) {
                ver += 1;
                recent.push_back(p.clone());
                if recent.len() > 12 { recent.pop_front(); }
                *PHOTO.lock().unwrap() = Some((ver, Arc::new(px), pw, ph, p));
                break;
            }
        }
    }
}

/// Arranca (una vez) el hilo que prepara las fotos y pide la primera.
pub(crate) fn photo_start() { PHOTO_STARTED.call_once(|| { std::thread::spawn(photo_thread); NEXT_ASKED.store(true, Ordering::Relaxed); }); }
/// La última foto preparada por el hilo.
pub(crate) fn photo_latest() -> Option<Photo> { PHOTO.lock().unwrap().clone() }
/// Pide al hilo la foto siguiente.
pub(crate) fn photo_next() { NEXT_ASKED.store(true, Ordering::Relaxed); }
fn deskdata_rand() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish()
}

/// Foto en pantalla: imagen, cuándo apareció y su recorrido lento (zoom y desplazamiento, efecto Ken Burns).
struct Shown { bmp: ID2D1Bitmap, w: f32, h: f32, at: Instant, path: String, z: (f32, f32), pan: (f32, f32, f32, f32) }

/// Un hueco del marco: la foto que se ve y la que se va (fundido).
#[derive(Default)]
struct Slot { cur: Option<Shown>, prev: Option<Shown> }

/// Marco de fotos. Responsive: una foto que llena el marco (Pequeño, Mediano, Alto, Grande), una tira
/// de 4 (Banda), 2 apiladas (Columna) o un collage de 5 (Extragrande). Cada hueco cambia a su ritmo.
#[derive(Default)]
pub struct PhotosW { slots: Vec<Slot>, rt: crate::RtKey, used: u64, want: Option<usize> }

/// Huecos de cada tamaño (DIP del widget) y su radio.
fn photo_slots(l: desk::Lay) -> (Vec<D2D_RECT_F>, f32) {
    let (w, h, g) = (l.w, l.h, 6.0);
    match l.sz {
        desk::Sz::Banda => { let cw = (w - 3.0 * g) / 4.0; ((0..4).map(|i| rect(i as f32 * (cw + g), 0.0, i as f32 * (cw + g) + cw, h)).collect(), 14.0) }
        desk::Sz::Columna => { let ch = (h - g) / 2.0; (vec![rect(0.0, 0.0, w, ch), rect(0.0, ch + g, w, h)], 14.0) }
        desk::Sz::Extragrande => {
            // La grande a la izquierda y cuatro pequeñas en rejilla a la derecha
            let big = h;
            let (x0, cw, ch) = (big + g, (w - big - 2.0 * g) / 2.0, (h - g) / 2.0);
            let mut v = vec![rect(0.0, 0.0, big, h)];
            for r in 0..2 { for c in 0..2 { let (x, y) = (x0 + c as f32 * (cw + g), r as f32 * (ch + g)); v.push(rect(x, y, x + cw, y + ch)); } }
            (v, 14.0)
        }
        _ => (vec![rect(0.0, 0.0, w, h)], 16.0),
    }
}

/// Recorrido de cada foto: acerca o aleja un 8 % y se desplaza un poco en una dirección al azar.
fn ken_burns(seed: u64) -> ((f32, f32), (f32, f32, f32, f32)) {
    let r = |n: u64| ((seed >> (n * 8)) & 0xFF) as f32 / 255.0;
    let zoom_in = r(0) > 0.5;
    let z = if zoom_in { (1.0, 1.08) } else { (1.08, 1.0) };
    let (ax, ay) = (r(1) * 2.0 - 1.0, r(2) * 2.0 - 1.0);
    (z, (ax, ay, -ax * 0.6, -ay * 0.6))
}

impl PhotosW {
    fn draw(ui: &Ui, s: &Shown, r: D2D_RECT_F, rad: f32, show: f32, alpha: f32) {
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let t = (s.at.elapsed().as_secs_f32() / (show + FADE_S)).clamp(0.0, 1.0);
        let z = s.z.0 + (s.z.1 - s.z.0) * t;
        // Cubrir el marco (como «rellenar» en un fondo de pantalla)
        let cover = (w / s.w).max(h / s.h) * z;
        let (dw, dh) = (s.w * cover, s.h * cover);
        let (sx, sy) = ((dw - w) / 2.0, (dh - h) / 2.0);
        let px = s.pan.0 + (s.pan.2 - s.pan.0) * t;
        let py = s.pan.1 + (s.pan.3 - s.pan.1) * t;
        let (ox, oy) = (r.left - sx + px * sx, r.top - sy + py * sy);
        unsafe {
            let props = D2D1_BRUSH_PROPERTIES { opacity: alpha, transform: Matrix3x2 { M11: cover, M12: 0.0, M21: 0.0, M22: cover, M31: ox, M32: oy } };
            if let Ok(br) = ui.rt.CreateBitmapBrush(&s.bmp, None, Some(&props)) {
                ui.rt.FillRoundedRectangle(&rrect(r, rad), &br);
            }
        }
    }
}

/// Lo que dura cada foto en el hueco `i` (los huecos no cambian todos a la vez).
fn show_s(i: usize) -> f32 { SHOW_S + 3.7 * i as f32 }

impl Widget for PhotosW {
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, _t: f32, _dt: f32) {
        photo_start();
        let l = desk::lay(Kind::Photos);
        let (w, h) = (l.w, l.h);
        let (rects, rad) = photo_slots(l);
        if self.rt.fresh(ui.rt) { self.slots.clear(); self.used = 0; self.want = None; }
        self.slots.resize_with(rects.len(), Slot::default);
        // ¿Hay una foto nueva preparada? Entra con un fundido sobre la anterior, en el hueco que la pidió
        // (o en el primero vacío)
        let ready = PHOTO.lock().unwrap().clone();
        if let Some((ver, px, pw, ph, path)) = ready {
            let to = self.want.or_else(|| self.slots.iter().position(|s| s.cur.is_none()));
            if let Some(i) = to.filter(|_| ver != self.used) {
                let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                if let Ok(bmp) = unsafe { ui.rt.CreateBitmap(D2D_SIZE_U { width: pw, height: ph }, Some(px.as_ptr() as _), pw * 4, &bp) } {
                    let (z, pan) = ken_burns(deskdata_rand());
                    let s = &mut self.slots[i];
                    let had = s.cur.is_some();
                    s.prev = s.cur.take();
                    // Al arrancar, sin fundido desde la nada
                    let at = if had { Instant::now() } else { Instant::now() - Duration::from_secs_f32(FADE_S) };
                    s.cur = Some(Shown { bmp, w: pw as f32, h: ph as f32, at, path, z, pan });
                    self.used = ver;
                    self.want = None;
                }
            }
        }
        // Se pide la siguiente para un hueco vacío o para el que ya casi ha cumplido su tiempo
        if self.want.is_none() {
            let due = self.slots.iter().position(|s| s.cur.is_none())
                .or_else(|| (0..self.slots.len()).find(|&i| self.slots[i].cur.as_ref().is_some_and(|c| c.at.elapsed().as_secs_f32() > show_s(i) - 2.0)));
            if let Some(i) = due { self.want = Some(i); NEXT_ASKED.store(true, Ordering::Relaxed); }
        }
        if self.slots.iter().all(|s| s.cur.is_none()) {
            unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(0.0, 0.0, w, h), 16.0), &ui.wa(0.06)); }
            ui.glyph(&ui.f.icon_xl, "\u{EB9F}", w / 2.0 - 20.0, h / 2.0 - 40.0, 40.0, 40.0, &ui.dim);
            let msg = if l.sz == desk::Sz::Pequeno || l.sz == desk::Sz::Alto { i18n::t("Clic derecho: carpeta") } else { i18n::t("Clic derecho para elegir una carpeta de fotos") };
            ui.text_c(&ui.f.sub, msg, w / 2.0, h / 2.0 + 8.0, 18.0, &ui.dim);
            ui.hit(rect(0.0, 0.0, w, h), 1);
            return;
        }
        for (i, r) in rects.iter().enumerate() {
            let s = &mut self.slots[i];
            match &s.cur {
                None => unsafe { ui.rt.FillRoundedRectangle(&rrect(*r, rad), &ui.wa(0.06)); },
                Some(c) => {
                    let k = (c.at.elapsed().as_secs_f32() / FADE_S).clamp(0.0, 1.0);
                    if let Some(p) = &s.prev { if k < 1.0 { Self::draw(ui, p, *r, rad, show_s(i), 1.0); } }
                    Self::draw(ui, c, *r, rad, show_s(i), popup::ease_out(k));
                    if k >= 1.0 { s.prev = None; }
                }
            }
            // Borde fino de cristal
            unsafe { ui.rt.DrawRoundedRectangle(&rrect(rect(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), rad - 0.5), &ui.wa(0.14), 1.0, None); }
            ui.hit(*r, 1 + i as u32);
        }
    }
    fn click(&mut self, id: Option<u32>, right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        if right { return Some(Box::new(pick_folder)); }
        // Clic: abre la foto de ese hueco
        let i = id?.checked_sub(1)? as usize;
        let p = self.slots.get(i)?.cur.as_ref()?.path.clone();
        Some(Box::new(move || sys::open(&p)))
    }
    fn tick(&mut self) -> bool { true }
    fn animating(&self, _t: f32) -> bool { self.slots.iter().any(|s| s.cur.is_some()) }
    fn frame_ms(&self) -> u32 { 33 }
    fn layouts(&self) -> bool { true }
}

/// Elegir la carpeta de las fotos (se guarda en widgets.ini como `fotos_carpeta`).
pub(crate) fn pick_folder() {
    std::thread::spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(dlg) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) else { return };
        let _ = dlg.SetOptions(FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
        let _ = dlg.SetTitle(&HSTRING::from(i18n::t("Carpeta del marco de fotos")));
        if dlg.Show(None).is_err() { return; }
        let Ok(item) = dlg.GetResult() else { return };
        let Ok(p) = item.GetDisplayName(SIGDN_FILESYSPATH) else { return };
        let path = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        if path.is_empty() || path.contains(['\n', '\r']) { return; }
        desk::set_ini_value("fotos_carpeta", &path);
        NEXT_ASKED.store(true, Ordering::Relaxed);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huecos_del_marco() {
        use desk::{Lay, Sz};
        for sz in Sz::ALL {
            let (w, h) = sz.dims();
            let (v, _) = photo_slots(Lay { w, h, sz });
            let n = match sz { Sz::Banda => 4, Sz::Columna => 2, Sz::Extragrande => 5, _ => 1 };
            assert_eq!(v.len(), n, "{sz:?}");
            // Dentro del widget, con tamaño y sin pisarse
            for (i, a) in v.iter().enumerate() {
                assert!(a.left >= 0.0 && a.top >= 0.0 && a.right <= w + 0.01 && a.bottom <= h + 0.01 && a.right - a.left > 100.0 && a.bottom - a.top > 100.0, "{sz:?} {i}");
                for b in &v[i + 1..] { assert!(a.right <= b.left || b.right <= a.left || a.bottom <= b.top || b.bottom <= a.top, "{sz:?}"); }
            }
        }
    }

    #[test]
    fn recorrido_de_las_fotos() {
        for seed in [0u64, 1, 0xFFFF_FFFF, 0x1234_5678_9ABC] {
            let (z, pan) = ken_burns(seed);
            // Siempre entre el 100 % y el 108 %, y el desplazamiento dentro de lo que sobra al ampliar
            assert!(z.0 >= 1.0 && z.1 >= 1.0 && z.0 <= 1.08 && z.1 <= 1.08);
            for v in [pan.0, pan.1, pan.2, pan.3] { assert!((-1.0..=1.0).contains(&v)); }
        }
    }
}
