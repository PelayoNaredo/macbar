//! Render fuera de pantalla de los widgets, para revisar cada combinación sin ventanas ni pantalla
//! (funciona con la sesión bloqueada). `MACBAR_WIDGETSHOT=<carpeta>` (con `MACBAR_DEMO=1`) pinta cada
//! widget × diseño × tamaño admitido sobre el trozo real de fondo de pantalla que tendría detrás, deja
//! correr las animaciones de entrada y guarda `<carpeta>\<widget>_<diseño>_<tamaño>.png`, una hoja por
//! widget (`hoja_<widget>.png`) y un resumen con un tamaño de cada diseño (`resumen.png`). Sale al acabar.
//!
//! Pasadas (variables opcionales):
//! - `MACBAR_WIDGETSHOT_TINTA=oscura`: tinta oscura (la de los fondos claros; va sobre un fondo claro
//!   liso salvo con `MACBAR_WIDGETSHOT_FONDO=real`; `=claro` pone ese fondo también con tinta clara).
//! - `MACBAR_WIDGETSHOT_IDIOMA=en`: en inglés.
//! - `MACBAR_WIDGETSHOT_ESTILO=tarjeta`: estilo tarjeta en vez de flotante.
//! - `MACBAR_WIDGETSHOT_VACIO=1`: estados vacíos (nada sonando, sin fechas, sin proyectos, sin
//!   previsión, sin red en Bolsa y Noticias).
//! - `MACBAR_WIDGETSHOT_SOLO=tiempo,musica`: solo esos widgets.
//! - `MACBAR_REDUCED=1`: «Reducir movimiento».
//!
//! Nunca escribe widgets.ini (ni lo lee: cada widget sale con lo de por defecto), no crea ventanas y es
//! un proceso aparte del macbar que esté abierto (se atiende antes del candado de instancia única).

use super::*;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP;

/// Escala del monitor con la que se pinta (150 %, la del monitor de referencia).
const DPI_SCALE: f32 = 1.5;
/// Segundos de animación que se dejan correr antes de guardar (las entradas acaban antes de 1,5 s).
const T_END: f32 = 2.4;
const DT: f32 = 1.0 / 30.0;

/// Una captura ya hecha: widget, diseño, tamaño, medidas y píxeles BGRA premultiplicados (opacos).
struct Shot { k: Kind, v: usize, s: Sz, w: u32, h: u32, px: Vec<u8> }

