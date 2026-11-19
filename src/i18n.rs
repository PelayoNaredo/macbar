//! Idioma de la interfaz: español (el original) o inglés. Los textos se escriben en español en el código
//! y `t("…")` devuelve su traducción si el idioma es inglés. Con huecos, `tf("hace {} min", &[&n])`.
//! Las tablas están en `i18n_tabla.rs` (español → inglés).

use std::sync::atomic::{AtomicI8, Ordering};

/// 0 = automático (el de Windows), 1 = español, 2 = inglés. -1 = sin leer.
static MODE: AtomicI8 = AtomicI8::new(-1);
/// true = inglés (cacheado; se recalcula al cambiar el modo).
static EN: AtomicI8 = AtomicI8::new(-1);

pub fn mode() -> i8 {
    let v = MODE.load(Ordering::Relaxed);
    if v >= 0 { return v; }
    let m = match crate::sys::config_value("idioma").as_deref().map(str::to_lowercase).as_deref() {
        Some("es") | Some("español") | Some("espanol") => 1,
        Some("en") | Some("english") | Some("inglés") | Some("ingles") => 2,
        _ => 0,
    };
    MODE.store(m, Ordering::Relaxed);
    m
}

/// ¿La interfaz va en inglés?
pub fn en() -> bool {
    let v = EN.load(Ordering::Relaxed);
    if v >= 0 { return v == 1; }
    let e = match mode() {
        1 => false,
        2 => true,
        // Automático: español si Windows está en español; si no, inglés
        _ => unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() & 0x3FF != 0x0A },
    };
    EN.store(e as i8, Ordering::Relaxed);
    e
}

/// Cambia el idioma (menú de macbar) y lo guarda en config.ini como `idioma`.
pub fn set_mode(m: i8) {
    MODE.store(m, Ordering::Relaxed);
    EN.store(-1, Ordering::Relaxed);
    let cfg = crate::sys::config_path();
    let old = std::fs::read_to_string(&cfg).unwrap_or_default();
    let mut lines: Vec<String> = old.lines().filter(|l| !l.trim_start().starts_with("idioma")).map(str::to_string).collect();
    lines.push(format!("idioma = {}", match m { 1 => "es", 2 => "en", _ => "auto" }));
    let _ = std::fs::create_dir_all(crate::sys::config_dir());
    let _ = std::fs::write(&cfg, lines.join("\n") + "\n");
}

/// Traducción de un texto fijo (si no está en la tabla, se queda en español).
pub fn t(s: &'static str) -> &'static str {
    if !en() { return s; }
    crate::i18n_tabla::EN.iter().find(|(es, _)| *es == s).map(|(_, e)| *e).unwrap_or(s)
}

/// Traducción con huecos `{}` que se rellenan en orden.
pub fn tf(s: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::new();
    let mut it = args.iter();
    let mut parts = t(s).split("{}").peekable();
    while let Some(p) = parts.next() {
        out.push_str(p);
        if parts.peek().is_some() { if let Some(a) = it.next() { out.push_str(&a.to_string()); } }
    }
    out
}

/// Nombres de meses y días en el idioma de la interfaz.
pub fn month(i: usize) -> &'static str {
    const ES: [&str; 12] = ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"];
    const EN_: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    if en() { EN_[i % 12] } else { ES[i % 12] }
}
pub fn weekday(i: usize) -> &'static str {
    const ES: [&str; 7] = ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"];
    const EN_: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    if en() { EN_[i % 7] } else { ES[i % 7] }
}
/// Mes abreviado (0-based): "nov" / "Nov".
pub fn month_short(i: usize) -> &'static str {
    const ES: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];
    const EN_: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    if en() { EN_[i % 12] } else { ES[i % 12] }
}
/// Día abreviado (0 = domingo): "Mié" / "Wed".
pub fn weekday_short(i: usize) -> &'static str {
    const ES: [&str; 7] = ["Dom", "Lun", "Mar", "Mié", "Jue", "Vie", "Sáb"];
    const EN_: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    if en() { EN_[i % 7] } else { ES[i % 7] }
}
/// Iniciales de la semana empezando en lunes: "L M X J V S D" / "M T W T F S S".
pub fn weekday_letters() -> [&'static str; 7] {
    if en() { ["M", "T", "W", "T", "F", "S", "S"] } else { ["L", "M", "X", "J", "V", "S", "D"] }
}
/// Fecha "18 nov" / "Nov 18" (mes 0-based).
pub fn day_month_short(day: u32, m: usize) -> String {
    if en() { format!("{} {}", month_short(m), day) } else { format!("{} {}", day, month_short(m)) }
}
