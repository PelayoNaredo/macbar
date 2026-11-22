//! Widgets del escritorio: tarjetas grandes de cristal oscuro que viven justo encima del fondo de
//! pantalla (por debajo de todas las ventanas). Se arrastran a cualquier sitio con imán a una rejilla
//! y la posición se guarda en `%APPDATA%\macbar\widgets.ini`. Solo se redibujan mientras se ven.

use super::*;
use deskw::Widget;
use popup::Ui;

const GAP: f32 = 16.0;
const SNAP: f32 = 8.0;
const TIMER_SEC: usize = 1;
const TIMER_FRAME: usize = 2;
const TIMER_FADE: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Calendar, Weather, Music, System, Storage, Projects, Phone, Visual, Photos, Clocks, Todo, Countdown }
const ALL: [Kind; 12] = [Kind::Calendar, Kind::Weather, Kind::Music, Kind::System, Kind::Storage, Kind::Projects, Kind::Phone, Kind::Visual, Kind::Photos, Kind::Clocks, Kind::Todo, Kind::Countdown];
/// Widgets que llegaron después: empiezan quitados (no aparecen de golpe al actualizar).
const NEW_OFF: [Kind; 5] = [Kind::Visual, Kind::Photos, Kind::Clocks, Kind::Todo, Kind::Countdown];
/// Los de `NEW_OFF` que ya traía la marca `nuevos_v2` (quien la tenga ya decidió sobre ellos).
const NEW_OFF_V2: [Kind; 2] = [Kind::Visual, Kind::Photos];

impl Kind {
    fn key(self) -> &'static str {
        match self { Kind::Calendar => "calendario", Kind::Weather => "tiempo", Kind::Music => "musica", Kind::System => "sistema", Kind::Storage => "almacenamiento", Kind::Projects => "proyectos", Kind::Phone => "movil", Kind::Visual => "visualizador", Kind::Photos => "fotos", Kind::Clocks => "relojes", Kind::Todo => "tareas", Kind::Countdown => "cuentaatras" }
    }
    fn label(self) -> &'static str {
        i18n::t(match self { Kind::Calendar => "Calendario", Kind::Weather => "Tiempo", Kind::Music => "Música", Kind::System => "Sistema", Kind::Storage => "Almacenamiento y papelera", Kind::Projects => "Proyectos", Kind::Phone => "Móvil (Enlace Móvil)", Kind::Visual => "Visualizador de audio", Kind::Photos => "Marco de fotos", Kind::Clocks => "Relojes del mundo", Kind::Todo => "Tareas", Kind::Countdown => "Cuenta atrás" })
    }
}

/// Estilo: flotantes (sin fondo ni borde, por defecto) o tarjetas de cristal oscuro.
static FLOAT: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(-1);
pub fn floating() -> bool {
    let v = FLOAT.load(Ordering::Relaxed);
    if v >= 0 { return v == 1; }
    let f = ini_get(&ini(), "estilo").map(|s| s != "tarjeta").unwrap_or(true);
    FLOAT.store(f as i8, Ordering::Relaxed);
    f
}

/// Bordes y esquinas de la ventana según el estilo (flotantes: ni esquinas redondeadas ni borde de Windows).
fn apply_style(hwnd: HWND) {
    unsafe {
        let fl = floating();
        let round: i32 = if fl { 1 } else { 2 }; // DWMWCP_DONOTROUND / DWMWCP_ROUND
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as _, 4);
        let border: u32 = if fl { 0xFFFF_FFFE } else { 0xFFFF_FFFF }; // DWMWA_COLOR_NONE / DWMWA_COLOR_DEFAULT
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &border as *const _ as _, 4);
    }
}

/// Cambia entre flotantes y tarjetas (clic derecho en un widget o ventana de Configuración).
pub fn toggle_style() {
    let fl = !floating();
    FLOAT.store(fl as i8, Ordering::Relaxed);
    let txt = ini();
    let line = format!("estilo = {}", if fl { "flotante" } else { "tarjeta" });
    let mut out: Vec<String> = txt.lines().filter(|l| !l.trim_start().starts_with("estilo")).map(str::to_string).collect();
    out.push(line);
    let _ = std::fs::write(ini_path(), out.join("\n") + "\n");
    let all: Vec<HWND> = DESKS.with(|d| d.borrow().iter().map(|w| w.hwnd).collect());
    for h in all { apply_style(h); unsafe { let _ = InvalidateRect(Some(h), None, false); } }
}

/// Todo un poco más pequeño que el diseño original (se dibuja en DIP y se escala entero).
pub(crate) const ZOOM: f32 = 0.86;

/// Escala de píxeles de un widget: la del monitor por el tamaño de los widgets.
fn scale(hwnd: HWND) -> f32 { scale_of(hwnd) * ZOOM }

/// Tipos de letra propios de los widgets (más grandes que los de los paneles).
pub struct DFonts {
    pub huge: IDWriteTextFormat,
    pub big: IDWriteTextFormat,
    pub title: IDWriteTextFormat,
    pub glyph_m: IDWriteTextFormat,
    pub glyph_l: IDWriteTextFormat,
}

struct Dw {
    hwnd: HWND,
    kind: Kind,
    target: Option<Gpu>,
    dw: IDWriteFactory,
    cache: RefCell<Caches>,
    f: popup::Fonts,
    d: DFonts,
    hits: Vec<(D2D_RECT_F, u32)>,
    hover: Option<u32>,
    glow: HashMap<u32, f32>,
    last_paint: Instant,
    born: Instant,
    w: Box<dyn Widget>,
    /// Arrastre en curso: (cursor al empezar, esquina de la ventana al empezar, ya se movió)
    drag: Option<(POINT, POINT, bool)>,
    /// Botón derecho pulsado (para no confundirlo con un arrastre)
    rdown: bool,
    /// Zona pulsada con el botón izquierdo y su hundimiento animado (0..1)
    pressed: Option<u32>,
    press: HashMap<u32, f32>,
    /// Elevación mientras se arrastra (0..1): el contenido crece un pelín, como si se levantara
    lift: f32,
    /// Deslizamiento al soltar hasta su sitio en la rejilla: (desde, hasta, inicio)
    glide: Option<(POINT, POINT, Instant)>,
    /// Quitándose: se encoge y se desvanece antes de cerrarse
    dying: Option<Instant>,
    /// Capa fija (lienzo de la ventana con el que se creó, la imagen, si quedó bien) y sus cachés y zonas
    layer: Option<(Option<windows::core::IUnknown>, ID2D1BitmapRenderTarget, bool)>,
    lcache: RefCell<Caches>,
    layer_hits: Vec<(D2D_RECT_F, u32)>,
    /// Ratón dentro del widget (DIP; negativo si está fuera)
    mouse: (f32, f32),
    /// Fondo claro detrás: texto oscuro. `ink` es la tinta animada (0..1) hacia ese objetivo.
    light: bool,
    ink: f32,
    lum_n: u32,
}

thread_local! {
    static DESKS: RefCell<Vec<Dw>> = const { RefCell::new(Vec::new()) };
    /// Hundimiento de cada zona del widget que se está pintando (lo leen los widgets al dibujar botones)
    static PRESS_NOW: RefCell<HashMap<u32, f32>> = RefCell::new(HashMap::new());
    /// Milisegundos hasta el próximo fotograma que pidió el último pintado
    static FAST: std::cell::Cell<u32> = const { std::cell::Cell::new(33) };
    static ENDDRAW_MS: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
    /// Punto del último clic dentro del widget (DIP), para lo que depende de dónde se pulsa
    static CLICK_AT: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((0.0, 0.0)) };
    /// Posición del ratón dentro del widget (DIP), para las sondas que lo siguen
    static MOUSE_AT: std::cell::Cell<(f32, f32)> = const { std::cell::Cell::new((-1.0, -1.0)) };
}

/// Dónde se hizo el último clic dentro del widget (DIP).
pub fn click_at() -> (f32, f32) { CLICK_AT.with(|c| c.get()) }

/// Dónde está el ratón dentro del widget que se pinta (DIP; negativo si está fuera).
pub fn mouse_at() -> (f32, f32) { MOUSE_AT.with(|c| c.get()) }

/// Cuánto está hundida una zona del widget que se pinta (0 = suelta, 1 = pulsada del todo).
pub fn press(id: u32) -> f32 { PRESS_NOW.with(|p| p.borrow().get(&id).copied().unwrap_or(0.0)) }

const TIMER_GLIDE: usize = 4;
const GLIDE_S: f32 = 0.22;
const DIE_S: f32 = 0.24;

