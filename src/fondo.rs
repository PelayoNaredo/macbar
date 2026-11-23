//! El fondo de pantalla real que queda debajo de cada widget. Los widgets viven a nivel de escritorio,
//! así que lo que hay detrás de ellos es el fondo de pantalla: aquí se lee (la ruta que da Windows y su
//! modo de ajuste: rellenar, ajustar, expandir, centrar, mosaico o extender), se recorta el trozo exacto
//! bajo la ventana de cada widget y se deja como imagen de Direct2D. Las piezas de cristal de los
//! diseños deforman ese recorte, así el vidrio dobla lo que de verdad tiene detrás.
//!
//! El recorte se rehace solo si se mueve el widget, cambia el monitor o cambia el fondo (se comprueba
//! la ruta, la fecha del archivo y el modo cada pocos segundos). La imagen decodificada se suelta al
//! rato de no usarse.

use super::*;
use popup::Ui;
use std::collections::HashMap;
use std::rc::Rc;

/// Cada cuánto se mira si ha cambiado el fondo (ruta, fecha, modo de ajuste o color).
const CHECK_S: f32 = 3.0;
/// La imagen decodificada se suelta tras este rato sin recortar nada nuevo.
const IDLE_S: f32 = 30.0;
/// Lado mayor con el que se guarda la imagen decodificada (de sobra para lo que se ve tras un cristal).
const MAX_SIDE: u32 = 2560;

/// Modo de ajuste del fondo (Configuración > Personalización > Fondo).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Fit { Fill, Fit, Stretch, Center, Tile, Span }

/// Imagen BGRA premultiplicada.
pub(crate) struct Img { pub w: u32, pub h: u32, pub px: Vec<u8> }

#[derive(Clone, PartialEq)]
struct Cfg { path: String, fit: Fit, bg: [u8; 3] }

struct State { cfg: Cfg, stamp: u64, checked: Instant, img: Option<Rc<Img>>, used: Instant }

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// Recorte ya subido a la GPU por ventana: (huella, lienzo, imagen)
    static CROPS: RefCell<HashMap<isize, (u64, windows::core::IUnknown, ID2D1Bitmap)>> = RefCell::new(HashMap::new());
    /// Render fuera de pantalla (`desk::shot`): sitio de la ventana imaginaria y monitor, en píxeles
    static SHOT_WIN: std::cell::Cell<Option<(R, R)>> = const { std::cell::Cell::new(None) };
    /// Render fuera de pantalla con un fondo liso en vez del de pantalla (p. ej. claro, para la tinta oscura)
    static SHOT_PLAIN: std::cell::Cell<Option<[u8; 3]>> = const { std::cell::Cell::new(None) };
}

/// Para el render fuera de pantalla: fondo liso de ese color en vez del fondo de pantalla.
pub(crate) fn set_shot_plain(c: Option<[u8; 3]>) { SHOT_PLAIN.with(|p| p.set(c)); }

/// Para el render fuera de pantalla: el widget que se pinta «está» en `win` dentro del monitor `mon`.
pub(crate) fn set_shot_window(v: Option<(R, R)>) { SHOT_WIN.with(|c| c.set(v)); }

fn hash(parts: &[u64]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts { h ^= *p; h = h.wrapping_mul(0x0100_0000_01b3); h ^= h >> 29; }
    h
}

fn hash_str(s: &str) -> u64 { hash(&s.bytes().map(|b| b as u64).collect::<Vec<_>>()) }

/// Valor de texto de HKEY_CURRENT_USER.
fn reg_hkcu(key: &str, val: &str) -> Option<String> {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut buf = vec![0u16; 512];
        let mut size = (buf.len() * 2) as u32;
        RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(val), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut size)).ok().ok()?;
        Some(String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]).trim().to_string())
    }
}

/// Modo de ajuste a partir de WallpaperStyle y TileWallpaper del registro.
pub(crate) fn fit_of(style: &str, tile: &str) -> Fit {
    if tile.trim() == "1" { return Fit::Tile; }
    match style.trim() { "6" => Fit::Fit, "2" => Fit::Stretch, "0" => Fit::Center, "22" => Fit::Span, _ => Fit::Fill }
}

/// «r g b» (Panel de control\Colores\Background) → color; negro si no se entiende.
pub(crate) fn color_of(s: &str) -> [u8; 3] {
    let v: Vec<u8> = s.split_whitespace().filter_map(|p| p.parse().ok()).collect();
    if v.len() == 3 { [v[0], v[1], v[2]] } else { [0, 0, 0] }
}