fn slug(s: &str) -> String {
    s.to_lowercase().chars().map(|c| match c { 'á' => 'a', 'é' => 'e', 'í' => 'i', 'ó' => 'o', 'ú' => 'u', 'ñ' => 'n', c => c }).filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// Nombre del fichero de una combinación (sin carpeta).
fn file_name(k: Kind, v: usize, s: Sz) -> String { format!("{}_{}_{}.png", k.key(), slug(variants(k)[v]), s.key()) }

/// Ejecuta el render y devuelve el código de salida del proceso.
pub fn run(dir: &str) -> i32 {
    SHOT.store(true, Ordering::Relaxed);
    let _ = std::fs::create_dir_all(dir);
    let env = |k: &str| std::env::var(k).unwrap_or_default().to_lowercase();
    unsafe { let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); }
    profiles::load_accent();
    i18n::force(env("MACBAR_WIDGETSHOT_IDIOMA") == "en");
    let fl = env("MACBAR_WIDGETSHOT_ESTILO") != "tarjeta";
    FLOAT.store(fl as i8, Ordering::Relaxed);
    let ink = if env("MACBAR_WIDGETSHOT_TINTA") == "oscura" { 1.0 } else { 0.0 };
    INK_MODE.store(if ink > 0.5 { 1 } else { 2 }, Ordering::Relaxed);
    // La tinta oscura es la de los fondos claros: con ella, fondo claro liso (o MACBAR_WIDGETSHOT_FONDO=real)
    let plain = match env("MACBAR_WIDGETSHOT_FONDO").as_str() { "real" => false, "claro" => true, _ => ink > 0.5 };
    if plain { fondo::set_shot_plain(Some([222, 226, 232])); }
    let solo: Vec<String> = env("MACBAR_WIDGETSHOT_SOLO").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    // Tamaños y diseños solo en memoria (nunca desde ni hacia widgets.ini)
    *SZ.lock().unwrap() = Some(HashMap::new());
    *VARIANT.lock().unwrap() = Some(HashMap::new());
    seed();

    let Some((dc, _dev)) = device() else { log(dir, "sin dispositivo de Direct2D"); return 2 };
    let rt: ID2D1RenderTarget = dc.cast().unwrap();
    let dw: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap() };
    let (f, d) = fonts(&dw);
    let px_s = DPI_SCALE * ZOOM;
    let mon = primary_monitor();
    let kinds: Vec<Kind> = ALL.iter().copied().filter(|k| solo.is_empty() || solo.iter().any(|s| s == k.key())).collect();

    // Primera vuelta: cada widget se crea y se pinta una vez, así arrancan sus datos (fotos, móvil, bolsa,
    // noticias, cola…) y les da tiempo a llegar antes de las capturas de verdad
    for k in &kinds {
        if let Some((v, s)) = (0..variants(*k).len()).find_map(|v| sizes(*k, v).first().map(|s| (v, *s))) {
            let _ = render(&dc, &rt, &dw, &f, &d, *k, v, s, px_s, mon, fl, ink, 0.2);
        }
    }
    std::thread::sleep(Duration::from_millis(3000));

    let mut shots: Vec<Shot> = Vec::new();
    let mut fails = Vec::new();
    for k in &kinds {
        for v in 0..variants(*k).len() {
            for s in sizes(*k, v) {
                match render(&dc, &rt, &dw, &f, &d, *k, v, *s, px_s, mon, fl, ink, T_END) {
                    Some(sh) => {
                        if save_png(&format!("{dir}\\{}", file_name(*k, v, *s)), sh.w, sh.h, &sh.px).is_none() { fails.push(file_name(*k, v, *s)); }
                        shots.push(sh);
                    }
                    None => fails.push(file_name(*k, v, *s)),
                }
            }
        }
    }
    // Hojas: una por widget (cada diseño en una fila) y el resumen
    for k in &kinds {
        let mine: Vec<&Shot> = shots.iter().filter(|s| s.k == *k).collect();
        sheet(&dc, &rt, &dw, &mine, 1.0, &format!("{dir}\\hoja_{}.png", k.key()));
    }
    let rep: Vec<&Shot> = shots.iter().filter(|sh| sh.s == nearest(legacy_size(sh.k, sh.v), sizes(sh.k, sh.v))).collect();
    sheet(&dc, &rt, &dw, &rep, 0.6, &format!("{dir}\\resumen.png"));
    log(dir, &format!("{} capturas; fallos: {}", shots.len(), if fails.is_empty() { "ninguno".to_string() } else { fails.join(", ") }));
    if fails.is_empty() { 0 } else { 1 }
}

fn log(dir: &str, s: &str) { let _ = std::fs::write(format!("{dir}\\widgetshot.txt"), format!("{s}\n")); }

/// Datos de la demo que en macbar llegan por otros hilos de la barra (canción y previsión del tiempo).
fn seed() {
    deskdata::start();
    crate::demo_media();
    if shot_empty() {
        // Nada sonando y sin previsión (la temperatura de ahora sí, como cuando la previsión falla)
        *crate::MEDIA.lock().unwrap() = None;
        *crate::TIMELINE.lock().unwrap() = None;
        deskdata::QUEUE.lock().unwrap().clear();
        return;
    }
    let now = unsafe { GetLocalTime() };
    let h0 = now.wHour as u32;
    let hours: Vec<(u32, f32, i32, bool)> = (0..25).map(|i| {
        let h = (h0 + i) % 24;
        let temp = 13.0 + 6.0 * (((h as f32 - 9.0) / 24.0) * std::f32::consts::TAU).sin();
        let code = [0, 0, 1, 2, 3, 3, 61, 61, 2, 1, 0, 0][(i as usize / 2) % 12];
        (h, temp, code, (8..18).contains(&h))
    }).collect();
    let rain = (0..25).map(|i| [0.0, 5.0, 10.0, 20.0, 45.0, 70.0, 60.0, 30.0, 10.0, 0.0][(i / 3) % 10]).collect();
    let dow = now.wDayOfWeek as u32;
    let days = (0..6).map(|i| ((dow + i) % 7, [0, 2, 61, 3, 1, 0][i as usize], 9.0 + i as f32 * 0.5, 18.0 - (i % 3) as f32)).collect();
    *weather::FORECAST.lock().unwrap() = Some(weather::Forecast { hours, days, rain, sun: Some(("08:12".into(), "17:53".into())), at: Instant::now() });
}

