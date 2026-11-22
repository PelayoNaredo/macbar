//! Widgets nuevos del escritorio: visualizador de audio y marco de fotos.

use super::*;
use desk::{size, DFonts, Kind};
use deskw::{header, Widget};
use popup::Ui;

const PAD: f32 = 20.0;

/// Color de la carátula que suena (el mismo que tiñe la isla), o el acento si no hay música.
fn tint() -> [f32; 3] {
    let t = GFX.with(|g| g.try_borrow().ok().and_then(|g| g.as_ref().map(|g| g.tint_now)));
    match t {
        Some(t) if MEDIA.lock().unwrap().is_some() && t != [1.0; 3] => t,
        _ => { let a = crate::profiles::accent(1.0); [a.r, a.g, a.b] }
    }
}

// ---------------------------------------------------------------- visualizador de audio
/// Barras del espectro de lo que suena, con el color de la carátula y su reflejo debajo.
pub struct VisualW { levels: [f32; audioviz::BANDS], alive: f32 }

impl Default for VisualW { fn default() -> Self { VisualW { levels: [0.0; audioviz::BANDS], alive: 0.0 } } }

impl Widget for VisualW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, _t: f32, dt: f32) {
        let (w, h) = size(Kind::Visual);
        let m = MEDIA.lock().unwrap().clone();
        let right = m.as_ref().map(|m| trunc(&m.title, 30)).unwrap_or_default();
        header(ui, d, i18n::t("Sonido"), &right, w);
        let src = *audioviz::LEVELS.lock().unwrap();
        // Lo que llega del hilo se suaviza aún un poco al pintar (sin saltos entre fotogramas)
        let k = 1.0 - (-dt / 0.05).exp();
        let mut energy = 0.0;
        for i in 0..audioviz::BANDS { self.levels[i] += (src[i] - self.levels[i]) * k; energy += self.levels[i]; }
        let on = energy / audioviz::BANDS as f32 > 0.02;
        self.alive += ((if on { 1.0 } else { 0.0 }) - self.alive) * (1.0 - (-dt / 0.3).exp());
        let tn = tint();
        let (x0, x1, base) = (PAD, w - PAD, h - 48.0);
        let n = audioviz::BANDS as f32;
        let gap = 3.0;
        let bw = ((x1 - x0) - gap * (n - 1.0)) / n;
        let maxh = base - 62.0;
        unsafe {
            for (i, v) in self.levels.iter().enumerate() {
                let x = x0 + i as f32 * (bw + gap);
                // En silencio quedan puntos que respiran despacio, como un ecualizador dormido
                let idle = 2.5 + 1.5 * (0.5 + 0.5 * (ui_time() * 1.6 + i as f32 * 0.35).sin());
                let bh = (v * maxh).max(idle * (1.0 - self.alive) + 3.0 * self.alive);
                let top = base - bh;
                let fill = ui.brush(rgba(tn[0], tn[1], tn[2], 0.55 + 0.45 * v));
                ui.rt.FillRoundedRectangle(&rrect(rect(x, top, x + bw, base), bw / 2.0), &fill);
                // Reflejo tenue bajo la línea base
                let refl = ui.brush(rgba(tn[0], tn[1], tn[2], 0.12 * v));
                ui.rt.FillRoundedRectangle(&rrect(rect(x, base + 4.0, x + bw, base + 4.0 + bh * 0.28), bw / 2.0), &refl);
            }
        }
        if m.is_none() && !on {
            ui.text_c(&ui.f.sub, i18n::t("Pon algo de música"), w / 2.0, h - 36.0, 18.0, &ui.dim);
        }
    }
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
fn ui_time() -> f32 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_secs_f32()
}

// ---------------------------------------------------------------- marco de fotos
/// Foto lista para pintar: versión, píxeles BGRA premultiplicados, ancho, alto y ruta.
type Photo = (u64, Arc<Vec<u8>>, u32, u32, String);
static PHOTO: Mutex<Option<Photo>> = Mutex::new(None);
static NEXT_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PHOTO_STARTED: std::sync::Once = std::sync::Once::new();
const SHOW_S: f32 = 14.0;
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
        let s = scale_of(bar_hwnd()) * desk::ZOOM;
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

fn deskdata_rand() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish()
}

/// Foto en pantalla: imagen, cuándo apareció y su recorrido lento (zoom y desplazamiento, efecto Ken Burns).
struct Shown { ver: u64, bmp: ID2D1Bitmap, w: f32, h: f32, at: Instant, path: String, z: (f32, f32), pan: (f32, f32, f32, f32) }

#[derive(Default)]
pub struct PhotosW { cur: Option<Shown>, prev: Option<Shown>, rt: crate::RtKey, asked_at: Option<Instant> }

/// Recorrido de cada foto: acerca o aleja un 8 % y se desplaza un poco en una dirección al azar.
fn ken_burns(seed: u64) -> ((f32, f32), (f32, f32, f32, f32)) {
    let r = |n: u64| ((seed >> (n * 8)) & 0xFF) as f32 / 255.0;
    let zoom_in = r(0) > 0.5;
    let z = if zoom_in { (1.0, 1.08) } else { (1.08, 1.0) };
    let (ax, ay) = (r(1) * 2.0 - 1.0, r(2) * 2.0 - 1.0);
    (z, (ax, ay, -ax * 0.6, -ay * 0.6))
}

