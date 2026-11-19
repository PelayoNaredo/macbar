//! Extras del buscador: historial del portapapeles, conversión de unidades y accesos rápidos
//! («yt gatos», «gh macbar», «github.com»…).

use super::*;

// ---------------------------------------------------------------- portapapeles
/// Últimos textos copiados (solo en memoria: no se guardan en disco). (texto, hora unix)
static CLIPS: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());
const CLIP_MAX: usize = 40;

/// Vigila el portapapeles (mira su número de secuencia; no lo abre si no cambió).
pub fn clip_start() {
    std::thread::Builder::new().name("portapapeles".into()).spawn(|| {
        use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
        let mut last = unsafe { GetClipboardSequenceNumber() };
        loop {
            std::thread::sleep(Duration::from_millis(700));
            let n = unsafe { GetClipboardSequenceNumber() };
            if n == last { continue; }
            last = n;
            // Gestores de contraseñas y apps que piden no ser recordadas
            if private_clip() { continue; }
            let Some(t) = clip_text(4000) else { continue };
            let t = t.trim().to_string();
            if t.is_empty() || looks_secret(&t) { continue; }
            let mut c = CLIPS.lock().unwrap();
            c.retain(|x| x.0 != t);
            c.insert(0, (t, deskdata::now_unix()));
            c.truncate(CLIP_MAX);
        }
    }).ok();
}

/// Lo copiado, salvo en modo presentación (la pantalla se está enseñando a otros).
pub fn clips() -> Vec<(String, u64)> {
    if crate::profiles::presenting() { return Vec::new(); }
    CLIPS.lock().unwrap().clone()
}

/// ¿Parece una contraseña, clave o token? (una sola «palabra» de 16 a 200 caracteres que mezcla
/// letras y cifras o símbolos, sin pinta de dirección ni de ruta): esas no se guardan nunca.
fn looks_secret(s: &str) -> bool {
    let n = s.chars().count();
    if s.contains(char::is_whitespace) || !(16..=200).contains(&n) { return false; }
    if s.contains("://") || s.starts_with("www.") || s.contains(":\\") || s.starts_with("\\\\") { return false; }
    let has = |f: fn(&char) -> bool| s.chars().any(|c| f(&c));
    let classes = [has(|c| c.is_ascii_lowercase()), has(|c| c.is_ascii_uppercase()), has(|c| c.is_ascii_digit()), has(|c| !c.is_alphanumeric())].iter().filter(|x| **x).count();
    classes >= 3 || (classes == 2 && has(|c| c.is_ascii_digit()) && n >= 24)
}

/// ¿Lo copiado pide no entrar en historiales? (formatos estándar que ponen KeePass, 1Password, Proton Pass…)
fn private_clip() -> bool {
    use windows::core::w;
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    unsafe {
        for f in [w!("ExcludeClipboardContentFromMonitorProcessing"), w!("Clipboard Viewer Ignore")] {
            let id = RegisterClipboardFormatW(f);
            if id != 0 && IsClipboardFormatAvailable(id).is_ok() { return true; }
        }
        let id = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
        if id != 0 && IsClipboardFormatAvailable(id).is_ok() {
            // Un DWORD: 0 = no se puede guardar
            if OpenClipboard(None).is_err() { return true; }
            let mut no = true;
            if let Ok(h) = GetClipboardData(id) {
                let p = GlobalLock(HGLOBAL(h.0)) as *const u32;
                if !p.is_null() { no = *p == 0; let _ = GlobalUnlock(HGLOBAL(h.0)); }
            }
            let _ = CloseClipboard();
            if no { return true; }
        }
    }
    false
}

/// Texto del portapapeles (como mucho `max` caracteres).
pub fn clip_text(max: usize) -> Option<String> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    unsafe {
        // Otro programa puede tenerlo abierto un instante
        let mut ok = false;
        for _ in 0..5 { if OpenClipboard(None).is_ok() { ok = true; break; } std::thread::sleep(Duration::from_millis(30)); }
        if !ok { return None; }
        let mut out = None;
        if let Ok(h) = GetClipboardData(13 /* CF_UNICODETEXT */) {
            let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
            if !p.is_null() {
                let mut n = 0; while *p.add(n) != 0 && n < max { n += 1; }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                let _ = GlobalUnlock(HGLOBAL(h.0));
            }
        }
        let _ = CloseClipboard();
        out
    }
}

