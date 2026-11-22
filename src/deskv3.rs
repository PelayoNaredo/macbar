//! Tercera tanda de diseños alternativos de los widgets, con estética de «matriz de puntos»: terminal
//! con la ficha del equipo, espectro en puntos, juego de la vida al ritmo de la música, esfera de puntos,
//! reloj redondo, casete con osciloscopio y unidades como barras de puntos. Todo con datos reales del PC
//! y el color de acento como único color vivo. Como los de `deskv`/`deskv2`, cada uno es otro `Widget`
//! con su propio tamaño (`desk::size`).

use super::*;
use desk::{size, DFonts, Kind};
use deskw::Widget;
use popup::Ui;
use std::f32::consts::TAU;

// ================================================================ utilidades comunes
fn ez(t: f32, delay: f32, dur: f32) -> f32 { popup::ease_out(((t - delay) / dur).clamp(0.0, 1.0)) }
fn ellipse(x: f32, y: f32, r: f32) -> D2D1_ELLIPSE { D2D1_ELLIPSE { point: Vector2 { X: x, Y: y }, radiusX: r, radiusY: r } }
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

/// Color de acento con opacidad.
fn acc(a: f32) -> D2D1_COLOR_F { crate::profiles::accent(a) }

/// Blanco fijo (para lo que va sobre un fondo oscuro propio: terminal, esfera del reloj, casete).
fn wh(a: f32) -> D2D1_COLOR_F { rgba(1.0, 1.0, 1.0, a) }

/// Opacidad redondeada a pasos de 1/`n` (así los pinceles se reutilizan de la caché en vez de crear uno por punto).
fn q(a: f32, n: f32) -> f32 { (a.clamp(0.0, 1.0) * n).round() / n }

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

/// Un punto relleno.
fn dot(rt: &ID2D1RenderTarget, x: f32, y: f32, r: f32, b: &ID2D1Brush) {
    if r > 0.05 { unsafe { rt.FillEllipse(&ellipse(x, y, r), b); } }
}

/// Un punto hueco (solo el borde).
fn ring_dot(rt: &ID2D1RenderTarget, x: f32, y: f32, r: f32, b: &ID2D1Brush, sw: f32) {
    if r > 0.05 { unsafe { rt.DrawEllipse(&ellipse(x, y, r), b, sw, None); } }
}

/// Fila de puntos tipo contador ●●●●○○: `on` de `n` encendidos. Devuelve el ancho.
#[allow(clippy::too_many_arguments)]
fn dot_row(rt: &ID2D1RenderTarget, x: f32, cy: f32, n: usize, on: usize, pitch: f32, r: f32, b_on: &ID2D1Brush, b_off: &ID2D1Brush) -> f32 {
    for i in 0..n {
        let px = x + i as f32 * pitch;
        if i < on { dot(rt, px, cy, r, b_on); } else { dot(rt, px, cy, r * 0.62, b_off); }
    }
    (n.max(1) - 1) as f32 * pitch + 2.0 * r
}

// ---------------------------------------------------------------- dígitos de puntos
/// Letra de puntos: pequeña de 3×5 o grande de 5×7 (solo dígitos y «:»).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DotFont { Small, Big }

/// Filas de cada dígito de 3×5 (el bit alto es la columna de la izquierda).
const SMALL: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111], [0b010, 0b110, 0b010, 0b010, 0b111], [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111], [0b101, 0b101, 0b111, 0b001, 0b001], [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111], [0b111, 0b001, 0b001, 0b001, 0b001], [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];
/// Filas de cada dígito de 5×7.
const BIG: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
];
const COLON_S: [u8; 5] = [0, 1, 0, 1, 0];
const COLON_B: [u8; 7] = [0, 0, 1, 0, 1, 0, 0];

/// Ancho en columnas y filas de un carácter (None si la letra no lo tiene).
fn glyph(c: char, f: DotFont) -> Option<(usize, &'static [u8])> {
    match (c, f) {
        ('0'..='9', DotFont::Small) => Some((3, &SMALL[c as usize - '0' as usize])),
        ('0'..='9', DotFont::Big) => Some((5, &BIG[c as usize - '0' as usize])),
        (':', DotFont::Small) => Some((1, &COLON_S)),
        (':', DotFont::Big) => Some((1, &COLON_B)),
        _ => None,
    }
}

/// Columnas que ocupa un texto (con una columna de separación entre caracteres).
fn dot_cols(s: &str, f: DotFont) -> usize {
    let n: usize = s.chars().map(|c| glyph(c, f).map(|g| g.0).unwrap_or(1) + 1).sum();
    n.saturating_sub(1)
}

/// Filas de la letra.
fn dot_rows(f: DotFont) -> usize { if f == DotFont::Small { 5 } else { 7 } }

/// ¿Está encendido el punto (col, fila) del carácter?
fn glyph_on(rows: &[u8], w: usize, col: usize, row: usize) -> bool { rows.get(row).is_some_and(|b| (b >> (w - 1 - col)) & 1 == 1) }

/// Dibuja `s` en puntos: (x, y) es el centro del primer punto de arriba a la izquierda. Con `off`, los
/// puntos apagados de cada carácter se ven tenues (pincel y radio). Devuelve el ancho.
#[allow(clippy::too_many_arguments)]
fn dot_text(rt: &ID2D1RenderTarget, s: &str, f: DotFont, x: f32, y: f32, pitch: f32, r: f32, on: &ID2D1Brush, off: Option<(&ID2D1Brush, f32)>) -> f32 {
    let mut cx = x;
    for c in s.chars() {
        let Some((gw, rows)) = glyph(c, f) else { cx += pitch * 2.0; continue };
        for row in 0..rows.len() {
            for col in 0..gw {
                let (px, py) = (cx + col as f32 * pitch, y + row as f32 * pitch);
                if glyph_on(rows, gw, col, row) { dot(rt, px, py, r, on); } else if let Some((b, r2)) = off { dot(rt, px, py, r2, b); }
            }
        }
        cx += (gw + 1) as f32 * pitch;
    }
    dot_cols(s, f).saturating_sub(1) as f32 * pitch
}

// ================================================================ Sistema: «Terminal»
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

/// Tiempo encendido para los dígitos de puntos: «H:MM:SS» (las horas pueden tener varias cifras).
fn uptime_clock(s: u64) -> String { format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) }

/// Ventana de terminal con la ficha del equipo (como un «fetch»): logo de puntos y tiempo encendido a la
/// izquierda, claves en el color de acento a la derecha y una barra de estado con contadores de puntos.
#[derive(Default)]
pub struct TermW { cpu: f32, ram: f32 }