impl PhotosW {
    fn draw(ui: &Ui, s: &Shown, w: f32, h: f32, alpha: f32) {
        let t = (s.at.elapsed().as_secs_f32() / (SHOW_S + FADE_S)).clamp(0.0, 1.0);
        let z = s.z.0 + (s.z.1 - s.z.0) * t;
        // Cubrir el marco (como «rellenar» en un fondo de pantalla)
        let cover = (w / s.w).max(h / s.h) * z;
        let (dw, dh) = (s.w * cover, s.h * cover);
        let (sx, sy) = ((dw - w) / 2.0, (dh - h) / 2.0);
        let px = s.pan.0 + (s.pan.2 - s.pan.0) * t;
        let py = s.pan.1 + (s.pan.3 - s.pan.1) * t;
        let (ox, oy) = (-sx + px * sx, -sy + py * sy);
        unsafe {
            let props = D2D1_BRUSH_PROPERTIES { opacity: alpha, transform: Matrix3x2 { M11: cover, M12: 0.0, M21: 0.0, M22: cover, M31: ox, M32: oy } };
            if let Ok(br) = ui.rt.CreateBitmapBrush(&s.bmp, None, Some(&props)) {
                ui.rt.FillRoundedRectangle(&rrect(rect(0.0, 0.0, w, h), 16.0), &br);
            }
        }
    }
}

impl Widget for PhotosW {
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, _t: f32, _dt: f32) {
        PHOTO_STARTED.call_once(|| { std::thread::spawn(photo_thread); NEXT_ASKED.store(true, Ordering::Relaxed); });
        let (w, h) = size(Kind::Photos);
        if self.rt.fresh(ui.rt) { self.cur = None; self.prev = None; }
        // ¿Hay una foto nueva preparada? Entra con un fundido sobre la anterior
        let ready = PHOTO.lock().unwrap().clone();
        if let Some((ver, px, pw, ph, path)) = ready {
            if self.cur.as_ref().is_none_or(|c| c.ver != ver) {
                let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                if let Ok(bmp) = unsafe { ui.rt.CreateBitmap(D2D_SIZE_U { width: pw, height: ph }, Some(px.as_ptr() as _), pw * 4, &bp) } {
                    let (z, pan) = ken_burns(deskdata_rand());
                    let had = self.cur.is_some();
                    self.prev = self.cur.take();
                    // Al arrancar, sin fundido desde la nada
                    let at = if had { Instant::now() } else { Instant::now() - Duration::from_secs_f32(FADE_S) };
                    self.cur = Some(Shown { ver, bmp, w: pw as f32, h: ph as f32, at, path, z, pan });
                    self.asked_at = None;
                }
            }
        }
        match &self.cur {
            None => {
                unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(0.0, 0.0, w, h), 16.0), &ui.wa(0.06)); }
                ui.glyph(&ui.f.icon_xl, "\u{EB9F}", w / 2.0 - 20.0, h / 2.0 - 40.0, 40.0, 40.0, &ui.dim);
                ui.text_c(&ui.f.sub, i18n::t("Clic derecho para elegir una carpeta de fotos"), w / 2.0, h / 2.0 + 8.0, 18.0, &ui.dim);
            }
            Some(c) => {
                let k = (c.at.elapsed().as_secs_f32() / FADE_S).clamp(0.0, 1.0);
                if let Some(p) = &self.prev { if k < 1.0 { Self::draw(ui, p, w, h, 1.0); } }
                Self::draw(ui, c, w, h, popup::ease_out(k));
                if k >= 1.0 { self.prev = None; }
                // Pide la siguiente un poco antes de que toque cambiar
                if c.at.elapsed().as_secs_f32() > SHOW_S - 2.0 && self.asked_at.is_none() { NEXT_ASKED.store(true, Ordering::Relaxed); self.asked_at = Some(Instant::now()); }
                // Borde fino de cristal
                unsafe { ui.rt.DrawRoundedRectangle(&rrect(rect(0.5, 0.5, w - 0.5, h - 0.5), 15.5), &ui.wa(0.14), 1.0, None); }
            }
        }
        ui.hit(rect(0.0, 0.0, w, h), 1);
    }
    fn click(&mut self, id: Option<u32>, right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        if right { return Some(Box::new(pick_folder)); }
        // Clic: abre la foto que se ve
        let p = self.cur.as_ref().map(|c| c.path.clone()).filter(|_| id.is_some())?;
        Some(Box::new(move || sys::open(&p)))
    }
    fn tick(&mut self) -> bool { true }
    fn animating(&self, _t: f32) -> bool { self.cur.is_some() }
    fn frame_ms(&self) -> u32 { 33 }
}

/// Elegir la carpeta de las fotos (se guarda en widgets.ini como `fotos_carpeta`).
fn pick_folder() {
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
    fn recorrido_de_las_fotos() {
        for seed in [0u64, 1, 0xFFFF_FFFF, 0x1234_5678_9ABC] {
            let (z, pan) = ken_burns(seed);
            // Siempre entre el 100 % y el 108 %, y el desplazamiento dentro de lo que sobra al ampliar
            assert!(z.0 >= 1.0 && z.1 >= 1.0 && z.0 <= 1.08 && z.1 <= 1.08);
            for v in [pan.0, pan.1, pan.2, pan.3] { assert!((-1.0..=1.0).contains(&v)); }
        }
    }
}