fn ini_path() -> String { sys::config_dir() + "\\widgets.ini" }

fn ini() -> String { std::fs::read_to_string(ini_path()).unwrap_or_default() }

fn ini_get(txt: &str, k: &str) -> Option<String> {
    txt.lines().find_map(|l| Some(l.trim().strip_prefix(k)?.trim().strip_prefix('=')?.trim().to_string()))
}

/// ¿Están activados? (por defecto sí)
pub fn enabled() -> bool { ini_get(&ini(), "activados").map(|v| v != "0").unwrap_or(true) }

/// Guarda el estado: activados, los que se quitaron y la posición de los que están puestos. Las
/// posiciones de los que no están se conservan (vuelven a su sitio al ponerlos otra vez).
fn save(enabled: bool) {
    let old = ini();
    let mut out = format!("activados = {}\n", if enabled { 1 } else { 0 });
    let hidden: Vec<&str> = ALL.iter().filter(|k| is_hidden_in(&old, **k) && !shown(**k)).map(|k| k.key()).collect();
    let hidden: Vec<&str> = if enabled { hidden } else { ALL.iter().filter(|k| is_hidden_in(&old, **k)).map(|k| k.key()).collect() };
    out += &format!("ocultos = {}\n", hidden.join(","));
    out += &format!("estilo = {}\n", if floating() { "flotante" } else { "tarjeta" });
    for k in ALL {
        let live = DESKS.with(|d| d.borrow().iter().find(|w| w.kind == k).map(|w| { let mut r = RECT::default(); unsafe { let _ = GetWindowRect(w.hwnd, &mut r); } (r.left, r.top) }));
        match live {
            Some((x, y)) => out += &format!("{} = {x},{y}\n", pos_key(k)),
            None => if let Some(v) = ini_get(&old, &pos_key(k)) { out += &format!("{} = {v}\n", pos_key(k)); },
        }
    }
    // El resto de claves (texto, fotos_carpeta, relojes, cuenta_atras, nuevos_v3…) se conservan tal cual
    let ours: Vec<String> = ["activados", "ocultos", "estilo"].iter().map(|s| s.to_string()).chain(ALL.iter().map(|k| pos_key(*k))).collect();
    for l in old.lines() {
        let key = l.split('=').next().unwrap_or("").trim();
        if l.trim().is_empty() || (l.contains('=') && ours.iter().any(|o| o == key)) { continue; }
        out += l;
        out.push('\n');
    }
    let _ = std::fs::create_dir_all(sys::config_dir());
    let _ = std::fs::write(ini_path(), out);
}

fn is_hidden_in(txt: &str, k: Kind) -> bool { ini_get(txt, "ocultos").is_some_and(|v| v.split(',').any(|s| s.trim() == k.key())) }

fn shown(k: Kind) -> bool { DESKS.with(|d| d.borrow().iter().any(|w| w.kind == k && w.dying.is_none())) }

/// ¿Queda algún widget puesto (sin contar los que se están yendo)?
fn any_live() -> bool { DESKS.with(|d| d.borrow().iter().any(|w| w.dying.is_none())) }

/// Quita un widget con su animación de salida (se cierra solo al terminar).
fn retire(h: HWND) {
    with_dw(h, |w| { if w.dying.is_none() { w.dying = Some(Instant::now()); } });
    unsafe { SetTimer(Some(h), TIMER_FADE, 15, None); }
}

/// Cambia (o añade) `k = v` en widgets.ini (lo usa el importador de temas).
pub fn set_ini_value(k: &str, v: &str) {
    let mut out: Vec<String> = ini().lines().filter(|l| l.trim().strip_prefix(k).is_none_or(|r| !r.trim_start().starts_with('='))).map(str::to_string).collect();
    out.push(format!("{k} = {v}"));
    let _ = std::fs::create_dir_all(sys::config_dir());
    let _ = std::fs::write(ini_path(), out.join("\n") + "\n");
}

/// Modo presentación: los widgets se esconden (sin quitarlos) y vuelven igual al salir.
pub fn set_hidden(hide: bool) {
    // En la cara de iconos siguen escondidos
    let hide = hide || faces::AWAY.load(Ordering::Relaxed);
    let all: Vec<HWND> = DESKS.with(|d| d.borrow().iter().map(|w| w.hwnd).collect());
    for h in all { unsafe { let _ = ShowWindow(h, if hide { SW_HIDE } else { SW_SHOWNOACTIVATE }); } }
    if !hide { restack(); }
}

/// Valor de widgets.ini.
pub fn ini_value(k: &str) -> Option<String> { ini_get(&ini(), k).map(|v| v.to_string()) }

/// La vista de los iconos del escritorio (a la que se le manda «mostrar u ocultar iconos»).
pub fn shell_view() -> Option<HWND> {
    unsafe { FindWindowExW(Some(desktop_window()), None, w!("SHELLDLL_DefView"), None).ok().filter(|h| !h.0.is_null()) }
}

/// Vuelven a la vista para entrar con su animación (cara de widgets).
pub fn show_all() {
    if crate::profiles::presenting() { return; }
    let all: Vec<HWND> = DESKS.with(|d| d.borrow().iter().filter(|w| w.dying.is_none()).map(|w| w.hwnd).collect());
    for h in all { unsafe { let _ = ShowWindow(h, SW_SHOWNOACTIVATE); } }
    restack();
}

/// Cuántos widgets hay puestos.
pub fn count() -> usize { DESKS.with(|d| d.borrow().len()) }

/// Esconde las ventanas de los widgets (cara de iconos, al acabar de apartarse).
pub fn hide_all() {
    let all: Vec<HWND> = DESKS.with(|d| d.borrow().iter().map(|w| w.hwnd).collect());
    for h in all { unsafe { let _ = ShowWindow(h, SW_HIDE); } }
}

/// Transición entre caras: la anima la composición de Windows (desplazamiento, escala y opacidad de cada
/// widget), sin repintar nada: va al ritmo de la pantalla y sin gastar CPU. Cada widget se aparta hacia su
/// lado de la pantalla, en cascada; al volver, al revés.
pub fn animate_all() {
    use windows::Win32::Graphics::DirectComposition::*;
    let away = faces::AWAY.load(Ordering::Relaxed);
    let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let items: Vec<(IDCompositionVisual, f32, f32, f32, f32)> = DESKS.with(|d| d.borrow().iter().filter_map(|w| {
        let g = w.target.as_ref()?;
        let mut r = RECT::default();
        unsafe { let _ = GetWindowRect(w.hwnd, &mut r); }
        let side = if (r.left + r.right) / 2 < sw / 2 { -1.0 } else { 1.0 };
        Some((g._visual.clone(), (r.right - r.left) as f32, (r.bottom - r.top) as f32, side, scale(w.hwnd)))
    }).collect());
    SHARED.with(|sh| {
        let sh = sh.borrow();
        let Some(sh) = sh.as_ref() else { return };
        unsafe {
            // Curva de `a` a `b` que empieza a los `delay` s y dura `dur` s: al irse acelera y frena
            // (smoothstep), al volver frena al llegar (ease out)
            let curve = |a: f32, b: f32, delay: f64| -> Option<IDCompositionAnimation> {
                let an = sh.dcomp.CreateAnimation().ok()?;
                let (d, t) = (b - a, faces::SLIDE_S);
                an.AddCubic(0.0, a, 0.0, 0.0, 0.0).ok()?;
                if away { an.AddCubic(delay, a, 0.0, 3.0 * d / (t * t), -2.0 * d / (t * t * t)).ok()?; }
                else { an.AddCubic(delay, a, 3.0 * d / t, -3.0 * d / (t * t), d / (t * t * t)).ok()?; }
                an.End(delay + t as f64, b).ok()?;
                Some(an)
            };
            for (i, (v, w, h, side, s)) in items.into_iter().enumerate() {
                let delay = i as f64 * faces::STAGGER_S as f64;
                let (from, to) = if away { (0.0, 1.0) } else { (1.0, 0.0) };
                // Desplazamiento corto hacia su lado (la ventana recorta lo que se sale: poco y desvaneciéndose)
                let dx = side * 22.0 * s;
                if let Some(an) = curve(dx * from, dx * to, delay) { let _ = v.SetOffsetX(&an); }
                // Algo más pequeño (desde su centro) y desvanecido
                if let Ok(st) = sh.dcomp.CreateScaleTransform() {
                    let _ = st.SetCenterX2(w / 2.0);
                    let _ = st.SetCenterY2(h / 2.0);
                    if let Some(an) = curve(1.0 - 0.08 * from, 1.0 - 0.08 * to, delay) { let _ = st.SetScaleX(&an); let _ = st.SetScaleY(&an); }
                    let _ = v.SetTransform(&st);
                }
                if let Ok(eg) = sh.dcomp.CreateEffectGroup() {
                    if let Some(an) = curve(1.0 - from, 1.0 - to, delay) { let _ = eg.SetOpacity(&an); }
                    let _ = v.SetEffect(&eg);
                }
            }
            let _ = sh.dcomp.Commit();
        }
    });
}