impl Widget for TermW {
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, t: f32, dt: f32) {
        let (w, h) = size(Kind::System);
        let info = sysinfo();
        let sd = deskdata::SYS.lock().unwrap().clone();
        let cpu_now = sd.as_ref().and_then(|s| s.cpu.last().copied()).unwrap_or(0.0);
        let ram_now = sd.as_ref().and_then(|s| s.ram.last().copied()).unwrap_or(0.0);
        let kin = ez(t, 0.1, 0.9);
        approach(&mut self.cpu, cpu_now * kin, dt.min(0.2), 0.3);
        approach(&mut self.ram, ram_now * kin, dt.min(0.2), 0.3);
        // Va sobre su propio fondo oscuro: sin el halo de los flotantes
        let halo = ui.shadow.take();
        let rt = ui.rt;
        let (white, dim, faint, off, line) = (ui.brush(wh(0.95)), ui.brush(wh(0.55)), ui.brush(wh(0.30)), ui.brush(wh(0.10)), ui.brush(wh(0.07)));
        let accent = ui.brush(acc(1.0));
        unsafe {
            rt.FillRoundedRectangle(&rrect(rect(0.0, 0.0, w, h), 16.0), &ui.brush(rgba(0.055, 0.056, 0.064, 0.94)));
            rt.DrawRoundedRectangle(&rrect(rect(0.5, 0.5, w - 0.5, h - 0.5), 15.5), &ui.brush(wh(0.10)), 1.0, None);
            // Barra de título y barra de estado
            rt.FillRectangle(&rect(0.0, 32.0, w, 33.0), &line);
            rt.FillRectangle(&rect(0.0, h - 31.0, w, h - 30.0), &line);
        }
        // Semáforo: el primero en el acento
        for (i, b) in [&accent, &off, &off].into_iter().enumerate() { dot(rt, 18.0 + i as f32 * 16.0, 16.5, 5.5, b); }
        let who = format!("{}@{}", if info.user.is_empty() { "usuario" } else { &info.user }, if info.pc.is_empty() { "equipo" } else { &info.pc });
        ui.text_c(&ui.f.lbl, &format!("{who} — fetch"), w / 2.0, 7.0, 20.0, &white);
        let ncores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
        if ncores > 0 { ui.text_r(&ui.f.small, &i18n::tf("{} hilos", &[&ncores]), w - 16.0, 7.0, 20.0, &faint); }

        // ---- Logo: la ventana de Windows en puntos (4 cuadros, uno en el acento)
        let (lx, ly, lp) = (30.0, 56.0, 15.0);
        for j in 0..9usize {
            for i in 0..9usize {
                let (x, y) = (lx + i as f32 * lp, ly + j as f32 * lp);
                if i == 4 || j == 4 { dot(rt, x, y, 1.3, &off); continue; }
                // Entran en diagonal, desde arriba a la izquierda
                let k = ez(t, 0.02 * (i + j) as f32, 0.35);
                let b = if i > 4 && j > 4 { &accent } else { &white };
                dot(rt, x, y, lp * 0.36 * crate::ease_back(k), b);
            }
        }
        // ---- Tiempo encendido en dígitos de puntos (cuenta cada segundo)
        let up = if sys::demo() { 3 * 3600 + 25 * 60 + 9 } else { (unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }) / 1000 };
        let s = uptime_clock(up);
        let span = 8.0 * lp;
        let cols = dot_cols(&s, DotFont::Small).max(1);
        let pp = (span / (cols - 1) as f32).min(6.0);
        let tx = lx + (span - (cols - 1) as f32 * pp) / 2.0;
        let ty = ly + 9.0 * lp + 12.0;
        popup::push_alpha(rt, ez(t, 0.3, 0.5));
        dot_text(rt, &s, DotFont::Small, tx, ty, pp, pp * 0.36, &white, Some((&off, pp * 0.14)));
        unsafe { rt.PopLayer(); }
        ui.text_c(&ui.f.small, i18n::t("encendido"), lx + span / 2.0, ty + (dot_rows(DotFont::Small) - 1) as f32 * pp + 8.0, 16.0, &dim);

        // ---- Ficha: usuario@equipo y las claves en el acento
        let x0 = 196.0;
        let (uw, at_w) = (ui.p.width(&ui.f.name, &info.user), ui.p.width(&ui.f.name, "@"));
        let hy = 42.0;
        ui.text(&ui.f.name, &info.user, x0, hy, 20.0, &white);
        ui.text(&ui.f.name, "@", x0 + uw, hy, 20.0, &accent);
        ui.text(&ui.f.name, &info.pc, x0 + uw + at_w, hy, 20.0, &white);
        let tw = ui.p.width(&ui.f.name, &who).max(60.0);
        let n = (tw / 6.0) as usize;
        for i in 0..n { dot(rt, x0 + 1.5 + i as f32 * 6.0, hy + 25.0, 1.1, &faint); }
        let st = deskdata::STORE.lock().unwrap().clone();
        let c_drive = st.as_ref().and_then(|s| s.drives.iter().find(|d| d.root.to_ascii_uppercase().starts_with("C:")).or(s.drives.first()).cloned());
        let media = MEDIA.lock().unwrap().clone();
        let mut rows: Vec<(&str, String)> = vec![
            (i18n::t("SO"), info.os.clone()),
            (i18n::t("Equipo"), { let pc = title_case(&info.pc); if info.model.is_empty() { pc } else { format!("{pc} · {}", info.model) } }),
            ("CPU", format!("{}{}{:.0} %", info.cpu, if info.cpu.is_empty() { "" } else { " · " }, cpu_now)),
            ("RAM", sd.as_ref().filter(|s| s.ram_total > 0).map(|s| format!("{} / {}", deskdata::fmt_bytes(s.ram_used), deskdata::fmt_bytes(s.ram_total))).unwrap_or_else(|| "—".into())),
        ];
        if let Some(dr) = &c_drive {
            let used = dr.total.saturating_sub(dr.free);
            let letter = dr.root.trim_end_matches('\\').to_string();
            let key: &str = if letter.eq_ignore_ascii_case("C:") { i18n::t("Disco C:") } else { i18n::t("Disco") };
            rows.push((key, format!("{} / {} ({:.0} %)", deskdata::fmt_bytes(used), deskdata::fmt_bytes(dr.total), used as f32 * 100.0 / dr.total.max(1) as f32)));
        }
        if let Some((p, ch)) = crate::battery() { rows.push((i18n::t("Batería"), if ch { i18n::tf("{} % · cargando", &[&p]) } else { format!("{p} %") })); }
        rows.push((i18n::t("Encendido"), uptime_words(up)));
        rows.push((i18n::t("Sonando"), media.as_ref().map(|m| if m.artist.is_empty() { m.title.clone() } else { format!("{} — {}", m.title, m.artist) }).unwrap_or_else(|| "—".into())));
        let kx = x0 + 84.0;
        let maxw = w - 18.0 - kx;
        let mut y = hy + 34.0;
        for (i, (k, v)) in rows.iter().enumerate() {
            let ki = ez(t, 0.12 + 0.04 * i as f32, 0.35);
            popup::push_alpha(rt, ki);
            let dx = 6.0 * (1.0 - ki);
            ui.text(&ui.f.lbl, k, x0 + dx, y, 19.0, &accent);
            ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, v, maxw), kx + dx, y, 19.0, &white);
            unsafe { rt.PopLayer(); }
            y += 19.5;
        }
        // Muestras de color: grises y el acento
        let ks = ez(t, 0.5, 0.4);
        popup::push_alpha(rt, ks);
        let sy = y + 5.0;
        for (i, g) in [0.0f32, 0.16, 0.30, 0.44, 0.58, 0.72, 0.86, 0.97].iter().enumerate() {
            let x = x0 + i as f32 * 17.0;
            unsafe {
                rt.FillRoundedRectangle(&rrect(rect(x, sy, x + 14.0, sy + 14.0), 2.5), &ui.brush(rgba(*g, *g, *g + 0.01, 1.0)));
                rt.DrawRoundedRectangle(&rrect(rect(x + 0.5, sy + 0.5, x + 13.5, sy + 13.5), 2.0), &off, 1.0, None);
            }
        }
        unsafe { rt.FillRoundedRectangle(&rrect(rect(x0 + 8.0 * 17.0, sy, x0 + 8.0 * 17.0 + 14.0, sy + 14.0), 2.5), &accent); rt.PopLayer(); }

        // ---- Barra de estado: pestaña, procesador y memoria en puntos, y la hora
        let by = h - 30.0;
        unsafe { rt.FillRoundedRectangle(&rrect(rect(12.0, by + 6.0, 62.0, by + 24.0), 5.0), &ui.brush(wh(0.92))); }
        ui.text_c(&ui.f.small, "fetch", 37.0, by + 6.0, 18.0, &ui.brush(rgba(0.06, 0.06, 0.07, 1.0)));
        let mut x = 78.0;
        for (lab, v) in [("cpu", self.cpu), ("ram", self.ram)] {
            ui.text(&ui.f.small, lab, x, by + 6.0, 18.0, &dim);
            x += ui.p.width(&ui.f.small, lab) + 8.0;
            let on = (v / 10.0).round().clamp(0.0, 10.0) as usize;
            let b_on = if v >= 85.0 { &accent } else { &white };
            x += dot_row(rt, x + 2.6, by + 15.0, 10, on, 7.0, 2.6, b_on, &faint) + 6.0;
            let pct = format!("{:.0} %", v);
            ui.text(&ui.f.small, &pct, x, by + 6.0, 18.0, &white);
            x += ui.p.width(&ui.f.small, "100 %") + 16.0;
        }
        let now = unsafe { GetLocalTime() };
        ui.text_r(&ui.f.lbl, &format!("{:02}:{:02}", now.wHour, now.wMinute), w - 16.0, by + 5.0, 20.0, &white);
        ui.shadow = halo;
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn animating(&self, t: f32) -> bool { t < 1.4 }
}