/// Direct3D (por hardware o, si no hay, WARP por CPU) y un contexto de Direct2D sobre él: el mismo
/// camino que en pantalla, así funcionan los efectos de los cristales (DisplacementMap).
fn device() -> Option<(ID2D1DeviceContext, ID2D1Device)> {
    unsafe {
        let mut d3d: Option<ID3D11Device> = None;
        let mut ok = D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut d3d), None, None).is_ok();
        if !ok { ok = D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut d3d), None, None).is_ok(); }
        if !ok { return None; }
        let dxgi: IDXGIDevice = d3d?.cast().ok()?;
        let f1 = D2D1CreateFactory::<ID2D1Factory1>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let dev = f1.CreateDevice(&dxgi).ok()?;
        let dc = dev.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE).ok()?;
        dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        Some((dc, dev))
    }
}

/// Monitor principal (píxeles): ahí «está» el widget, arriba a la derecha como los de por defecto.
fn primary_monitor() -> fondo::R {
    unsafe {
        let hm = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(hm, &mut mi);
        let m = mi.rcMonitor;
        if m.right > m.left { (m.left, m.top, m.right - m.left, m.bottom - m.top) } else { (0, 0, 2560, 1440) }
    }
}

fn bitmap(dc: &ID2D1DeviceContext, w: u32, h: u32, dpi: f32, opts: D2D1_BITMAP_OPTIONS) -> Option<ID2D1Bitmap1> {
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: dpi, dpiY: dpi, bitmapOptions: opts, colorContext: std::mem::ManuallyDrop::new(None),
    };
    unsafe { dc.CreateBitmap(D2D_SIZE_U { width: w, height: h }, None, 0, &props).ok() }
}

/// Píxeles del lienzo (copiados a una imagen legible por la CPU).
fn read_back(dc: &ID2D1DeviceContext, src: &ID2D1Bitmap1, w: u32, h: u32) -> Option<Vec<u8>> {
    unsafe {
        let cpu = bitmap(dc, w, h, 96.0, D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW)?;
        cpu.CopyFromBitmap(None, src, None).ok()?;
        let m = cpu.Map(D2D1_MAP_OPTIONS_READ).ok()?;
        let mut out = vec![0u8; (w * h * 4) as usize];
        for y in 0..h as usize {
            let row = std::slice::from_raw_parts(m.bits.add(y * m.pitch as usize), w as usize * 4);
            out[y * w as usize * 4..(y + 1) * w as usize * 4].copy_from_slice(row);
        }
        let _ = cpu.Unmap();
        Some(out)
    }
}