/// Para el menú: nombre de cada widget y si está puesto.
pub fn menu_items() -> Vec<(&'static str, bool)> { ALL.iter().map(|k| (k.label(), shown(*k))).collect() }

/// Enciende o apaga los widgets desde el menú de la barra.
pub fn toggle() {
    if !any_live() {
        save(true);
        start();
    } else {
        save(false);
        // Se van todos a la vez con su animación de salida
        let all: Vec<HWND> = DESKS.with(|d| d.borrow().iter().map(|w| w.hwnd).collect());
        for h in all { retire(h); }
    }
}

/// Pone o quita un widget concreto (menú de la barra o clic derecho en el widget).
pub fn toggle_kind(i: usize) {
    let Some(&k) = ALL.get(i) else { return };
    let txt = ini();
    if let Some(h) = DESKS.with(|d| d.borrow().iter().find(|w| w.kind == k).map(|w| w.hwnd)) {
        // Se guarda su sitio antes de quitarlo, para que vuelva al mismo lugar
        save(true);
        let mut hidden: Vec<String> = ini_get(&txt, "ocultos").map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
        hidden.push(k.key().to_string());
        retire(h);
        let new = ini().lines().map(|l| if l.trim_start().starts_with("ocultos") { format!("ocultos = {}", hidden.join(",")) } else { l.to_string() }).collect::<Vec<_>>().join("\n");
        let _ = std::fs::write(ini_path(), new + "\n");
    } else {
        let hidden: Vec<String> = ini_get(&txt, "ocultos").map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && s != k.key()).collect()).unwrap_or_default();
        let new = txt.lines().map(|l| if l.trim_start().starts_with("ocultos") { format!("ocultos = {}", hidden.join(",")) } else { l.to_string() }).collect::<Vec<_>>().join("\n");
        let _ = std::fs::write(ini_path(), new + "\n");
        if !any_live() { save(true); start(); } else { place(k, 0); save(true); }
    }
}

/// Ventana del escritorio (la que contiene los iconos): los widgets se colocan justo encima.
fn desktop_window() -> HWND {
    unsafe {
        let pm = FindWindowW(w!("Progman"), None).unwrap_or_default();
        if FindWindowExW(Some(pm), None, w!("SHELLDLL_DefView"), None).is_ok_and(|h| !h.0.is_null()) { return pm; }
        let mut after: Option<HWND> = None;
        while let Ok(ww) = FindWindowExW(None, after, w!("WorkerW"), None) {
            if ww.0.is_null() { break; }
            if FindWindowExW(Some(ww), None, w!("SHELLDLL_DefView"), None).is_ok_and(|h| !h.0.is_null()) { return ww; }
            after = Some(ww);
        }
        pm
    }
}

fn is_desk(h: HWND) -> bool { DESKS.with(|d| d.borrow().iter().any(|w| w.hwnd == h)) }