// ================================================================ Visualizador: «Puntos»
const DCOLS: usize = 40;

/// Espectro en columnas de puntos: puntos apagados de fondo, columnas encendidas, un pico hueco que cae
/// despacio en cada una y el pico más alto en el acento.
pub struct SpecDotsW { lv: [f32; DCOLS], peak: [f32; DCOLS], hold: [f32; DCOLS], alive: f32, moving: bool }

impl Default for SpecDotsW { fn default() -> Self { SpecDotsW { lv: [0.0; DCOLS], peak: [0.0; DCOLS], hold: [0.0; DCOLS], alive: 0.0, moving: false } } }

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
        // Línea de estado
        let live = self.alive > 0.5;
        dot(rt, 20.0, 19.0, 3.0, &ui.brush(acc(if live { 1.0 } else { 0.35 })));
        let status = if live { i18n::tf("barras {} · en vivo", &[&DCOLS]) } else { i18n::t("en pausa").to_string() };
        ui.text(&ui.f.small, &status, 30.0, 10.0, 18.0, &ui.dim);
        let title = MEDIA.lock().unwrap().as_ref().map(|m| m.title.clone());
        if let Some(tt) = title { let s = fit(ui, &ui.f.small, &tt, w * 0.45); ui.text_r(&ui.f.small, &s, w - 18.0, 10.0, 18.0, &ui.faint); }
        // Rejilla
        let (x0, x1, base) = (20.0, w - 20.0, h - 16.0);
        let pitch = (x1 - x0) / (DCOLS - 1) as f32;
        let rows = ((base - 40.0) / pitch).floor() as usize + 1;
        let (b_on, b_off, b_ring) = (ui.wa(0.95), ui.wa(0.11), ui.wa(0.55));
        let (r_on, r_off) = (pitch * 0.36, pitch * 0.13);
        // Columna con el pico más alto (va en el acento)
        let top = (0..DCOLS).fold(0, |a, c| if self.peak[c] > self.peak[a] { c } else { a });
        let b_acc = ui.brush(acc(1.0));
        for c in 0..DCOLS {
            let x = x0 + c as f32 * pitch;
            // Al aparecer, las columnas suben de izquierda a derecha
            let kin = ez(t, 0.012 * c as f32, 0.5);
            let n = ((shape(self.lv[c]) * rows as f32).round() * kin) as usize;
            let pr = ((shape(self.peak[c]) * rows as f32).round() * kin) as usize;
            for r in 0..rows {
                let y = base - r as f32 * pitch;
                if r < n { dot(rt, x, y, r_on, &b_on); }
                else if pr > 0 && r == pr - 1 && pr > n {
                    if c == top && self.peak[c] > 0.06 { dot(rt, x, y, r_on, &b_acc); } else { ring_dot(rt, x, y, r_on * 0.82, &b_ring, 1.3); }
                } else { dot(rt, x, y, r_off, &b_off); }
            }
        }
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
const LC: usize = 46;
const LR: usize = 22;

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

/// Huella de una rejilla (para ver si se ha quedado quieta o oscilando).
fn grid_hash(g: &[bool]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (i, v) in g.iter().enumerate() { if *v { h ^= i as u64 + 1; h = h.wrapping_mul(0x0100_0000_01b3); } }
    h
}

/// Juego de la vida en una rejilla de puntos: cada golpe de graves siembra células que nacen en el acento
/// y pasan a blanco; si se extingue o se estanca, se vuelve a sembrar. Sin música avanza despacio solo.
pub struct LifeW { cells: Vec<bool>, next: Vec<bool>, born: Vec<f32>, gens: u64, pop: usize, acc: f32, rng: u64, hist: [u64; 2], stale: u32, bass_avg: f32, last_beat: f32, now: f32, alive: f32 }

impl Default for LifeW {
    fn default() -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
        let mut l = LifeW { cells: vec![false; LC * LR], next: vec![false; LC * LR], born: vec![-10.0; LC * LR], gens: 0, pop: 0, acc: 0.0, rng: seed, hist: [0; 2], stale: 0, bass_avg: 0.0, last_beat: -10.0, now: 0.0, alive: 0.0 };
        l.reseed();
        l
    }
}

