//! Menús desplegables estilo macOS sobre menús nativos de Windows en modo oscuro.
//! Mientras un menú está abierto, pasar el ratón por otro título de la barra salta a ese menú.

use crate::i18n::{t, tf};
use crate::sys::{self, combo, vk};
use std::cell::RefCell;
use std::sync::Mutex;
use windows::core::{HSTRING, PCSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Lo que hace cada opción de menú.
pub enum A {
    Keys(Vec<VIRTUAL_KEY>),
    Open(String),
    QuitApp(u32),
    HideApp(u32),
    HideOthers(u32),
    ShowAll,
    Reveal(String),
    Relaunch(String),
    Lock,
    SignOut,
    Sleep,
    Restart,
    Shutdown,
    Autostart(bool),
    EditConfig,
    /// Ventana de Configuración de macbar.
    Settings,
    /// Acciones que resuelve la barra (tiempo, salir…): se devuelven a main.
    Bar(u32),
}

pub enum Entry { Item { label: String, enabled: bool, checked: bool, act: Option<A> }, Sep }

#[derive(Default)]
pub struct Menu { pub items: Vec<Entry> }

impl Menu {
    pub fn item(&mut self, label: impl Into<String>, act: A) -> &mut Self {
        self.items.push(Entry::Item { label: label.into(), enabled: true, checked: false, act: Some(act) });
        self
    }
    pub fn check(&mut self, label: impl Into<String>, on: bool, act: A) -> &mut Self {
        self.items.push(Entry::Item { label: label.into(), enabled: true, checked: on, act: Some(act) });
        self
    }
    /// Texto informativo, no pulsable.
    pub fn info(&mut self, label: impl Into<String>) -> &mut Self {
        self.items.push(Entry::Item { label: label.into(), enabled: false, checked: false, act: None });
        self
    }
    pub fn sep(&mut self) -> &mut Self { self.items.push(Entry::Sep); self }
}

// ---------------------------------------------------------------- modo oscuro (uxtheme, sin documentar)
pub fn enable_dark_menus() {
    unsafe {
        let Ok(ux) = LoadLibraryW(windows::core::w!("uxtheme.dll")) else { return };
        // 135 = SetPreferredAppMode, 136 = FlushMenuThemes (estables desde Windows 10 1903)
        if let Some(f) = GetProcAddress(ux, PCSTR(135 as *const u8)) {
            let set: extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set(2); // ForceDark
        }
        if let Some(f) = GetProcAddress(ux, PCSTR(136 as *const u8)) {
            let flush: extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

// ---------------------------------------------------------------- seguimiento con salto entre menús
struct Hook { current: usize, probe: Box<dyn Fn(POINT) -> Option<usize>>, pending: Option<usize> }
thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }

unsafe extern "system" fn filter(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code == MSGF_MENU as i32 {
            let msg = &*(lp.0 as *const MSG);
            if msg.message == WM_MOUSEMOVE {
                let switch = HOOK.with(|h| {
                    let mut h = h.borrow_mut();
                    let h = h.as_mut()?;
                    if h.pending.is_some() { return None; }
                    let i = (h.probe)(msg.pt)?;
                    (i != h.current).then(|| { h.pending = Some(i); })
                });
                if switch.is_some() { let _ = EndMenu(); }
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}

pub enum Outcome { Picked(A), Switch(usize), Closed }

pub fn track(owner: HWND, menu: Menu, x: i32, y: i32, right_align: bool, current: usize, probe: Box<dyn Fn(POINT) -> Option<usize>>) -> Outcome {
    unsafe {
        let h = CreatePopupMenu().unwrap();
        let mut acts: Vec<Option<A>> = Vec::new();
        for e in menu.items {
            match e {
                Entry::Sep => { let _ = AppendMenuW(h, MF_SEPARATOR, 0, None); }
                Entry::Item { label, enabled, checked, act } => {
                    acts.push(act);
                    let mut fl = MF_STRING;
                    if !enabled { fl |= MF_GRAYED; }
                    if checked { fl |= MF_CHECKED; }
                    let _ = AppendMenuW(h, fl, acts.len(), &HSTRING::from(menu_text(&label)));
                }
            }
        }
        HOOK.with(|s| *s.borrow_mut() = Some(Hook { current, probe, pending: None }));
        let hook = SetWindowsHookExW(WH_MSGFILTER, Some(filter), None, GetCurrentThreadId()).ok();
        // Necesario para que el menú se cierre al hacer clic fuera
        let _ = SetForegroundWindow(owner);
        let align = if right_align { TPM_RIGHTALIGN } else { TPM_LEFTALIGN };
        // Con la barra abajo, el menú sale hacia arriba desde el borde de arriba de la barra
        let (vert, y) = if crate::bar_bottom() {
            (TPM_BOTTOMALIGN, GetSystemMetrics(SM_CYSCREEN) - (crate::BAR_H * crate::scale_of(owner)) as i32)
        } else { (TPM_TOPALIGN, y) };
        let cmd = TrackPopupMenu(h, TPM_RETURNCMD | TPM_NONOTIFY | vert | align, x, y, Some(0), owner, None);
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        if let Some(hk) = hook { let _ = UnhookWindowsHookEx(hk); }
        let _ = DestroyMenu(h);
        let pending = HOOK.with(|s| s.borrow_mut().take().and_then(|h| h.pending));
        if let Some(p) = pending { return Outcome::Switch(p); }
        let i = cmd.0 as usize;
        if i == 0 || i > acts.len() { return Outcome::Closed; }
        match acts.swap_remove(i - 1) { Some(a) => Outcome::Picked(a), None => Outcome::Closed }
    }
}

// ---------------------------------------------------------------- ejecutar acciones
static HIDDEN: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// Devuelve el foco a la app y espera a que Windows lo haya aplicado de verdad.
/// Sin esta espera el atajo (p. ej. F11) llegaba antes que el foco y se perdía.
fn focus(h: HWND) {
    if h.0.is_null() || sys::is_shell_window(h) { return; }
    unsafe {
        // Tras cerrar el menú la barra es la ventana activa, así que puede devolver el foco
        if !SetForegroundWindow(h).as_bool() { sys::activate(h); }
        let t = std::time::Instant::now();
        while GetForegroundWindow() != h && t.elapsed().as_millis() < 700 {
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }
    // Margen para que la app procese la activación y recoloque su foco de teclado
    std::thread::sleep(std::time::Duration::from_millis(120));
}

/// En los menús, "&" marca la letra subrayada: hay que doblarlo para que salga tal cual.
fn menu_text(s: &str) -> String { s.replace('&', "&&") }

/// Minimiza todas las ventanas (o las restaura si ya lo estaban). No oculta la barra, a diferencia de Win+D.
pub fn toggle_desktop() {
    let mut hidden = HIDDEN.lock().unwrap();
    let still: Vec<HWND> = hidden.iter().map(|h| HWND(*h as _)).filter(|h| unsafe { IsWindow(Some(*h)).as_bool() && IsIconic(*h).as_bool() }).collect();
    if !still.is_empty() {
        for h in still.iter().rev() { unsafe { let _ = ShowWindow(*h, SW_SHOWNOACTIVATE); } }
        if let Some(top) = still.first() { focus(*top); }
        hidden.clear();
        return;
    }
    hidden.clear();
    for h in sys::app_windows() {
        if unsafe { !IsIconic(h).as_bool() } { hidden.push(h.0 as isize); sys::minimize(h); }
    }
}

/// Ejecuta la acción. Devuelve Some(id) si es de la barra y debe resolverla main.
pub fn perform(owner: HWND, target: HWND, a: A) -> Option<u32> {
    match a {
        A::Keys(k) => { focus(target); combo(&k); }
        A::Open(s) => sys::open(&s),
        A::QuitApp(pid) => { for h in sys::windows_of(pid) { sys::close(h); } }
        A::HideApp(pid) => { for h in sys::windows_of(pid) { sys::minimize(h); } }
        A::HideOthers(pid) => {
            let mut hidden = HIDDEN.lock().unwrap();
            for h in sys::app_windows() {
                if sys::real_pid(h) != pid && unsafe { !IsIconic(h).as_bool() } { hidden.push(h.0 as isize); sys::minimize(h); }
            }
        }
        A::ShowAll => {
            let list: Vec<isize> = std::mem::take(&mut *HIDDEN.lock().unwrap());
            for h in list { unsafe { let _ = ShowWindow(HWND(h as _), SW_SHOWNOACTIVATE); } }
        }
        A::Reveal(path) => sys::open_args("explorer.exe", &format!("/select,\"{path}\"")),
        A::Relaunch(path) => sys::open(&path),
        A::Lock => sys::lock(),
        A::SignOut => { if sys::ask(owner, t("¿Cerrar la sesión? Se cerrarán todas las apps.")) { sys::sign_out(); } }
        A::Sleep => sys::sleep(),
        A::Restart => { if sys::ask(owner, t("¿Reiniciar el equipo ahora?")) { sys::restart(); } }
        A::Shutdown => { if sys::ask(owner, t("¿Apagar el equipo ahora?")) { sys::shutdown(); } }
        A::Autostart(on) => sys::set_autostart(on),
        A::EditConfig => sys::edit_config(),
        A::Settings => crate::settings::open(),
        A::Bar(id) => return Some(id),
    }
    None
}

// ---------------------------------------------------------------- contenido de cada menú
/// Contexto capturado al hacer clic: la ventana y la app activas en ese momento.
pub struct Ctx {
    pub pid: u32,
    pub app: String,
    pub exe: Option<String>,
    pub desktop: bool,
}

fn k(keys: &[VIRTUAL_KEY]) -> A { A::Keys(keys.to_vec()) }

pub fn system_menu() -> Menu {
    let mut m = Menu::default();
    m.item(t("Acerca de este equipo"), A::Open("ms-settings:about".into()))
        .sep()
        .item(t("Configuración…\tWin+I"), A::Open("ms-settings:".into()))
        .item("Microsoft Store…", A::Open("ms-windows-store:".into()))
        .sep()
        .item(t("Elementos recientes"), A::Open("shell:recent".into()))
        .sep()
        .item(t("Forzar salida…\tCtrl+Mayús+Esc"), A::Open("taskmgr.exe".into()))
        .sep()
        .item(t("Suspender"), A::Sleep)
        .item(t("Reiniciar…"), A::Restart)
        .item(t("Apagar…"), A::Shutdown)
        .sep()
        .item(t("Bloquear pantalla\tWin+L"), A::Lock)
        .item(tf("Cerrar sesión de {}…", &[&sys::display_name()]), A::SignOut);
    m
}

pub fn app_menu(c: &Ctx) -> Menu {
    let mut m = Menu::default();
    if c.desktop {
        m.info(t("Escritorio")).sep().item(t("Abrir el Explorador\tWin+E"), k(&[VK_LWIN, vk(b'E')])).item(t("Mostrar todo"), A::ShowAll);
        return m;
    }
    if let Some(exe) = &c.exe {
        m.item(tf("Mostrar {} en el Explorador", &[&c.app]), A::Reveal(exe.clone()));
        if !exe.to_lowercase().contains("\\windowsapps\\") {
            m.item(tf("Abrir otra ventana de {}", &[&c.app]), A::Relaunch(exe.clone()));
        }
        m.sep();
    }
    m.item(tf("Ocultar {}", &[&c.app]), A::HideApp(c.pid))
        .item(t("Ocultar las demás"), A::HideOthers(c.pid))
        .item(t("Mostrar todo"), A::ShowAll)
        .sep()
        .item(tf("Salir de {}", &[&c.app]), A::QuitApp(c.pid));
    m
}







/// Clic derecho en la isla: controles de reproducción.
pub fn island_menu(title: &str, artist: &str, playing: bool) -> Menu {
    let mut m = Menu::default();
    m.info(crate::trunc(title, 44));
    if !artist.is_empty() { m.info(crate::trunc(artist, 44)); }
    m.sep()
        .item(t("Anterior"), A::Bar(crate::BAR_PREV))
        .item(if playing { t("Pausa") } else { t("Reproducir") }, A::Bar(crate::BAR_TOGGLE))
        .item(t("Siguiente"), A::Bar(crate::BAR_NEXT))
        .sep()
        .item(t("Abrir el reproductor"), A::Bar(crate::BAR_PLAYER));
    m
}

pub fn battery_menu(pct: u8, charging: bool, secs: Option<u32>) -> Menu {
    let mut m = Menu::default();
    m.info(tf("Batería: {} %", &[&pct]));
    m.info(if charging && pct >= 100 { t("Conectado, carga completa").to_string() } else if charging { t("Cargando").to_string() } else {
        match secs { Some(s) => { let mm = format!("{:02}", (s % 3600) / 60); tf("Quedan {} h {} min", &[&(s / 3600), &mm]) }, None => t("Con batería").to_string() }
    });
    m.sep()
        .item(t("Ahorro de batería…"), A::Open("ms-settings:batterysaver".into()))
        .item(t("Energía y suspensión…"), A::Open("ms-settings:powersleep".into()));
    m
}

pub fn weather_menu(city: &str, temp: f32, desc: &str) -> Menu {
    let mut m = Menu::default();
    m.info(if city.is_empty() { t("Tiempo").to_string() } else { city.to_string() })
        .info(format!("{} °C · {desc}", temp.round() as i32))
        .sep()
        .item(t("Actualizar ahora"), A::Bar(crate::BAR_WEATHER_NOW))
        .item(t("Cambiar ciudad…"), A::EditConfig)
        .item(t("Panel de widgets\tWin+W"), k(&[VK_LWIN, vk(b'W')]));
    m
}


pub fn user_menu() -> Menu {
    let mut m = Menu::default();
    m.info(sys::display_name())
        .sep()
        .item(t("Tu cuenta…"), A::Open("ms-settings:yourinfo".into()))
        .item(t("Opciones de inicio de sesión…"), A::Open("ms-settings:signinoptions".into()))
        .sep()
        .item(t("Bloquear\tWin+L"), A::Lock)
        .item(t("Cerrar sesión…"), A::SignOut);
    m
}

pub fn bar_menu() -> Menu {
    let mut m = Menu::default();
    let on = sys::autostart_enabled();
    // Todas las opciones de abajo (y las de config.ini) en una ventana
    m.item(t("Configuración…"), A::Settings).sep();
    m.check(t("Iniciar con Windows"), on, A::Autostart(!on))
        .check(t("Widgets del escritorio"), crate::desk::enabled(), A::Bar(crate::BAR_WIDGETS));
    // Doble escritorio: cara de widgets o de iconos
    if crate::desk::enabled() {
        let away = crate::faces::AWAY.load(std::sync::atomic::Ordering::Relaxed);
        m.item(format!("      {}", if away { t("Ver los widgets (Win+Alt+W)") } else { t("Ver los iconos del escritorio (Win+Alt+W)") }), A::Bar(crate::BAR_FACE))
            .check(format!("      {}", t("Doble clic en el escritorio para cambiar")), crate::faces::dblclick_on(), A::Bar(crate::BAR_FACE_DBL));
    }
    // Cada widget por separado (solo con los widgets encendidos)
    if crate::desk::enabled() {
        for (i, (label, on)) in crate::desk::menu_items().into_iter().enumerate() {
            m.check(format!("      {label}"), on, A::Bar(crate::BAR_WIDGET0 + i as u32));
        }
        // Color del texto de los widgets: según el fondo, o fijo para fondos claros u oscuros
        m.sep();
        let mode = crate::desk::ink_mode();
        for (i, label) in ["Texto según el fondo", "Texto oscuro (fondos claros)", "Texto claro (fondos oscuros)"].into_iter().enumerate() {
            m.check(format!("      {}", t(label)), mode == i as i8, A::Bar(crate::BAR_INK0 + i as u32));
        }
    }
    // Perfiles: cada uno recuerda sus widgets y ajustes
    m.sep().info(t("Perfiles"));
    let cur = crate::profiles::active();
    for (i, name) in crate::profiles::NAMES.iter().enumerate() {
        let label = if crate::profiles::exists(i) || cur == Some(i) { t(name).to_string() } else { crate::i18n::tf("{} (guardar el actual)", &[&t(name)]) };
        m.check(format!("      {label}"), cur == Some(i), A::Bar(crate::BAR_PROFILE0 + i as u32));
    }
    m.item(format!("      {}", t("Abrir la carpeta de perfiles…")), A::Bar(crate::BAR_PROFILES_DIR));
    m.check(t("Modo presentación (sin avisos ni widgets)"), crate::profiles::presenting(), A::Bar(crate::BAR_PRESENT));
    m.check(t("Cafeína (no dejar que el equipo se duerma)"), crate::live::caffeine_on(), A::Bar(crate::BAR_CAFFEINE));
    m.check(t("Pomodoro (25 min de concentración)"), crate::live::POMO.load(std::sync::atomic::Ordering::Relaxed) != 0, A::Bar(crate::BAR_POMODORO));
    m.item(t("Cuentagotas de color"), A::Bar(crate::BAR_PICKER));
    m.check(t("Nota rápida (no sale al compartir pantalla)"), crate::notes::is_open(), A::Bar(crate::BAR_NOTE));
    // Tema: color de acento, y un archivo para compartirlo
    m.sep().info(t("Color de acento"));
    let ai = crate::profiles::accent_index();
    for (i, (name, _)) in crate::profiles::ACCENTS.iter().enumerate() {
        m.check(format!("      {}", t(name)), ai == Some(i), A::Bar(crate::BAR_ACCENT0 + i as u32));
    }
    m.item(format!("      {}", t("Exportar tema…")), A::Bar(crate::BAR_THEME_EXPORT))
        .item(format!("      {}", t("Importar tema…")), A::Bar(crate::BAR_THEME_IMPORT));
    // Tarjeta de la isla
    m.sep().info(t("Tarjeta de la isla"));
    m.check(format!("      {}", t("Abrir con un clic en la isla")), crate::card::opt(crate::card::OPT_CLICK), A::Bar(crate::BAR_CARD_CLICK))
        .check(format!("      {}", t("Abrir al dejar el ratón encima")), crate::card::opt(crate::card::OPT_HOVER), A::Bar(crate::BAR_CARD_HOVER))
        .check(format!("      {}", t("Vistazo con la isla vacía")), crate::card::opt(crate::card::OPT_GLANCE), A::Bar(crate::BAR_CARD_GLANCE));
    // Posición de la barra
    m.sep().info(t("Posición de la barra"));
    let b = crate::bar_bottom();
    m.check(format!("      {}", t("Arriba")), !b, A::Bar(crate::BAR_POS_TOP))
        .check(format!("      {}", t("Abajo")), b, A::Bar(crate::BAR_POS_BOTTOM))
        .check(format!("      {}", t("Barra sólida con ventanas maximizadas")), crate::solid_bar(), A::Bar(crate::BAR_SOLID));
    // Idioma de la interfaz
    m.sep().info(t("Idioma"));
    let lm = crate::i18n::mode();
    for (i, label) in ["Automático (el de Windows)", "Español", "English"].into_iter().enumerate() {
        m.check(format!("      {}", t(label)), lm == i as i8, A::Bar(crate::BAR_LANG0 + i as u32));
    }
    m.sep()
        .item(t("Editar configuración…"), A::EditConfig)
        .sep()
        .item(t("Salir de macbar"), A::Bar(crate::BAR_QUIT));
    m
}