/// Vuelve a ponerlos encima del fondo (tras "Mostrar escritorio" o si algo los movió).
pub fn restack() {
    let all: Vec<HWND> = DESKS.with(|d| d.borrow().iter().map(|w| w.hwnd).collect());
    for h in all {
        unsafe {
            if !IsWindowVisible(h).as_bool() && !faces::AWAY.load(Ordering::Relaxed) { let _ = ShowWindow(h, SW_SHOWNOACTIVATE); }
            let _ = SetWindowPos(h, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

/// ¿Lo tapa del todo alguna ventana que esté por encima? (entonces no se pinta ni se anima)
fn covered_w(hwnd: HWND) -> bool {
    if FULLSCREEN.load(Ordering::Relaxed) { return true; }
    unsafe {
        let mut me = RECT::default();
        if GetWindowRect(hwnd, &mut me).is_err() { return false; }
        let mut h = GetWindow(hwnd, GW_HWNDPREV).unwrap_or_default();
        let mut n = 0;
        while !h.0.is_null() && n < 600 {
            n += 1;
            let mut r = RECT::default();
            // Primero lo barato (tamaño); preguntar a DWM si está oculta cuesta más
            if GetWindowRect(h, &mut r).is_ok() && r.left <= me.left && r.top <= me.top && r.right >= me.right && r.bottom >= me.bottom
                && IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool() && h != bar_hwnd() {
                let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
                let st = GetWindowLongW(h, GWL_STYLE) as u32;
                // Solo tapan ventanas de apps de verdad: las capas del sistema (superposiciones de
                // NVIDIA, Click to Do, avisos…) son transparentes o herramientas sin marco
                let app = ex & WS_EX_TOOLWINDOW.0 == 0 && ex & WS_EX_NOACTIVATE.0 == 0 && !(ex & WS_EX_LAYERED.0 != 0 && ex & WS_EX_TRANSPARENT.0 != 0)
                    && (st & WS_CAPTION.0 == WS_CAPTION.0 || IsZoomed(h).as_bool());
                if app {
                    let mut cloak = 0u32;
                    let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloak as *mut _ as _, 4);
                    if cloak == 0 { return true; }
                }
            }
            h = GetWindow(h, GW_HWNDPREV).unwrap_or_default();
        }
        false
    }
}

/// ¿Están todos tapados?
pub fn covered() -> bool {
    let all: Vec<HWND> = DESKS.with(|d| d.borrow().iter().map(|w| w.hwnd).collect());
    all.iter().all(|h| covered_w(*h))
}

pub fn start() {
    if !enabled() || any_live() { return; }
    deskdata::start();
    let hinst = unsafe { GetModuleHandleW(None).unwrap() };
    unsafe {
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: w!("MacBarDesk"), hCursor: LoadCursorW(None, IDC_ARROW).unwrap(), ..Default::default() };
        RegisterClassW(&wc);
    }
    // Primera vez con widgets nuevos: se apuntan como quitados (se ponen desde el menú o la configuración)
    if ini_get(&ini(), "nuevos_v3").is_none() {
        let txt = ini();
        // Quien ya tenía la marca anterior decidió sobre los de entonces: solo se quitan los nuevos
        let had_v2 = ini_get(&txt, "nuevos_v2").is_some();
        let mut hidden: Vec<String> = ini_get(&txt, "ocultos").map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
        for k in NEW_OFF {
            if had_v2 && NEW_OFF_V2.contains(&k) { continue; }
            if !hidden.iter().any(|h| h == k.key()) { hidden.push(k.key().to_string()); }
        }
        let new = txt.lines().filter(|l| ini_get(l, "nuevos_v2").is_none()).map(|l| if l.trim_start().starts_with("ocultos") { format!("ocultos = {}", hidden.join(",")) } else { l.to_string() }).collect::<Vec<_>>().join("\n");
        let new = if txt.lines().any(|l| l.trim_start().starts_with("ocultos")) { new } else { format!("{new}\nocultos = {}", hidden.join(",")) };
        let _ = std::fs::write(ini_path(), new + "\nnuevos_v3 = 1\n");
    }
    let txt = ini();
    // Prueba visual: MACBAR_WIDGETS=clave,clave enseña solo esos (por encima de todo, ver WM_WINDOWPOSCHANGING)
    let only = std::env::var("MACBAR_WIDGETS").ok();
    for (i, k) in ALL.iter().enumerate() {
        if let Some(o) = &only { if o.split(',').any(|x| x == k.key()) { place(*k, i); } continue; }
        if !is_hidden_in(&txt, *k) { place(*k, i); }
    }
}

/// Crea un widget en su sitio guardado o, si nunca se movió, en el de por defecto: tres columnas
/// arriba a la derecha (los iconos suelen estar a la izquierda). Derecha: calendario y música;
/// centro: tiempo y sistema; izquierda: proyectos y almacenamiento.
fn place(k: Kind, order: usize) {
    let s = scale_of(bar_hwnd()) * ZOOM;
    let mut wa = RECT::default();
    unsafe { let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut wa as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
    let px = |v: f32| (v * s) as i32;
    let (gap, m) = (px(GAP), px(28.0));
    let w_of = |k: Kind| px(size(k).0);
    let h_of = |k: Kind| px(size(k).1);
    let x3 = wa.right - m - w_of(Kind::Calendar);
    let x2 = x3 - gap - w_of(Kind::Music);
    let x1 = x2 - gap - w_of(Kind::System);
    let y0 = wa.top + m;
    // Columna izquierda, de arriba abajo: fotos, visualizador, relojes, cuenta atrás y tareas. Lo que
    // no quepa hasta abajo empieza otra columna a su derecha (nada se sale del área de trabajo)
    let left_col = |k: Kind| -> (i32, i32) {
        let (mut x, mut y, mut col_w) = (wa.left + m, y0, 0);
        for c in [Kind::Photos, Kind::Visual, Kind::Clocks, Kind::Countdown, Kind::Todo] {
            if y > y0 && y + h_of(c) > wa.bottom - m { x += col_w + gap; y = y0; col_w = 0; }
            if c == k { return ((x).min(wa.right - w_of(c)).max(wa.left), y.min(wa.bottom - h_of(c)).max(wa.top)); }
            col_w = col_w.max(w_of(c));
            y += h_of(c) + gap;
        }
        (wa.left + m, y0)
    };
    let def = match k {
        Kind::Calendar => (x3, y0), Kind::Storage => (x3, y0 + h_of(Kind::Calendar) + gap),
        Kind::Music => (x2, y0), Kind::Weather => (x2, y0 + h_of(Kind::Music) + gap),
        Kind::System => (x1, y0), Kind::Projects => (x1, y0 + h_of(Kind::System) + gap),
        // El del móvil, vertical: una columna más a la izquierda
        Kind::Phone => (x1 - gap - w_of(Kind::Phone), y0),
        // Los nuevos, a la izquierda de la pantalla (en la cara de widgets los iconos no están): fotos arriba
        // y el visualizador debajo
        Kind::Photos | Kind::Visual | Kind::Clocks | Kind::Countdown | Kind::Todo => left_col(k),
    };
    let (x, y) = ini_get(&ini(), &pos_key(k)).and_then(|v| { let (a, b) = v.split_once(',')?; Some((a.trim().parse().ok()?, b.trim().parse().ok()?)) }).unwrap_or(def);
    create(k, x, y, w_of(k), h_of(k), order);
}

// ---------------------------------------------------------------- diseños de cada widget
/// Diseños que tiene cada widget (el primero es el de siempre). Se elige por widget, desde su clic
/// derecho o desde la Configuración, y se guarda en widgets.ini como `diseno_<clave> = n`.
pub fn variants(k: Kind) -> &'static [&'static str] {
    match k {
        Kind::Weather => &["Clásico", "Cielo", "Mínimo"],
        Kind::Calendar => &["Clásico", "Hoy"],
        Kind::System => &["Clásico", "Anillos"],
        Kind::Music => &["Clásico", "Carátula"],
        _ => &["Clásico"],
    }
}

static VARIANT: Mutex<Option<HashMap<&'static str, usize>>> = Mutex::new(None);

/// Diseño elegido para el widget (0 = el de siempre).
pub fn variant(k: Kind) -> usize {
    let mut v = VARIANT.lock().unwrap();
    let m = v.get_or_insert_with(|| {
        let txt = ini();
        ALL.iter().map(|k| (k.key(), ini_get(&txt, &format!("diseno_{}", k.key())).and_then(|x| x.trim().parse().ok()).unwrap_or(0))).collect()
    });
    m.get(k.key()).copied().unwrap_or(0).min(variants(k).len().saturating_sub(1))
}

/// Cambia el diseño de un widget: el viejo se va con su animación y entra el nuevo en el mismo sitio.
pub fn set_variant(k: Kind, n: usize) {
    if n >= variants(k).len() || n == variant(k) { return; }
    // Se guarda su sitio antes de quitarlo
    if shown(k) { save(true); }
    set_ini_value(&format!("diseno_{}", k.key()), &n.to_string());
    if let Some(m) = VARIANT.lock().unwrap().as_mut() { m.insert(k.key(), n); }
    if let Some(h) = DESKS.with(|d| d.borrow().iter().find(|w| w.kind == k && w.dying.is_none()).map(|w| w.hwnd)) {
        retire(h);
        place(k, 0);
    }
}

/// Para la Configuración: diseños de cada widget (en el orden de `ALL`) y el elegido.
pub fn variant_items() -> Vec<(&'static [&'static str], usize)> { ALL.iter().map(|k| (variants(*k), variant(*k))).collect() }
pub fn set_variant_i(i: usize, n: usize) { if let Some(k) = ALL.get(i) { set_variant(*k, n); } }

/// Tamaño de cada widget (DIP): cada uno el que necesita su contenido.
pub fn size(k: Kind) -> (f32, f32) {
    // Los diseños alternativos tienen su propio tamaño
    match (k, variant(k)) {
        (Kind::Weather, 1) => return (340.0, 170.0),
        (Kind::Weather, 2) => return (300.0, 150.0),
        (Kind::Calendar, 1) => return (180.0, 180.0),
        (Kind::System, 1) => return (380.0, 170.0),
        (Kind::Music, 1) => return (300.0, 300.0),
        _ => {}
    }
    match k {
        Kind::Calendar => (380.0, 350.0),
        Kind::Storage => (380.0, 300.0),
        Kind::Music => (640.0, 340.0),
        Kind::Weather => (640.0, 344.0),
        Kind::System => (540.0, 360.0),
        Kind::Projects => (540.0, 460.0),
        Kind::Phone => (330.0, 780.0),
        Kind::Visual => (380.0, 200.0),
        Kind::Photos => (380.0, 260.0),
        Kind::Clocks => (380.0, 170.0),
        Kind::Todo => (320.0, 300.0),
        Kind::Countdown => (300.0, 170.0),
    }
}

/// Clave de la posición guardada (cambia con los tamaños: así no se hereda un sitio que ya no encaja).
fn pos_key(k: Kind) -> String { format!("{}.3", k.key()) }

fn create(kind: Kind, x: i32, y: i32, wpx: i32, hpx: i32, order: usize) {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        // Sin superficie propia de Windows: el contenido lo pone la composición (GPU), con transparencia real
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP, w!("MacBarDesk"), w!("macbar"), WS_POPUP, x, y, wpx, hpx, None, None, Some(hinst.into()), None) else { return };
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
        apply_style(hwnd);
        // Sin desenfoque: el fondo de pantalla se ve nítido a través de un tinte oscuro
        // (Windows no permite elegir la intensidad del desenfoque, solo tenerlo o no)
        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();
        let mk = |fam, size, wt| make_format(&dw, fam, size, wt);
        let f = popup::Fonts {
            big: mk(FONT, 30.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            name: mk(FONT, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            lbl: mk(FONT, 12.5, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            sub: mk(FONT, 11.5, DWRITE_FONT_WEIGHT_NORMAL),
            small: mk(FONT, 11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            val: mk(FONT, 15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            icon: mk(ICONS, 14.0, DWRITE_FONT_WEIGHT_NORMAL),
            icon_l: mk(ICONS, 16.0, DWRITE_FONT_WEIGHT_NORMAL),
            icon_xl: mk(ICONS, 30.0, DWRITE_FONT_WEIGHT_NORMAL),
            q: mk(FONT, 19.0, DWRITE_FONT_WEIGHT_NORMAL),
        };
        let d = DFonts {
            huge: mk(FONT, 40.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            big: mk(FONT, 24.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            title: mk(FONT, 16.5, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            glyph_m: mk(ICONS, 20.0, DWRITE_FONT_WEIGHT_NORMAL),
            glyph_l: mk(ICONS, 24.0, DWRITE_FONT_WEIGHT_NORMAL),
        };
        let w = deskw::new(kind);
        // Aparecen en cascada al arrancar
        let born = Instant::now() + Duration::from_millis(90 * order as u64);
        DESKS.with(|ds| ds.borrow_mut().push(Dw { hwnd, kind, target: None, dw, cache: RefCell::new(Caches::default()), f, d, hits: Vec::new(), hover: None,
            glow: HashMap::new(), last_paint: Instant::now(), born, w, drag: None, rdown: false,
            pressed: None, press: HashMap::new(), lift: 0.0, glide: None, dying: None,
            layer: None, lcache: RefCell::new(Caches::default()), layer_hits: Vec::new(), mouse: (-1.0, -1.0),
            light: ink_mode() == 1, ink: if ink_mode() == 1 { 1.0 } else { 0.0 }, lum_n: 0 }));
        // En la cara de iconos se crea escondido (aparecerá al volver a la de widgets)
        let show = if faces::AWAY.load(Ordering::Relaxed) { SET_WINDOW_POS_FLAGS(0) } else { SWP_SHOWWINDOW };
        let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | show);
        SetTimer(Some(hwnd), TIMER_SEC, 1000, None);
        SetTimer(Some(hwnd), TIMER_FADE, 15, None);
    }
}

fn with_dw<R>(hwnd: HWND, f: impl FnOnce(&mut Dw) -> R) -> Option<R> {
    DESKS.with(|d| d.borrow_mut().iter_mut().find(|w| w.hwnd == hwnd).map(f))
}

/// Segundos desde que apareció (negativo mientras espera su turno en la cascada).
fn age(born: Instant) -> f32 {
    let now = Instant::now();
    if now >= born { (now - born).as_secs_f32() } else { -(born - now).as_secs_f32() }
}

/// Medición opcional del coste de pintar (con MACBAR_PROF=1): pintados y milisegundos por widget cada 10 s.
static PROF: Mutex<([u32; ALL.len()], [f32; ALL.len()], Option<Instant>)> = Mutex::new(([0; ALL.len()], [0.0; ALL.len()], None));
thread_local! {
}

fn paint(hwnd: HWND) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("MACBAR_PROF").is_ok()) { return paint_real(hwnd); }
    let t0 = Instant::now();
    paint_real(hwnd);
    if let Some(k) = with_dw(hwnd, |w| w.kind as usize) {
        let mut p = PROF.lock().unwrap();
        p.0[k] += 1; p.1[k] += t0.elapsed().as_secs_f32() * 1000.0;
        let st = *p.2.get_or_insert_with(Instant::now);
        if st.elapsed().as_secs() >= 10 {
            let per: Vec<String> = (0..ALL.len()).map(|i| if p.0[i] > 0 { format!("{:.2}", p.1[i] / p.0[i] as f32) } else { "-".into() }).collect();
            let ed = ENDDRAW_MS.with(|e| e.replace(0.0));
            let _ = std::fs::write(std::env::temp_dir().join("macbar_prof.txt"), format!("pintados={:?}\nms_por_pintado={:?}\nms_total={:?}\nms_en_EndDraw={:.0}", p.0, per, p.1.map(|v| v.round()), ed));
            *p = ([0; ALL.len()], [0.0; ALL.len()], Some(Instant::now()));
        }
    }
}

/// Opacidad de todo el widget: 0 hasta que le toca en la cascada, sube al aparecer y baja al quitarlo.
fn fade_alpha(w: &Dw) -> f32 {
    let t = age(w.born);
    let a = if t < 0.0 { 0.0 } else { popup::ease_out((t / 0.35).min(1.0)) };
    match w.dying { Some(d) => a * (1.0 - popup::ease_out((d.elapsed().as_secs_f32() / DIE_S).min(1.0))), None => a }
}

// ---------------------------------------------------------------- dibujo por GPU (DirectComposition)
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION};
use windows::Win32::Graphics::DirectComposition::{DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT};

/// Dispositivos compartidos por todos los widgets (Direct3D, Direct2D y la composición de Windows).
struct Shared { d3d: ID3D11Device, factory: IDXGIFactory2, d2d: ID2D1Device, dcomp: IDCompositionDevice }

thread_local! { static SHARED: RefCell<Option<Shared>> = const { RefCell::new(None) }; }

fn shared() -> Option<()> {
    if SHARED.with(|s| s.borrow().is_some()) { return Some(()); }
    use windows::core::Interface;
    unsafe {
        let mut d3d: Option<ID3D11Device> = None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut d3d), None, None).ok()?;
        let d3d = d3d?;
        let dxgi: IDXGIDevice = d3d.cast().ok()?;
        let factory: IDXGIFactory2 = dxgi.GetAdapter().ok()?.GetParent().ok()?;
        let f1 = D2D1CreateFactory::<ID2D1Factory1>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let d2d = f1.CreateDevice(&dxgi).ok()?;
        let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi).ok()?;
        SHARED.with(|s| *s.borrow_mut() = Some(Shared { d3d, factory, d2d, dcomp }));
        Some(())
    }
}