/// Pinta una combinación dejando correr `t_end` segundos de animación y devuelve sus píxeles.
#[allow(clippy::too_many_arguments)]
fn render(dc: &ID2D1DeviceContext, rt: &ID2D1RenderTarget, dw: &IDWriteFactory, f: &popup::Fonts, d: &DFonts, k: Kind, v: usize, s: Sz, px_s: f32, mon: fondo::R, fl: bool, ink: f32, t_end: f32) -> Option<Shot> {
    // Las fotos llegan de una en una (el montón de polaroids se llena con los segundos): más rato
    let t_end = if k == Kind::Photos && t_end > 1.0 { t_end.max(9.0) } else { t_end };
    VARIANT.lock().unwrap().as_mut()?.insert(k.key(), v);
    SZ.lock().unwrap().as_mut()?.insert(k.key(), s);
    let mut w = deskw::new(k);
    RESPONSIVE[k as usize].store(w.layouts(), Ordering::Relaxed);
    let (dims, ldims) = (size(k), layout_size(k));
    let (pw, ph) = ((dims.0 * px_s).round() as u32, (dims.1 * px_s).round() as u32);
    // La ventana imaginaria: arriba a la derecha del monitor, con el margen de los de por defecto
    let m = (28.0 * px_s) as i32;
    let win = ((mon.0 + mon.2 - m - pw as i32).max(mon.0), mon.1 + m + (40.0 * px_s) as i32, pw as i32, ph as i32);
    fondo::set_shot_window(Some((win, mon)));
    let target = bitmap(dc, pw, ph, 96.0 * px_s, D2D1_BITMAP_OPTIONS_TARGET)?;
    let cache = RefCell::new(Caches::default());
    let lcache = RefCell::new(Caches::default());
    let (fk, fox, foy) = fit(dims, ldims);
    let fit_m = windows_numerics::Matrix3x2 { M11: fk, M12: 0.0, M21: 0.0, M22: fk, M31: fox, M32: foy };
    unsafe {
        dc.SetTarget(&target);
        dc.SetDpi(96.0 * px_s, 96.0 * px_s);
        let layer = if w.layered() { rt.CreateCompatibleRenderTarget(Some(&D2D_SIZE_F { width: ldims.0, height: ldims.1 }), None, None, D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE).ok() } else { None };
        let started = Instant::now();
        let (mut t, mut next_tick) = (0.0f32, 0.0f32);
        loop {
            if t >= next_tick { w.tick(); next_tick += 1.0; }
            // Capa fija (como en pantalla: se pinta aparte y se pega)
            if let Some(l) = &layer {
                let brt: &ID2D1RenderTarget = l;
                brt.BeginDraw();
                brt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
                let p2 = Painter { rt: brt, dw, cache: &lcache, h: 100.0, s: px_s };
                let mut ui2 = make_ui(p2, brt, f, ldims.0, px_s, HashMap::new(), None, fl, ink);
                w.paint(&mut ui2, d, t, DT);
                drop(ui2);
                let _ = brt.EndDraw(None, None);
            }
            let p = Painter { rt, dw, cache: &cache, h: 100.0, s: px_s };
            let mut ui = make_ui(p, rt, f, ldims.0, px_s, HashMap::new(), None, fl, ink);
            rt.BeginDraw();
            rt.SetTransform(&popup::IDENTITY);
            rt.Clear(Some(&rgba(0.0, 0.0, 0.0, 1.0)));
            // Lo de detrás: el fondo de pantalla real bajo ese sitio
            if let Some((b, _)) = fondo::under(&ui) { rt.DrawBitmap(&b, Some(&rect(0.0, 0.0, dims.0, dims.1)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None); }
            if !fl { draw_card(&ui, [0.0, 0.0, dims.0, dims.1], ink); }
            rt.SetTransform(&fit_m);
            if let Some(l) = &layer {
                if let Ok(bmp) = l.GetBitmap() { rt.DrawBitmap(&bmp, Some(&rect(0.0, 0.0, ldims.0, ldims.1)), 1.0, if fk == 1.0 { D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR } else { D2D1_BITMAP_INTERPOLATION_MODE_LINEAR }, None); }
                w.paint_live(&mut ui, d, t, DT);
            } else {
                w.paint(&mut ui, d, t, DT);
            }
            rt.SetTransform(&popup::IDENTITY);
            drop(ui);
            if rt.EndDraw(None, None).is_err() { fondo::set_shot_window(None); return None; }
            if t >= t_end { break; }
            t += DT;
            // Lo que se anima con el reloj de verdad (textos que ruedan, cargas en otros hilos) también avanza
            let want = Duration::from_secs_f32(t * 0.5);
            if let Some(rest) = want.checked_sub(started.elapsed()) { std::thread::sleep(rest); }
        }
        fondo::set_shot_window(None);
        dc.SetTarget(None);
        let px = read_back(dc, &target, pw, ph)?;
        Some(Shot { k, v, s, w: pw, h: ph, px })
    }
}

/// Guarda BGRA opaco como PNG.
fn save_png(path: &str, w: u32, h: u32, px: &[u8]) -> Option<()> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let stream = wic.CreateStream().ok()?;
        stream.InitializeFromFilename(&HSTRING::from(path), GENERIC_WRITE.0).ok()?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null()).ok()?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache).ok()?;
        let mut fe: Option<IWICBitmapFrameEncode> = None;
        let mut props = None;
        enc.CreateNewFrame(&mut fe, &mut props).ok()?;
        let fe = fe?;
        fe.Initialize(props.as_ref()).ok()?;
        fe.SetSize(w, h).ok()?;
        let mut pf = GUID_WICPixelFormat32bppBGRA;
        fe.SetPixelFormat(&mut pf).ok()?;
        fe.WritePixels(h, w * 4, px).ok()?;
        fe.Commit().ok()?;
        enc.Commit().ok()?;
        Some(())
    }
}

