//! Perfiles, temas y modo presentación.
//!
//! - Perfil: una copia de `config.ini` y `widgets.ini` (qué widgets, dónde, ciudad, idioma, acento…).
//!   Cada perfil recuerda cómo lo dejaste: al cambiar a otro, el actual se guarda antes.
//! - Tema: el color de acento y el aspecto de los widgets, en un archivo `.macbartema` para compartir.
//! - Modo presentación: oculta los widgets y calla los avisos de la isla hasta que se apague.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU32};

pub const NAMES: [&str; 3] = ["Trabajo", "Ocio", "Personal"];
const FILES: [&str; 2] = ["config.ini", "widgets.ini"];

fn root() -> String { sys::config_dir() + "\\perfiles" }
fn pdir(i: usize) -> String { format!("{}\\{}", root(), NAMES[i]) }
fn active_path() -> String { root() + "\\activo.txt" }

/// Perfil en uso (si se eligió alguno).
pub fn active() -> Option<usize> {
    let n = std::fs::read_to_string(active_path()).ok()?;
    NAMES.iter().position(|x| *x == n.trim())
}
fn set_active(i: usize) { let _ = std::fs::create_dir_all(root()); let _ = std::fs::write(active_path(), NAMES[i]); }

/// ¿Ya tiene algo guardado?
pub fn exists(i: usize) -> bool { FILES.iter().any(|f| std::path::Path::new(&format!("{}\\{f}", pdir(i))).exists()) }

fn copy_set(from: &str, to: &str) {
    let _ = std::fs::create_dir_all(to);
    for f in FILES {
        let (s, d) = (format!("{from}\\{f}"), format!("{to}\\{f}"));
        if std::path::Path::new(&s).exists() { let _ = std::fs::copy(&s, &d); } else { let _ = std::fs::remove_file(&d); }
    }
}

/// Clic en un perfil del menú. Vacío: se guarda en él lo de ahora. Con algo: se guarda el actual y se
/// carga ese (macbar se reinicia para aplicarlo todo de una vez).
pub fn choose(i: usize) {
    if i >= NAMES.len() { return; }
    let cur = active();
    if cur == Some(i) { return; }
    if let Some(a) = cur { copy_set(&sys::config_dir(), &pdir(a)); }
    if exists(i) {
        copy_set(&pdir(i), &sys::config_dir());
        set_active(i);
        relaunch();
    } else {
        copy_set(&sys::config_dir(), &pdir(i));
        set_active(i);
        live::show(live::Hud::Profile(i as u8), 1800);
    }
}

pub fn open_folder() {
    let _ = std::fs::create_dir_all(root());
    sys::open(&root());
}

/// Cierra esta instancia y abre otra (que espera a que esta suelte el sitio).
pub fn relaunch() {
    if let Ok(exe) = std::env::current_exe() {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new(exe).arg("--reinicio").creation_flags(0x0000_0008 /* DETACHED_PROCESS */).spawn();
    }
    unsafe { let _ = PostMessageW(Some(bar_hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0)); }
}

// ---------------------------------------------------------------- modo presentación
static PRESENT: AtomicBool = AtomicBool::new(false);
pub fn presenting() -> bool { PRESENT.load(Ordering::Relaxed) }
pub fn toggle_present() {
    let on = !presenting();
    PRESENT.store(on, Ordering::Relaxed);
    desk::set_hidden(on);
    live::dismiss();
    live::show(live::Hud::Present(on), 1600);
    unsafe { let _ = InvalidateRect(Some(bar_hwnd()), None, false); }
}

// ---------------------------------------------------------------- color de acento
/// (nombre, color). El último, «Color de Windows», se lee del sistema.
pub const ACCENTS: [(&str, u32); 8] = [
    ("Azul", 0x3B82F5), ("Violeta", 0x8B5CF6), ("Rosa", 0xEC4899), ("Naranja", 0xF97316),
    ("Verde", 0x22C55E), ("Rojo", 0xEF4444), ("Grafito", 0x9CA3AF), ("Color de Windows", 0),
];
static ACCENT: AtomicU32 = AtomicU32::new(0x3B82F5);

/// Color de acento con opacidad `a` (lo usan interruptores, cursor, selección, avisos…).
pub fn accent(a: f32) -> D2D1_COLOR_F {
    let c = ACCENT.load(Ordering::Relaxed);
    let f = |s: u32| ((c >> s) & 0xFF) as f32 / 255.0;
    D2D1_COLOR_F { r: f(16), g: f(8), b: f(0), a }
}

/// Lee `acento` de config.ini (al arrancar y al cambiarlo).
pub fn load_accent() { ACCENT.store(resolve(&sys::config_value("acento").unwrap_or_default()), Ordering::Relaxed); }

/// «#RRGGBB», «windows» o vacío (azul) → color.
fn resolve(v: &str) -> u32 {
    let v = v.trim();
    if v.eq_ignore_ascii_case("windows") { return windows_accent().unwrap_or(0x3B82F5); }
    u32::from_str_radix(v.trim_start_matches('#'), 16).ok().filter(|_| v.trim_start_matches('#').len() == 6).unwrap_or(0x3B82F5)
}