impl LifeW {
    fn rand(&mut self) -> u64 { let mut x = self.rng; x ^= x << 13; x ^= x >> 7; x ^= x << 17; self.rng = x; x }
    /// Siembra al azar toda la rejilla (un 28 %).
    fn reseed(&mut self) {
        for i in 0..LC * LR {
            let v = self.rand() % 100 < 28;
            if v && !self.cells[i] { self.born[i] = self.now; }
            self.cells[i] = v;
        }
        self.gens = 0;
        self.stale = 0;
        self.pop = self.cells.iter().filter(|c| **c).count();
    }
    /// Un golpe: un grupo de células nuevas en un sitio al azar (nacen en el acento).
    fn burst(&mut self) {
        let (cx, cy) = ((self.rand() % LC as u64) as usize, (self.rand() % LR as u64) as usize);
        // Pentominó R (crece mucho) o una mancha al azar de 4×4
        const R_PENTO: [(usize, usize); 5] = [(1, 0), (2, 0), (0, 1), (1, 1), (1, 2)];
        let cells: Vec<(usize, usize)> = if self.rand() % 3 == 0 { R_PENTO.to_vec() } else { (0..16).filter(|_| self.rand() % 2 == 0).map(|i| (i % 4, i / 4)).collect() };
        for (dx, dy) in cells {
            let i = ((cy + dy) % LR) * LC + (cx + dx) % LC;
            if !self.cells[i] { self.cells[i] = true; self.born[i] = self.now; }
        }
    }
    fn step(&mut self) {
        self.pop = life_step(&self.cells, &mut self.next, LC, LR);
        // Lo que nace por las reglas sale ya en blanco (en el acento solo lo que siembra un golpe)
        for i in 0..LC * LR { if self.next[i] && !self.cells[i] { self.born[i] = self.now - 0.9 + 0.12; } }
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
        let every = if reduced() { 1.0 } else if self.alive > 0.5 { 0.11 } else { 0.5 };
        self.acc += dt;
        let mut n = 0;
        while self.acc >= every && n < 2 { self.acc -= every; self.step(); n += 1; }
        if self.acc > every { self.acc = 0.0; }

        let rt = ui.rt;
        // Línea de estado: generación y población; un punto de acento que destella con cada golpe
        let flash = (1.0f32 - (self.now - self.last_beat) / 0.5).clamp(0.0, 1.0);
        dot(rt, 20.0, 19.0, 3.0, &ui.brush(acc(0.3 + 0.7 * flash)));
        ui.text(&ui.f.small, &i18n::tf("gen {} · pob {}", &[&self.gens, &self.pop]), 30.0, 10.0, 18.0, &ui.dim);
        let right = if self.alive > 0.5 { i18n::t("al ritmo") } else { i18n::t("en pausa") };
        ui.text_r(&ui.f.small, right, w - 18.0, 10.0, 18.0, &ui.faint);
        // Rejilla: recién nacidas en el acento que pasan a blanco en ~1 s (8 pasos de color, pinceles de la caché)
        let (x0, y0) = (20.0, 40.0);
        let pitch = (w - 40.0) / (LC - 1) as f32;
        let (a, wc) = (acc(1.0), ui.ic(0.92));
        let ramp: [ID2D1Brush; 8] = std::array::from_fn(|i| { let k = i as f32 / 7.0; ui.brush(rgba(lerp(a.r, wc.r, k), lerp(a.g, wc.g, k), lerp(a.b, wc.b, k), lerp(1.0, wc.a, k))) });
        let b_off = ui.wa(0.10);
        let kin = ez(t, 0.0, 0.6);
        for y in 0..LR {
            for x in 0..LC {
                let i = y * LC + x;
                let (px, py) = (x0 + x as f32 * pitch, y0 + y as f32 * pitch);
                if self.cells[i] {
                    let age = (self.now - self.born[i]).max(0.0);
                    let step = ((age / 0.9).min(1.0) * 7.0).round() as usize;
                    let pop_in = 0.6 + 0.4 * (age / 0.12).min(1.0);
                    dot(rt, px, py, pitch * 0.36 * pop_in * kin, &ramp[step]);
                } else { dot(rt, px, py, pitch * 0.12, &b_off); }
            }
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
    fn frame_ms(&self) -> u32 { if self.alive > 0.01 { 33 } else if reduced() { 250 } else { 100 } }
}

// ================================================================ Visualizador: «Órbita»
const ORB_N: usize = 400;
const ORB_RING: usize = 120;
/// Ángulo áureo (radianes): reparte los puntos de la esfera sin que se amontonen.
const GOLDEN: f32 = 2.399_963;

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

/// Esfera de puntos que gira, con profundidad (tamaño y brillo según lo cerca que está) y un anillo
/// inclinado de puntos con una «luna» en el acento; el radio late con la energía del audio.
pub struct OrbW { pts: Vec<[f32; 3]>, yaw: f32, energy: f32, alive: f32 }

impl Default for OrbW { fn default() -> Self { OrbW { pts: fib_sphere(ORB_N), yaw: 3.77, energy: 0.0, alive: 0.0 } } }

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
        let pitch = -0.36;
        let rt = ui.rt;
        ui.text(&ui.f.small, &i18n::tf("puntos {} · giro {}°", &[&ORB_N, &(self.yaw.to_degrees().round() as i32)]), 18.0, 8.0, 18.0, &ui.dim);
        let ki = ez(t, 0.0, 0.8);
        let (cx, cy) = (w / 2.0, h / 2.0 + 10.0);
        let rad = (w.min(h) * 0.28) * (0.7 + 0.3 * ki) * (1.0 + 0.10 * self.energy);
        // 12 brillos de punto (pinceles de la caché)
        let shades: [ID2D1Brush; 13] = std::array::from_fn(|i| ui.wa(q(i as f32 / 12.0, 12.0) * ki));
        let proj = |p: [f32; 3], r: f32| -> (f32, f32, f32) { let f = 1.0 + 0.10 * p[2]; (cx + p[0] * r * f, cy - p[1] * r * f, p[2]) };
        // Anillo inclinado: gira despacio por su cuenta
        let ring_r = 1.42;
        let ring_pt = |i: usize| -> [f32; 3] {
            let a = i as f32 / ORB_RING as f32 * TAU + self.yaw * 0.6;
            let p = [a.cos() * ring_r, 0.0, a.sin() * ring_r];
            // Inclinado respecto al ecuador y luego con la misma cabeceada que la esfera
            let (s, c) = (0.42f32).sin_cos();
            let p = [p[0] * c - p[1] * s, p[0] * s + p[1] * c, p[2]];
            rotate(p, 0.0, pitch)
        };
        let ring_dot_at = |i: usize, front: bool| {
            let p = ring_pt(i);
            if (p[2] >= 0.0) != front { return; }
            let (x, y, z) = proj(p, rad);
            // Por detrás y dentro del disco de la esfera: tapado
            if z < 0.0 && ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() < rad * 0.98 { return; }
            let a = 0.18 + 0.30 * (z / ring_r + 1.0) / 2.0;
            dot(rt, x, y, 0.85, &shades[(a * 12.0).round() as usize]);
        };
        let sphere = |front: bool| {
            for p in &self.pts {
                let p = rotate(*p, self.yaw, pitch);
                if (p[2] >= 0.0) != front { continue; }
                let (x, y, z) = proj(p, rad);
                let d = (z + 1.0) / 2.0;
                let a = 0.14 + 0.82 * d.powf(1.4);
                dot(rt, x, y, 0.55 + 1.45 * d, &shades[(a * 12.0).round().min(12.0) as usize]);
            }
        };
        // De atrás hacia delante: anillo de detrás, esfera, anillo de delante
        for i in 0..ORB_RING { ring_dot_at(i, false); }
        sphere(false);
        sphere(true);
        for i in 0..ORB_RING { ring_dot_at(i, true); }
        // La luna: un punto del anillo en el acento
        let p = ring_pt(0);
        let (x, y, z) = proj(p, rad);
        let hidden = z < 0.0 && ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() < rad * 0.98;
        if !hidden { dot(rt, x, y, 2.4 + 0.8 * (z / ring_r).max(0.0), &ui.brush(acc(ki))); }
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
/// Radio de la esfera del reloj en puntos (la rejilla va de -10 a 10).
const EYE_R: f32 = 10.45;
/// Desde aquí hacia fuera, los puntos forman el arco de los segundos.
const EYE_BAND: f32 = 8.9;

/// ¿Encienden los dígitos el punto (i, j) de la rejilla del ojo? Horas arriba (filas -7..-1) y minutos
/// abajo (1..7), cada número con dos dígitos de 5×7 y una columna libre en medio.
fn eye_digit_on(i: i32, j: i32, hh: u16, mm: u16) -> bool {
    let (val, row) = match j { -7..=-1 => (hh, (j + 7) as usize), 1..=7 => (mm, (j - 1) as usize), _ => return false };
    let (d, col) = match i { -5..=-1 => (val / 10 % 10, (i + 5) as usize), 1..=5 => (val % 10, (i - 1) as usize), _ => return false };
    glyph(char::from(b'0' + d as u8), DotFont::Big).is_some_and(|(w, rows)| glyph_on(rows, w, col, row))
}

/// Ángulo (0 arriba, en el sentido del reloj) del punto (i, j).
fn eye_angle(i: i32, j: i32) -> f32 { let a = (i as f32).atan2(-(j as f32)); if a < 0.0 { a + TAU } else { a } }

/// Reloj redondo de matriz de puntos: la hora arriba, los minutos abajo y un arco de puntos por el borde
/// que marca los segundos, con el punto de ahora en el acento.
#[derive(Default)]
pub struct EyeW { band: Vec<(f32, i32, i32)> }

impl Widget for EyeW {
    fn paint(&mut self, ui: &mut Ui, _d: &DFonts, t: f32, _dt: f32) {
        let (w, h) = size(Kind::Clocks);
        if self.band.is_empty() {
            for j in -10..=10 { for i in -10..=10 {
                let d = ((i * i + j * j) as f32).sqrt();
                if d <= EYE_R && d >= EYE_BAND { self.band.push((eye_angle(i, j), i, j)); }
            } }
            self.band.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        let halo = ui.shadow.take();
        let rt = ui.rt;
        let (cx, cy) = (w / 2.0, h / 2.0);
        let rad = w.min(h) / 2.0 - 6.0;
        let pitch = (rad - 7.0) / EYE_R;
        let k0 = ez(t, 0.0, 0.5);
        unsafe {
            rt.FillEllipse(&ellipse(cx, cy, rad), &ui.brush(rgba(0.045, 0.046, 0.052, 0.95)));
            rt.DrawEllipse(&ellipse(cx, cy, rad - 0.5), &ui.brush(wh(0.10)), 1.0, None);
            rt.DrawEllipse(&ellipse(cx, cy, rad + 3.5), &ui.brush(wh(0.05 * k0)), 1.0, None);
        }
        let now = unsafe { GetLocalTime() };
        let frac = (now.wSecond as f32 + 1.0) / 60.0;
        let cut = frac * TAU;
        // El punto de ahora: el último del arco que ya ha llegado
        let cur = self.band.iter().rposition(|b| b.0 <= cut + 1e-4);
        let (b_on, b_off, b_sec, b_acc) = (ui.brush(wh(0.96)), ui.brush(wh(0.085)), ui.brush(wh(0.42)), ui.brush(acc(1.0)));
        for j in -10..=10i32 {
            for i in -10..=10i32 {
                let d = ((i * i + j * j) as f32).sqrt();
                if d > EYE_R { continue; }
                let (x, y) = (cx + i as f32 * pitch, cy + j as f32 * pitch);
                if eye_digit_on(i, j, now.wHour, now.wMinute) {
                    // Los dígitos aparecen de arriba abajo
                    let k = ez(t, 0.15 + 0.02 * (j + 10) as f32, 0.3);
                    dot(rt, x, y, pitch * 0.38 * k, &b_on);
                    if k < 1.0 { dot(rt, x, y, pitch * 0.16 * (1.0 - k), &b_off); }
                } else if d >= EYE_BAND && eye_angle(i, j) <= cut + 1e-4 && cur.is_some() {
                    let is_cur = cur.is_some_and(|c| self.band[c].1 == i && self.band[c].2 == j);
                    let k = ez(t, 0.3, 0.5);
                    if is_cur { dot(rt, x, y, pitch * 0.44 * k, &b_acc); } else { dot(rt, x, y, pitch * 0.30 * k, &b_sec); }
                } else { dot(rt, x, y, pitch * 0.16 * k0, &b_off); }
            }
        }
        ui.shadow = halo;
    }
    fn click(&mut self, _id: Option<u32>, _right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> { None }
    fn animating(&self, t: f32) -> bool { t < 1.2 }
}

// ================================================================ Música: «Casete»
const WN: usize = 72;
const TRAIL: usize = 5;
const HUB: f32 = 13.0;
const PACK: f32 = 33.0;

/// Radio de la cinta en cada bobina (izquierda, derecha) para un progreso 0..1: la cinta que pasa es
/// superficie, así que el radio va con la raíz (la de la izquierda se vacía y la de la derecha se llena).
fn reel_radii(prog: f32) -> (f32, f32) {
    let p = prog.clamp(0.0, 1.0);
    let span = PACK * PACK - HUB * HUB;
    ((HUB * HUB + span * (1.0 - p)).sqrt(), (HUB * HUB + span * p).sqrt())
}

/// Casete con dos bobinas que pasan la cinta según el progreso real y giran solo mientras suena; a la
/// derecha, un osciloscopio con estela y contadores de puntos de graves, medios y agudos.
pub struct TapeW { ang: [f32; 2], spin: f32, wave: [[f32; WN]; TRAIL], head: usize, trail_t: f32, phase: [f32; 6], lv: [f32; audioviz::BANDS], alive: f32, playing: bool, moving: bool }

impl Default for TapeW {
    fn default() -> Self { TapeW { ang: [0.0, 1.1], spin: 0.0, wave: [[0.0; WN]; TRAIL], head: 0, trail_t: 0.0, phase: [0.0; 6], lv: [0.0; audioviz::BANDS], alive: 0.0, playing: false, moving: false } }
}

impl TapeW {
    /// Una bobina: cinta enrollada (con su borde de puntos), buje con dientes y eje.
    fn reel(ui: &Ui, cx: f32, cy: f32, r: f32, ang: f32) {
        let rt = ui.rt;
        unsafe {
            rt.FillEllipse(&ellipse(cx, cy, r), &ui.brush(rgba(0.23, 0.215, 0.205, 1.0)));
            rt.DrawEllipse(&ellipse(cx, cy, r - 0.5), &ui.brush(wh(0.08)), 1.0, None);
        }
        // Borde de la cinta en puntos (giran con la bobina)
        let n = ((TAU * r) / 6.5).floor().max(6.0) as usize;
        let b = ui.brush(wh(0.30));
        for i in 0..n {
            let a = ang + i as f32 / n as f32 * TAU;
            dot(rt, cx + a.cos() * (r - 3.0), cy + a.sin() * (r - 3.0), 1.1, &b);
        }
        // Buje: aro claro y seis dientes
        unsafe {
            rt.FillEllipse(&ellipse(cx, cy, HUB - 1.0), &ui.brush(rgba(0.09, 0.09, 0.10, 1.0)));
            rt.DrawEllipse(&ellipse(cx, cy, HUB - 2.0), &ui.brush(wh(0.78)), 2.0, None);
        }
        let teeth = ui.brush(wh(0.85));
        for i in 0..6 {
            let a = ang * 1.0 + i as f32 / 6.0 * TAU;
            dot(rt, cx + a.cos() * 6.5, cy + a.sin() * 6.5, 1.6, &teeth);
        }
        dot(rt, cx, cy, 2.4, &ui.brush(wh(0.5)));
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
            let v = 30.0 * self.spin;
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

        let halo = ui.shadow.take();
        let rt = ui.rt;
        let base = { let mut mm = Matrix3x2::default(); unsafe { rt.GetTransform(&mut mm); } mm };
        let (white, dim, faint, off) = (ui.brush(wh(0.95)), ui.brush(wh(0.55)), ui.brush(wh(0.28)), ui.brush(wh(0.10)));
        let accent = ui.brush(acc(1.0));

        // ---- La casete (entra desde la izquierda)
        let kc = ez(t, 0.0, 0.6);
        let (bx0, by0, bx1, by1) = (8.0 - 14.0 * (1.0 - kc), 8.0, 300.0 - 14.0 * (1.0 - kc), h - 8.0);
        popup::push_alpha(rt, kc);
        ui.hit(rect(bx0, by0, bx1, by1), 4);
        let hk = ui.heat(4);
        unsafe {
            rt.FillRoundedRectangle(&rrect(rect(bx0 + 1.0, by0 + 3.0, bx1 + 1.0, by1 + 4.0), 13.0), &ui.brush(rgba(0.0, 0.0, 0.0, 0.30)));
            rt.FillRoundedRectangle(&rrect(rect(bx0, by0, bx1, by1), 12.0), &ui.brush(rgba(0.075 + 0.02 * hk, 0.075 + 0.02 * hk, 0.085 + 0.02 * hk, 0.96)));
            rt.DrawRoundedRectangle(&rrect(rect(bx0 + 0.5, by0 + 0.5, bx1 - 0.5, by1 - 0.5), 11.5), &ui.brush(wh(0.11)), 1.0, None);
        }
        for (sx, sy) in [(bx0 + 8.0, by0 + 8.0), (bx1 - 8.0, by0 + 8.0), (bx0 + 8.0, by1 - 8.0), (bx1 - 8.0, by1 - 8.0)] { ring_dot(rt, sx, sy, 2.4, &faint, 1.0); }
        // Etiqueta: cara «A» en el acento, título y artista
        let (lx0, ly0, lx1, ly1) = (bx0 + 18.0, by0 + 12.0, bx1 - 18.0, by0 + 52.0);
        unsafe { rt.FillRoundedRectangle(&rrect(rect(lx0, ly0, lx1, ly1), 6.0), &ui.brush(wh(0.06))); }
        ui.text(&d.title, "A", lx0 + 9.0, ly0 + 9.0, 22.0, &accent);
        let tx = lx0 + 32.0;
        let maxw = lx1 - tx - 8.0;
        let (title, artist) = match &m { Some(m) => (m.title.clone(), m.artist.clone()), None => (i18n::t("Nada sonando").to_string(), i18n::t("Pon algo de música").to_string()) };
        ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &title, maxw), tx, ly0 + 4.0, 18.0, &white);
        ui.text(&ui.f.small, &fit(ui, &ui.f.small, &artist, maxw), tx, ly0 + 21.0, 16.0, &dim);
        // Ventana con las dos bobinas y la cinta que pasa por abajo
        let (wx0, wy0, wx1, wy1) = (bx0 + 26.0, by0 + 60.0, bx1 - 26.0, by0 + 132.0);
        unsafe {
            rt.FillRoundedRectangle(&rrect(rect(wx0, wy0, wx1, wy1), 10.0), &ui.brush(rgba(0.0, 0.0, 0.0, 0.40)));
            rt.DrawRoundedRectangle(&rrect(rect(wx0 + 0.5, wy0 + 0.5, wx1 - 0.5, wy1 - 0.5), 9.5), &ui.brush(wh(0.07)), 1.0, None);
        }
        let rcy = (wy0 + wy1) / 2.0;
        let (c1, c2) = (bx0 + 84.0, bx1 - 84.0);
        let (g1, g2, gy) = (wx0 + 10.0, wx1 - 10.0, wy1 + 9.0);
        let tape = ui.brush(rgba(0.30, 0.28, 0.26, 1.0));
        let st = round_stroke(rt);
        unsafe {
            rt.DrawLine(v2(c1 - rl * 0.75, rcy + rl * 0.66), v2(g1, gy), &tape, 1.4, st.as_ref());
            rt.DrawLine(v2(g1, gy), v2(g2, gy), &tape, 1.4, st.as_ref());
            rt.DrawLine(v2(g2, gy), v2(c2 + rr * 0.75, rcy + rr * 0.66), &tape, 1.4, st.as_ref());
        }
        Self::reel(ui, c1, rcy, rl, self.ang[0]);
        Self::reel(ui, c2, rcy, rr, self.ang[1]);
        for gx in [g1, g2] { dot(rt, gx, gy, 2.6, &ui.brush(wh(0.35))); }
        // Cabezal
        unsafe { rt.FillRoundedRectangle(&rrect(rect((bx0 + bx1) / 2.0 - 16.0, gy - 1.0, (bx0 + bx1) / 2.0 + 16.0, gy + 7.0), 2.5), &ui.brush(wh(0.16))); }
        // Abajo: tiempo transcurrido en puntos y controles
        let el = secs.map(|s| s.0 as u32).unwrap_or(0);
        let ts = format!("{:02}:{:02}", (el / 60).min(99), el % 60);
        let pp = 3.4;
        let ty = by1 - 31.0;
        dot_text(rt, &ts, DotFont::Small, bx0 + 20.0, ty, pp, pp * 0.40, &white, Some((&off, pp * 0.16)));
        let tw = dot_cols(&ts, DotFont::Small) as f32 * pp;
        ui.text(&ui.f.small, if playing { "PLAY" } else { i18n::t("PAUSA") }, bx0 + 26.0 + tw, ty - 3.0, 16.0, if playing { &white } else { &dim });
        if playing { dot(rt, bx0 + 34.0 + tw + ui.p.width(&ui.f.small, "PLAY"), ty + 5.0, 2.4, &accent); }
        let cyb = ty + 6.0;
        for (i, (g, id)) in [("\u{E892}", 1u32), (if playing { "\u{E769}" } else { "\u{E768}" }, 2), ("\u{E893}", 3)].into_iter().enumerate() {
            let bx = bx1 - 92.0 + i as f32 * 34.0;
            let hk = ui.heat(id);
            let pr = 1.0 - 0.12 * desk::press(id);
            unsafe {
                rt.SetTransform(&mulm(&Matrix3x2 { M11: pr, M12: 0.0, M21: 0.0, M22: pr, M31: bx * (1.0 - pr), M32: cyb * (1.0 - pr) }, &base));
                if hk > 0.01 || id == 2 { rt.FillEllipse(&ellipse(bx, cyb, 13.0), &ui.brush(wh(if id == 2 { 0.10 + 0.08 * hk } else { 0.08 * hk }))); }
            }
            ui.glyph(&ui.f.icon, g, bx - 10.0, cyb - 10.0, 20.0, 20.0, &white);
            unsafe { rt.SetTransform(&base); }
            ui.hit(rect(bx - 15.0, cyb - 15.0, bx + 15.0, cyb + 15.0), id);
        }
        unsafe { rt.PopLayer(); }

        // ---- Osciloscopio: suma de senos con las bandas como amplitudes, con estela
        let ks = ez(t, 0.15, 0.6);
        popup::push_alpha(rt, ks);
        let (sx0, sy0, sx1, sy1) = (316.0, 8.0, w - 8.0, 118.0);
        unsafe {
            rt.FillRoundedRectangle(&rrect(rect(sx0, sy0, sx1, sy1), 10.0), &ui.brush(rgba(0.035, 0.036, 0.042, 0.95)));
            rt.DrawRoundedRectangle(&rrect(rect(sx0 + 0.5, sy0 + 0.5, sx1 - 0.5, sy1 - 0.5), 9.5), &ui.brush(wh(0.10)), 1.0, None);
        }
        // Rejilla de puntos
        let gb = ui.brush(wh(0.07));
        let mut gy2 = sy0 + 11.0;
        while gy2 < sy1 - 6.0 { let mut gx = sx0 + 11.0; while gx < sx1 - 6.0 { dot(rt, gx, gy2, 0.8, &gb); gx += 11.0; } gy2 += 11.0; }
        if self.alive > 0.5 { dot(rt, sx1 - 10.0, sy0 + 10.0, 2.6, &accent); }
        let groups = [band_mean(&self.lv, 0, 4), band_mean(&self.lv, 4, 9), band_mean(&self.lv, 9, 15), band_mean(&self.lv, 15, 22), band_mean(&self.lv, 22, 29), band_mean(&self.lv, 29, 36)];
        const FREQ: [f32; 6] = [1.0, 2.0, 3.5, 5.0, 8.0, 13.0];
        const SPEED: [f32; 6] = [1.3, 2.1, 3.4, 4.6, 6.1, 8.3];
        if !reduced() { for i in 0..6 { self.phase[i] = (self.phase[i] + dt * SPEED[i]) % TAU; } }
        let amp = (sy1 - sy0) * 0.40;
        let mut cur = [0.0f32; WN];
        for (x, c) in cur.iter_mut().enumerate() {
            let u = x as f32 / (WN - 1) as f32;
            let mut v = 0.0;
            for i in 0..6 { v += groups[i] * (TAU * FREQ[i] * u + self.phase[i] + i as f32).sin() / (1.0 + i as f32 * 0.35); }
            // Se apaga suave hacia los bordes
            let edge = (u * (1.0 - u) * 4.0).min(1.0).sqrt();
            *c = (v * 0.55).clamp(-1.0, 1.0) * edge;
        }
        self.trail_t += dt;
        if self.trail_t >= 0.045 { self.trail_t = 0.0; self.head = (self.head + 1) % TRAIL; }
        self.wave[self.head] = cur;
        let (mid, sw) = ((sy0 + sy1) / 2.0, (sx1 - sx0 - 20.0) / (WN - 1) as f32);
        for age in (0..TRAIL).rev() {
            let wv = &self.wave[(self.head + TRAIL - age) % TRAIL];
            let (b, width) = if age == 0 { (ui.brush(wh(0.95)), 1.6) } else { (ui.brush(wh(q(0.30 - 0.05 * age as f32, 20.0))), 1.2) };
            for x in 1..WN {
                let (xa, xb) = (sx0 + 10.0 + (x - 1) as f32 * sw, sx0 + 10.0 + x as f32 * sw);
                unsafe { rt.DrawLine(v2(xa, mid - wv[x - 1] * amp), v2(xb, mid - wv[x] * amp), &b, width, st.as_ref()); }
            }
        }
        // Contadores de puntos: graves, medios y agudos
        let bands = [(i18n::t("graves"), band_mean(&self.lv, 0, 8)), (i18n::t("medios"), band_mean(&self.lv, 8, 22)), (i18n::t("agudos"), band_mean(&self.lv, 22, 36))];
        let n = 18usize;
        let dx0 = sx0 + 60.0;
        let pitch = (sx1 - 6.0 - dx0) / (n - 1) as f32;
        for (i, (lab, v)) in bands.iter().enumerate() {
            let y = sy1 + 20.0 + i as f32 * 21.0;
            ui.text(&ui.f.small, lab, sx0 + 2.0, y - 9.0, 18.0, &dim);
            let on = (v * 1.25 * n as f32).round().clamp(0.0, n as f32) as usize;
            dot_row(rt, dx0, y, n, on, pitch, 2.4, &white, &off);
        }
        unsafe { rt.PopLayer(); }
        ui.shadow = halo;
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

/// a · b (primero a, luego b) para transformaciones 2D de Direct2D.
fn mulm(a: &Matrix3x2, b: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: a.M11 * b.M11 + a.M12 * b.M21, M12: a.M11 * b.M12 + a.M12 * b.M22,
        M21: a.M21 * b.M11 + a.M22 * b.M21, M22: a.M21 * b.M12 + a.M22 * b.M22,
        M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31, M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    }
}

// ================================================================ Almacenamiento: «Matriz»
const MDOTS: usize = 36;

/// Cuántos puntos de `n` van encendidos para un uso `frac` (al menos uno si hay algo usado).
fn used_dots(frac: f32, n: usize) -> usize {
    if frac <= 0.0 { return 0; }
    ((frac.clamp(0.0, 1.0) * n as f32).round() as usize).clamp(1, n)
}

/// Cada unidad como una barra de puntos: lo usado en blanco, lo libre tenue y, por encima del 90 %, en el acento.
#[derive(Default)]
pub struct DriveDotsW { fills: Vec<f32>, moving: bool }

impl Widget for DriveDotsW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (w, _h) = size(Kind::Storage);
        let st = deskdata::STORE.lock().unwrap().clone();
        let n = st.as_ref().map(|s| s.drives.len()).unwrap_or(0);
        deskw::header(ui, d, i18n::t("Almacenamiento"), &match n { 0 => String::new(), 1 => i18n::t("1 unidad").into(), n => i18n::tf("{} unidades", &[&n]) }, w);
        let Some(st) = st else { return };
        let rt = ui.rt;
        let pad = 20.0;
        let mut y = 54.0;
        self.fills.resize(st.drives.len().min(3), 0.0);
        self.moving = false;
        let (b_on, b_off, b_acc) = (ui.white.clone(), ui.wa(0.13), ui.brush(acc(1.0)));
        for (i, dr) in st.drives.iter().take(3).enumerate() {
            let used = dr.total.saturating_sub(dr.free);
            let frac = used as f32 / dr.total.max(1) as f32;
            let letter = dr.root.trim_end_matches('\\');
            let name = if dr.label.is_empty() { i18n::tf("Disco ({})", &[&letter]) } else { format!("{} ({letter})", dr.label) };
            let id = 40 + i as u32;
            let hk = ui.heat(id);
            ui.card_k(rect(pad - 8.0, y - 5.0, w - pad + 8.0, y + 40.0), hk);
            let free = i18n::tf("{} libres", &[&deskdata::fmt_bytes(dr.free)]);
            let fw = ui.p.width(&ui.f.sub, &free);
            ui.text(&ui.f.name, &fit(ui, &ui.f.name, &name, w - 2.0 * pad - fw - 12.0), pad + 3.0 * hk, y, 20.0, &ui.white);
            ui.text_r(&ui.f.sub, &free, w - pad, y, 20.0, &ui.dim);
            // Se llena en cascada al aparecer; después sigue al dato con suavidad
            let k = ez(t, 0.15 + i as f32 * 0.12, 0.9);
            if t < 1.1 { self.fills[i] = frac * k; } else { self.moving |= approach(&mut self.fills[i], frac, dt, 0.4); }
            let on = used_dots(self.fills[i], MDOTS);
            let full = frac >= 0.9;
            let pitch = (w - 2.0 * pad - 6.0) / (MDOTS - 1) as f32;
            let r = pitch * 0.34 * (1.0 + 0.12 * hk);
            dot_row(rt, pad + 3.0, y + 29.0, MDOTS, on, pitch, r, if full { &b_acc } else { &b_on }, &b_off);
            ui.hit(rect(pad - 8.0, y - 5.0, w - pad + 8.0, y + 40.0), id);
            y += 48.0;
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
    fn digitos_de_puntos() {
        // Todos los dígitos existen en las dos letras, con su ancho y alto
        for c in '0'..='9' {
            let (w, rows) = glyph(c, DotFont::Small).unwrap();
            assert_eq!((w, rows.len()), (3, 5));
            assert!(rows.iter().all(|r| *r < 8));
            let (w, rows) = glyph(c, DotFont::Big).unwrap();
            assert_eq!((w, rows.len()), (5, 7));
            assert!(rows.iter().all(|r| *r < 32));
        }
        assert!(glyph('x', DotFont::Small).is_none());
        // «12:34» en pequeña: 4 dígitos de 3 + «:» de 1 + 4 separaciones = 17 columnas
        assert_eq!(dot_cols("12:34", DotFont::Small), 17);
        assert_eq!(dot_cols("00", DotFont::Big), 11);
        assert_eq!(dot_rows(DotFont::Big), 7);
        // El 1 pequeño: columna del medio encendida en todas las filas
        let (w, one) = glyph('1', DotFont::Small).unwrap();
        assert!((0..5).all(|r| glyph_on(one, w, 1, r)));
    }

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
    fn vida_se_resiembra() {
        let mut l = LifeW::default();
        assert!(l.pop > 0);
        // Vacía: al dar un paso se vuelve a sembrar
        l.cells.iter_mut().for_each(|c| *c = false);
        l.step();
        assert!(l.pop > 0 && l.gens == 0);
    }

    #[test]
    fn esfera_de_fibonacci() {
        let p = fib_sphere(ORB_N);
        assert_eq!(p.len(), ORB_N);
        // Todos sobre la esfera y repartidos (el centro de masas cerca del centro)
        for v in &p { assert!(((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt() - 1.0).abs() < 1e-4); }
        let c: [f32; 3] = p.iter().fold([0.0; 3], |a, v| [a[0] + v[0], a[1] + v[1], a[2] + v[2]]);
        for k in c { assert!((k / ORB_N as f32).abs() < 0.02); }
        // Girar no cambia la distancia al centro
        let r = rotate([0.3, 0.4, 0.866], 1.2, -0.36);
        assert!(((r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn reloj_ojo() {
        // 10:58 → arriba el 1 y el 0, abajo el 5 y el 8; nada en la fila del medio ni fuera de los dígitos
        assert!(eye_digit_on(-3, -7, 10, 58)); // el 1 de arriba: columna del medio, primera fila
        assert!(!eye_digit_on(0, -4, 10, 58));
        assert!(!eye_digit_on(-3, 0, 10, 58));
        assert!(!eye_digit_on(7, 3, 10, 58));
        // Los dígitos no tocan la banda de los segundos
        for j in -10..=10i32 { for i in -10..=10i32 {
            if eye_digit_on(i, j, 88, 88) { assert!((((i * i + j * j) as f32).sqrt()) < EYE_BAND); }
        } }
        // Ángulos: arriba 0, derecha un cuarto, abajo media vuelta
        assert!(eye_angle(0, -10).abs() < 1e-5);
        assert!((eye_angle(10, 0) - TAU / 4.0).abs() < 1e-5);
        assert!((eye_angle(0, 10) - TAU / 2.0).abs() < 1e-5);
        assert!((eye_angle(-10, 0) - 3.0 * TAU / 4.0).abs() < 1e-5);
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
    fn textos_del_equipo() {
        assert_eq!(short_cpu("AMD Ryzen 7 5800H with Radeon Graphics"), "AMD Ryzen 7 5800H");
        assert_eq!(short_cpu("Intel(R) Core(TM) i7-9750H CPU @ 2.60GHz"), "Intel Core i7-9750H");
        assert_eq!(short_cpu("AMD Ryzen 9 5900X 12-Core Processor"), "AMD Ryzen 9 5900X");
        assert_eq!(short_cpu("AMD Ryzen AI 7 350 w/ Radeon 860M"), "AMD Ryzen AI 7 350");
        assert_eq!(title_case("OMEN-PELAYO"), "Omen-Pelayo");
        assert_eq!(uptime_clock(3 * 3600 + 25 * 60 + 9), "03:25:09");
        assert_eq!(uptime_clock(125 * 3600), "125:00:00");
        assert_eq!(uptime_words(90), "1 min");
        assert_eq!(uptime_words(2 * 86400 + 3 * 3600 + 4 * 60), "2 d 3 h 4 min");
        assert_eq!(used_dots(0.0, 36), 0);
        assert_eq!(used_dots(0.001, 36), 1);
        assert_eq!(used_dots(0.5, 36), 18);
        assert_eq!(used_dots(3.0, 36), 36);
    }
}