/// Lienzo de un widget en la GPU: una cadena de imágenes que Windows compone directamente sobre el
/// escritorio (sin copiar nada por la CPU, como hacen sus propias animaciones).
struct Gpu { dc: ID2D1DeviceContext, swap: IDXGISwapChain1, _target: IDCompositionTarget, _visual: IDCompositionVisual }

fn make_target(hwnd: HWND) -> Option<Gpu> {
    shared()?;
    SHARED.with(|s| {
        let s = s.borrow();
        let sh = s.as_ref()?;
        unsafe {
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let (w, h) = ((rc.right - rc.left).max(1) as u32, (rc.bottom - rc.top).max(1) as u32);
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: w, Height: h, Format: DXGI_FORMAT_B8G8R8A8_UNORM, Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT, BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH, SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED, Flags: 0,
            };
            let swap = sh.factory.CreateSwapChainForComposition(&sh.d3d, &desc, None).ok()?;
            let dc = sh.d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE).ok()?;
            let sc = scale(hwnd);
            let surf: IDXGISurface = swap.GetBuffer(0).ok()?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0 * sc, dpiY: 96.0 * sc, bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let bmp = dc.CreateBitmapFromDxgiSurface(&surf, Some(&props)).ok()?;
            dc.SetTarget(&bmp);
            // Se dibuja en las mismas DIP de siempre y Direct2D lo reduce entero
            dc.SetDpi(96.0 * sc, 96.0 * sc);
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let target = sh.dcomp.CreateTargetForHwnd(hwnd, true).ok()?;
            let visual = sh.dcomp.CreateVisual().ok()?;
            visual.SetContent(&swap).ok()?;
            target.SetRoot(&visual).ok()?;
            sh.dcomp.Commit().ok()?;
            Some(Gpu { dc, swap, _target: target, _visual: visual })
        }
    })
}

/// Pinceles y medidas comunes con los que se pinta un widget (en su ventana o en su capa fija).
fn make_ui<'a>(p: Painter<'a>, rt: &'a ID2D1RenderTarget, f: &'a popup::Fonts, w: f32, s: f32, glow: HashMap<u32, f32>, hover: Option<u32>, fl: bool, ink: f32) -> Ui<'a> {
    // Flotantes: sin tarjeta, así que los grises suben un poco y el texto lleva sombra
    let (dim, faint, track) = if fl { (0.72, 0.50, 0.24) } else { (0.58, 0.32, 0.14) };
    let c = |a: f32| popup::ink_color(ink, a);
    // La sombra va al revés que la tinta: oscura bajo texto blanco, clara bajo texto oscuro
    let sh = 1.0 - ink;
    Ui {
        glow,
        white: p.brush(c(0.97)), dim: p.brush(c(dim)), faint: p.brush(c(faint)),
        card: p.brush(c(0.075)), card_edge: p.brush(c(if fl { 0.16 } else { 0.10 })),
        off: p.brush(c(0.13)), on: p.brush(crate::profiles::accent(1.0)),
        hov: p.brush(c(0.07)), track: p.brush(c(track)),
        // Bajo texto oscuro, el halo claro casi desaparece (si no, el texto parece en relieve)
        shadow: fl.then(|| p.brush(rgba(1.0 - sh, 1.0 - sh, 1.0 - sh, 0.13 * (1.0 - ink) + 0.06 * ink))),
        p, rt, f, w, s, hover, hits: Vec::new(), ink,
    }
}

// ---------------------------------------------------------------- texto claro u oscuro
/// Tinta de los widgets: 0 = automática (según el fondo de cada uno), 1 = oscura (fondos claros),
/// 2 = clara (fondos oscuros). -1 = sin leer aún.
static INK_MODE: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(-1);
pub fn ink_mode() -> i8 {
    let v = INK_MODE.load(Ordering::Relaxed);
    if v >= 0 { return v; }
    let m = match ini_get(&ini(), "texto").as_deref() { Some("oscuro") => 1, Some("claro") => 2, _ => 0 };
    INK_MODE.store(m, Ordering::Relaxed);
    m
}
/// Repinta todos los widgets (al cambiar el idioma).
pub fn repaint_all() {
    let all: Vec<HWND> = DESKS.with(|d| d.borrow().iter().map(|w| w.hwnd).collect());
    for h in all { with_dw(h, |w| w.layer = None); unsafe { let _ = InvalidateRect(Some(h), None, false); } }
}

