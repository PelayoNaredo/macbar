//! Menús desplegables estilo macOS sobre menús nativos de Windows en modo oscuro.
//! Mientras un menú está abierto, pasar el ratón por otro título de la barra salta a ese menú.

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
        let cmd = TrackPopupMenu(h, TPM_RETURNCMD | TPM_NONOTIFY | TPM_TOPALIGN | align, x, y, Some(0), owner, None);
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
        A::SignOut => { if sys::ask(owner, "¿Cerrar la sesión? Se cerrarán todas las apps.") { sys::sign_out(); } }
        A::Sleep => sys::sleep(),
        A::Restart => { if sys::ask(owner, "¿Reiniciar el equipo ahora?") { sys::restart(); } }
        A::Shutdown => { if sys::ask(owner, "¿Apagar el equipo ahora?") { sys::shutdown(); } }
        A::Autostart(on) => sys::set_autostart(on),
        A::EditConfig => sys::edit_config(),
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
    m.item("Acerca de este equipo", A::Open("ms-settings:about".into()))
        .sep()
        .item("Configuración…\tWin+I", A::Open("ms-settings:".into()))
        .item("Microsoft Store…", A::Open("ms-windows-store:".into()))
        .sep()
        .item("Elementos recientes", A::Open("shell:recent".into()))
        .sep()
        .item("Forzar salida…\tCtrl+Mayús+Esc", A::Open("taskmgr.exe".into()))
        .sep()
        .item("Suspender", A::Sleep)
        .item("Reiniciar…", A::Restart)
        .item("Apagar…", A::Shutdown)
        .sep()
        .item("Bloquear pantalla\tWin+L", A::Lock)
        .item(format!("Cerrar sesión de {}…", sys::display_name()), A::SignOut);
    m
}

pub fn app_menu(c: &Ctx) -> Menu {
    let mut m = Menu::default();
    if c.desktop {
        m.info("Escritorio").sep().item("Abrir el Explorador\tWin+E", k(&[VK_LWIN, vk(b'E')])).item("Mostrar todo", A::ShowAll);
        return m;
    }
    if let Some(exe) = &c.exe {
        m.item(format!("Mostrar {} en el Explorador", c.app), A::Reveal(exe.clone()));
        if !exe.to_lowercase().contains("\\windowsapps\\") {
            m.item(format!("Abrir otra ventana de {}", c.app), A::Relaunch(exe.clone()));
        }
        m.sep();
    }
    m.item(format!("Ocultar {}", c.app), A::HideApp(c.pid))
        .item("Ocultar las demás", A::HideOthers(c.pid))
        .item("Mostrar todo", A::ShowAll)
        .sep()
        .item(format!("Salir de {}", c.app), A::QuitApp(c.pid));
    m
}







/// Clic derecho en la isla: controles de reproducción.
pub fn island_menu(title: &str, artist: &str, playing: bool) -> Menu {
    let mut m = Menu::default();
    m.info(crate::trunc(title, 44));
    if !artist.is_empty() { m.info(crate::trunc(artist, 44)); }
    m.sep()
        .item("Anterior", A::Bar(crate::BAR_PREV))
        .item(if playing { "Pausa" } else { "Reproducir" }, A::Bar(crate::BAR_TOGGLE))
        .item("Siguiente", A::Bar(crate::BAR_NEXT))
        .sep()
        .item("Abrir el reproductor", A::Bar(crate::BAR_PLAYER));
    m
}

pub fn battery_menu(pct: u8, charging: bool, secs: Option<u32>) -> Menu {
    let mut m = Menu::default();
    m.info(format!("Batería: {pct} %"));
    m.info(if charging && pct >= 100 { "Conectado, carga completa".to_string() } else if charging { "Cargando".to_string() } else {
        match secs { Some(s) => format!("Quedan {} h {:02} min", s / 3600, (s % 3600) / 60), None => "Con batería".to_string() }
    });
    m.sep()
        .item("Ahorro de batería…", A::Open("ms-settings:batterysaver".into()))
        .item("Energía y suspensión…", A::Open("ms-settings:powersleep".into()));
    m
}

pub fn weather_menu(city: &str, temp: f32, desc: &str) -> Menu {
    let mut m = Menu::default();
    m.info(if city.is_empty() { "Tiempo".to_string() } else { city.to_string() })
        .info(format!("{} °C · {desc}", temp.round() as i32))
        .sep()
        .item("Actualizar ahora", A::Bar(crate::BAR_WEATHER_NOW))
        .item("Cambiar ciudad…", A::EditConfig)
        .item("Panel de widgets\tWin+W", k(&[VK_LWIN, vk(b'W')]));
    m
}


pub fn user_menu() -> Menu {
    let mut m = Menu::default();
    m.info(sys::display_name())
        .sep()
        .item("Tu cuenta…", A::Open("ms-settings:yourinfo".into()))
        .item("Opciones de inicio de sesión…", A::Open("ms-settings:signinoptions".into()))
        .sep()
        .item("Bloquear\tWin+L", A::Lock)
        .item("Cerrar sesión…", A::SignOut);
    m
}

pub fn bar_menu() -> Menu {
    let mut m = Menu::default();
    let on = sys::autostart_enabled();
    m.check("Iniciar con Windows", on, A::Autostart(!on))
        .item("Editar configuración…", A::EditConfig)
        .sep()
        .item("Salir de macbar", A::Bar(crate::BAR_QUIT));
    m
}
