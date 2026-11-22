//! Doble escritorio: dos «caras» del mismo escritorio, una con los widgets (iconos ocultos) y otra con
//! los iconos (los widgets se apartan). Se cambia con doble clic en un hueco del escritorio, Win+Alt+W,
//! el menú de la barra o «widgets»/«iconos» en el buscador.

use super::*;
use std::sync::atomic::AtomicBool;

/// Cara de iconos: los widgets están apartados.
pub static AWAY: AtomicBool = AtomicBool::new(false);
static AWAY_AT: Mutex<Option<Instant>> = Mutex::new(None);
/// Duración del apartarse/volver de cada widget y retraso entre uno y el siguiente (cascada).
pub const SLIDE_S: f32 = 0.34;
pub const STAGGER_S: f32 = 0.045;
pub const HOTKEY_ID: i32 = 0xB0;

/// Avance (0 = en su sitio, 1 = apartado) del widget número `i` (la animación la hace la composición;
/// esto queda para las pruebas y para saber por dónde va).
#[allow(dead_code)]
pub fn away_k(i: usize) -> f32 {
    let away = AWAY.load(Ordering::Relaxed);
    let Some(at) = *AWAY_AT.lock().unwrap() else { return if away { 1.0 } else { 0.0 } };
    let t = ((at.elapsed().as_secs_f32() - i as f32 * STAGGER_S) / SLIDE_S).clamp(0.0, 1.0);
    // Al irse acelera (ease in), al volver frena (ease out): como una cortina que se abre y se cierra
    if away { t * t * (3.0 - 2.0 * t) } else { 1.0 - popup::ease_out(t) }
}

/// ¿Sigue la transición? (para pedir fotogramas)
pub fn moving(n: usize) -> bool {
    AWAY_AT.lock().unwrap().is_some_and(|at| at.elapsed().as_secs_f32() < SLIDE_S + n as f32 * STAGGER_S + 0.05)
}

/// ¿Están ocultos los iconos del escritorio? (Ver → Mostrar iconos del escritorio)
pub fn icons_hidden() -> bool {
    use windows::Win32::System::Registry::*;
    let mut v = 0u32;
    let mut sz = 4u32;
    unsafe { let _ = RegGetValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"), w!("HideIcons"), RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as _), Some(&mut sz)); }
    v != 0
}

/// Enseña u oculta los iconos con la misma orden que el menú del escritorio de Windows.
fn set_icons(visible: bool) {
    if icons_hidden() != visible { return; }
    unsafe {
        let Some(view) = desk::shell_view() else { return };
        let _ = PostMessageW(Some(view), WM_COMMAND, WPARAM(0x7402), LPARAM(0));
    }
}

fn save() { desk::set_ini_value("cara", if AWAY.load(Ordering::Relaxed) { "iconos" } else { "widgets" }); }

/// Pasa a la cara de iconos (`away` = true) o a la de widgets.
pub fn set(away: bool) {
    if AWAY.swap(away, Ordering::Relaxed) == away { return; }
    // Si se cambia a mitad de una transición, la nueva arranca desde donde iba la anterior
    *AWAY_AT.lock().unwrap() = Some(Instant::now());
    save();
    if away {
        // Los iconos aparecen cuando los widgets ya se han apartado un poco; al terminar, las ventanas de
        // los widgets se esconden (para que los iconos de debajo se puedan pulsar)
        let end = ((SLIDE_S + desk::count() as f32 * STAGGER_S) * 1000.0) as u64 + 60;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(140));
            unsafe { let _ = PostMessageW(Some(bar_hwnd()), WM_FACE, WPARAM(1), LPARAM(0)); }
            std::thread::sleep(Duration::from_millis(end.saturating_sub(140)));
            unsafe { let _ = PostMessageW(Some(bar_hwnd()), WM_FACE, WPARAM(3), LPARAM(0)); }
        });
    } else {
        // Primero se van los iconos y los widgets vuelven a entrar
        set_icons(false);
        desk::show_all();
    }
    desk::animate_all();
    live::show(live::Hud::Face(away), 1400);
}

pub fn toggle() { set(!AWAY.load(Ordering::Relaxed)); }

/// Mensaje a la barra para enseñar los iconos desde el hilo de la interfaz.
pub const WM_FACE: u32 = WM_USER + 61;
pub fn on_face_msg(wp: WPARAM) {
    if !AWAY.load(Ordering::Relaxed) { return; }
    match wp.0 {
        1 => set_icons(true),
        // Solo si la transición acabó (si se volvió a cambiar entretanto, no)
        3 if !moving(desk::count()) => desk::hide_all(),
        _ => {}
    }
}

/// ¿Doble clic en el escritorio para cambiar de cara? (ajuste «doble_clic_escritorio», por defecto sí)
pub fn dblclick_on() -> bool { !sys::config_value("doble_clic_escritorio").is_some_and(|v| v.eq_ignore_ascii_case("no")) }