/// El acento elegido en Personalización de Windows.
fn windows_accent() -> Option<u32> {
    use windows::Win32::System::Registry::*;
    let mut v = 0u32;
    let mut sz = 4u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\DWM"), w!("AccentColor"), RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as _), Some(&mut sz)).ok().ok()?;
    }
    // Se guarda como 0xAABBGGRR
    Some(((v & 0xFF) << 16) | (v & 0xFF00) | ((v >> 16) & 0xFF))
}

/// Índice del acento actual en ACCENTS (None si es uno propio puesto a mano).
pub fn accent_index() -> Option<usize> {
    let v = sys::config_value("acento").unwrap_or_default();
    if v.eq_ignore_ascii_case("windows") { return Some(ACCENTS.len() - 1); }
    let c = resolve(&v);
    ACCENTS.iter().position(|a| a.1 == c && a.1 != 0)
}

pub fn set_accent(i: usize) {
    let Some(&(_, c)) = ACCENTS.get(i) else { return };
    sys::set_config_value("acento", &if c == 0 { "windows".to_string() } else { format!("#{c:06X}") });
    load_accent();
    unsafe { let _ = InvalidateRect(Some(bar_hwnd()), None, false); }
    desk::repaint_all();
}

// ---------------------------------------------------------------- temas (archivo para compartir)
const THEME_KEYS: [(&str, bool); 3] = [("acento", false), ("texto", true), ("estilo", true)];

fn theme_text() -> String {
    let w = std::fs::read_to_string(sys::config_dir() + "\\widgets.ini").unwrap_or_default();
    let get_w = |k: &str| w.lines().find_map(|l| Some(l.trim().strip_prefix(k)?.trim().strip_prefix('=')?.trim().to_string()));
    let mut s = String::from("# Tema de macbar\n");
    for (k, in_widgets) in THEME_KEYS {
        let v = if in_widgets { get_w(k) } else { sys::config_value(k) };
        if let Some(v) = v.filter(|v| !v.is_empty()) { s += &format!("{k} = {v}\n"); }
    }
    s
}

/// Valores admitidos de un tema importado (lo demás se ignora: un archivo ajeno no puede tocar nada más).
fn valid(k: &str, v: &str) -> bool {
    match k {
        "acento" => v.eq_ignore_ascii_case("windows") || (v.starts_with('#') && v.len() == 7 && v[1..].chars().all(|c| c.is_ascii_hexdigit())),
        "texto" => matches!(v, "auto" | "oscuro" | "claro"),
        "estilo" => matches!(v, "flotante" | "tarjeta"),
        _ => false,
    }
}

fn apply_theme(txt: &str) -> bool {
    let mut any = false;
    for l in txt.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let Some(&(_, in_widgets)) = THEME_KEYS.iter().find(|x| x.0 == k) else { continue };
        if !valid(k, v) { continue; }
        if in_widgets { desk::set_ini_value(k, v); } else { sys::set_config_value(k, v); }
        any = true;
    }
    any
}

fn dialog(save: bool) -> Option<String> {
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    unsafe {
        let name = HSTRING::from(i18n::t("Tema de macbar"));
        let spec = [COMDLG_FILTERSPEC { pszName: PCWSTR(name.as_ptr()), pszSpec: w!("*.macbartema") }];
        let dlg: IFileDialog = if save {
            let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            let _ = d.SetFileName(w!("mi-tema.macbartema"));
            let _ = d.SetDefaultExtension(w!("macbartema"));
            d.into()
        } else {
            let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
            d.into()
        };
        let _ = dlg.SetFileTypes(&spec);
        let _ = dlg.SetTitle(&HSTRING::from(i18n::t(if save { "Exportar tema" } else { "Importar tema" })));
        dlg.Show(None).ok()?;
        let item = dlg.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = p.to_string().unwrap_or_default();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _));
        (!path.is_empty()).then_some(path)
    }
}

pub fn export_theme() {
    let txt = theme_text();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if let Some(p) = dialog(true) { let _ = std::fs::write(p, txt); }
    });
}

pub fn import_theme() {
    std::thread::spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Some(p) = dialog(false) else { return };
        // Un tema es un archivo pequeño: uno enorme no es un tema
        if std::fs::metadata(&p).map(|m| m.len() > 4096).unwrap_or(true) { return; }
        let Ok(txt) = std::fs::read_to_string(&p) else { return };
        if apply_theme(&txt) { relaunch(); }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn theme_values() {
        assert!(valid("acento", "#8B5CF6"));
        assert!(valid("acento", "windows"));
        assert!(!valid("acento", "#8B5CF"));
        assert!(!valid("acento", "rojo"));
        assert!(valid("texto", "oscuro"));
        assert!(!valid("ciudad", "Madrid"));
        assert_eq!(resolve("#EC4899"), 0xEC4899);
        assert_eq!(resolve("basura"), 0x3B82F5);
    }
}