/// Lo que Windows dice del fondo ahora mismo.
fn read_cfg() -> Cfg {
    let mut buf = [0u16; 520];
    unsafe { let _ = SystemParametersInfoW(SPI_GETDESKWALLPAPER, buf.len() as u32, Some(buf.as_mut_ptr() as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
    let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    let mut path = String::from_utf16_lossy(&buf[..n]).trim().to_string();
    // Si el original ya no existe, Windows sigue enseñando su copia convertida
    if !path.is_empty() && !std::path::Path::new(&path).exists() {
        let tr = std::env::var("APPDATA").unwrap_or_default() + "\\Microsoft\\Windows\\Themes\\TranscodedWallpaper";
        path = if std::path::Path::new(&tr).exists() { tr } else { String::new() };
    }
    let d = "Control Panel\\Desktop";
    let fit = fit_of(&reg_hkcu(d, "WallpaperStyle").unwrap_or_default(), &reg_hkcu(d, "TileWallpaper").unwrap_or_default());
    let bg = color_of(&reg_hkcu("Control Panel\\Colors", "Background").unwrap_or_default());
    Cfg { path, fit, bg }
}

/// Huella del fondo: cambia con la ruta, la fecha y el tamaño del archivo, el modo y el color.
fn stamp_of(c: &Cfg) -> u64 {
    let (mt, len) = std::fs::metadata(&c.path).ok().map(|m| (m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0), m.len())).unwrap_or((0, 0));
    hash(&[hash_str(&c.path), mt, len, c.fit as u64, u32::from_le_bytes([c.bg[0], c.bg[1], c.bg[2], 0]) as u64])
}

/// Decodifica el fondo (reducido si es enorme) a BGRA premultiplicado.
fn decode(path: &str) -> Option<Img> {
    if path.is_empty() { return None; }
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let dec = wic.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnLoad).ok()?;
        let src: IWICBitmapSource = dec.GetFrame(0).ok()?.into();
        let (mut w, mut h) = (0u32, 0u32);
        src.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 { return None; }
        let k = (MAX_SIDE as f32 / w.max(h) as f32).min(1.0);
        let (nw, nh) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
        let sc = wic.CreateBitmapScaler().ok()?;
        sc.Initialize(&src, nw, nh, WICBitmapInterpolationModeHighQualityCubic).ok()?;
        let cv = wic.CreateFormatConverter().ok()?;
        cv.Initialize(&sc, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let mut px = vec![0u8; (nw * nh * 4) as usize];
        cv.CopyPixels(std::ptr::null(), nw * 4, &mut px).ok()?;
        Some(Img { w: nw, h: nh, px })
    }
}

/// Rectángulo en píxeles de pantalla: (x, y, ancho, alto).
pub(crate) type R = (i32, i32, i32, i32);

/// Dónde cae la imagen en la pantalla: (escala x, escala y, origen x, origen y) para un área (el monitor,
/// o todo el escritorio virtual si es «extender»).
pub(crate) fn placement(iw: u32, ih: u32, fit: Fit, area: R) -> (f32, f32, f32, f32) {
    let (ax, ay, aw, ah) = (area.0 as f32, area.1 as f32, area.2 as f32, area.3 as f32);
    let (iw, ih) = (iw as f32, ih as f32);
    let (sx, sy) = match fit {
        Fit::Fill | Fit::Span => { let s = (aw / iw).max(ah / ih); (s, s) }
        Fit::Fit => { let s = (aw / iw).min(ah / ih); (s, s) }
        Fit::Stretch => (aw / iw, ah / ih),
        Fit::Center | Fit::Tile => (1.0, 1.0),
    };
    if fit == Fit::Tile { return (1.0, 1.0, ax, ay); }
    (sx, sy, ax + (aw - iw * sx) / 2.0, ay + (ah - ih * sy) / 2.0)
}

/// El trozo de fondo que queda bajo `win` (BGRA premultiplicado, opaco). Sin imagen, el color de fondo.
pub(crate) fn crop(img: Option<&Img>, fit: Fit, bg: [u8; 3], area: R, win: R) -> Vec<u8> {
    let (ww, wh) = (win.2.max(1) as usize, win.3.max(1) as usize);
    let mut out = vec![0u8; ww * wh * 4];
    let bgpx = [bg[2], bg[1], bg[0], 255];
    let Some(img) = img.filter(|i| i.w > 0 && i.h > 0) else {
        for p in out.chunks_exact_mut(4) { p.copy_from_slice(&bgpx); }
        return out;
    };
    let (sx, sy, ox, oy) = placement(img.w, img.h, fit, area);
    let (iw, ih) = (img.w as i64, img.h as i64);
    let tile = fit == Fit::Tile;
    let at = |x: i64, y: i64| -> [f32; 4] {
        let (x, y) = if tile { (x.rem_euclid(iw), y.rem_euclid(ih)) } else { (x.clamp(0, iw - 1), y.clamp(0, ih - 1)) };
        let o = ((y * iw + x) * 4) as usize;
        [img.px[o] as f32, img.px[o + 1] as f32, img.px[o + 2] as f32, img.px[o + 3] as f32]
    };
    for j in 0..wh {
        let sy_ = win.1 as f32 + j as f32 + 0.5;
        let v = (sy_ - oy) / sy - 0.5;
        for i in 0..ww {
            let sx_ = win.0 as f32 + i as f32 + 0.5;
            let u = (sx_ - ox) / sx - 0.5;
            let o = (j * ww + i) * 4;
            // Fuera de la imagen (ajustar o centrar): el color de fondo de Windows
            if !tile && (u < -0.5 || v < -0.5 || u > img.w as f32 - 0.5 || v > img.h as f32 - 0.5) {
                out[o..o + 4].copy_from_slice(&bgpx);
                continue;
            }
            let (x0, y0) = (u.floor(), v.floor());
            let (fx, fy) = (u - x0, v - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
            for k in 0..3 {
                let top = a[k] + (b[k] - a[k]) * fx;
                let bot = c[k] + (d[k] - c[k]) * fx;
                out[o + k] = (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8;
            }
            out[o + 3] = 255;
        }
    }
    out
}

/// Fondo actual (comprobado cada pocos segundos) y su huella.
fn current() -> (Cfg, u64) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let fresh = s.as_ref().is_some_and(|st| st.checked.elapsed().as_secs_f32() < CHECK_S);
        if !fresh {
            let cfg = read_cfg();
            let stamp = stamp_of(&cfg);
            match s.as_mut() {
                Some(st) if st.stamp == stamp => { st.checked = Instant::now(); }
                _ => { *s = Some(State { cfg, stamp, checked: Instant::now(), img: None, used: Instant::now() }); }
            }
        }
        let st = s.as_mut().unwrap();
        // Sin usar un rato: la imagen grande se suelta (los recortes ya están en la GPU)
        if st.img.is_some() && st.used.elapsed().as_secs_f32() > IDLE_S { st.img = None; }
        (st.cfg.clone(), st.stamp)
    })
}