// ---------------------------------------------------------------- unidades
/// (nombres, magnitud, factor a la unidad base). La temperatura va aparte.
const UNITS: &[(&[&str], u8, f64)] = &[
    (&["mm", "milimetro", "milimetros", "millimeter", "millimeters"], 0, 0.001),
    (&["cm", "centimetro", "centimetros", "centimeter", "centimeters"], 0, 0.01),
    (&["m", "metro", "metros", "meter", "meters", "metre", "metres"], 0, 1.0),
    (&["km", "kilometro", "kilometros", "kilometer", "kilometers"], 0, 1000.0),
    (&["in", "pulgada", "pulgadas", "inch", "inches", "\""], 0, 0.0254),
    (&["ft", "pie", "pies", "foot", "feet"], 0, 0.3048),
    (&["yd", "yarda", "yardas", "yard", "yards"], 0, 0.9144),
    (&["mi", "milla", "millas", "mile", "miles"], 0, 1609.344),
    (&["mg", "miligramo", "miligramos", "milligram", "milligrams"], 1, 0.001),
    (&["g", "gr", "gramo", "gramos", "gram", "grams"], 1, 1.0),
    (&["kg", "kilo", "kilos", "kilogramo", "kilogramos", "kilogram", "kilograms"], 1, 1000.0),
    (&["t", "tonelada", "toneladas", "tonne", "tonnes"], 1, 1e6),
    (&["oz", "onza", "onzas", "ounce", "ounces"], 1, 28.349523125),
    (&["lb", "lbs", "libra", "libras", "pound", "pounds"], 1, 453.59237),
    (&["ml", "mililitro", "mililitros", "milliliter", "milliliters"], 2, 0.001),
    (&["cl", "centilitro", "centilitros"], 2, 0.01),
    (&["l", "litro", "litros", "liter", "liters", "litre", "litres"], 2, 1.0),
    (&["gal", "galon", "galones", "gallon", "gallons"], 2, 3.785411784),
    (&["b", "byte", "bytes"], 3, 1.0),
    (&["kb"], 3, 1e3), (&["mb"], 3, 1e6), (&["gb"], 3, 1e9), (&["tb"], 3, 1e12),
    (&["kib"], 3, 1024.0), (&["mib"], 3, 1048576.0), (&["gib"], 3, 1073741824.0), (&["tib"], 3, 1099511627776.0),
    (&["ms", "milisegundo", "milisegundos", "millisecond", "milliseconds"], 4, 0.001),
    (&["s", "seg", "segundo", "segundos", "second", "seconds", "sec"], 4, 1.0),
    (&["min", "minuto", "minutos", "minute", "minutes"], 4, 60.0),
    (&["h", "hora", "horas", "hour", "hours"], 4, 3600.0),
    (&["d", "dia", "dias", "day", "days"], 4, 86400.0),
    (&["sem", "semana", "semanas", "week", "weeks"], 4, 604800.0),
    (&["kmh", "km/h"], 5, 1.0 / 3.6), (&["mph"], 5, 0.44704), (&["m/s", "mps"], 5, 1.0), (&["nudo", "nudos", "knot", "knots", "kn"], 5, 0.514444),
];

fn unit(s: &str) -> Option<(u8, f64, &'static str)> {
    UNITS.iter().find(|u| u.0.contains(&s)).map(|u| (u.1, u.2, u.0[0]))
}
fn temp(s: &str) -> Option<char> {
    match s { "c" | "ºc" | "°c" | "celsius" | "centigrados" => Some('c'), "f" | "ºf" | "°f" | "fahrenheit" => Some('f'), "k" | "kelvin" => Some('k'), _ => None }
}