/// Cambia el modo (menú de la barra) y lo guarda.
pub fn set_ink_mode(m: i8) {
    INK_MODE.store(m, Ordering::Relaxed);
    let txt = ini();
    let mut out: Vec<String> = txt.lines().filter(|l| !l.trim_start().starts_with("texto")).map(str::to_string).collect();
    out.push(format!("texto = {}", match m { 1 => "oscuro", 2 => "claro", _ => "auto" }));
    let _ = std::fs::write(ini_path(), out.join("\n") + "\n");
    let all: Vec<HWND> = DESKS.with(|d| d.borrow().iter().map(|w| w.hwnd).collect());
    for h in all { refresh_ink(h, true); }
}

/// Luminosidad media (0..1) del fondo que rodea al widget: una franja justo por fuera de su borde, que
/// es fondo de escritorio visible (lo de debajo del widget no se puede ver sin quitarlo).
fn backdrop_luma(hwnd: HWND) -> Option<f32> {
    unsafe {
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r).ok()?;
        let m = 18;
        let (x0, y0, w, h) = (r.left - m, r.top - m, r.right - r.left + 2 * m, r.bottom - r.top + 2 * m);
        // Ventana sin tamaño todavía (o pantalla dormida): no se mide
        if w <= 2 * m + 16 || h <= 2 * m + 16 { return None; }
        let sdc = GetDC(None);
        let mdc = CreateCompatibleDC(Some(sdc));
        let bi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, ..Default::default() }, ..Default::default() };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(mdc), &bi, DIB_RGB_COLORS, &mut bits, None, 0).ok();
        let mut out = None;
        if let Some(dib) = dib {
            let old = SelectObject(mdc, dib.into());
            if BitBlt(mdc, 0, 0, w, h, Some(sdc), x0, y0, SRCCOPY).is_ok() && !bits.is_null() {
                let px = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
                let (mut sum, mut n) = (0.0f32, 0u32);
                // Cada 3 píxeles de la franja exterior (12 px de ancho, dejando 6 de margen)
                for y in (0..h).step_by(3) {
                    for x in (0..w).step_by(3) {
                        let inside_out = x < m - 6 || x >= w - m + 6 || y < m - 6 || y >= h - m + 6;
                        if !inside_out { continue; }
                        let i = ((y * w + x) * 4) as usize;
                        if i + 2 >= px.len() { continue; }
                        let (b, g, rr) = (px[i] as f32, px[i + 1] as f32, px[i + 2] as f32);
                        sum += (0.2126 * rr + 0.7152 * g + 0.0722 * b) / 255.0;
                        n += 1;
                    }
                }
                if n > 0 { out = Some(sum / n as f32); }
            }
            SelectObject(mdc, old);
            let _ = DeleteObject(dib.into());
        }
        let _ = DeleteDC(mdc);
        ReleaseDC(None, sdc);
        out
    }
}

/// Decide la tinta de un widget (con histéresis: no parpadea en fondos de luminosidad media).
fn refresh_ink(hwnd: HWND, force: bool) {
    if crate::fullscreen() && !force { return; }
    let mode = ink_mode();
    let want = match mode {
        1 => Some(true),
        2 => Some(false),
        _ => {
            let cur = with_dw(hwnd, |w| w.light).unwrap_or(false);
            backdrop_luma(hwnd).map(|l| if cur { l > 0.50 } else { l > 0.62 })
        }
    };
    let Some(light) = want else { return };
    let ch = with_dw(hwnd, |w| { let c = w.light != light; w.light = light; c }).unwrap_or(false);
    if ch || force { unsafe { let _ = InvalidateRect(Some(hwnd), None, false); } }
}

fn paint_real(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        BeginPaint(hwnd, &mut ps);
        let anim = with_dw(hwnd, |w| {
            if w.target.is_none() {
                // Al cambiar de lienzo, la capa fija y las cachés de pinceles se rehacen
                w.target = make_target(hwnd);
                w.layer = None;
            }
            let Some(gpu) = w.target.as_ref() else { return false };
            let rt: ID2D1RenderTarget = { use windows::core::Interface; gpu.dc.cast().unwrap() };
            let swap = gpu.swap.clone();
            let s = scale(hwnd);
            let dt = w.last_paint.elapsed().as_secs_f32().min(1.0 / 20.0);
            w.last_paint = Instant::now();
            if let Some(h) = w.hover { w.glow.entry(h).or_insert(0.0); }
            let mut moving = false;
            for (id, v) in w.glow.iter_mut() {
                let t = if w.hover == Some(*id) { 1.0 } else { 0.0 };
                let tau = if t > *v { 0.05 } else { 0.14 };
                if (t - *v).abs() < 0.01 { *v = t; } else { *v += (t - *v) * (1.0 - (-dt / tau).exp()); moving = true; }
            }
            w.glow.retain(|_, v| *v > 0.0);
            // Hundimiento al pulsar: baja rápido y vuelve algo más despacio
            if let Some(p) = w.pressed { w.press.entry(p).or_insert(0.0); }
            for (id, v) in w.press.iter_mut() {
                let t = if w.pressed == Some(*id) { 1.0 } else { 0.0 };
                let tau = if t > *v { 0.035 } else { 0.11 };
                if (t - *v).abs() < 0.01 { *v = t; } else { *v += (t - *v) * (1.0 - (-dt / tau).exp()); moving = true; }
            }
            w.press.retain(|_, v| *v > 0.0);
            PRESS_NOW.with(|p| *p.borrow_mut() = w.press.clone());
            MOUSE_AT.with(|m| m.set(w.mouse));
            // Elevación al arrastrar
            let lift_to = if w.drag.is_some_and(|d| d.2) { 1.0 } else { 0.0 };
            if (lift_to - w.lift).abs() > 0.005 { w.lift += (lift_to - w.lift) * (1.0 - (-dt / 0.09).exp()); moving = true; } else { w.lift = lift_to; }
            let fl = floating();
            // La tinta pasa de clara a oscura (o al revés) con un fundido corto
            let ink_to = if w.light { 1.0 } else { 0.0 };
            if (ink_to - w.ink).abs() > 0.004 { w.ink += (ink_to - w.ink) * (1.0 - (-dt / 0.18).exp()); moving = true; } else { w.ink = ink_to; }
            let ink = w.ink;
            let (ww, wh) = size(w.kind);
            let t = age(w.born).max(0.0);
            // Capa fija: lo que cambia como mucho una vez por segundo se pinta en una imagen y en cada
            // fotograma solo se pega; encima va lo que se mueve (`paint_live`)
            let layered = w.w.layered();
            if layered {
                let rtk = { use windows::core::Interface; rt.cast::<windows::core::IUnknown>().ok() };
                if w.layer.as_ref().is_none_or(|l| l.0 != rtk) {
                    w.layer = rt.CreateCompatibleRenderTarget(Some(&D2D_SIZE_F { width: ww, height: wh }), None, None, D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE).ok().map(|b| (rtk.clone(), b, false));
                }
                let need = moving || t < 1.4 || w.dying.is_some() || w.w.layer_stale() || w.layer.as_ref().is_some_and(|l| !l.2);
                if need {
                    if let Some((_, bt, ok)) = w.layer.as_mut() {
                        let brt: &ID2D1RenderTarget = bt;
                        brt.BeginDraw();
                        brt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
                        let p2 = Painter { rt: brt, dw: &w.dw, cache: &w.lcache, h: 100.0, s };
                        let mut ui2 = make_ui(p2, brt, &w.f, ww, s, w.glow.clone(), w.hover, fl, ink);
                        w.w.paint(&mut ui2, &w.d, t, dt);
                        w.layer_hits = std::mem::take(&mut ui2.hits);
                        drop(ui2);
                        *ok = brt.EndDraw(None, None).is_ok();
                    }
                }
            }
            let p = Painter { rt: &rt, dw: &w.dw, cache: &w.cache, h: 100.0, s };
            let mut ui = make_ui(p, &rt, &w.f, ww, s, w.glow.clone(), w.hover, fl, ink);
            rt.BeginDraw();
            rt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
            // Fundido de entrada y de salida dentro del propio dibujo (sin ventanas "layered", que
            // obligaban a Windows a copiar cada fotograma por la CPU)
            // Cara de iconos: el widget se aparta hacia su lado de la pantalla y se desvanece
            popup::push_alpha(&rt, fade_alpha(w));
            if !fl {
                // Tarjeta: algo más tapada que los paneles, encima del fondo de pantalla hace falta contraste
                // (con la composición por GPU, Windows ya no redondea la ventana: la tarjeta lo hace sola)
                // Con tinta oscura, la tarjeta es clara (cristal blanco)
                let mix = |a: f32, b: f32| a + (b - a) * ink;
                let tint = ui.brush(rgba(mix(0.10, 0.96), mix(0.11, 0.96), mix(0.12, 0.97), mix(0.58, 0.62)));
                rt.FillRoundedRectangle(&rrect(rect(0.0, 0.0, ww, wh), 16.0), &tint);
                rt.DrawRoundedRectangle(&rrect(rect(0.5, 0.5, ww - 0.5, wh - 0.5), 15.5), &ui.wa(0.10), 1.0, None);
            }
            // Entrada: sube 12 px mientras crece de 0,965 a 1 con un pelín de rebote (desde el centro)
            let rise = 1.0 - popup::ease_out((t / 0.55).min(1.0));
            let mut sc = 0.965 + 0.035 * crate::ease_back((t / 0.6).min(1.0));
            // Al arrastrar se "levanta" (crece un 2,5 %) y al quitarlo se encoge mientras se desvanece
            sc *= 1.0 + 0.025 * w.lift;
            let die = w.dying.map(|d| popup::ease_out((d.elapsed().as_secs_f32() / DIE_S).min(1.0))).unwrap_or(0.0);
            sc *= 1.0 - 0.07 * die;
            rt.SetTransform(&windows_numerics::Matrix3x2 { M11: sc, M12: 0.0, M21: 0.0, M22: sc, M31: ww / 2.0 * (1.0 - sc), M32: wh / 2.0 * (1.0 - sc) + 12.0 * rise + 6.0 * die });
            if layered {
                if let Some(bmp) = w.layer.as_ref().and_then(|l| l.1.GetBitmap().ok()) {
                    // Sin escalar, píxel a píxel (nítida); escalada (entrada, arrastre) con suavizado
                    let mode = if (sc - 1.0).abs() < 0.0005 { D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR } else { D2D1_BITMAP_INTERPOLATION_MODE_LINEAR };
                    rt.DrawBitmap(&bmp, Some(&rect(0.0, 0.0, ww, wh)), 1.0, mode, None);
                }
                w.w.paint_live(&mut ui, &w.d, t, dt);
                let mut h = w.layer_hits.clone();
                h.append(&mut ui.hits);
                ui.hits = h;
            } else {
                w.w.paint(&mut ui, &w.d, t, dt);
            }
            rt.SetTransform(&popup::IDENTITY);
            let _ = s;
            rt.PopLayer();
            let anim = moving || w.w.animating(t) || w.dying.is_some();

            // Las transiciones cortas van a 60 fotogramas; lo continuo (gráficas, letra) a 30
            // (la transición entre caras también, a 60 fotogramas)
            let fast = moving || t < 1.4 || w.dying.is_some() || w.w.smooth();
            FAST.with(|f| f.set(if fast { 15 } else { w.w.frame_ms() }));
            w.hits = std::mem::take(&mut ui.hits);
            drop(ui);
            let te = Instant::now();
            // Entrega a la composición de Windows sin esperar al refresco (no bloquea el hilo)
            if rt.EndDraw(None, None).is_err() || swap.Present(0, DXGI_PRESENT(0)).is_err() { w.target = None; }
            ENDDRAW_MS.with(|e| e.set(e.get() + te.elapsed().as_secs_f32() * 1000.0));
            anim
        }).unwrap_or(false);
        let _ = EndPaint(hwnd, &ps);
        if anim && !covered_w(hwnd) { SetTimer(Some(hwnd), TIMER_FRAME, FAST.with(|f| f.get()), None); }
    }
}