pub fn toggle_dblclick() {
    let on = !dblclick_on();
    sys::set_config_value("doble_clic_escritorio", if on { "si" } else { "no" });
    DBL_ON.store(on, Ordering::Relaxed);
}
static DBL_ON: AtomicBool = AtomicBool::new(true);

/// Arranque: recupera la cara guardada y pone el atajo y el vigilante del doble clic.
pub fn start(bar: HWND) {
    let away = desk::ini_value("cara").is_some_and(|v| v == "iconos");
    AWAY.store(away, Ordering::Relaxed);
    DBL_ON.store(dblclick_on(), Ordering::Relaxed);
    unsafe { let _ = RegisterHotKey(Some(bar), HOTKEY_ID, MOD_WIN | MOD_ALT | MOD_NOREPEAT, 'W' as u32); }
    std::thread::spawn(mouse_thread);
}

// ---------------------------------------------------------------- doble clic en el escritorio
static LAST_DOWN: Mutex<(u32, i32, i32)> = Mutex::new((0, 0, 0));

fn class_of(h: HWND) -> String {
    let mut b = [0u16; 64];
    let n = unsafe { GetClassNameW(h, &mut b) };
    String::from_utf16_lossy(&b[..n.max(0) as usize])
}

/// ¿El punto cae en el propio escritorio (no en una ventana ni en un widget)?
fn on_desktop(pt: POINT) -> bool {
    let h = unsafe { WindowFromPoint(pt) };
    if h.0.is_null() { return false; }
    matches!(class_of(h).as_str(), "SysListView32" | "SHELLDLL_DefView" | "WorkerW" | "Progman")
}

unsafe extern "system" fn mouse_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 && wp.0 as u32 == WM_LBUTTONDOWN && DBL_ON.load(Ordering::Relaxed) {
            let m = &*(lp.0 as *const MSLLHOOKSTRUCT);
            let mut last = LAST_DOWN.lock().unwrap();
            let (t0, x0, y0) = *last;
            let (dx, dy) = (GetSystemMetrics(SM_CXDOUBLECLK) / 2, GetSystemMetrics(SM_CYDOUBLECLK) / 2);
            let dbl = m.time.wrapping_sub(t0) <= GetDoubleClickTime() && (m.pt.x - x0).abs() <= dx && (m.pt.y - y0).abs() <= dy;
            *last = if dbl { (0, 0, 0) } else { (m.time, m.pt.x, m.pt.y) };
            drop(last);
            // Lo decide el hilo de la interfaz (mira si había un icono debajo); aquí solo se avisa
            if dbl && on_desktop(m.pt) {
                let _ = PostMessageW(Some(bar_hwnd()), WM_FACE, WPARAM(2), LPARAM(((m.pt.y as i64) << 32 | (m.pt.x as u32 as i64)) as isize));
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}

fn mouse_thread() {
    unsafe {
        let Ok(_h) = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) else { return };
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() { let _ = TranslateMessage(&msg); DispatchMessageW(&msg); }
    }
}

/// Doble clic en el escritorio (desde el gancho): cambia de cara si no era sobre un icono.
pub fn on_dblclick(_lp: LPARAM) {
    if crate::profiles::presenting() || crate::fullscreen() { return; }
    // Con los iconos a la vista: el primer clic sobre un icono lo deja seleccionado, uno en un hueco
    // quita la selección. Así se sabe sin mirar coordenadas (UI Automation no ve los iconos del
    // escritorio en todas las versiones de Windows)
    if !icons_hidden() && icons_selected() > 0 { return; }
    toggle();
}

/// Iconos seleccionados ahora mismo en el escritorio.
fn icons_selected() -> usize {
    unsafe {
        let Some(view) = desk::shell_view() else { return 1 };
        let Ok(lv) = FindWindowExW(Some(view), None, w!("SysListView32"), None) else { return 1 };
        // LVM_GETSELECTEDCOUNT (si no responde, se da por hecho que hay uno: mejor no cambiar)
        let mut out = 0usize;
        let r = SendMessageTimeoutW(lv, 0x1032, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 200, Some(&mut out));
        if r.0 == 0 { 1 } else { out }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avance_de_la_cortina() {
        // Sin transición: en su sitio o apartado del todo
        AWAY.store(false, Ordering::Relaxed);
        *AWAY_AT.lock().unwrap() = None;
        assert_eq!(away_k(0), 0.0);
        AWAY.store(true, Ordering::Relaxed);
        assert_eq!(away_k(3), 1.0);
        // Recién empezada: el primero ya se mueve un poco más que el cuarto (cascada)
        *AWAY_AT.lock().unwrap() = Some(Instant::now() - Duration::from_millis(120));
        assert!(away_k(0) > away_k(3));
        assert!(moving(4));
        *AWAY_AT.lock().unwrap() = Some(Instant::now() - Duration::from_secs(5));
        assert_eq!(away_k(5), 1.0);
        assert!(!moving(6));
        AWAY.store(false, Ordering::Relaxed);
        *AWAY_AT.lock().unwrap() = None;
    }
}