/// Número bonito: sin decimales si es entero, como mucho 4 si no; coma decimal en español.
pub fn fmt_num(v: f64) -> String {
    let t = if (v - v.round()).abs() < 1e-9 && v.abs() < 1e15 { format!("{}", v.round() as i64) } else {
        let d = if v.abs() >= 100.0 { 2 } else { 4 };
        let t = format!("{v:.d$}");
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    if i18n::en() { t } else { t.replace('.', ",") }
}

/// «10 km a mi», «5kg en lb», «30 c to f», «2,5 h en min» → «6,2137 mi».
pub fn convert(q: &str) -> Option<String> {
    let q = q.trim().to_lowercase().replace(',', ".");
    let (lhs, rhs) = [" a ", " en ", " to ", " in ", " -> ", " = "].iter().find_map(|sep| q.split_once(sep))?;
    let lhs = lhs.trim();
    let rhs = rhs.trim();
    // Número y unidad, juntos o separados
    let cut = lhs.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(lhs.len());
    let v: f64 = lhs[..cut].trim().parse().ok()?;
    let from = lhs[cut..].trim();
    if from.is_empty() || rhs.is_empty() { return None; }
    if let (Some(a), Some(b)) = (temp(from), temp(rhs)) {
        let c = match a { 'c' => v, 'f' => (v - 32.0) * 5.0 / 9.0, _ => v - 273.15 };
        let r = match b { 'c' => c, 'f' => c * 9.0 / 5.0 + 32.0, _ => c + 273.15 };
        let sym = match b { 'c' => "°C", 'f' => "°F", _ => "K" };
        return Some(format!("{} {sym}", fmt_num(r)));
    }
    let (ka, fa, _) = unit(from)?;
    let (kb, fb, name) = unit(rhs)?;
    if ka != kb { return None; }
    Some(format!("{} {name}", fmt_num(v * fa / fb)))
}

// ---------------------------------------------------------------- accesos rápidos
/// (prefijo, nombre, url de búsqueda)
const LINKS: &[(&str, &str, &str)] = &[
    ("yt", "YouTube", "https://www.youtube.com/results?search_query="),
    ("gh", "GitHub", "https://github.com/search?q="),
    ("w", "Wikipedia", "https://{lang}.wikipedia.org/w/index.php?search="),
    ("maps", "Google Maps", "https://www.google.com/maps/search/"),
    ("tr", "Google Translate", "https://translate.google.com/?text="),
    ("r", "Reddit", "https://www.reddit.com/search/?q="),
    ("img", "Google Imágenes", "https://www.google.com/search?tbm=isch&q="),
    ("so", "Stack Overflow", "https://stackoverflow.com/search?q="),
    ("crates", "crates.io", "https://crates.io/search?q="),
    ("npm", "npm", "https://www.npmjs.com/search?q="),
];

/// «yt gatos» → (título, url). También una dirección escrita tal cual («github.com/x»).
pub fn quicklink(q: &str, enc: fn(&str) -> String) -> Option<(String, String, String)> {
    let q = q.trim();
    if let Some((p, rest)) = q.split_once(' ') {
        let rest = rest.trim();
        if !rest.is_empty() {
            if let Some(l) = LINKS.iter().find(|l| l.0.eq_ignore_ascii_case(p)) {
                let url = l.2.replace("{lang}", if i18n::en() { "en" } else { "es" }) + &enc(rest);
                return Some((i18n::tf("Buscar «{}» en {}", &[&trunc(rest, 40), &l.1]), l.1.to_string(), url));
            }
        }
    }
    // Dirección: sin espacios, con punto y una terminación de letras («github.com», «x.dev/algo»)
    if !q.contains(' ') && q.len() >= 4 && !q.contains('@') {
        let bare = q.trim_start_matches("https://").trim_start_matches("http://");
        let host = bare.split('/').next().unwrap_or("");
        let tld = host.rsplit('.').next().unwrap_or("");
        if host.contains('.') && tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
            && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') && !host.starts_with('.') {
            let url = if q.starts_with("http") { q.to_string() } else { format!("https://{q}") };
            return Some((i18n::tf("Abrir {}", &[&trunc(bare, 50)]), i18n::t("Navegador").to_string(), url));
        }
    }
    None
}

// ---------------------------------------------------------------- ventanas
/// (código, nombre, palabras clave en español, en inglés): colocar la ventana que estaba delante.
pub const WIN_ACTS: &[(&str, &str, &str, &str)] = &[
    ("left", "Ventana a la mitad izquierda", "izquierda mitad lado", "left half side"),
    ("right", "Ventana a la mitad derecha", "derecha mitad lado", "right half side"),
    ("top", "Ventana a la mitad de arriba", "arriba superior mitad", "top upper half"),
    ("bottom", "Ventana a la mitad de abajo", "abajo inferior mitad", "bottom lower half"),
    ("tl", "Ventana arriba a la izquierda", "esquina cuarto arriba izquierda", "corner quarter top left"),
    ("tr", "Ventana arriba a la derecha", "esquina cuarto arriba derecha", "corner quarter top right"),
    ("bl", "Ventana abajo a la izquierda", "esquina cuarto abajo izquierda", "corner quarter bottom left"),
    ("br", "Ventana abajo a la derecha", "esquina cuarto abajo derecha", "corner quarter bottom right"),
    ("max", "Maximizar ventana", "maximizar grande pantalla completa", "maximize full"),
    ("almost", "Ventana casi maximizada", "casi grande", "almost maximize"),
    ("center", "Centrar ventana", "centrar centro medio", "center middle"),
    ("min", "Minimizar ventana", "minimizar ocultar", "minimize hide"),
];