/// Hoja de contactos: cada diseño en una fila (que salta de línea si no cabe), con su rótulo encima,
/// a la escala `k` de las capturas.
fn sheet(dc: &ID2D1DeviceContext, rt: &ID2D1RenderTarget, dw: &IDWriteFactory, shots: &[&Shot], k: f32, path: &str) {
    if shots.is_empty() { return; }
    const MAX_W: f32 = 2400.0;
    const GAP: f32 = 24.0;
    const LBL: f32 = 30.0;
    // Colocación: (índice, x, y) y medidas totales
    let mut place = Vec::new();
    let (mut x, mut y, mut row_h, mut total_w) = (GAP, GAP, 0.0f32, 0.0f32);
    let mut prev: Option<(Kind, usize)> = None;
    for (i, sh) in shots.iter().enumerate() {
        // (el hueco nunca más estrecho que su rótulo)
        let (w, h) = ((sh.w as f32 * k).max(360.0), sh.h as f32 * k);
        let new_row = prev.is_some_and(|p| p != (sh.k, sh.v)) && k >= 1.0;
        if (new_row || x + w > MAX_W) && x > GAP { x = GAP; y += row_h + GAP; row_h = 0.0; }
        place.push((i, x, y));
        x += w + GAP;
        total_w = total_w.max(x);
        row_h = row_h.max(h + LBL);
        prev = Some((sh.k, sh.v));
    }
    let (tw, th) = (total_w.ceil() as u32, (y + row_h + GAP).ceil() as u32);
    let Some(target) = bitmap(dc, tw, th, 96.0, D2D1_BITMAP_OPTIONS_TARGET) else { return };
    let fmt = make_format(dw, FONT, 17.0, DWRITE_FONT_WEIGHT_SEMI_BOLD);
    unsafe {
        dc.SetTarget(&target);
        dc.SetDpi(96.0, 96.0);
        rt.BeginDraw();
        rt.SetTransform(&popup::IDENTITY);
        rt.Clear(Some(&rgba(0.11, 0.11, 0.12, 1.0)));
        let white = rt.CreateSolidColorBrush(&rgba(0.92, 0.92, 0.94, 1.0), None).ok();
        for (i, x, y) in place {
            let sh = shots[i];
            let props = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
            let Ok(b) = rt.CreateBitmap(D2D_SIZE_U { width: sh.w, height: sh.h }, Some(sh.px.as_ptr() as _), sh.w * 4, &props) else { continue };
            let lbl: Vec<u16> = format!("{} · {} · {}", sh.k.key(), variants(sh.k)[sh.v], sh.s.name()).encode_utf16().collect();
            if let Some(br) = &white { rt.DrawText(&lbl, &fmt, &rect(x, y, x + 1200.0, y + LBL), br, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL); }
            rt.DrawBitmap(&b, Some(&rect(x, y + LBL, x + sh.w as f32 * k, y + LBL + sh.h as f32 * k)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
        }
        if rt.EndDraw(None, None).is_err() { return; }
        dc.SetTarget(None);
        if let Some(px) = read_back(dc, &target, tw, th) { let _ = save_png(path, tw, th, &px); }
    }
}