fn hit_at(hwnd: HWND, x: i32, y: i32) -> Option<u32> {
    let s = scale(hwnd);
    let (x, y) = (x as f32 / s, y as f32 / s);
    with_dw(hwnd, |w| w.hits.iter().rev().find(|(r, _)| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom).map(|(_, i)| *i)).flatten()
}

fn lp_xy(lp: LPARAM) -> (i32, i32) { ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32) }

/// Imán: la esquina cae en una rejilla de 8 DIP y se pega a los bordes del área de trabajo y a los
/// demás widgets cuando está cerca.
fn snapped(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    let s = scale(hwnd);
    let g = (SNAP * s) as i32;
    let mut me = RECT::default();
    unsafe { let _ = GetWindowRect(hwnd, &mut me); }
    let (wpx, hpx) = (me.right - me.left, me.bottom - me.top);
    let (gap, near) = ((GAP * s) as i32, (14.0 * s) as i32);
    let (mut nx, mut ny) = ((x + g / 2) / g * g, (y + g / 2) / g * g);
    let others: Vec<RECT> = DESKS.with(|d| d.borrow().iter().filter(|w| w.hwnd != hwnd).map(|w| { let mut r = RECT::default(); unsafe { let _ = GetWindowRect(w.hwnd, &mut r); } r }).collect());
    // Se pega al lado de otro widget, o se alinea con sus bordes (izquierdo/derecho, arriba/abajo)
    for r in others {
        for cand in [r.left, r.right + gap, r.left - wpx - gap, r.right - wpx] { if (x - cand).abs() < near { nx = cand; } }
        for cand in [r.top, r.bottom + gap, r.top - hpx - gap, r.bottom - hpx] { if (y - cand).abs() < near { ny = cand; } }
    }
    (nx, ny)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => { paint(hwnd); LRESULT(0) }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_WINDOWPOSCHANGING => {
                // Siempre justo encima del fondo: nunca por delante de una ventana normal
                let pos = &mut *(lp.0 as *mut WINDOWPOS);
                if std::env::var_os("MACBAR_WIDGETS").is_some() { pos.hwndInsertAfter = HWND_TOPMOST; pos.flags &= !SWP_NOZORDER; return LRESULT(0); }
                let dragging = with_dw(hwnd, |w| w.drag.is_some()).unwrap_or(false);
                if !dragging {
                    // Los widgets forman un bloque justo encima del escritorio. Si este ya está en el
                    // bloque no se mueve; si no, se mete debajo de la primera ventana que no sea widget.
                    let desk = desktop_window();
                    let mut cur = GetWindow(desk, GW_HWNDPREV).unwrap_or_default();
                    let mut inside = false;
                    while !cur.0.is_null() && (is_desk(cur) || cur == hwnd) {
                        if cur == hwnd { inside = true; break; }
                        cur = GetWindow(cur, GW_HWNDPREV).unwrap_or_default();
                    }
                    if inside { pos.flags |= SWP_NOZORDER; }
                    else {
                        pos.hwndInsertAfter = if cur.0.is_null() { HWND_TOP } else { cur };
                        pos.flags &= !SWP_NOZORDER;
                    }
                }
                LRESULT(0)
            }
            WM_TIMER => {
                match wp.0 {
                    TIMER_FRAME => { let _ = KillTimer(Some(hwnd), TIMER_FRAME); let _ = InvalidateRect(Some(hwnd), None, false); }
                    TIMER_SEC => {
                        let r = with_dw(hwnd, |w| w.w.tick()).unwrap_or(false);
                        let hidden = covered_w(hwnd);
                        // El fondo se mira cada 4 s, y solo si se ve (tapado, se leería la ventana de encima)
                        let tick_n = with_dw(hwnd, |w| { w.lum_n = w.lum_n.wrapping_add(1); w.lum_n }).unwrap_or(0);
                        if !hidden && tick_n % 4 == 1 { refresh_ink(hwnd, false); }
                        // El primero de la lista decide si los hilos de datos trabajan
                        if DESKS.with(|d| d.borrow().first().map(|w| w.hwnd) == Some(hwnd)) { deskdata::VISIBLE.store(!covered(), Ordering::Relaxed); }
                        // Al destaparse retoma también las animaciones (el pintado vuelve a pedir fotogramas)
                        if r && !hidden { let _ = InvalidateRect(Some(hwnd), None, false); }
                    }
                    TIMER_FADE => {
                        let (t, dying) = with_dw(hwnd, |w| (age(w.born), w.dying)).unwrap_or((1.0, None));
                        if let Some(d) = dying {
                            // Saliendo: se desvanece y, al acabar, se cierra de verdad
                            let k = (d.elapsed().as_secs_f32() / DIE_S).min(1.0);
                            // (la opacidad la pone el propio pintado: ver `fade_alpha`)
                            let _ = InvalidateRect(Some(hwnd), None, false);
                            if k >= 1.0 {
                                let _ = KillTimer(Some(hwnd), TIMER_FADE);
                                DESKS.with(|ds| ds.borrow_mut().retain(|w| w.hwnd != hwnd));
                                let _ = DestroyWindow(hwnd);
                            }
                            return LRESULT(0);
                        }
                        if t >= 0.35 { let _ = KillTimer(Some(hwnd), TIMER_FADE); }
                        if t >= 0.0 { let _ = InvalidateRect(Some(hwnd), None, false); }
                    }
                    TIMER_GLIDE => {
                        // Deslizamiento suave hasta su sitio de la rejilla al soltarlo
                        let g = with_dw(hwnd, |w| w.glide).flatten();
                        match g {
                            Some((a, b, t0)) => {
                                let k = popup::ease_out((t0.elapsed().as_secs_f32() / GLIDE_S).min(1.0));
                                let (x, y) = (a.x + ((b.x - a.x) as f32 * k).round() as i32, a.y + ((b.y - a.y) as f32 * k).round() as i32);
                                let _ = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                                if k >= 1.0 {
                                    let _ = KillTimer(Some(hwnd), TIMER_GLIDE);
                                    with_dw(hwnd, |w| w.glide = None);
                                    save(true);
                                }
                            }
                            None => { let _ = KillTimer(Some(hwnd), TIMER_GLIDE); }
                        }
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let mut c = POINT::default();
                let _ = GetCursorPos(&mut c);
                let mut r = RECT::default();
                let _ = GetWindowRect(hwnd, &mut r);
                let (x, y) = lp_xy(lp);
                let id = hit_at(hwnd, x, y);
                with_dw(hwnd, |w| { w.drag = Some((c, POINT { x: r.left, y: r.top }, false)); w.pressed = id; w.glide = None; });
                SetCapture(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let drag = with_dw(hwnd, |w| w.drag).flatten();
                if let Some((c0, w0, moved)) = drag {
                    let mut c = POINT::default();
                    let _ = GetCursorPos(&mut c);
                    let (dx, dy) = (c.x - c0.x, c.y - c0.y);
                    let th = (5.0 * scale_of(hwnd)) as i32;
                    if moved || dx.abs() > th || dy.abs() > th {
                        // Empieza a arrastrarse: suelta el botón que hubiera bajo el ratón y se levanta
                        with_dw(hwnd, |w| { if let Some(d) = w.drag.as_mut() { d.2 = true; } w.pressed = None; });
                        let _ = SetWindowPos(hwnd, None, w0.x + dx, w0.y + dy, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                        if !moved { let _ = InvalidateRect(Some(hwnd), None, false); }
                    }
                    return LRESULT(0);
                }
                let (x, y) = lp_xy(lp);
                let h = hit_at(hwnd, x, y);
                let s = scale(hwnd);
                let ch = with_dw(hwnd, |w| { let c = w.hover != h; w.hover = h; w.mouse = (x as f32 / s, y as f32 / s); c || w.w.follows_mouse() }).unwrap_or(false);
                if ch { let _ = InvalidateRect(Some(hwnd), None, false); }
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
                LRESULT(0)
            }
            WM_MOUSELEAVE => { with_dw(hwnd, |w| { w.hover = None; w.mouse = (-1.0, -1.0); }); let _ = InvalidateRect(Some(hwnd), None, false); LRESULT(0) }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                let drag = with_dw(hwnd, |w| { w.pressed = None; w.drag.take() }).flatten();
                let _ = InvalidateRect(Some(hwnd), None, false);
                if let Some((_, _, true)) = drag {
                    let mut r = RECT::default();
                    let _ = GetWindowRect(hwnd, &mut r);
                    let (x, y) = snapped(hwnd, r.left, r.top);
                    let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                    // En vez de saltar al imán, se desliza hasta él (y se guarda al llegar)
                    with_dw(hwnd, |w| w.glide = Some((POINT { x: r.left, y: r.top }, POINT { x, y }, Instant::now())));
                    SetTimer(Some(hwnd), TIMER_GLIDE, 10, None);
                } else {
                    let (x, y) = lp_xy(lp);
                    click(hwnd, x, y, false);
                }
                LRESULT(0)
            }
            WM_RBUTTONDOWN => { with_dw(hwnd, |w| w.rdown = true); LRESULT(0) }
            WM_RBUTTONUP => {
                if with_dw(hwnd, |w| std::mem::take(&mut w.rdown)).unwrap_or(false) {
                    let (x, y) = lp_xy(lp);
                    click(hwnd, x, y, true);
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let d = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
                if with_dw(hwnd, |w| w.w.wheel(d)).unwrap_or(false) { let _ = InvalidateRect(Some(hwnd), None, false); }
                LRESULT(0)
            }
            WM_DPICHANGED | WM_DISPLAYCHANGE => {
                let k = with_dw(hwnd, |w| { w.target = None; w.kind });
                let s = scale(hwnd);
                if let Some(k) = k { let (ww, wh) = size(k); let _ = SetWindowPos(hwnd, None, 0, 0, (ww * s) as i32, (wh * s) as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE); }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Clic en una zona: el widget decide y lo que haya que hacer con Windows se ejecuta fuera del préstamo.
fn click(hwnd: HWND, x: i32, y: i32, right: bool) {
    let id = hit_at(hwnd, x, y);
    let s = scale(hwnd);
    CLICK_AT.with(|c| c.set((x as f32 / s, y as f32 / s)));
    let mut pt = POINT { x, y };
    unsafe { let _ = ClientToScreen(hwnd, &mut pt); }
    let act = with_dw(hwnd, |w| w.w.click(id, right, pt)).flatten();
    match act {
        Some(f) => f(),
        // Clic derecho donde el widget no tiene nada propio: quitarlo o poner otros
        None if right => {
            let Some(kind) = with_dw(hwnd, |w| w.kind) else { return };
            let others: Vec<(usize, &str)> = ALL.iter().enumerate().filter(|(_, k)| !shown(**k)).map(|(i, k)| (i, k.label())).collect();
            // Cada entrada con lo que hace: quitar, estilo, diseños de este widget y añadir otros
            enum Do { Remove, Style, Design(usize), Add(usize), Nothing }
            let mut items: Vec<(String, Do)> = vec![(i18n::tf("Quitar «{}»", &[&kind.label()]), Do::Remove), (if floating() { i18n::t("Con fondo").into() } else { i18n::t("Sin fondo (flotantes)").into() }, Do::Style)];
            let vs = variants(kind);
            if vs.len() > 1 {
                items.push((String::new(), Do::Nothing));
                let cur = variant(kind);
                for (n, v) in vs.iter().enumerate() { items.push((format!("{}{}", if n == cur { "✓  " } else { "      " }, i18n::tf("Diseño «{}»", &[&i18n::t(v)])), Do::Design(n))); }
            }
            if !others.is_empty() { items.push((String::new(), Do::Nothing)); for (i, l) in &others { items.push((i18n::tf("Añadir «{}»", &[l]), Do::Add(*i))); } }
            let refs: Vec<&str> = items.iter().map(|s| s.0.as_str()).collect();
            match context_menu(&refs, pt).and_then(|n| items.get(n)).map(|x| &x.1) {
                Some(Do::Remove) => { if let Some(i) = ALL.iter().position(|k| *k == kind) { toggle_kind(i); } return; }
                Some(Do::Style) => toggle_style(),
                Some(Do::Design(n)) => { set_variant(kind, *n); return; }
                Some(Do::Add(i)) => toggle_kind(*i),
                _ => {}
            }
        }
        None => {}
    }
    unsafe { let _ = InvalidateRect(Some(hwnd), None, false); }
}

/// Menú contextual nativo (oscuro). Devuelve el índice elegido.
pub fn context_menu(items: &[&str], pt: POINT) -> Option<usize> {
    unsafe {
        let h = CreatePopupMenu().ok()?;
        for (i, it) in items.iter().enumerate() {
            if it.is_empty() { let _ = AppendMenuW(h, MF_SEPARATOR, 0, None); }
            else { let _ = AppendMenuW(h, MF_STRING, i + 1, &HSTRING::from(it.replace('&', "&&"))); }
        }
        let owner = bar_hwnd();
        let _ = SetForegroundWindow(owner);
        let cmd = TrackPopupMenu(h, TPM_RETURNCMD | TPM_NONOTIFY | TPM_LEFTALIGN | TPM_TOPALIGN, pt.x, pt.y, Some(0), owner, None);
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(h);
        let i = cmd.0 as usize;
        (i > 0).then(|| i - 1)
    }
}