fn image(path: &str) -> Option<Rc<Img>> {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let st = s.as_mut()?;
        st.used = Instant::now();
        if st.img.is_none() { st.img = decode(path).map(Rc::new); }
        st.img.clone()
    })
}

/// Trozo de fondo de pantalla bajo el widget que se está pintando, como imagen de Direct2D del mismo
/// tamaño que la ventana (en DIP: todo el widget), y su huella (cambia si cambia el recorte). None si
/// no se está pintando un widget.
pub fn under(ui: &Ui) -> Option<(ID2D1Bitmap, u64)> {
    use windows::core::Interface;
    let (cfg, stamp) = current();
    let span = || unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
    let (win, area, k): (R, R, isize) = if let Some((win, mon)) = SHOT_WIN.with(|c| c.get()) {
        // Render fuera de pantalla: una ventana imaginaria en ese sitio del monitor
        (win, if cfg.fit == Fit::Span { span() } else { mon }, -1)
    } else {
        let hwnd = desk::current_hwnd()?;
        let mut wr = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut wr).ok()?; }
        let win: R = (wr.left, wr.top, wr.right - wr.left, wr.bottom - wr.top);
        let area: R = if cfg.fit == Fit::Span { span() } else {
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            unsafe { let _ = GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi); }
            let m = mi.rcMonitor;
            (m.left, m.top, m.right - m.left, m.bottom - m.top)
        };
        (win, area, hwnd.0 as isize)
    };
    if win.2 <= 0 || win.3 <= 0 { return None; }
    let key = hash(&[stamp ^ SHOT_PLAIN.with(|p| p.get()).map(|c| 1 + c[0] as u64).unwrap_or(0), win.0 as u64, win.1 as u64, win.2 as u64, win.3 as u64, area.0 as u64, area.1 as u64, area.2 as u64, area.3 as u64]);
    let unk = ui.rt.cast::<windows::core::IUnknown>().ok()?;
    if let Some(b) = CROPS.with(|c| c.borrow().get(&k).filter(|e| e.0 == key && e.1 == unk).map(|e| e.2.clone())) { return Some((b, key)); }
    let plain = if k == -1 { SHOT_PLAIN.with(|p| p.get()) } else { None };
    let img = if plain.is_some() { None } else { image(&cfg.path) };
    let px = crop(img.as_deref(), cfg.fit, plain.unwrap_or(cfg.bg), area, win);
    let s = ui.s.max(0.5);
    let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0 * s, dpiY: 96.0 * s };
    let bmp = unsafe { ui.rt.CreateBitmap(D2D_SIZE_U { width: win.2 as u32, height: win.3 as u32 }, Some(px.as_ptr() as _), win.2 as u32 * 4, &bp).ok()? };
    CROPS.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 32 { c.clear(); }
        c.insert(k, (key, unk, bmp.clone()));
    });
    Some((bmp, key))
}