/// Coloca la ventana `h` según `code`, dentro del área de trabajo de su pantalla.
pub fn win_apply(code: &str, h: HWND) {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::Graphics::Dwm::*;
    unsafe {
        if h.0.is_null() || !IsWindow(Some(h)).as_bool() { return; }
        match code {
            "min" => { let _ = ShowWindow(h, SW_MINIMIZE); return; }
            "max" => { let _ = ShowWindow(h, SW_MAXIMIZE); let _ = SetForegroundWindow(h); return; }
            _ => {}
        }
        let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() { return; }
        let wa = mi.rcWork;
        let (x, y, w, hh) = (wa.left, wa.top, wa.right - wa.left, wa.bottom - wa.top);
        let _ = ShowWindow(h, SW_RESTORE);
        // Windows 11 deja bordes invisibles alrededor: se mide el marco visible para que encaje justo
        let mut wr = RECT::default();
        let _ = GetWindowRect(h, &mut wr);
        let mut fr = wr;
        let _ = DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut fr as *mut _ as _, std::mem::size_of::<RECT>() as u32);
        let (bl, bt, br, bb) = (fr.left - wr.left, fr.top - wr.top, wr.right - fr.right, wr.bottom - fr.bottom);
        let (fw, fh) = (fr.right - fr.left, fr.bottom - fr.top);
        let (w2, h2) = (w / 2, hh / 2);
        let r = match code {
            "left" => (x, y, w2, hh), "right" => (x + w2, y, w - w2, hh),
            "top" => (x, y, w, h2), "bottom" => (x, y + h2, w, hh - h2),
            "tl" => (x, y, w2, h2), "tr" => (x + w2, y, w - w2, h2),
            "bl" => (x, y + h2, w2, hh - h2), "br" => (x + w2, y + h2, w - w2, hh - h2),
            "almost" => (x + w / 20, y + hh / 20, w * 9 / 10, hh * 9 / 10),
            "center" => { let (fw, fh) = (fw.min(w), fh.min(hh)); (x + (w - fw) / 2, y + (hh - fh) / 2, fw, fh) }
            _ => return,
        };
        let _ = SetWindowPos(h, None, r.0 - bl, r.1 - bt, r.2 + bl + br, r.3 + bt + bb, SWP_NOZORDER | SWP_NOACTIVATE);
        let _ = SetForegroundWindow(h);
    }
}

/// Título de una ventana (para decir a cuál se aplica).
pub fn win_title(h: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}
// ---------------------------------------------------------------- temporizador
/// «5 min», «temporizador 10», «timer 90s», «25m», «1h 30m» → segundos. Un número suelto no cuenta
/// (choca con la calculadora): hace falta unidad o la palabra «temporizador».
pub fn timer_secs(q: &str) -> Option<u32> {
    let q = q.trim().to_lowercase();
    let (kw, rest) = ["temporizador", "timer", "cuenta atrás", "alarma"].iter()
        .find_map(|k| q.strip_prefix(k).map(|r| (true, r.trim().to_string()))).unwrap_or((false, q.clone()));
    if rest.is_empty() { return None; }
    let mut total = 0u64;
    let mut any_unit = false;
    let mut num = String::new();
    let mut chars = rest.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() { num.push(c); continue; }
        if c == ' ' && chars.peek().is_some_and(|n| n.is_ascii_digit()) && !num.is_empty() { return None; }
        if c.is_whitespace() { continue; }
        let mut unit = c.to_string();
        while let Some(&n) = chars.peek() { if n.is_alphabetic() { unit.push(n); chars.next(); } else { break; } }
        let mult = match unit.as_str() { "s" | "seg" | "segs" | "segundo" | "segundos" | "sec" | "secs" => 1, "m" | "min" | "mins" | "minuto" | "minutos" | "minute" | "minutes" => 60, "h" | "hora" | "horas" | "hour" | "hours" => 3600, _ => return None };
        let n: u64 = num.parse().ok()?;
        total += n * mult;
        num.clear();
        any_unit = true;
    }
    if !num.is_empty() { if kw && !any_unit { total += num.parse::<u64>().ok()? * 60; } else { return None; } }
    (total > 0 && total <= 24 * 3600).then_some(total as u32)
}

/// «cafeína 2 h», «despierto 30 min»: segundos que hay que mantener el equipo despierto.
pub fn caffeine_secs(q: &str) -> Option<u32> {
    let q = q.trim().to_lowercase();
    let rest = ["cafeína", "cafeina", "caffeine", "despierto"].iter().find_map(|k| q.strip_prefix(k))?.trim();
    if rest.is_empty() { return None; }
    timer_secs(&format!("timer {rest}")).filter(|s| *s >= 60)
}

// ---------------------------------------------------------------- utilidades de desarrollador
/// Bytes aleatorios del generador criptográfico del sistema (contraseñas y UUID).
fn os_random(buf: &mut [u8]) {
    use windows::Win32::Security::Cryptography::*;
    unsafe { let _ = BCryptGenRandom(None, buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG); }
}

fn rand64() -> u64 { let mut b = [0u8; 8]; os_random(&mut b); u64::from_le_bytes(b) }

fn uuid4() -> String {
    let (a, b) = (rand64(), rand64());
    let a = (a & 0xFFFF_FFFF_FFFF_0FFF) | 0x4000;
    let b = (b & 0x3FFF_FFFF_FFFF_FFFF) | 0x8000_0000_0000_0000;
    format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", a >> 32, (a >> 16) & 0xFFFF, a & 0xFFFF, b >> 48, b & 0xFFFF_FFFF_FFFF)
}

fn password(n: usize) -> String {
    // Sin caracteres que se confunden (0/O, 1/l/I)
    const CH: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789-_.!?#";
    loop {
        // Muestreo por rechazo: sin sesgo hacia los primeros caracteres
        let lim = (256 / CH.len() * CH.len()) as u8;
        let p: String = (0..n).map(|_| loop { let mut b = [0u8; 1]; os_random(&mut b); if b[0] < lim { break CH[b[0] as usize % CH.len()] as char; } }).collect();
        // Que tenga de todo: minúscula, mayúscula, cifra y símbolo
        if p.chars().any(|c| c.is_ascii_lowercase()) && p.chars().any(|c| c.is_ascii_uppercase()) && p.chars().any(|c| c.is_ascii_digit()) && p.chars().any(|c| !c.is_alphanumeric()) { return p; }
    }
}

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::new();
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 { if i <= c.len() { out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char); } else { out.push('='); } }
    }
    out
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s: Vec<u8> = s.trim().bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let s = s.iter().map(|&b| match b { b'-' => b'+', b'_' => b'/', x => x }).collect::<Vec<u8>>();
    let body: Vec<u8> = s.iter().copied().take_while(|&b| b != b'=').collect();
    if body.is_empty() || s[body.len()..].iter().any(|&b| b != b'=') || body.len() % 4 == 1 { return None; }
    let mut out = Vec::new();
    for c in body.chunks(4) {
        let mut n = 0u32;
        for (i, &b) in c.iter().enumerate() { n |= (B64.iter().position(|&x| x == b)? as u32) << (18 - 6 * i); }
        for i in 0..c.len() - 1 { out.push((n >> (16 - 8 * i)) as u8); }
    }
    Some(out)
}

/// Días desde 1970-01-01 a (año, mes, día) (algoritmo de Howard Hinnant).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub fn ts_text(secs: i64) -> String {
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let r = secs.rem_euclid(86_400);
    format!("{d:02}/{m:02}/{y} {:02}:{:02}:{:02} UTC", r / 3600, r / 60 % 60, r % 60)
}

fn local_ip() -> Option<String> {
    // Conectar un socket UDP no manda nada: solo elige la interfaz de salida
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

/// Prefijo del texto a copiar que pide copiarlo como privado (fuera de cualquier historial).
pub const SECRET: &str = "\u{1}secreto:";

/// Utilidades al estilo de Raycast: (título, subtítulo, texto que se copia con Intro).
pub fn dev_tools(q: &str) -> Vec<(String, String, String)> {
    let t = q.trim();
    let low = t.to_lowercase();
    let (cmd, arg) = match low.split_once(' ') { Some((c, _)) => (c.to_string(), t[c.len()..].trim().to_string()), None => (low.clone(), String::new()) };
    let mut out = Vec::new();
    let copy = i18n::t("Intro para copiar").to_string();
    match cmd.as_str() {
        "uuid" | "guid" if arg.is_empty() => { let u = uuid4(); out.push((u.clone(), format!("UUID v4 · {copy}"), u)); }
        "contraseña" | "contrasena" | "password" | "pass" | "clave" => {
            let n = arg.parse::<usize>().unwrap_or(20).clamp(8, 64);
            let p = password(n);
            out.push((p.clone(), i18n::tf("Contraseña de {} caracteres · no se guarda en el historial", &[&n]), format!("{SECRET}{p}")));
        }
        "b64" | "base64" if !arg.is_empty() => {
            let e = b64_encode(arg.as_bytes());
            out.push((e.clone(), format!("Base64 · {copy}"), e));
            if let Some(d) = b64_decode(&arg).and_then(|v| String::from_utf8(v).ok()).filter(|d| !d.chars().any(|c| c.is_control() && c != '\n')) {
                out.push((d.clone(), format!("{} · {copy}", i18n::t("Base64 decodificado")), d));
            }
        }
        "ts" | "timestamp" | "unix" => match arg.parse::<i64>() {
            Ok(n) => { let secs = if n.abs() > 100_000_000_000 { n / 1000 } else { n }; let txt = ts_text(secs); out.push((txt.clone(), format!("{} · {copy}", i18n::t("Fecha del timestamp")), txt)); }
            Err(_) if arg.is_empty() => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                out.push((now.to_string(), format!("{} · {}", i18n::t("Timestamp de ahora"), ts_text(now)), now.to_string()));
            }
            _ => {}
        },
        "ip" if arg.is_empty() => if let Some(ip) = local_ip() { out.push((ip.clone(), format!("{} · {copy}", i18n::t("IP local")), ip)); },
        "lorem" => {
            let l = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat.";
            out.push(("Lorem ipsum dolor sit amet…".into(), format!("{} · {copy}", i18n::t("Texto de relleno")), l.into()));
        }
        _ => {}
    }
    out
}