/// Pinta el fondo bajo el widget como fondo opaco (solo en las capturas de prueba: MACBAR_DEMO con
/// MACBAR_WIDGETS, donde el widget sale encima de otras ventanas y así la captura es la del escritorio).
pub fn paint_test_backdrop(ui: &Ui, w: f32, h: f32) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| sys::demo() && std::env::var_os("MACBAR_WIDGETS").is_some()) { return; }
    if let Some((b, _)) = under(ui) {
        unsafe { ui.rt.DrawBitmap(&b, Some(&rect(0.0, 0.0, w, h)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modo_de_ajuste_y_color() {
        assert_eq!(fit_of("10", "0"), Fit::Fill);
        assert_eq!(fit_of("6", "0"), Fit::Fit);
        assert_eq!(fit_of("2", "0"), Fit::Stretch);
        assert_eq!(fit_of("0", "0"), Fit::Center);
        assert_eq!(fit_of("0", "1"), Fit::Tile);
        assert_eq!(fit_of("22", "0"), Fit::Span);
        assert_eq!(fit_of("", ""), Fit::Fill);
        assert_eq!(color_of("10 20 30"), [10, 20, 30]);
        assert_eq!(color_of("basura"), [0, 0, 0]);
    }

    #[test]
    fn colocacion_del_fondo() {
        // Imagen 2:1 en monitor 16:9 rellenando: escala por alto y se recorta por los lados
        let (sx, sy, ox, oy) = placement(2000, 1000, Fit::Fill, (0, 0, 1920, 1080));
        assert!((sx - 1.08).abs() < 1e-4 && (sy - 1.08).abs() < 1e-4);
        assert!(ox < 0.0 && oy.abs() < 1e-3);
        // Ajustar: escala por ancho y quedan bandas arriba y abajo
        let (s, _, ox, oy) = placement(2000, 1000, Fit::Fit, (0, 0, 1920, 1080));
        assert!((s - 0.96).abs() < 1e-4 && ox.abs() < 1e-3 && oy > 0.0);
        // Segundo monitor a la derecha: el origen va con el monitor
        let (_, _, ox, _) = placement(1920, 1080, Fit::Fill, (1920, 0, 1920, 1080));
        assert!((ox - 1920.0).abs() < 1e-3);
    }

    #[test]
    fn recorte_del_fondo() {
        // Imagen 2×2: rojo, verde / azul, blanco; centrada en un área de 10×10 con fondo gris
        let px = vec![0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255];
        let img = Img { w: 2, h: 2, px };
        let out = crop(Some(&img), Fit::Center, [9, 9, 9], (0, 0, 10, 10), (0, 0, 10, 10));
        assert_eq!(out.len(), 400);
        // Esquina: color de fondo; píxel (4,4): rojo puro (arriba a la izquierda de la imagen)
        assert_eq!(&out[0..4], &[9, 9, 9, 255]);
        let o = (4 * 10 + 4) * 4;
        assert_eq!(&out[o..o + 4], &[0, 0, 255, 255]);
        // Mosaico: se repite
        let t = crop(Some(&img), Fit::Tile, [0, 0, 0], (0, 0, 10, 10), (2, 2, 2, 2));
        assert_eq!(&t[0..4], &[0, 0, 255, 255]);
        // Sin imagen: todo del color de fondo (en BGRA)
        let c = crop(None, Fit::Fill, [10, 20, 30], (0, 0, 4, 4), (0, 0, 2, 2));
        assert_eq!(&c[0..4], &[30, 20, 10, 255]);
    }
}