// ---------------------------------------------------------------- comandos del sistema
/// (código, nombre, palabras clave en español, en inglés, glifo)
pub const CMDS: &[(&str, &str, &str, &str, &str)] = &[
    ("lock", "Bloquear el equipo", "bloquear bloqueo cerrar sesion", "lock", "\u{E72E}"),
    ("sleep", "Suspender", "suspender dormir reposo", "sleep suspend", "\u{E708}"),
    ("dark", "Cambiar entre modo claro y oscuro", "modo oscuro claro tema", "dark light mode theme", "\u{E793}"),
    ("mute", "Silenciar o activar el sonido", "silenciar silencio sonido mute", "mute sound", "\u{E74F}"),
    ("shot", "Recortar la pantalla", "captura recorte pantallazo recortes", "screenshot snip capture", "\u{E722}"),
    ("desktop", "Mostrar el escritorio", "escritorio minimizar todo", "show desktop", "\u{E7F4}"),
    ("trash", "Vaciar la papelera", "papelera vaciar", "empty trash recycle bin", "\u{E74D}"),
    ("caffeine", "Cafeína: mantener el equipo despierto", "cafeina despierto no dormir suspension pantalla encendida", "caffeine keep awake amphetamine", "\u{EC32}"),
    ("pomodoro", "Pomodoro: 25 min de concentración", "pomodoro concentracion foco estudiar trabajar", "pomodoro focus study", "\u{E916}"),
    ("picker", "Cuentagotas: copiar un color de la pantalla", "cuentagotas color pipeta hex selector", "color picker eyedropper hex", "\u{E790}"),
    ("present", "Modo presentación", "presentacion no molestar silencio", "presentation do not disturb", "\u{E7F4}"),
    ("taskmgr", "Administrador de tareas", "administrador tareas procesos", "task manager processes", "\u{E9D9}"),
    ("restart", "Reiniciar el equipo…", "reiniciar", "restart reboot", "\u{E72C}"),
    ("shutdown", "Apagar el equipo…", "apagar", "shut down power off", "\u{E7E8}"),
];

pub fn run_cmd(code: &str) {
    if let Some(s) = code.strip_prefix("timer:") { if let Ok(n) = s.parse() { live::timer_start(n); } return; }
    // «cafeína 2 h»: despierto durante ese rato
    if let Some(s) = code.strip_prefix("caf:") { if let Ok(n) = s.parse::<u32>() { live::caffeine_set(true, Some(n / 60)); } return; }
    match code {
        "lock" => sys::lock(),
        "sleep" => sys::sleep(),
        "dark" => crate::ccenter::toggle_dark(),
        "mute" => crate::ccenter::toggle_mute(),
        "shot" => sys::open("ms-screenclip:"),
        "desktop" => crate::menus::toggle_desktop(),
        // Sin «no preguntar»: Windows pide confirmación antes de vaciarla
        "trash" => unsafe { let _ = windows::Win32::UI::Shell::SHEmptyRecycleBinW(Some(bar_hwnd()), windows::core::PCWSTR::null(), 0); },
        "present" => crate::profiles::toggle_present(),
        "caffeine" => live::caffeine_toggle(),
        "pomodoro" => live::pomodoro_start(),
        "picker" => crate::pick_color(),
        "taskmgr" => sys::open_args("taskmgr.exe", ""),
        "restart" => if sys::ask(bar_hwnd(), i18n::t("¿Reiniciar el equipo ahora?")) { sys::restart(); },
        "shutdown" => if sys::ask(bar_hwnd(), i18n::t("¿Apagar el equipo ahora?")) { sys::shutdown(); },
        _ => {}
    }
}

// ---------------------------------------------------------------- emojis («:fuego»)
/// (emoji, palabras en español, en inglés)
pub const EMOJIS: &[(&str, &str, &str)] = &[
    ("😀", "sonrisa feliz cara", "grin smile happy face"), ("😂", "risa llorar lagrimas jaja", "joy laugh tears lol"),
    ("🤣", "carcajada revolcarse", "rofl laugh"), ("😊", "sonrojado contento", "blush smile"),
    ("😍", "enamorado corazones ojos", "heart eyes love"), ("😘", "beso", "kiss"),
    ("😎", "gafas guay chulo", "cool sunglasses"), ("🤔", "pensar pensando duda", "thinking hmm"),
    ("😅", "sudor nervioso", "sweat smile"), ("😭", "llorar llanto triste", "cry sob sad"),
    ("😡", "enfadado ira", "angry rage mad"), ("😱", "miedo grito susto", "scream fear shock"),
    ("🙄", "ojos en blanco", "eye roll"), ("😴", "dormir sueño", "sleep tired"),
    ("🤯", "explota cabeza alucinar", "mind blown"), ("🥳", "fiesta celebrar cumpleaños", "party celebrate birthday"),
    ("😬", "mueca incomodo", "grimace awkward"), ("🤗", "abrazo", "hug"),
    ("🙃", "al reves ironia", "upside down"), ("😇", "angel inocente", "angel innocent"),
    ("👍", "ok bien vale pulgar arriba", "thumbs up like ok"), ("👎", "mal pulgar abajo", "thumbs down dislike"),
    ("👏", "aplausos aplaudir", "clap applause"), ("🙏", "gracias por favor rezar", "pray thanks please"),
    ("💪", "fuerza musculo", "strong muscle flex"), ("👋", "hola adios saludo mano", "wave hello bye"),
    ("🤝", "acuerdo trato apreton", "handshake deal"), ("✌️", "paz victoria", "peace victory"),
    ("👀", "ojos mirar", "eyes look"), ("🧠", "cerebro", "brain"),
    ("❤️", "corazon amor rojo", "heart love red"), ("💔", "corazon roto", "broken heart"),
    ("🔥", "fuego llama", "fire lit hot"), ("✨", "brillo destellos magia", "sparkles magic"),
    ("⭐", "estrella", "star"), ("⚡", "rayo electricidad", "zap lightning"),
    ("💯", "cien perfecto", "hundred perfect"), ("✅", "hecho check correcto si", "check done yes"),
    ("❌", "no error cruz mal", "cross no wrong"), ("⚠️", "aviso cuidado alerta", "warning caution"),
    ("❓", "pregunta", "question"), ("‼️", "exclamacion", "exclamation"),
    ("🎉", "fiesta confeti", "tada party confetti"), ("🎁", "regalo", "gift present"),
    ("🚀", "cohete lanzar", "rocket launch ship"), ("💡", "idea bombilla", "idea bulb"),
    ("📌", "chincheta fijar", "pin"), ("📎", "clip adjunto", "paperclip attach"),
    ("📅", "calendario fecha", "calendar date"), ("⏰", "alarma despertador", "alarm clock"),
    ("💻", "portatil ordenador", "laptop computer"), ("📱", "movil telefono", "phone mobile"),
    ("🐛", "bug fallo error bicho", "bug"), ("🛠️", "herramientas arreglar", "tools fix"),
    ("📦", "paquete caja", "package box"), ("🔒", "candado seguro", "lock secure"),
    ("☕", "cafe", "coffee"), ("🍕", "pizza", "pizza"), ("🍺", "cerveza", "beer"),
    ("🎵", "musica nota", "music note"), ("🎮", "juego mando videojuego", "game controller"),
    ("⚽", "futbol balon", "soccer football"), ("🐶", "perro", "dog"), ("🐱", "gato", "cat"),
    ("🌞", "sol", "sun"), ("🌧️", "lluvia", "rain"), ("❄️", "nieve frio", "snow cold"),
    ("🌈", "arcoiris", "rainbow"), ("🇪🇸", "españa bandera", "spain flag"),
    ("💀", "calavera muerto", "skull dead"), ("🤡", "payaso", "clown"), ("👻", "fantasma", "ghost"),
    ("🙌", "manos arriba celebrar", "raised hands"), ("🫡", "saludo militar", "salute"),
];

/// Emojis cuyo nombre empieza por lo escrito tras «:» (ya en minúsculas y sin tildes).
pub fn emoji_matches(q: &str, fold: fn(&str) -> String) -> Vec<(&'static str, String)> {
    let q = q.trim();
    if q.is_empty() { return EMOJIS.iter().take(8).map(|e| (e.0, first_word(if i18n::en() { e.2 } else { e.1 }))).collect(); }
    EMOJIS.iter().filter_map(|(e, es, en)| {
        let words = fold(&format!("{es} {en}"));
        let hit = words.split_whitespace().find(|w| w.starts_with(q))?;
        Some((*e, hit.to_string()))
    }).take(8).collect()
}
fn first_word(s: &str) -> String { s.split_whitespace().next().unwrap_or("").to_string() }

// ---------------------------------------------------------------- colores («#FF8800», «rgb(255, 136, 0)»)
/// (r, g, b) de un color escrito en hexadecimal o rgb().
pub fn color_of(q: &str) -> Option<(u8, u8, u8)> {
    let q = q.trim().to_lowercase();
    if let Some(h) = q.strip_prefix('#') {
        let h: Vec<u8> = h.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect::<Option<_>>()?;
        return match h.len() { 3 => Some((h[0] * 17, h[1] * 17, h[2] * 17)), 6 => Some((h[0] * 16 + h[1], h[2] * 16 + h[3], h[4] * 16 + h[5])), _ => None };
    }
    let inner = q.strip_prefix("rgb(")?.strip_suffix(')')?;
    let v: Vec<u8> = inner.split(',').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
    (v.len() == 3).then(|| (v[0], v[1], v[2]))
}

/// Muestra del color: un círculo de `px`×`px` (BGRA premultiplicado) para el icono de la fila.
pub fn swatch(c: (u8, u8, u8), px: u32) -> Vec<u8> {
    let mut out = vec![0u8; (px * px * 4) as usize];
    let r = px as f32 / 2.0;
    for y in 0..px { for x in 0..px {
        let d = ((x as f32 + 0.5 - r).powi(2) + (y as f32 + 0.5 - r).powi(2)).sqrt();
        let a = (r - d).clamp(0.0, 1.0);
        let i = ((y * px + x) * 4) as usize;
        out[i] = (c.2 as f32 * a) as u8; out[i + 1] = (c.1 as f32 * a) as u8; out[i + 2] = (c.0 as f32 * a) as u8; out[i + 3] = (255.0 * a) as u8;
    } }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conversions() {
        assert!(convert("10 km a mi").unwrap().starts_with("6"));
        assert_eq!(convert("100 c to f").unwrap().replace(',', "."), "212 °F");
        assert!(convert("5kg en lb").unwrap().contains("lb"));
        assert_eq!(convert("1 gib en mib").unwrap().replace(',', "."), "1024 mib");
        assert!(convert("10 km a kg").is_none());
        assert!(convert("hola a todos").is_none());
    }
    #[test]
    fn dev_utilities() {
        assert_eq!(b64_encode(b"hola"), "aG9sYQ==");
        assert_eq!(b64_encode(b"macbar!"), "bWFjYmFyIQ==");
        assert_eq!(b64_decode("aG9sYQ==").unwrap(), b"hola");
        assert_eq!(b64_decode("aG9sYQ").unwrap(), b"hola");
        assert!(b64_decode("a").is_none());
        assert_eq!(ts_text(0), "01/01/1970 00:00:00 UTC");
        assert_eq!(ts_text(1_700_000_000), "14/11/2023 22:13:20 UTC");
        let u = uuid4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!(looks_secret(&password(20)));
        assert_eq!(dev_tools("ts 1700000000000")[0].0, "14/11/2023 22:13:20 UTC");
        assert!(dev_tools("uuid algo").is_empty());
    }

    #[test]
    fn caffeine_durations() {
        assert_eq!(caffeine_secs("cafeína 2h"), Some(7200));
        assert_eq!(caffeine_secs("despierto 30 min"), Some(1800));
        assert_eq!(caffeine_secs("cafeina 45"), Some(2700));
        assert_eq!(caffeine_secs("cafeína"), None);
        assert_eq!(caffeine_secs("caf 2h"), None);
    }

    #[test]
    fn timers_and_colors() {
        assert_eq!(timer_secs("5 min"), Some(300));
        assert_eq!(timer_secs("temporizador 10"), Some(600));
        assert_eq!(timer_secs("timer 90s"), Some(90));
        assert_eq!(timer_secs("1h 30m"), Some(5400));
        assert_eq!(timer_secs("25"), None);
        assert_eq!(timer_secs("hola"), None);
        assert_eq!(timer_secs("5 kg"), None);
        assert_eq!(color_of("#ff8800"), Some((255, 136, 0)));
        assert_eq!(color_of("#fff"), Some((255, 255, 255)));
        assert_eq!(color_of("rgb(1, 2, 3)"), Some((1, 2, 3)));
        assert_eq!(color_of("#zzz"), None);
        let f = |s: &str| s.to_lowercase();
        assert!(emoji_matches("fue", f).iter().any(|e| e.0 == "🔥"));
    }
    #[test]
    fn secrets() {
        assert!(looks_secret("ghp_A1b2C3d4E5f6G7h8I9j0KlMnOpQr"));
        assert!(looks_secret("sk-proj-9fK2mQx7Lp0aZ3vB8nC1"));
        assert!(looks_secret("Contraseña#2026Segura!"));
        assert!(!looks_secret("hola qué tal"));
        assert!(!looks_secret("https://github.com/PelayoNaredo/macbar"));
        assert!(!looks_secret("C:\\Users\\nared\\Desktop\\macbar"));
        assert!(!looks_secret("supercalifragilistico"));
    }
    #[test]
    fn links() {
        let e = |s: &str| s.replace(' ', "+");
        assert!(quicklink("yt gatos monos", e).unwrap().2.ends_with("gatos+monos"));
        assert!(quicklink("github.com", e).unwrap().2 == "https://github.com");
        assert!(quicklink("hola.", e).is_none());
        assert!(quicklink("3.14", e).is_none());
        assert!(quicklink("yo@x.com", e).is_none());
    }
}
