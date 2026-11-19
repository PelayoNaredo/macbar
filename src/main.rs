#![windows_subsystem = "windows"]
//! macbar: barra superior estilo macOS para Windows 11.
//! Win32 + Direct2D, sin webview. Fondo transparente con "píldoras" flotantes.

use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as MediaManager,
    GlobalSystemMediaTransportControlsSessionMediaProperties as MediaProps,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlayStatus,
};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::*;
use windows::Win32::System::WinRT::{CreateStreamOverRandomAccessStream, RoInitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::{Matrix3x2, Vector2};

mod calendar;
mod ccenter;
mod desk;
mod deskdata;
mod deskw;
mod i18n;
mod i18n_tabla;
mod live;
mod menus;
mod phone;
mod picker;
mod popup;
mod search;
mod search_extra;
mod share;
mod profiles;
mod sys;
mod tray;
mod trayhost;
mod weather;
mod wifi;

use popup::ease_out;
use std::collections::HashMap;

/// Contenido de la isla central. Cada modo entra con un fundido y la isla cambia de ancho con un muelle.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode { Idle, Media, Timer, Shelf, Hud(live::Hud) }

impl Mode {
    fn key(&self) -> u8 {
        match self {
            Mode::Idle => 0, Mode::Media => 1, Mode::Timer => 2, Mode::Shelf => 15,
            Mode::Hud(live::Hud::Copied) => 16, Mode::Hud(live::Hud::Shelf) => 17,
            Mode::Hud(live::Hud::Volume(..)) => 3, Mode::Hud(live::Hud::Bright(_)) => 4,
            Mode::Hud(live::Hud::Power(..)) => 5, Mode::Hud(live::Hud::TimerDone) => 6,
            Mode::Hud(live::Hud::Net(_)) => 7, Mode::Hud(live::Hud::LowBat(_)) => 8,
            Mode::Hud(live::Hud::Caps(_)) => 9, Mode::Hud(live::Hud::Audio) => 10, Mode::Hud(live::Hud::Claude(_)) => 11,
            Mode::Hud(live::Hud::Download) => 12, Mode::Hud(live::Hud::Shot) => 13, Mode::Hud(live::Hud::Usb(_)) => 14,
            Mode::Hud(live::Hud::Mic(_)) => 18, Mode::Hud(live::Hud::Num(_)) => 19, Mode::Hud(live::Hud::Lang) => 20,
            Mode::Hud(live::Hud::Desk(..)) => 21, Mode::Hud(live::Hud::Mem(_)) => 22,
            Mode::Hud(live::Hud::Profile(_)) => 23, Mode::Hud(live::Hud::Present(_)) => 24,
            Mode::Hud(live::Hud::Phone) => 25, Mode::Hud(live::Hud::CamOn) => 26, Mode::Hud(live::Hud::MicOn) => 27,
            Mode::Hud(live::Hud::Full) => 28, Mode::Hud(live::Hud::Saver(_)) => 29, Mode::Hud(live::Hud::Wifi) => 30,
            Mode::Hud(live::Hud::Disk(_)) => 31,
            Mode::Hud(live::Hud::Pomo(..)) => 32, Mode::Hud(live::Hud::Caffeine(_)) => 33, Mode::Hud(live::Hud::Color(_)) => 34,
        }
    }
}

/// Curva con un pequeño rebote al final (para los "pop").
fn ease_back(t: f32) -> f32 { let t = t.clamp(0.0, 1.0) - 1.0; 1.0 + 2.70158 * t * t * t + 1.70158 * t * t }

/// Icono, texto y si lleva barra de cada aviso de la isla.
fn hud_parts(h: live::Hud) -> (&'static str, String, bool) {
    use live::Hud::*;
    match h {
        Volume(v, m) => (if m || v <= 0.001 { "\u{E74F}" } else if v < 0.34 { "\u{E993}" } else if v < 0.67 { "\u{E994}" } else { "\u{E995}" }, if m { i18n::t("Silencio").into() } else { i18n::t("Volumen").into() }, true),
        Bright(_) => ("\u{E706}", i18n::t("Brillo").into(), true),
        Power(_, ch) => (if ch { "\u{EA93}" } else { "\u{E83F}" }, if ch { i18n::t("Cargando").into() } else { i18n::t("Con batería").into() }, true),
        TimerDone => ("\u{EA8F}", i18n::t("Se acabó el tiempo").into(), false),
        Net(true) => ("\u{E701}", i18n::t("Conectado a internet").into(), false),
        Net(false) => ("\u{F384}", i18n::t("Sin conexión").into(), false),
        LowBat(_) => ("\u{E851}", i18n::t("Batería baja").into(), true),
        Caps(on) => ("\u{E765}", if on { i18n::t("Bloq Mayús activado").into() } else { i18n::t("Bloq Mayús desactivado").into() }, false),
        Audio => ("\u{E7F5}", trunc(&live::AUDIO_NAME.lock().unwrap(), 34), false),
        Claude(_) => ("", i18n::t("Uso semanal").into(), true),
        Download => ("\u{E896}", live::LAST_FILE.lock().unwrap().as_ref().map(|f| trunc(f.0.rsplit('\\').next().unwrap_or(""), 30)).unwrap_or_default(), false),
        Shot => ("\u{E722}", i18n::t("Captura guardada").into(), false),
        Copied => ("\u{E8C8}", i18n::tf("Copiado  {}", &[&*live::CLIP.lock().unwrap()]), false),
        Shelf => ("\u{E8B7}", { let n = live::SHELF.lock().unwrap().len(); if n == 1 { i18n::t("1 archivo en la bandeja").to_string() } else { i18n::tf("{} archivos en la bandeja", &[&n]) } }, false),
        Mic(muted) => (if muted { "\u{F781}" } else { "\u{E720}" }, if muted { i18n::t("Micrófono silenciado").into() } else { i18n::t("Micrófono activado").into() }, false),
        Num(on) => ("\u{E75F}", if on { i18n::t("Bloq Num activado").into() } else { i18n::t("Bloq Num desactivado").into() }, false),
        Lang => ("\u{E8D3}", live::LANG.lock().unwrap().clone(), false),
        Desk(..) => ("\u{E7C4}", trunc(&live::DESK_NAME.lock().unwrap(), 26), false),
        // Sin barra lineal: lleva un anillo radial pequeño a la derecha (ver Mode::Hud)
        Mem(_) => ("\u{E964}", i18n::t("Memoria casi llena").into(), false),
        Profile(i) => ("\u{E77B}", i18n::tf("Perfil «{}» guardado", &[&i18n::t(profiles::NAMES[(i as usize).min(2)])]), false),
        Phone => { let (app, title) = live::PHONE_NOTE.lock().unwrap().clone(); ("\u{E8EA}", trunc(&if app.is_empty() { title } else { format!("{app} · {title}") }, 38), false) }
        CamOn => ("\u{E722}", i18n::t("La cámara se está usando").into(), false),
        MicOn => ("\u{E720}", i18n::t("El micrófono se está usando").into(), false),
        Full => ("\u{EBB5}", i18n::t("Batería cargada").into(), true),
        Saver(on) => ("\u{EBB6}", if on { i18n::t("Ahorro de batería activado").into() } else { i18n::t("Ahorro de batería desactivado").into() }, false),
        Wifi => ("\u{E701}", trunc(&i18n::tf("Conectado a {}", &[&*live::WIFI_NAME.lock().unwrap()]), 34), false),
        Disk(_) => ("\u{EDA2}", i18n::t("Disco casi lleno").into(), true),
        Pomo(focus, m) => (if focus { "\u{E916}" } else { "\u{EC32}" }, if focus { i18n::tf("A concentrarse · {} min", &[&m]) } else { i18n::tf("Descanso · {} min", &[&m]) }, false),
        Caffeine(on) => ("\u{EC32}", if on { match live::caffeine_left() { Some(m) => i18n::tf("Cafeína: despierto {} min", &[&m]), None => i18n::t("Cafeína: el equipo no se dormirá").into() } } else { i18n::t("Cafeína desactivada").into() }, false),
        Color(c) => ("\u{E790}", i18n::tf("{} copiado", &[&format!("#{:06X}", c & 0xFF_FFFF)]), false),
        Present(on) => ("\u{E7F4}", if on { i18n::t("Modo presentación: sin avisos").into() } else { i18n::t("Modo presentación desactivado").into() }, false),
        Usb(on) => { let l = live::USB_LETTER.load(Ordering::Relaxed) as char; ("\u{E88E}", if on { i18n::tf("USB conectado · {}:", &[&l]) } else { i18n::t("USB expulsado").into() }, false) }
    }
}

/// ¿Está abierta la app de escritorio de Claude? (lo comprueba el hilo de uso)
static CLAUDE_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Acciones de menú que resuelve la propia barra.
pub const BAR_DESKTOP: u32 = 1;
pub const BAR_WEATHER_NOW: u32 = 2;
pub const BAR_QUIT: u32 = 3;
pub const BAR_PREV: u32 = 4;
pub const BAR_TOGGLE: u32 = 5;
pub const BAR_NEXT: u32 = 6;
pub const BAR_PLAYER: u32 = 7;
pub const BAR_WIDGETS: u32 = 8;
/// Del 20 al 25: poner o quitar cada widget del escritorio.
pub const BAR_WIDGET0: u32 = 20;
/// Modo del texto de los widgets (auto, oscuro, claro): 40..42
pub const BAR_INK0: u32 = 40;
/// Idioma (automático, español, inglés): 50..52
pub const BAR_LANG0: u32 = 50;
/// Color de acento: 60..67; tema: exportar 70, importar 71; perfiles 80..82, su carpeta 84; presentación 85
pub const BAR_ACCENT0: u32 = 60;
pub const BAR_THEME_EXPORT: u32 = 70;
pub const BAR_THEME_IMPORT: u32 = 71;
pub const BAR_PROFILE0: u32 = 80;
pub const BAR_PROFILES_DIR: u32 = 84;
pub const BAR_PRESENT: u32 = 85;
pub const BAR_CAFFEINE: u32 = 88;
pub const BAR_PICKER: u32 = 89;
pub const BAR_POMODORO: u32 = 90;
/// Posición: arriba 86, abajo 87
pub const BAR_POS_TOP: u32 = 86;
pub const BAR_POS_BOTTOM: u32 = 87;
static WEATHER_NOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Última ventana activa que no es la barra: destino de los menús y atajos.
static LAST_FG: AtomicIsize = AtomicIsize::new(0);

// ---------------------------------------------------------------- constantes
const BAR_H: f32 = 40.0; // alto en DIP (medido en la referencia a 1920 px)
const PILL_H: f32 = 30.0;
const WM_APPBAR: u32 = WM_USER + 1;
const WM_MEDIA: u32 = WM_USER + 2;
const WM_WEATHER: u32 = WM_USER + 3;
const TIMER_TICK: usize = 1;
const TIMER_ANIM: usize = 2;
const TIMER_KEEP: usize = 3;
const TIMER_FOCUS: usize = 4;
const TIMER_COLLAPSE: usize = 6;
/// Fotograma de animación (solo mientras algo se mueve)
const TIMER_FX: usize = 7;
/// Cuenta atrás del temporizador
const TIMER_COUNT: usize = 8;
/// Fin de un aviso de la isla
const TIMER_HUD: usize = 9;
/// Progreso de la canción (isla desplegada)
const TIMER_PROG: usize = 10;
/// Nueva petición del hueco tras volver de suspensión
const TIMER_RESERVE: usize = 11;
/// Vigilancia del hueco de la barra y de ventanas maximizadas que la tapen.
const TIMER_GUARD: usize = 12;
static FOCUS_TARGET: AtomicIsize = AtomicIsize::new(0);
static KEEP_LEFT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static FULLSCREEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// ¿Hay una app a pantalla completa? (juego, vídeo, F11). Entonces no se lee nada de otras apps ni de la pantalla.
pub fn fullscreen() -> bool { FULLSCREEN.load(Ordering::Relaxed) }
/// Fondo claro detrás de la barra: texto e iconos oscuros (lo decide `bar_ink_thread`).
static BAR_LIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Identidad de un lienzo para las cachés ligadas a él. Guarda una referencia: mientras la tenga, el lienzo
/// viejo no se libera y uno nuevo no puede caer en su misma dirección (comparar solo la dirección fallaba:
/// tras recrearse, pinceles e imágenes del viejo hacían fallar cada fotograma y la barra salía vacía).
#[derive(Default)]
pub struct RtKey(Option<windows::core::IUnknown>);
impl RtKey {
    /// ¿Es un lienzo distinto del último visto? (y lo recuerda)
    pub fn fresh(&mut self, rt: &impl windows::core::Interface) -> bool {
        let u = rt.cast::<windows::core::IUnknown>().ok();
        if self.0 == u { return false; }
        self.0 = u;
        true
    }
}
/// Deja en graficos.log por qué falló un fotograma (como mucho una línea por minuto).
fn log_draw_fail(where_: &str, e: &windows::core::Error) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut l = LAST.lock().unwrap();
    if l.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) { return; }
    *l = Some(Instant::now());
    use std::io::Write;
    let t = unsafe { GetLocalTime() };
    let line = format!("{}-{:02}-{:02} {:02}:{:02}:{:02} {}: {:?}\n", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, where_, e);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(sys::config_dir() + "\\graficos.log") { let _ = f.write_all(line.as_bytes()); }
}
thread_local! { static BAR_INK: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) }; }

/// Vigía: cada 2 s comprueba que la interfaz de macbar responde. Si tarda más de un segundo, apunta en
/// crash.log cuánto estuvo parada (para saber, si algo se congela, si fue macbar o no).
fn hang_watch() {
    std::thread::spawn(|| {
        let mut stuck_since: Option<Instant> = None;
        loop {
            std::thread::sleep(Duration::from_millis(2000));
            let bar = bar_hwnd();
            if bar.0.is_null() { continue; }
            let t0 = Instant::now();
            let mut r = 0usize;
            let ok = unsafe { SendMessageTimeoutW(bar, WM_NULL, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG | SMTO_BLOCK, 1000, Some(&mut r)) }.0 != 0;
            match (ok, stuck_since) {
                (false, None) => stuck_since = Some(t0),
                (true, Some(s)) => {
                    let secs = s.elapsed().as_secs_f32();
                    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
                    let line = format!("{}-{:02}-{:02} {:02}:{:02}:{:02} interfaz parada {:.1} s{}\n", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, secs, if fullscreen() { " (con una app a pantalla completa)" } else { "" });
                    use std::io::Write;
                    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(sys::config_dir() + "\\bloqueos.log") { let _ = f.write_all(line.as_bytes()); }
                    stuck_since = None;
                }
                _ => {}
            }
        }
    });
}

/// Mira cada 3 s el fondo justo debajo de la barra (solo donde se ve el escritorio, no ventanas) y
/// decide la tinta, con histéresis. Con el modo fijo del menú, la tinta es la elegida.
fn bar_ink_thread() {
    std::thread::spawn(|| loop {
        // A pantalla completa la barra no se ve: no se captura la pantalla (estorbaría al juego o al vídeo)
        if fullscreen() { std::thread::sleep(Duration::from_millis(1500)); continue; }
        let want = match desk::ink_mode() {
            1 => Some(true),
            2 => Some(false),
            _ => bar_backdrop_luma().map(|l| if BAR_LIGHT.load(Ordering::Relaxed) { l > 0.50 } else { l > 0.62 }),
        };
        if let Some(w) = want {
            if BAR_LIGHT.swap(w, Ordering::Relaxed) != w { unsafe { let _ = InvalidateRect(Some(bar_hwnd()), None, false); } }
        }
        std::thread::sleep(Duration::from_millis(if desk::ink_mode() == 0 { 3000 } else { 700 }));
    });
}

/// Luminosidad media de una franja bajo la barra, contando solo los puntos donde asoma el escritorio.
fn bar_backdrop_luma() -> Option<f32> {
    unsafe {
        let bar = bar_hwnd();
        if bar.0.is_null() { return None; }
        let s = scale_of(bar);
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let hh = (10.0 * s) as i32;
        // La franja de fondo junto a la barra: debajo si va arriba, encima si va abajo
        let y = if bar_bottom() { GetSystemMetrics(SM_CYSCREEN) - ((BAR_H + 4.0) * s) as i32 - hh } else { ((BAR_H + 4.0) * s) as i32 };
        // Al volver de suspensión la pantalla puede no tener aún tamaño ni escala: no se mide
        if sw < 96 || hh < 4 || s <= 0.0 { return None; }
        let sdc = GetDC(None);
        let mdc = CreateCompatibleDC(Some(sdc));
        let bi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: sw, biHeight: -hh, biPlanes: 1, biBitCount: 32, ..Default::default() }, ..Default::default() };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut out = None;
        if let Ok(dib) = CreateDIBSection(Some(mdc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) {
            let old = SelectObject(mdc, dib.into());
            if BitBlt(mdc, 0, 0, sw, hh, Some(sdc), 0, y, SRCCOPY).is_ok() && !bits.is_null() {
                let px = std::slice::from_raw_parts(bits as *const u8, (sw * hh * 4) as usize);
                let (mut sum, mut n) = (0.0f32, 0u32);
                for i in 0..48 {
                    if px.is_empty() { break; }
                    let x = sw * (2 * i + 1) / 96;
                    // ¿Es escritorio (fondo o un widget) lo que hay ahí, o una ventana?
                    let wnd = WindowFromPoint(POINT { x, y: y + hh / 2 });
                    let mut cls = [0u16; 32];
                    let n_ = GetClassNameW(wnd, &mut cls) as usize;
                    let c = String::from_utf16_lossy(&cls[..n_]);
                    if !["Progman", "WorkerW", "SHELLDLL_DefView", "SysListView32", "MacBarDesk"].contains(&c.as_str()) { continue; }
                    for yy in [1, hh / 2, hh - 2] {
                        let k = ((yy * sw + x) * 4) as usize;
                        if k + 2 >= px.len() { continue; }
                        sum += (0.2126 * px[k + 2] as f32 + 0.7152 * px[k + 1] as f32 + 0.0722 * px[k] as f32) / 255.0;
                        n += 1;
                    }
                }
                // Con menos de un tercio de escritorio a la vista no se cambia nada
                if n >= 48 { out = Some(sum / n as f32); }
            }
            SelectObject(mdc, old);
            let _ = DeleteObject(dib.into());
        }
        let _ = DeleteDC(mdc);
        ReleaseDC(None, sdc);
        out
    }
}
const WM_MOUSELEAVE: u32 = 0x02A3;
const ART_PX: u32 = 96;
const FONT: PCWSTR = w!("Segoe UI Variable Display");
const ICONS: PCWSTR = w!("Segoe Fluent Icons");

static BAR_HWND: AtomicIsize = AtomicIsize::new(0);
static MEDIA: Mutex<Option<MediaInfo>> = Mutex::new(None);
static ART: Mutex<Option<(u64, Arc<Vec<u8>>)>> = Mutex::new(None);
static WEATHER: Mutex<Option<Weather>> = Mutex::new(None);
static CMD_TX: OnceLock<Sender<Cmd>> = OnceLock::new();
/// Línea de tiempo de la canción: (posición, duración, cuándo se midió), en unidades de 100 ns.
static TIMELINE: Mutex<Option<(i64, i64, i64)>> = Mutex::new(None);

/// Progreso 0..1 de la canción, avanzando solo mientras suena.
fn media_progress(playing: bool) -> Option<f32> {
    let (pos, end, upd) = (*TIMELINE.lock().unwrap())?;
    if end <= 0 { return None; }
    let now = unsafe { let f = windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime(); ((f.dwHighDateTime as i64) << 32) | f.dwLowDateTime as i64 };
    let p = if playing && upd > 0 { pos + (now - upd).max(0) } else { pos };
    Some((p as f64 / end as f64).clamp(0.0, 1.0) as f32)
}

#[derive(Clone, PartialEq)]
struct MediaInfo { title: String, artist: String, playing: bool, art_ver: u64, app: String, can_prev: bool, can_next: bool, album: String }

#[derive(Clone, Copy, PartialEq)]
struct Weather { temp: f32, is_day: bool, code: i32 }

#[derive(Clone, Copy)]
enum Cmd { Toggle, Next, Prev, Seek(i64) }

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Act {
    // Abren un menú nativo
    AppMenu, Battery, User,
    // Abren un panel propio
    Logo, Weather, Tray, Wifi, Clock,
    // Acción directa
    Desktop, OpenPlayer, OpenYtm, Prev, Toggle, Next, Island, Claude, Search, TimerStop, HudDismiss, TimerOpen, ShelfOpen(u8), ShelfClear, ShelfShare, Caffeine,
}

impl Act {
    fn is_menu(self) -> bool { matches!(self, Act::AppMenu | Act::Battery | Act::User) }
    fn is_panel(self) -> bool { matches!(self, Act::Logo | Act::Weather | Act::Tray | Act::Wifi | Act::Clock) }
    /// Menús y paneles se abren al pulsar (como en macOS); el resto, al soltar.
    fn on_press(self) -> bool { self.is_menu() || self.is_panel() }
    /// Los menús de la derecha se alinean por su borde derecho.
    fn right_side(self) -> bool { matches!(self, Act::Battery | Act::User) }
    /// Botones dentro de la isla: su resaltado es un círculo pequeño, no una píldora de la barra.
    fn in_island(self) -> bool { matches!(self, Act::Island | Act::OpenPlayer | Act::OpenYtm | Act::HudDismiss | Act::TimerOpen) }
}

// ---------------------------------------------------------------- estado de dibujo
struct Fonts { app: IDWriteTextFormat, small: IDWriteTextFormat, mid: IDWriteTextFormat, bold: IDWriteTextFormat, title: IDWriteTextFormat, icon: IDWriteTextFormat, icon_s: IDWriteTextFormat }

struct Gfx {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: Option<ID2D1HwndRenderTarget>,
    f: Fonts,
    app_name: String,
    user: String,
    hits: Vec<(D2D_RECT_F, Act)>,
    hover: Option<Act>,
    /// Menú desplegado ahora mismo (su título se queda resaltado).
    open: Option<Act>,
    avatar_px: Option<Vec<u8>>,
    avatar_path: Option<String>,
    /// Uso semanal de Claude (porcentaje, reinicio), leído de la línea de estado.
    usage: Option<(f32, u64)>,
    /// Trazado del logotipo de Claude (se crea una vez).
    claude_geo: Option<ID2D1PathGeometry>,
    // Isla reactiva: ancho actual (animado) y objetivo, si el ratón está encima y hasta cuándo se
    // queda desplegada tras un cambio de canción
    isl_w: f32,
    isl_target: f32,
    isl_hover: bool,
    isl_rect: D2D_RECT_F,
    expand_until: Option<Instant>,
    track_key: String,
    avatar: Option<ID2D1Bitmap>,
    art: Option<(u64, ID2D1Bitmap)>,
    meter: Option<IAudioMeterInformation>,
    eq: [f32; 5],
    eq_rect: Option<D2D_RECT_F>,
    wcache: RefCell<Caches>,
    t0: Instant,
    // ---- animación
    last_frame: Instant,
    /// Intensidad actual del resaltado de cada botón (se acerca a su objetivo con suavidad)
    hl: HashMap<Act, f32>,
    pressed: Option<Act>,
    /// Cuándo apareció cada elemento opcional (tiempo, uso de Claude…): entra con fundido
    born: HashMap<&'static str, Instant>,
    app_prev: String,
    app_at: Instant,
    isl_mode: Mode,
    isl_prev: Option<Mode>,
    isl_mode_at: Instant,
    last_hud: Option<live::Hud>,
    hud_val: f32,
    count_on: bool,
    eq_visible: bool,
    last_ac: Option<bool>,
    isl_v: f32,
    tray_rot: f32,
    shelf_bmps: (u64, Vec<Option<ID2D1Bitmap>>),
    hud_bmp: Option<(u64, ID2D1Bitmap)>,
    /// Ya se avisó de batería llena / disco lleno; último estado del ahorro de batería
    full_warned: bool, disk_warned: bool, saver: Option<bool>,
    usage_disp: f32,
    low_warned: u8,
    art_tint: [f32; 3],
    tint_now: [f32; 3],
    art_at: Instant,
    desk_x: f32,
    round_cap: Option<ID2D1StrokeStyle>,
    glide: (D2D_RECT_F, f32),
    clock_roll: Roll,
    timer_roll: std::cell::RefCell<Roll>,
    temp_roll: Roll,
    morph: HashMap<Act, Morph>,
    play_state: (bool, Instant),
}

/// Texto que "rueda" al cambiar (reloj, temperatura): lo que cambia sube y se va, lo nuevo entra desde abajo.
#[derive(Default)]
struct Roll { prev: String, cur: String, at: Option<Instant> }

impl Roll {
    /// Dibuja `text` en x. Devuelve true mientras anima.
    #[allow(clippy::too_many_arguments)]
    fn draw(&mut self, p: &Painter, rt: &ID2D1RenderTarget, base: &Matrix3x2, f: &IDWriteTextFormat, text: &str, x: f32, top: f32, bottom: f32, b: &ID2D1Brush, sh: Option<&ID2D1Brush>) -> bool {
        if self.cur.is_empty() { self.cur = text.to_string(); self.prev = text.to_string(); }
        if self.cur != text { self.prev = std::mem::replace(&mut self.cur, text.to_string()); self.at = Some(Instant::now()); }
        let roll = self.at.map(|t| ease_out(t.elapsed().as_secs_f32() / 0.42)).unwrap_or(1.0);
        if roll >= 1.0 || self.prev == text { p.text(f, text, x, b, sh); return false; }
        // Parte común del principio: fija; el resto rueda
        let common = text.char_indices().zip(self.prev.chars()).take_while(|((_, a), c)| a == c).last().map(|((i, c), _)| i + c.len_utf8()).unwrap_or(0);
        let (head, new_tail) = text.split_at(common);
        let old_tail = self.prev.get(common..).unwrap_or("");
        let hx = x + p.width(f, head);
        unsafe {
            if !head.is_empty() { p.text(f, head, x, b, sh); }
            rt.PushAxisAlignedClip(&rect(hx - 1.0, top - 2.0, hx + p.width(f, new_tail).max(p.width(f, old_tail)) + 2.0, bottom + 2.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let lift = 14.0;
            rt.SetTransform(&Matrix3x2 { M32: base.M32 - lift * roll, ..*base });
            popup::push_alpha(rt, 1.0 - roll);
            p.text(f, old_tail, hx, b, None);
            rt.PopLayer();
            rt.SetTransform(&Matrix3x2 { M32: base.M32 + lift * (1.0 - roll), ..*base });
            popup::push_alpha(rt, roll);
            p.text(f, new_tail, hx, b, None);
            rt.PopLayer();
            rt.SetTransform(base);
            rt.PopAxisAlignedClip();
        }
        true
    }
}

/// Glifo de icono que se funde con el anterior cuando cambia (batería que sube, señal Wi-Fi…).
struct Morph { prev: String, cur: String, at: Instant }

/// Color dominante de una carátula (BGRA premultiplicado), avivado para que luzca sobre negro.
/// Si es casi gris, blanco.
fn art_tint(px: &[u8]) -> [f32; 3] {
    let (mut r, mut g, mut b, mut wsum) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for c in px.chunks_exact(4).step_by(3) {
        let (bb, gg, rr) = (c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0);
        let mx = rr.max(gg).max(bb);
        let mn = rr.min(gg).min(bb);
        // Pesan más los píxeles saturados y no demasiado oscuros
        let w = (mx - mn) * mx + 0.01;
        r += rr * w; g += gg * w; b += bb * w; wsum += w;
    }
    if wsum <= 0.0 { return [1.0; 3]; }
    let (r, g, b) = (r / wsum, g / wsum, b / wsum);
    let mx = r.max(g).max(b).max(0.001);
    let sat = (mx - r.min(g).min(b)) / mx;
    if sat < 0.18 { return [1.0; 3]; }
    // Brillo al máximo y un toque de blanco para que no quede chillón
    let (r, g, b) = (r / mx, g / mx, b / mx);
    [0.8 * r + 0.2, 0.8 * g + 0.2, 0.8 * b + 0.2]
}

thread_local! { static GFX: RefCell<Option<Gfx>> = const { RefCell::new(None) }; }

fn bar_hwnd() -> HWND { HWND(BAR_HWND.load(Ordering::Relaxed) as *mut _) }
fn wide(s: &str) -> Vec<u16> { s.encode_utf16().collect() }
fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F { D2D1_COLOR_F { r, g, b, a } }
fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F { D2D_RECT_F { left: l, top: t, right: r, bottom: b } }
fn rrect(r: D2D_RECT_F, rad: f32) -> D2D1_ROUNDED_RECT { D2D1_ROUNDED_RECT { rect: r, radiusX: rad, radiusY: rad } }
fn scale_of(hwnd: HWND) -> f32 { unsafe { GetDpiForWindow(hwnd) as f32 / 96.0 } }
fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max - 1).collect::<String>().trim_end().to_string() + "…" }
}

fn make_format(dw: &IDWriteFactory, family: PCWSTR, size: f32, weight: DWRITE_FONT_WEIGHT) -> IDWriteTextFormat {
    unsafe {
        let f = dw.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("es-ES")).expect("formato");
        let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        f
    }
}

// ---------------------------------------------------------------- posición (arriba o abajo)
/// ¿La barra va abajo? (`posicion = abajo` en config.ini; se lee una vez al arrancar)
pub fn bar_bottom() -> bool {
    static B: OnceLock<bool> = OnceLock::new();
    *B.get_or_init(|| sys::config_value("posicion").is_some_and(|v| v.eq_ignore_ascii_case("abajo")))
}

/// Dónde se abre un panel de alto `hpx`: bajo la barra si va arriba, encima si va abajo.
pub fn panel_y(hpx: i32, s: f32) -> i32 {
    let gap = ((BAR_H + 4.0) * s) as i32;
    if bar_bottom() { (unsafe { GetSystemMetrics(SM_CYSCREEN) }) - gap - hpx } else { gap }
}

/// Con la barra abajo, la barra de tareas de Windows (en ocultación automática) saldría encima al tocar
/// el borde: se esconde mientras macbar está abajo y vuelve al cerrar macbar o al subirla.
pub fn windows_taskbar(show: bool) {
    if let Some(h) = trayhost::explorer_tray() {
        unsafe { let _ = ShowWindow(h, if show { SW_SHOWNOACTIVATE } else { SW_HIDE }); }
    }
}

/// Cambia la posición (menú): se guarda y macbar se reinicia para colocarlo todo de nuevo.
pub fn set_bar_bottom(bottom: bool) {
    if bottom == bar_bottom() { return; }
    sys::set_config_value("posicion", if bottom { "abajo" } else { "arriba" });
    if !bottom { windows_taskbar(true); }
    profiles::relaunch();
}

// ---------------------------------------------------------------- AppBar
/// Todas las peticiones del hueco de la barra van DIRECTAS al Explorador (ver `trayhost::direct`).
fn appbar(msg: u32, abd: &mut APPBARDATA) -> usize {
    trayhost::direct(|| unsafe { SHAppBarMessage(msg, abd) })
}

fn position_appbar(hwnd: HWND) {
    unsafe {
        let h = (BAR_H * scale_of(hwnd)).round() as i32;
        let bottom = bar_bottom();
        let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: hwnd, uEdge: if bottom { ABE_BOTTOM } else { ABE_TOP }, ..Default::default() };
        abd.rc = RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) };
        appbar(ABM_QUERYPOS, &mut abd);
        if bottom { abd.rc.top = abd.rc.bottom - h; } else { abd.rc.bottom = abd.rc.top + h; }
        appbar(ABM_SETPOS, &mut abd);
        if bottom { abd.rc.top = abd.rc.bottom - h; windows_taskbar(false); }
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), abd.rc.left, abd.rc.top, abd.rc.right - abd.rc.left, h, SWP_NOACTIVATE);
        GFX.with(|g| {
            if let Some(g) = g.borrow_mut().as_mut() {
                g.target = None; // se recrea con el tamaño y DPI nuevos
                g.art = None; g.avatar = None;
            }
        });
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// Vuelve a pedir a Windows el hueco de la barra (tras suspender, reiniciarse el Explorador o
/// cambiar de pantalla Windows puede olvidarlo y las ventanas maximizadas la tapan).
fn reserve(hwnd: HWND) {
    unsafe {
        let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: hwnd, uCallbackMessage: WM_APPBAR, ..Default::default() };
        appbar(ABM_REMOVE, &mut abd);
        appbar(ABM_NEW, &mut abd);
    }
    position_appbar(hwnd);
    fix_maximized(hwnd);
}

/// Ventanas maximizadas que se quedaron tapando la barra (Windows recupera el hueco pero no siempre
/// recoloca las que ya estaban maximizadas): se encajan de nuevo en el área de trabajo, sin desmaximizar.
/// Devuelve cuántas se arreglaron.
fn fix_maximized(bar: HWND) -> usize {
    if FULLSCREEN.load(Ordering::Relaxed) { return 0; }
    let mut fixed = 0;
    unsafe {
        let mut wa = RECT::default();
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut wa as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).is_err() { return 0; }
        let h_bar = (BAR_H * scale_of(bar)).round() as i32;
        let bottom = bar_bottom();
        let sh = GetSystemMetrics(SM_CYSCREEN);
        // El hueco aún no está: primero hay que reservarlo
        if if bottom { wa.bottom > sh - h_bar + 1 } else { wa.top < h_bar - 1 } { return 0; }
        for h in sys::app_windows() {
            if h == bar || !IsZoomed(h).as_bool() || !IsWindowVisible(h).as_bool() { continue; }
            let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(mon, &mut mi).as_bool() || mi.dwFlags & 1 /* MONITORINFOF_PRIMARY */ == 0 { continue; }
            let mut r = RECT::default();
            if GetWindowRect(h, &mut r).is_err() { continue; }
            // Una maximizada sobresale por su marco (unos 8 px) a cada lado: se mide en el borde izquierdo
            let frame = (wa.left - r.left).clamp(0, 16);
            let want_top = wa.top - frame;
            let want_bottom = wa.bottom + frame;
            if (!bottom && r.top < want_top - 2) || (bottom && r.bottom > want_bottom + 2) {
                let _ = SetWindowPos(h, None, r.left, want_top, r.right - r.left, want_bottom - want_top, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
                fixed += 1;
            }
        }
    }
    fixed
}

/// Vigilancia rápida (cada 2 s, muy barata): hueco perdido o ventanas maximizadas encima de la barra.
fn guard(hwnd: HWND) {
    // El estado de pantalla completa se revisa siempre: la pantalla de bloqueo al volver de suspender
    // cuenta como pantalla completa y Windows no siempre avisa al quitarla (la barra se quedaba "oculta"
    // y esta vigilancia se saltaba para siempre).
    sync_fullscreen(hwnd);
    if FULLSCREEN.load(Ordering::Relaxed) { return; }
    // El Explorador puede volver a mostrar su barra (al reiniciarse, al cambiar de pantalla…)
    if bar_bottom() && trayhost::explorer_tray().is_some_and(|h| unsafe { IsWindowVisible(h).as_bool() }) { windows_taskbar(false); }
    if !reserved_ok(hwnd) { reserve(hwnd); } else { fix_maximized(hwnd); }
}

/// ¿Sigue reservado el hueco? (el área de trabajo del monitor principal debe empezar bajo la barra)
fn reserved_ok(hwnd: HWND) -> bool {
    if FULLSCREEN.load(Ordering::Relaxed) { return true; }
    unsafe {
        let mut wa = RECT::default();
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut wa as *mut _ as _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).is_err() { return true; }
        let h = (BAR_H * scale_of(hwnd)).round() as i32;
        if bar_bottom() { wa.bottom <= GetSystemMetrics(SM_CYSCREEN) - h + 1 } else { wa.top >= h - 1 }
    }
}

// ---------------------------------------------------------------- datos de widgets
fn clock_text() -> String {
    let t = unsafe { GetLocalTime() };
    // "Mié, 18 nov  14:05" / "Wed, Nov 18  14:05"
    format!("{}, {}  {}:{:02}", i18n::weekday_short(t.wDayOfWeek as usize), i18n::day_month_short(t.wDay as u32, (t.wMonth as usize + 11) % 12), t.wHour, t.wMinute)
}

/// (porcentaje, cargando). None si el equipo no tiene batería.
fn battery() -> Option<(u8, bool)> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).ok()?; }
    if s.BatteryFlag & 128 != 0 || s.BatteryLifePercent > 100 { return None; }
    Some((s.BatteryLifePercent, s.ACLineStatus == 1))
}

fn last_fg() -> HWND { HWND(LAST_FG.load(Ordering::Relaxed) as *mut _) }

/// Nombre bonito de la app de una ventana. "Escritorio" si es el fondo.
fn app_name_of(fg: HWND) -> Option<String> {
    if fg.0.is_null() { return None; }
    if sys::is_shell_window(fg) { return Some(i18n::t("Escritorio").into()); }
    let path = sys::process_path(sys::real_pid(fg))?;
    let file = path.rsplit('\\').next()?;
    let stem = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file);
    Some(match stem.to_lowercase().as_str() {
        "explorer" => i18n::t("Explorador").to_string(),
        "msedge" => "Edge".to_string(),
        "windowsterminal" => "Terminal".to_string(),
        "applicationframehost" => return None,
        _ => { let mut c = stem.chars(); c.next()?.to_uppercase().chain(c).collect() }
    })
}

fn weather_desc(code: i32) -> &'static str {
    i18n::t(match code {
        0 => "Despejado", 1 => "Casi despejado", 2 => "Parcialmente nuboso", 3 => "Nublado",
        45 | 48 => "Niebla", 51..=57 => "Llovizna", 61..=67 => "Lluvia", 71..=77 => "Nieve",
        80..=82 => "Chubascos", 85 | 86 => "Chubascos de nieve", 95..=99 => "Tormenta", _ => "—",
    })
}

fn load_file_square(path: &str) -> Option<Vec<u8>> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let dec = wic.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let frame: IWICBitmapSource = dec.GetFrame(0).ok()?.into();
        wic_square(&wic, &frame)
    }
}

// ---------------------------------------------------------------- imágenes (WIC)
fn wic_square(wic: &IWICImagingFactory, src: &IWICBitmapSource) -> Option<Vec<u8>> { wic_square_n(wic, src, ART_PX) }

/// Recorte cuadrado central escalado a `px` × `px` (BGRA premultiplicado).
fn wic_square_n(wic: &IWICImagingFactory, src: &IWICBitmapSource, px: u32) -> Option<Vec<u8>> {
    unsafe {
        let (mut w, mut h) = (0u32, 0u32);
        src.GetSize(&mut w, &mut h).ok()?;
        let side = w.min(h);
        let clip = wic.CreateBitmapClipper().ok()?;
        let r = WICRect { X: ((w - side) / 2) as i32, Y: ((h - side) / 2) as i32, Width: side as i32, Height: side as i32 };
        clip.Initialize(src, &r).ok()?;
        let sc = wic.CreateBitmapScaler().ok()?;
        sc.Initialize(&clip, px, px, WICBitmapInterpolationModeHighQualityCubic).ok()?;
        let cv = wic.CreateFormatConverter().ok()?;
        cv.Initialize(&sc, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let mut buf = vec![0u8; (px * px * 4) as usize];
        cv.CopyPixels(std::ptr::null(), px * 4, &mut buf).ok()?;
        Some(buf)
    }
}

/// Carátula a tamaño de la isla y, además, a tamaño grande para el widget de música.
fn load_art(props: &MediaProps) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
    let s = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    unsafe {
        let stream: IStream = CreateStreamOverRandomAccessStream(&s).ok()?;
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let dec = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let frame: IWICBitmapSource = dec.GetFrame(0).ok()?.into();
        Some((wic_square(&wic, &frame)?, wic_square_n(&wic, &frame, ART_BIG_PX)))
    }
}

/// Tamaño de la carátula grande (widget de música).
const ART_BIG_PX: u32 = 256;
static ART_BIG: Mutex<Option<(u64, Arc<Vec<u8>>)>> = Mutex::new(None);

/// Posición y duración de la canción en segundos (avanzando solo mientras suena).
fn media_secs(playing: bool) -> Option<(f32, f32)> {
    let (pos, end, upd) = (*TIMELINE.lock().unwrap())?;
    if end <= 0 { return None; }
    let now = unsafe { let f = windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime(); ((f.dwHighDateTime as i64) << 32) | f.dwLowDateTime as i64 };
    let p = if playing && upd > 0 { pos + (now - upd).max(0) } else { pos };
    Some(((p.min(end)) as f32 / 1e7, end as f32 / 1e7))
}

// ---------------------------------------------------------------- hilo multimedia
/// Sesión de música a seguir. YouTube Music suena en el navegador, así que manda la del navegador:
/// primero una que esté sonando, luego cualquiera del navegador y, si no hay, la sesión actual de Windows.
fn pick_session(mgr: &MediaManager) -> Option<windows::Media::Control::GlobalSystemMediaTransportControlsSession> {
    const BROWSERS: [&str; 7] = ["brave", "chrome", "msedge", "firefox", "opera", "vivaldi", "youtube"];
    let all = mgr.GetSessions().ok()?;
    let mut paused_browser = None;
    for i in 0..all.Size().unwrap_or(0) {
        let Ok(s) = all.GetAt(i) else { continue };
        let app = s.SourceAppUserModelId().map(|h| h.to_string().to_lowercase()).unwrap_or_default();
        if !BROWSERS.iter().any(|b| app.contains(b)) { continue; }
        let playing = s.GetPlaybackInfo().and_then(|p| p.PlaybackStatus()).map(|st| st == PlayStatus::Playing).unwrap_or(false);
        if playing { return Some(s); }
        if paused_browser.is_none() { paused_browser = Some(s); }
    }
    paused_browser.or_else(|| mgr.GetCurrentSession().ok())
}

fn media_thread(rx: Receiver<Cmd>) {
    unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
    let Some(mgr) = MediaManager::RequestAsync().ok().and_then(|o| o.join().ok()) else { return };
    let mut key = String::new();
    let mut ver = 0u64;
    let mut art_tries = 0;
    let mut prev_art: Option<Arc<Vec<u8>>> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(1500)) {
            Ok(cmd) => {
                if let Some(s) = pick_session(&mgr) {
                    let _ = match cmd {
                        Cmd::Toggle => s.TryTogglePlayPauseAsync().and_then(|o| o.join()),
                        Cmd::Next => s.TrySkipNextAsync().and_then(|o| o.join()),
                        Cmd::Prev => s.TrySkipPreviousAsync().and_then(|o| o.join()),
                        // Ir a un punto de la canción (en unidades de 100 ns), si el reproductor lo admite
                        Cmd::Seek(t) => s.TryChangePlaybackPositionAsync(t).and_then(|o| o.join()),
                    };
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if std::env::var_os("MACBAR_DEMO").is_some() {
            // Modo demo para revisar el diseño sin reproducir nada
            // Carátula inventada: un degradado cálido con un círculo, para que se vea el color de la isla
            if ART.lock().unwrap().is_none() {
                let art = |n: u32| -> Vec<u8> {
                    let mut v = vec![0u8; (n * n * 4) as usize];
                    for y in 0..n { for x in 0..n {
                        let (fx, fy) = (x as f32 / n as f32, y as f32 / n as f32);
                        let d = ((fx - 0.62).powi(2) + (fy - 0.4).powi(2)).sqrt();
                        let sun = (1.0 - (d / 0.22)).clamp(0.0, 1.0).powf(0.6);
                        let r = (0.95 - 0.35 * fy + 0.05 * sun).min(1.0);
                        let g = (0.45 - 0.25 * fy + 0.45 * sun).min(1.0);
                        let b = (0.35 + 0.35 * fy + 0.2 * sun).min(1.0);
                        let i = ((y * n + x) * 4) as usize;
                        v[i] = (b * 255.0) as u8; v[i + 1] = (g * 255.0) as u8; v[i + 2] = (r * 255.0) as u8; v[i + 3] = 255;
                    } }
                    v
                };
                *ART.lock().unwrap() = Some((1, Arc::new(art(ART_PX))));
                *ART_BIG.lock().unwrap() = Some((1, Arc::new(art(ART_BIG_PX))));
            }
            *MEDIA.lock().unwrap() = Some(MediaInfo { title: "Tumblr Girls (feat. Christoph Andersson)".into(), artist: "G-Eazy".into(), playing: true, art_ver: 1, app: "Spotify.exe".into(), can_prev: true, can_next: true, album: "These Things Happen".into() });
            *WEATHER.lock().unwrap() = Some(Weather { temp: 18.0, is_day: false, code: 0 });
            unsafe { let _ = PostMessageW(Some(bar_hwnd()), WM_MEDIA, WPARAM(0), LPARAM(0)); }
            continue;
        }
        let snap = (|| {
            let s = pick_session(&mgr)?;
            let p = s.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
            let title = p.Title().ok()?.to_string();
            if title.is_empty() { return None; }
            let artist = p.Artist().ok()?.to_string();
            let album = p.AlbumTitle().map(|a| a.to_string()).unwrap_or_default();
            // Posición de la canción (para la barra de progreso de la isla)
            if let Ok(tl) = s.GetTimelineProperties() {
                let pos = tl.Position().map(|d| d.Duration).unwrap_or(0);
                let end = tl.EndTime().map(|d| d.Duration).unwrap_or(0);
                let upd = tl.LastUpdatedTime().map(|d| d.UniversalTime).unwrap_or(0);
                *TIMELINE.lock().unwrap() = Some((pos, end, upd));
            }
            let pi = s.GetPlaybackInfo().ok()?;
            let playing = pi.PlaybackStatus().ok()? == PlayStatus::Playing;
            let ctl = pi.Controls().ok();
            let can_prev = ctl.as_ref().and_then(|c| c.IsPreviousEnabled().ok()).unwrap_or(true);
            let can_next = ctl.as_ref().and_then(|c| c.IsNextEnabled().ok()).unwrap_or(true);
            let app = s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
            Some((p, title, artist, playing, app, can_prev, can_next, album))
        })();
        let info = match snap {
            Some((p, title, artist, playing, app, can_prev, can_next, album)) => {
                let k = format!("{title}\u{1}{artist}");
                if k != key {
                    key = k; ver += 1; art_tries = 0;
                    prev_art = ART.lock().unwrap().take().map(|a| a.1);
                    *ART_BIG.lock().unwrap() = None;
                }
                // La carátula a veces llega después del título, y al principio Windows aún entrega la de la
                // canción anterior: se vuelve a leer durante unos segundos y, si cambia, se sustituye
                if art_tries < 8 {
                    art_tries += 1;
                    if let Some((b, big)) = load_art(&p) {
                        let same = ART.lock().unwrap().as_ref().is_some_and(|a| *a.1 == b);
                        // La de la canción anterior no vale (salvo que tras varios intentos siga siendo esa)
                        let stale = art_tries < 5 && prev_art.as_ref().is_some_and(|pa| **pa == b);
                        if !same && !stale {
                            if ART.lock().unwrap().is_some() { ver += 1; }
                            *ART.lock().unwrap() = Some((ver, Arc::new(b)));
                            if let Some(big) = big { *ART_BIG.lock().unwrap() = Some((ver, Arc::new(big))); }
                        }
                    }
                }
                let v = ART.lock().unwrap().as_ref().map(|a| a.0).unwrap_or(0);
                Some(MediaInfo { title, artist, playing, art_ver: v, app, can_prev, can_next, album })
            }
            None => { key.clear(); None }
        };
        let changed = { let mut m = MEDIA.lock().unwrap(); let c = *m != info; *m = info; c };
        if changed { unsafe { let _ = PostMessageW(Some(bar_hwnd()), WM_MEDIA, WPARAM(0), LPARAM(0)); } }
    }
}

// ---------------------------------------------------------------- hilo del tiempo (Open-Meteo, sin clave)
/// %APPDATA%\macbar\config.ini admite `ciudad = Madrid` o `lat = ...` + `lon = ...`.
fn config_location() -> Option<(f64, f64)> {
    if sys::demo() { return Some((40.42, -3.70)); } // Madrid
    let path = std::env::var("APPDATA").ok()? + "\\macbar\\config.ini";
    let txt = std::fs::read_to_string(path).ok()?;
    let val = |k: &str| txt.lines().find_map(|l| Some(l.trim().strip_prefix(k)?.trim().strip_prefix('=')?.trim().to_string()));
    if let (Some(a), Some(b)) = (val("lat").and_then(|v| v.parse().ok()), val("lon").and_then(|v| v.parse().ok())) {
        return Some((a, b));
    }
    geocode(&val("ciudad")?)
}

/// GET HTTPS mínimo con WinHTTP (incluido en Windows, sin dependencias).
fn http_get(url: String) -> Option<String> {
    use windows::Win32::Networking::WinHttp::*;
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_at(rest.find('/')?);
    unsafe {
        let s = WinHttpOpen(w!("macbar/0.1"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if s.is_null() { return None; }
        let c = WinHttpConnect(s, &HSTRING::from(host), INTERNET_DEFAULT_HTTPS_PORT, 0);
        let r = if c.is_null() { std::ptr::null_mut() } else {
            WinHttpOpenRequest(c, w!("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE)
        };
        let mut out = Vec::new();
        let ok = !r.is_null()
            && WinHttpSendRequest(r, None, None, 0, 0, 0).is_ok()
            && WinHttpReceiveResponse(r, std::ptr::null_mut()).is_ok();
        if ok {
            loop {
                let mut avail = 0u32;
                if WinHttpQueryDataAvailable(r, &mut avail).is_err() || avail == 0 { break; }
                let mut buf = vec![0u8; avail as usize];
                let mut read = 0u32;
                if WinHttpReadData(r, buf.as_mut_ptr() as _, avail, &mut read).is_err() { break; }
                out.extend_from_slice(&buf[..read as usize]);
            }
        }
        for h in [r, c, s] { if !h.is_null() { let _ = WinHttpCloseHandle(h); } }
        if ok { String::from_utf8(out).ok() } else { None }
    }
}

fn geocode(city: &str) -> Option<(f64, f64)> {
    let q: String = city.chars().map(|c| if c == ' ' { '+' } else { c }).collect();
    let body = http_get(format!("https://geocoding-api.open-meteo.com/v1/search?name={q}&count=1&language=es"))?;
    let r = &body[body.find("\"results\":")?..];
    Some((json_num(r, "latitude")?, json_num(r, "longitude")?))
}

fn system_location() -> Option<(f64, f64)> {
    use windows::Devices::Geolocation::Geolocator;
    let g = Geolocator::new().ok()?;
    let pos = g.GetGeopositionAsync().ok()?.join().ok()?;
    let p = pos.Coordinate().ok()?.Point().ok()?.Position().ok()?;
    Some((p.Latitude, p.Longitude))
}

fn json_num(s: &str, key: &str) -> Option<f64> {
    let k = format!("\"{key}\":");
    let rest = &s[s.find(&k)? + k.len()..];
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn fetch_weather(lat: f64, lon: f64) -> Option<Weather> {
    // Coordenadas redondeadas a ~1 km: suficiente para el tiempo y más privado
    let body = http_get(format!("https://api.open-meteo.com/v1/forecast?latitude={lat:.2}&longitude={lon:.2}&current=temperature_2m,is_day,weather_code&hourly=temperature_2m,weather_code,is_day,precipitation_probability&daily=weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset&forecast_days=6&timezone=auto"))?;
    // Previsión para el panel del tiempo (si falla, el dato actual sigue valiendo)
    if let Some(f) = weather::parse(&body) { *weather::FORECAST.lock().unwrap() = Some(f); }
    let cur = &body[body.find("\"current\":{")?..];
    Some(Weather { temp: json_num(cur, "temperature_2m")? as f32, is_day: json_num(cur, "is_day")? > 0.5, code: json_num(cur, "weather_code")? as i32 })
}

fn weather_thread() {
    unsafe { let _ = RoInitialize(RO_INIT_MULTITHREADED); }
    loop {
        let wait = match config_location().or_else(system_location).and_then(|(a, b)| fetch_weather(a, b)) {
            Some(w) => {
                *WEATHER.lock().unwrap() = Some(w);
                unsafe { let _ = PostMessageW(Some(bar_hwnd()), WM_WEATHER, WPARAM(0), LPARAM(0)); }
                30 * 60
            }
            None => 5 * 60,
        };
        popup::notify(popup::Kind::Weather);
        // Espera troceada para poder atender "Actualizar ahora" del menú
        for _ in 0..wait / 2 {
            if WEATHER_NOW.swap(false, Ordering::Relaxed) { break; }
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}


// ---------------------------------------------------------------- acciones
use sys::{combo, vk};

/// Trae al frente el reproductor que está sonando.
fn open_player(aumid: &str) {
    let a = aumid.to_lowercase();
    // Win32 (Spotify.exe) o UWP/PWA ("SpotifyAB.SpotifyMusic_…!Spotify", "Chrome")
    let key = a.strip_suffix(".exe").map(str::to_string).unwrap_or_else(|| {
        a.split(['!', '_']).next().unwrap_or(&a).rsplit('.').next().unwrap_or(&a).to_string()
    });
    if !key.is_empty() {
        for h in sys::app_windows() {
            let exe = sys::process_path(sys::real_pid(h)).unwrap_or_default().to_lowercase();
            let stem = exe.rsplit('\\').next().unwrap_or("").trim_end_matches(".exe").to_string();
            if stem == key || stem.contains(&key) { sys::activate(h); return; }
        }
    }
    if !aumid.is_empty() { sys::open(&format!("shell:AppsFolder\\{aumid}")); }
}

/// Trae la app de escritorio de Claude al frente, o la abre si está cerrada.
fn open_claude() {
    for h in sys::app_windows() {
        let exe = sys::process_path(sys::real_pid(h)).unwrap_or_default().to_lowercase();
        // La app de escritorio es "claude.exe" con ventana; Claude Code (CLI) no tiene ventanas
        if exe.ends_with("\\claude.exe") && !exe.contains("claude-code") {
            sys::activate(h);
            return;
        }
    }
    sys::open("shell:AppsFolder\\Claude_pzs8sxrjxfjjc!Claude");
}

/// Menú de macbar (el de clic derecho en la barra), donde esté el ratón: lo abre su icono en "Iconos ocultos".
fn macbar_menu() {
    let hwnd = bar_hwnd();
    unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let none: Box<dyn Fn(POINT) -> Option<usize>> = Box::new(|_| None);
        let fg = last_fg();
        if let menus::Outcome::Picked(a) = menus::track(hwnd, menus::bar_menu(), pt.x, pt.y, false, usize::MAX, none) {
            if let Some(id) = menus::perform(hwnd, fg, a) { bar_action(hwnd, id); }
        }
        give_back_focus(hwnd, fg);
    }
}

fn media(cmd: Cmd) { if let Some(tx) = CMD_TX.get() { let _ = tx.send(cmd); } }

fn bar_action(hwnd: HWND, id: u32) {
    match id {
        BAR_DESKTOP => menus::toggle_desktop(),
        BAR_WEATHER_NOW => WEATHER_NOW.store(true, Ordering::Relaxed),
        BAR_QUIT => unsafe { let _ = DestroyWindow(hwnd); },
        BAR_PREV => media(Cmd::Prev),
        BAR_TOGGLE => media(Cmd::Toggle),
        BAR_NEXT => media(Cmd::Next),
        BAR_PLAYER => run(Act::OpenPlayer),
        BAR_WIDGETS => desk::toggle(),
        i if (BAR_WIDGET0..BAR_WIDGET0 + 7).contains(&i) => desk::toggle_kind((i - BAR_WIDGET0) as usize),
        i if (BAR_INK0..BAR_INK0 + 3).contains(&i) => desk::set_ink_mode((i - BAR_INK0) as i8),
        i if (BAR_ACCENT0..BAR_ACCENT0 + profiles::ACCENTS.len() as u32).contains(&i) => profiles::set_accent((i - BAR_ACCENT0) as usize),
        BAR_THEME_EXPORT => profiles::export_theme(),
        BAR_THEME_IMPORT => profiles::import_theme(),
        i if (BAR_PROFILE0..BAR_PROFILE0 + 3).contains(&i) => profiles::choose((i - BAR_PROFILE0) as usize),
        BAR_PROFILES_DIR => profiles::open_folder(),
        BAR_PRESENT => profiles::toggle_present(),
        BAR_CAFFEINE => live::caffeine_toggle(),
        BAR_PICKER => pick_color(),
        BAR_POMODORO => if live::POMO.load(Ordering::Relaxed) != 0 { live::timer_stop() } else { live::pomodoro_start() },
        BAR_POS_TOP => set_bar_bottom(false),
        BAR_POS_BOTTOM => set_bar_bottom(true),
        i if (BAR_LANG0..BAR_LANG0 + 3).contains(&i) => { i18n::set_mode((i - BAR_LANG0) as i8); desk::repaint_all(); unsafe { let _ = InvalidateRect(Some(hwnd), None, false); } }
        _ => {}
    }
}

/// Cuentagotas: elige un color de la pantalla, lo copia en hexadecimal y lo enseña en la isla.
pub(crate) fn pick_color() {
    std::thread::spawn(|| {
        // Un respiro para que se cierre el buscador o el menú que lo lanzó
        std::thread::sleep(Duration::from_millis(160));
        if let Some((r, g, b)) = picker::pick() {
            let c = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
            deskw::copy_text(&format!("#{c:06X}"));
            // Después del aviso de «Copiado», para que se quede el del color
            std::thread::sleep(Duration::from_millis(60));
            live::show(live::Hud::Color(c), 2600);
        }
    });
}

/// Acciones directas (sin menú).
fn run(a: Act) {
    match a {
        Act::Desktop => menus::toggle_desktop(),
        Act::Claude => open_claude(),
        Act::Search => search::toggle(),
        Act::TimerStop => live::timer_stop(),
        Act::Caffeine => live::caffeine_toggle(),
        Act::ShelfOpen(i) => { let p = live::SHELF.lock().unwrap().get(i as usize).map(|x| x.0.clone()); if let Some(p) = p { sys::open(&p); } }
        Act::ShelfClear => live::shelf_clear(),
        Act::ShelfShare => {
            let paths: Vec<String> = live::SHELF.lock().unwrap().iter().map(|x| x.0.clone()).collect();
            let _ = share::files(bar_hwnd(), &paths);
        }
        Act::HudDismiss => {
            // Descarga: se muestra en su carpeta; captura: se abre; el resto, se cierra el aviso
            let file = live::LAST_FILE.lock().unwrap().as_ref().map(|f| f.0.clone());
            match (live::current(), file) {
                (Some(live::Hud::Download), Some(p)) => sys::open_args("explorer.exe", &format!("/select,\"{p}\"")),
                (Some(live::Hud::Shot), Some(p)) => sys::open(&p),
                (Some(live::Hud::Phone), _) => sys::open("ms-phone:"),
                (Some(live::Hud::Disk(_)), _) => sys::open("ms-settings:storagesense"),
                (Some(live::Hud::Saver(_)), _) => sys::open("ms-settings:batterysaver"),
                _ => {}
            }
            live::dismiss();
            unsafe { let _ = InvalidateRect(Some(bar_hwnd()), None, false); }
        }
        Act::Island | Act::Toggle => media(Cmd::Toggle),
        Act::Prev => media(Cmd::Prev),
        Act::Next => media(Cmd::Next),
        Act::OpenYtm => sys::open("https://music.youtube.com"),
        Act::OpenPlayer => { if let Some(m) = MEDIA.lock().unwrap().clone() { open_player(&m.app); } }
        _ => {}
    }
}

fn build_menu(a: Act, c: &menus::Ctx) -> menus::Menu {
    match a {
        Act::AppMenu => menus::app_menu(c),
        Act::Battery => {
            let mut s = SYSTEM_POWER_STATUS::default();
            unsafe { let _ = GetSystemPowerStatus(&mut s); }
            let secs = (s.BatteryLifeTime != u32::MAX).then_some(s.BatteryLifeTime);
            menus::battery_menu(s.BatteryLifePercent.min(100), s.ACLineStatus == 1, secs)
        }
        Act::User => menus::user_menu(),
        _ => menus::Menu::default(),
    }
}

/// Títulos de menú en orden de la barra, con su rectángulo.
fn menu_hits() -> Vec<(D2D_RECT_F, Act)> {
    GFX.with(|g| g.borrow().as_ref().map(|g| g.hits.iter().filter(|(_, a)| a.is_menu()).copied().collect()).unwrap_or_default())
}

fn set_open(hwnd: HWND, a: Option<Act>) {
    GFX.with(|g| if let Some(g) = g.borrow_mut().as_mut() { g.open = a; g.pressed = None; if a.is_some() { g.hover = None; } });
    unsafe { let _ = InvalidateRect(Some(hwnd), None, false); let _ = UpdateWindow(hwnd); }
}

/// Abre un menú de la barra. Si el ratón pasa a otro título, salta a ese menú (como en macOS).
fn open_menu(hwnd: HWND, start: Act) {
    let fg = last_fg();
    let desktop = fg.0.is_null() || sys::is_shell_window(fg);
    let pid = if desktop { 0 } else { sys::real_pid(fg) };
    let ctx = menus::Ctx {
        pid, desktop,
        app: GFX.with(|g| g.borrow().as_ref().map(|g| g.app_name.clone()).unwrap_or_default()),
        exe: if desktop { None } else { sys::process_path(pid) },
    };
    let s = scale_of(hwnd);
    let mut cur = start;
    loop {
        let list = menu_hits();
        let Some(idx) = list.iter().position(|(_, a)| *a == cur) else { break };
        let r = list[idx].0;
        let menu = build_menu(cur, &ctx);
        set_open(hwnd, Some(cur));
        let x = if cur.right_side() { r.right } else { r.left };
        let y = (BAR_H - 2.0) * s;
        let probe: Box<dyn Fn(POINT) -> Option<usize>> = Box::new(move |pt: POINT| {
            let (x, y) = (pt.x as f32 / s, pt.y as f32 / s);
            if y < 0.0 || y > BAR_H { return None; }
            menu_hits().iter().position(|(r, _)| x >= r.left && x <= r.right)
        });
        let out = menus::track(hwnd, menu, (x * s) as i32, y as i32, cur.right_side(), idx, probe);
        set_open(hwnd, None);
        match out {
            menus::Outcome::Switch(i) => { match menu_hits().get(i) { Some((_, a)) => cur = *a, None => break } }
            menus::Outcome::Picked(a) => {
                if let Some(id) = menus::perform(hwnd, fg, a) { bar_action(hwnd, id); }
                give_back_focus(hwnd, fg);
                break
            }
            menus::Outcome::Closed => { give_back_focus(hwnd, fg); break }
        }
    }
}

/// La barra nunca se queda con el foco: si tras el menú sigue activa, se lo devuelve a la app.
/// (Si la acción abrió otra ventana o minimizó la app, ya no es la barra la activa y no se toca.)
fn give_back_focus(bar: HWND, fg: HWND) {
    unsafe {
        if GetForegroundWindow() != bar { return; }
        let usable = |h: HWND| !h.0.is_null() && IsWindow(Some(h)).as_bool() && IsWindowVisible(h).as_bool() && !IsIconic(h).as_bool();
        let target = if usable(fg) { fg } else {
            // La app ya no está (cerrada o minimizada): la siguiente ventana, como hace Windows; si no hay, el escritorio
            sys::app_windows().into_iter().find(|h| *h != bar && *h != fg && usable(*h))
                .unwrap_or_else(|| FindWindowW(w!("Progman"), None).unwrap_or_default())
        };
        if !target.0.is_null() { let _ = SetForegroundWindow(target); }
    }
    // Una app que se está cerrando tarda un poco en desaparecer: se vuelve a comprobar en 400 ms
    FOCUS_TARGET.store(fg.0 as isize, Ordering::Relaxed);
    unsafe { SetTimer(Some(bar), TIMER_FOCUS, 400, None); }
}

/// Borde derecho (px de pantalla) del botón de una acción, más un margen.
fn anchor_right(hwnd: HWND, a: Act, extra: f32) -> i32 {
    let r = GFX.with(|g| g.borrow().as_ref().and_then(|g| g.hits.iter().find(|(_, x)| *x == a).map(|(r, _)| *r)));
    r.map(|r| ((r.right + extra) * scale_of(hwnd)) as i32).unwrap_or(0)
}

fn click(hwnd: HWND, a: Act) {
    // Un panel nuevo cierra los demás
    let close_others = |keep: Act| {
        if keep != Act::Logo { ccenter::close(); }
        if keep != Act::Tray { tray::close(); }
        if !matches!(keep, Act::Clock | Act::Weather | Act::Wifi) { popup::hide_all(); }
    };
    match a {
        Act::Logo => { close_others(a); ccenter::toggle(hwnd); }
        Act::Tray => { close_others(a); trayhost::mark_seen(); tray::toggle(anchor_right(hwnd, a, 40.0)); }
        Act::Wifi => { close_others(a); popup::toggle(popup::Kind::Wifi, anchor_right(hwnd, a, 60.0)); }
        Act::Weather => { close_others(a); popup::toggle(popup::Kind::Weather, anchor_right(hwnd, a, 24.0)); }
        Act::Clock | Act::TimerOpen => {
            close_others(Act::Clock);
            let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
            popup::toggle(popup::Kind::Calendar, sw);
        }
        _ if a.is_menu() => open_menu(hwnd, a),
        _ => run(a),
    }
    unsafe { let _ = InvalidateRect(Some(hwnd), None, false); }
}

// ---------------------------------------------------------------- dibujo
/// Destello del logotipo de Claude (trazado del SVG oficial, sin el texto). Caja ≈ 0..127 × 0..125.
const CLAUDE_MARK: &str = "m54.375 118.75 1.75-7.75 2-10 1.625-8 1.5-9.875.875-3.25-.125-.25-.625.125-7.5 10.25L42.5 105.375l-9 9.5-2.125.875-3.75-1.875.375-3.5 2.125-3L42.5 91.5l7.5-9.875L54.875 76l-.125-.75h-.25l-33 21.5-5.875.75L13 95.125l.375-3.875 1.25-1.25 9.875-6.875 24.625-13.75.375-1.25-.375-.625h-1.25l-4.125-.25-14-.375-12.125-.5L5.75 65.75l-3-.625L0 61.375.25 59.5l2.5-1.625 3.625.25 7.875.625 11.875.75 8.625.5 12.75 1.375h2l.25-.875-.625-.5-.5-.5-12.375-8.25L23 42.5l-7-5.125-3.75-2.625-1.875-2.375-.75-5.25L13 23.375l4.625.375 1.125.25 4.625 3.625 9.875 7.625 13 9.625 1.875 1.5.875-.5V45.5l-.875-1.375-7-12.75-7.5-13L30.25 13l-.875-3.25q-.5-1.687-.5-3.75L32.75.75 34.875 0l5.25.75 2.125 1.875L45.5 10l5.125 11.625 8.125 15.75 2.375 4.75 1.25 4.25.5 1.375h.875V47l.625-9 1.25-10.875 1.25-14 .375-4 2-4.75 3.875-2.5 3 1.375 2.5 3.625-.375 2.25-1.375 9.625-3 15.125L72 44.125h1.125l1.25-1.375L79.5 36l8.625-10.75 3.75-4.25 4.5-4.75L99.25 14h5.375l3.875 5.875L106.75 26l-5.5 7-4.625 5.875L90 47.75l-4 7.125.375.5h.875l14.875-3.25 8.125-1.375 9.5-1.625 4.375 2 .5 2-1.75 4.25-10.25 2.5-12 2.375L82.75 66.5l-.25.125.25.375 8 .75 3.5.25h8.5l15.75 1.125 4.125 2.75 2.375 3.25-.375 2.625-6.375 3.125-8.5-2-20-4.75L83 72.5h-1v.5l5.75 5.625L98.125 88l13.125 12.125.625 3-1.625 2.5-1.75-.25-11.5-8.75-4.5-3.875-10-8.375h-.625v.875l2.25 3.375L96.375 107l.625 5.625-.875 1.75-3.25 1.125-3.375-.625-7.25-10L74.875 93.5l-6-10.125-.625.5-3.625 37.75L63 123.5l-3.75 1.5-3.125-2.375z";

/// Lector mínimo de trazados SVG (M L H V Q Z, absolutos y relativos). Las curvas Q se aproximan con
/// 6 segmentos: a 18 px no se distinguen de la curva real.
fn svg_polys(d: &str) -> Vec<Vec<(f32, f32)>> {
    let b = d.as_bytes();
    let mut i = 0;
    let num = |i: &mut usize| -> Option<f32> {
        while *i < b.len() && (b[*i] == b' ' || b[*i] == b',') { *i += 1; }
        let s = *i;
        if *i < b.len() && (b[*i] == b'-' || b[*i] == b'+') { *i += 1; }
        let mut dot = false;
        while *i < b.len() && (b[*i].is_ascii_digit() || (b[*i] == b'.' && !dot)) { if b[*i] == b'.' { dot = true; } *i += 1; }
        if *i == s { return None; }
        d[s..*i].parse().ok()
    };
    let (mut polys, mut cur): (Vec<Vec<(f32, f32)>>, Vec<(f32, f32)>) = (Vec::new(), Vec::new());
    let (mut x, mut y, mut cmd) = (0f32, 0f32, b'M');
    loop {
        while i < b.len() && b[i] == b' ' { i += 1; }
        if i >= b.len() { break; }
        if b[i].is_ascii_alphabetic() { cmd = b[i]; i += 1; if cmd == b'z' || cmd == b'Z' { if !cur.is_empty() { polys.push(std::mem::take(&mut cur)); } continue; } }
        let rel = cmd.is_ascii_lowercase();
        let (bx, by) = if rel { (x, y) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'M' | b'L' => {
                let (Some(a), Some(c)) = (num(&mut i), num(&mut i)) else { break };
                x = bx + a; y = by + c;
                if cmd.to_ascii_uppercase() == b'M' && !cur.is_empty() { polys.push(std::mem::take(&mut cur)); }
                cur.push((x, y));
                // Tras un M, los pares siguientes son L
                if cmd == b'M' { cmd = b'L'; } else if cmd == b'm' { cmd = b'l'; }
            }
            b'H' => { let Some(a) = num(&mut i) else { break }; x = bx + a; cur.push((x, y)); }
            b'V' => { let Some(a) = num(&mut i) else { break }; y = by + a; cur.push((x, y)); }
            b'Q' => {
                let (Some(a), Some(c), Some(e), Some(f)) = (num(&mut i), num(&mut i), num(&mut i), num(&mut i)) else { break };
                let (x0, y0, qx, qy, ex, ey) = (x, y, bx + a, by + c, bx + e, by + f);
                for k in 1..=6 {
                    let t = k as f32 / 6.0;
                    let u = 1.0 - t;
                    cur.push((u * u * x0 + 2.0 * u * t * qx + t * t * ex, u * u * y0 + 2.0 * u * t * qy + t * t * ey));
                }
                x = ex; y = ey;
            }
            _ => break,
        }
    }
    if !cur.is_empty() { polys.push(cur); }
    polys
}

fn claude_geometry(factory: &ID2D1Factory) -> Option<ID2D1PathGeometry> {
    unsafe {
        let geo = factory.CreatePathGeometry().ok()?;
        let sink = geo.Open().ok()?;
        sink.SetFillMode(D2D1_FILL_MODE_WINDING);
        for poly in svg_polys(CLAUDE_MARK) {
            let Some(&(fx, fy)) = poly.first() else { continue };
            sink.BeginFigure(Vector2 { X: fx, Y: fy }, D2D1_FIGURE_BEGIN_FILLED);
            let pts: Vec<Vector2> = poly[1..].iter().map(|&(x, y)| Vector2 { X: x, Y: y }).collect();
            sink.AddLines(&pts);
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        }
        sink.Close().ok()?;
        Some(geo)
    }
}

/// Medidas de texto cacheadas: no se recalculan en cada fotograma.
#[derive(Default)]
struct Caches {
    width: std::collections::HashMap<(usize, String), f32>,
    /// Desplazamiento de la caja de texto para que la altura de mayúscula quede centrada.
    base: std::collections::HashMap<usize, f32>,
    /// Caja de tinta real de un glifo: (izq, arriba, ancho, alto).
    ink: std::collections::HashMap<(usize, String), [f32; 4]>,
    /// Textos ya maquetados (maquetar en cada fotograma era lo que más CPU gastaba)
    layouts: std::collections::HashMap<(usize, String), IDWriteTextLayout>,
    /// Pinceles de color sólido y degradados, ligados al lienzo `rt_key` (si cambia, se tiran)
    brushes: std::collections::HashMap<u32, ID2D1Brush>,
    grads: std::collections::HashMap<(i32, i32, i32, i32, u32, u32), ID2D1Brush>,
    rt_key: RtKey,
}

struct Painter<'a> { rt: &'a ID2D1RenderTarget, dw: &'a IDWriteFactory, cache: &'a RefCell<Caches>, h: f32, s: f32 }

fn draw_eq(rt: &ID2D1RenderTarget, eq: &[f32; 5], x0: f32, cy: f32, b: &ID2D1Brush) {
    for (i, v) in eq.iter().enumerate() {
        let bh = (3.0 + v * 13.0).round(); // altura entera: barras nítidas y simétricas
        let bx = x0 + i as f32 * 4.8;
        unsafe { rt.FillRoundedRectangle(&rrect(rect(bx, cy - bh / 2.0, bx + 3.0, cy + bh / 2.0), 1.5), b); }
    }
}

fn fkey(f: &IDWriteTextFormat) -> usize { use windows::core::Interface; f.as_raw() as usize }

/// Tamaño visible común de los iconos de la barra (DIP, el lado mayor de la tinta).
const ICON_PX: f32 = 16.0;

/// Escala que lleva un glifo a `ICON_PX` por su lado mayor. Los muy planos (la flecha) se miden algo
/// más grandes para que no parezcan diminutos; se limita para no deformar nada raro.
fn icon_norm(p: &Painter, f: &IDWriteTextFormat, g: &str) -> f32 {
    let [_, _, iw, ih] = p.ink(f, g);
    let (iw, ih) = (iw.max(0.1), ih.max(0.1));
    let flat = (iw / ih).max(ih / iw);
    let side = iw.max(ih) * if flat > 1.8 { 0.88 } else { 1.0 };
    // Ajuste óptico: la lupa, la flecha y el monitor son trazos abiertos que llenan su caja y se ven
    // más grandes que la batería o el Wi-Fi al mismo tamaño
    let optical = match g { "\u{E721}" | "\u{E70D}" | "\u{E7F4}" => 0.86, _ => 1.0 };
    (ICON_PX / side).clamp(0.7, 1.5) * optical
}

/// Color empaquetado en 32 bits (8 por canal): clave de la caché de pinceles.
fn pack(c: &D2D1_COLOR_F) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (q(c.r) << 24) | (q(c.g) << 16) | (q(c.b) << 8) | q(c.a)
}

impl Painter<'_> {
    /// Las cachés de pinceles valen solo para el lienzo con el que se crearon.
    fn check_rt(&self) {
        let mut c = self.cache.borrow_mut();
        if c.rt_key.fresh(self.rt) { c.brushes.clear(); c.grads.clear(); }
    }
    fn brush(&self, c: D2D1_COLOR_F) -> ID2D1Brush {
        self.check_rt();
        let key = pack(&c);
        if let Some(b) = self.cache.borrow().brushes.get(&key) { return b.clone(); }
        let b: ID2D1Brush = unsafe { self.rt.CreateSolidColorBrush(&c, None).unwrap().into() };
        let mut cache = self.cache.borrow_mut();
        if cache.brushes.len() > 512 { cache.brushes.clear(); }
        cache.brushes.insert(key, b.clone());
        b
    }
    fn gradient(&self, a: Vector2, b: Vector2, c0: D2D1_COLOR_F, c1: D2D1_COLOR_F) -> ID2D1Brush {
        self.check_rt();
        let key = ((a.X * 4.0) as i32, (a.Y * 4.0) as i32, (b.X * 4.0) as i32, (b.Y * 4.0) as i32, pack(&c0), pack(&c1));
        if let Some(g) = self.cache.borrow().grads.get(&key) { return g.clone(); }
        let g: ID2D1Brush = unsafe {
            let stops = [D2D1_GRADIENT_STOP { position: 0.0, color: c0 }, D2D1_GRADIENT_STOP { position: 1.0, color: c1 }];
            let col = self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).unwrap();
            let lp = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: a, endPoint: b };
            self.rt.CreateLinearGradientBrush(&lp, None, &col).unwrap().into()
        };
        let mut cache = self.cache.borrow_mut();
        if cache.grads.len() > 128 { cache.grads.clear(); }
        cache.grads.insert(key, g.clone());
        g
    }
    /// Texto maquetado una vez y reutilizado (la maquetación no depende del lienzo).
    fn cached_layout(&self, fmt: &IDWriteTextFormat, s: &str) -> IDWriteTextLayout {
        let k = (fkey(fmt), s.to_string());
        if let Some(l) = self.cache.borrow().layouts.get(&k) { return l.clone(); }
        let l = self.layout(fmt, s);
        let mut c = self.cache.borrow_mut();
        if c.layouts.len() > 800 { c.layouts.clear(); }
        c.layouts.insert(k, l.clone());
        l
    }
    /// Degradado vertical de `top` a `bottom`.
    fn vgrad(&self, top: f32, bottom: f32, c0: D2D1_COLOR_F, c1: D2D1_COLOR_F) -> ID2D1Brush {
        self.gradient(Vector2 { X: 0.0, Y: top }, Vector2 { X: 0.0, Y: bottom }, c0, c1)
    }
    /// Degradado diagonal (avatar).
    fn dgrad(&self, l: f32, t: f32, r: f32, b: f32, c0: D2D1_COLOR_F, c1: D2D1_COLOR_F) -> ID2D1Brush {
        self.gradient(Vector2 { X: l, Y: t }, Vector2 { X: r, Y: b }, c0, c1)
    }
    fn layout(&self, fmt: &IDWriteTextFormat, s: &str) -> IDWriteTextLayout {
        unsafe { self.dw.CreateTextLayout(&wide(s), fmt, 4000.0, 100.0).unwrap() }
    }
    fn width(&self, fmt: &IDWriteTextFormat, s: &str) -> f32 {
        let k = (fkey(fmt), s.to_string());
        if let Some(v) = self.cache.borrow().width.get(&k) { return *v; }
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { let _ = self.layout(fmt, s).GetMetrics(&mut m); }
        let mut c = self.cache.borrow_mut();
        if c.width.len() > 400 { c.width.clear(); }
        c.width.insert(k, m.widthIncludingTrailingWhitespace);
        m.widthIncludingTrailingWhitespace
    }
    /// Arriba de la caja de texto para que la mitad de la altura de mayúscula caiga en el centro.
    fn text_top(&self, fmt: &IDWriteTextFormat) -> f32 {
        if let Some(v) = self.cache.borrow().base.get(&fkey(fmt)) { return *v; }
        let v = unsafe {
            let mut lines = [DWRITE_LINE_METRICS::default()];
            let mut n = 0u32;
            let _ = self.layout(fmt, "H").GetLineMetrics(Some(&mut lines), &mut n);
            let baseline = lines[0].baseline;
            let mut cap = fmt.GetFontSize() * 0.7;
            let mut name = [0u16; 64];
            if fmt.GetFontFamilyName(&mut name).is_ok() {
                let mut coll = None;
                if self.dw.GetSystemFontCollection(&mut coll, false).is_ok() {
                    if let Some(coll) = coll {
                        let (mut idx, mut found) = (0u32, windows::core::BOOL(0));
                        if coll.FindFamilyName(PCWSTR(name.as_ptr()), &mut idx, &mut found).is_ok() && found.as_bool() {
                            if let Ok(font) = coll.GetFontFamily(idx).and_then(|f| f.GetFirstMatchingFont(fmt.GetFontWeight(), fmt.GetFontStretch(), fmt.GetFontStyle())) {
                                let mut m = DWRITE_FONT_METRICS::default();
                                font.GetMetrics(&mut m);
                                cap = m.capHeight as f32 / m.designUnitsPerEm as f32 * fmt.GetFontSize();
                            }
                        }
                    }
                }
            }
            // la línea base cae en una fila física exacta
            ((self.h / 2.0 + cap / 2.0) * self.s).round() / self.s - baseline
        };
        self.cache.borrow_mut().base.insert(fkey(fmt), v);
        v
    }
    /// Caja de tinta de un glifo, para centrar iconos y letras sueltas al píxel.
    fn ink(&self, fmt: &IDWriteTextFormat, s: &str) -> [f32; 4] {
        let k = (fkey(fmt), s.to_string());
        if let Some(v) = self.cache.borrow().ink.get(&k) { return *v; }
        let v = unsafe {
            let l = self.layout(fmt, s);
            let o = l.GetOverhangMetrics().unwrap_or_default();
            let (left, top) = (-o.left, -o.top);
            [left, top, 4000.0 + o.right - left, 100.0 + o.bottom - top]
        };
        self.cache.borrow_mut().ink.insert(k, v);
        v
    }
    fn draw_at(&self, fmt: &IDWriteTextFormat, s: &str, x: f32, y: f32, b: &ID2D1Brush, shadow: Option<&ID2D1Brush>) {
        // Ajuste a la rejilla física: sin medios píxeles, texto más nítido
        let x = (x * self.s).round() / self.s;
        // Misma caja que antes (4000 × 100) pero con la maquetación guardada: DrawText la rehacía cada vez
        let lay = self.cached_layout(fmt, s);
        unsafe {
            if let Some(sb) = shadow {
                self.rt.DrawTextLayout(Vector2 { X: x, Y: y + 1.0 }, &lay, sb, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
            // Sin sombra, los emojis salen en color (con sombra, la fuente de color la pintaría también)
            let opt = if shadow.is_none() { D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT } else { D2D1_DRAW_TEXT_OPTIONS_NONE };
            self.rt.DrawTextLayout(Vector2 { X: x, Y: y }, &lay, b, opt);
        }
    }
    /// Texto con su altura de mayúscula centrada en la barra: todos comparten línea base.
    fn text(&self, fmt: &IDWriteTextFormat, s: &str, x: f32, b: &ID2D1Brush, shadow: Option<&ID2D1Brush>) {
        self.draw_at(fmt, s, x, self.text_top(fmt), b, shadow);
    }
    /// Glifo centrado por su tinta en una caja de ancho `w` que empieza en `x`.
    fn glyph(&self, fmt: &IDWriteTextFormat, g: &str, x: f32, w: f32, b: &ID2D1Brush, shadow: Option<&ID2D1Brush>) {
        let [l, t, iw, ih] = self.ink(fmt, g);
        let y = ((self.h / 2.0 - ih / 2.0 - t) * self.s).round() / self.s;
        self.draw_at(fmt, g, x + (w - iw) / 2.0 - l, y, b, shadow);
    }
    fn snap(&self, v: f32) -> f32 { (v * self.s).round() / self.s }
    /// Píldora con bordes en píxeles físicos exactos y trazo de 1 px real.
    fn pill(&self, r: D2D_RECT_F, fill: &ID2D1Brush, border: Option<&ID2D1Brush>) {
        let r = rect(self.snap(r.left), self.snap(r.top), self.snap(r.right), self.snap(r.bottom));
        let rad = (r.bottom - r.top) / 2.0;
        let px = 1.0 / self.s;
        unsafe {
            self.rt.FillRoundedRectangle(&rrect(r, rad), fill);
            if let Some(b) = border {
                let i = rect(r.left + px / 2.0, r.top + px / 2.0, r.right - px / 2.0, r.bottom - px / 2.0);
                self.rt.DrawRoundedRectangle(&rrect(i, rad - px / 2.0), b, px, None);
            }
        }
    }
    /// Carátula recortada en círculo (`radius` = None) o cuadrado redondeado.
    fn art(&self, bmp: &ID2D1Bitmap, x: f32, y: f32, size: f32, radius: Option<f32>) {
        unsafe {
            let Ok(br) = self.rt.CreateBitmapBrush(bmp, None, None) else { return };
            let s = size / ART_PX as f32;
            br.SetTransform(&Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: x, M32: y });
            let r = rect(x, y, x + size, y + size);
            match radius {
                None => {
                    let e = D2D1_ELLIPSE { point: Vector2 { X: x + size / 2.0, Y: y + size / 2.0 }, radiusX: size / 2.0, radiusY: size / 2.0 };
                    self.rt.FillEllipse(&e, &br);
                }
                Some(rad) => self.rt.FillRoundedRectangle(&rrect(r, rad), &br),
            }
        }
    }
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        BeginPaint(hwnd, &mut ps);
        GFX.with(|g| {
            let mut g = g.borrow_mut();
            let Some(g) = g.as_mut() else { return };
            let scale = scale_of(hwnd);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            if g.target.is_none() {
                let props = D2D1_RENDER_TARGET_PROPERTIES {
                    pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                    ..Default::default()
                };
                let hp = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                    hwnd,
                    pixelSize: D2D_SIZE_U { width: (rc.right - rc.left) as u32, height: (rc.bottom - rc.top) as u32 },
                    // Conservar el contenido permite redibujar solo el ecualizador
                    presentOptions: D2D1_PRESENT_OPTIONS_RETAIN_CONTENTS,
                };
                g.target = g.factory.CreateHwndRenderTarget(&props, &hp).ok();
                if let Some(t) = &g.target {
                    t.SetDpi(96.0 * scale, 96.0 * scale);
                    // Escala de grises: ClearType deja halos sobre fondo transparente
                    t.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                }
                // Todo bitmap pertenece a su superficie: con una nueva, los viejos hacen fallar cada EndDraw
                g.art = None; g.avatar = None; g.hud_bmp = None; g.shelf_bmps = (u64::MAX, Vec::new());
            }
            let Some(rt) = g.target.clone() else { return };
            let media = MEDIA.lock().unwrap().clone();
            // Sube la carátula a la GPU solo cuando cambia
            if let Some(m) = &media {
                if g.art.as_ref().map(|a| a.0) != Some(m.art_ver) {
                    let mut tint = None;
                    g.art = ART.lock().unwrap().as_ref().filter(|a| a.0 == m.art_ver).and_then(|(v, bytes)| {
                        tint = Some(art_tint(bytes));
                        let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                        rt.CreateBitmap(D2D_SIZE_U { width: ART_PX, height: ART_PX }, Some(bytes.as_ptr() as _), ART_PX * 4, &bp).ok().map(|b| (*v, b))
                    });
                    // Carátula nueva: entra con un "pop" y la isla toma su color
                    match tint { Some(t) => { if t != g.art_tint { g.art_at = Instant::now(); } g.art_tint = t; } None => g.art_tint = [1.0; 3] }
                }
            }

            let w = (rc.right - rc.left) as f32 / scale;
            let h = BAR_H;
            let p = Painter { rt: &rt, dw: &g.dwrite, cache: &g.wcache, h, s: scale };
            let mut eq_rect = None;
            // ---- Paleta (medida sobre la referencia) ----
            // Tinta de la barra: texto blanco sobre fondos oscuros; casi negro sobre fondos claros (con
            // un fundido al cambiar). La isla va siempre en blanco: su cápsula es negra.
            let bk = BAR_INK.with(|c| {
                let to = if BAR_LIGHT.load(Ordering::Relaxed) { 1.0 } else { 0.0 };
                let mut v = c.get();
                let step = (g.last_frame.elapsed().as_secs_f32()).min(1.0 / 30.0);
                if (to - v).abs() > 0.004 { v += (to - v) * (1.0 - (-step / 0.18).exp()); } else { v = to; }
                c.set(v);
                v
            });
            let ink_moving = bk > 0.0 && bk < 1.0;
            let ic = |a: f32| popup::ink_color(bk, a);
            let (bar_white, bar_soft, bar_dim) = (p.brush(ic(0.96)), p.brush(ic(0.86)), p.brush(ic(0.52)));
            // Sombra bajo el texto: oscura con tinta blanca, un halo claro muy suave con tinta oscura
            let bar_shadow = p.brush(rgba(bk, bk, bk, 0.26 * (1.0 - bk) + 0.08 * bk));
            let (isl_white, isl_soft, isl_dim, isl_shadow) = (p.brush(rgba(1.0, 1.0, 1.0, 0.96)), p.brush(rgba(1.0, 1.0, 1.0, 0.86)), p.brush(rgba(1.0, 1.0, 1.0, 0.52)), p.brush(rgba(0.0, 0.0, 0.0, 0.38)));
            let (white, soft, dim, shadow) = (bar_white.clone(), bar_soft.clone(), bar_dim.clone(), bar_shadow.clone());
            let black = p.brush(rgba(0.0, 0.0, 0.0, 1.0));
            let hover_b = p.brush(ic(0.13));
            let sep = p.brush(ic(0.12));
            let sh = Some(&shadow);
            let mut hits: Vec<(D2D_RECT_F, Act)> = Vec::new();
            let py = (h - PILL_H) / 2.0;
            let pb = py + PILL_H;
            let cy = h / 2.0;
            // Cristal: degradado vertical (arriba algo más claro) + borde con luz arriba
            // Con tinta oscura, las píldoras son de cristal claro
            let mx = |a: f32, b: f32| a + (b - a) * bk;
            let glass = p.vgrad(py, pb, rgba(mx(0.16, 0.98), mx(0.17, 0.98), mx(0.19, 0.99), mx(0.66, 0.62)), rgba(mx(0.05, 0.93), mx(0.055, 0.93), mx(0.065, 0.95), mx(0.70, 0.66)));
            let edge = p.vgrad(py, pb, ic(0.20), ic(0.06));
            let (sy, sb) = (py + 3.0, pb - 3.0); // sub-píldoras de 24
            let sub_fill = p.vgrad(sy, sb, ic(0.09), ic(0.045));
            let sub_edge = p.vgrad(sy, sb, ic(0.16), ic(0.05));
            let isl_edge = p.vgrad(py, pb, rgba(1.0, 1.0, 1.0, 0.14), rgba(1.0, 1.0, 1.0, 0.03));

            // ---- Reloj de animación: un paso por fotograma, con dt acotado (tras un rato quieta no salta)
            let now = Instant::now();
            let dt = (now - g.last_frame).as_secs_f32().min(1.0 / 30.0);
            g.last_frame = now;
            let mut animating = ink_moving;
            let mut born = std::mem::take(&mut g.born);
            let mut seen: Vec<&'static str> = Vec::new();
            // Elemento opcional que aparece: devuelve 0..1 (fundido y ancho)
            let mut appear = |key: &'static str, animating: &mut bool| -> f32 {
                seen.push(key);
                let t0 = *born.entry(key).or_insert(now);
                let k = ease_out(t0.elapsed().as_secs_f32() / 0.30);
                if k < 1.0 { *animating = true; }
                k
            };
            // Entrada de la barra al arrancar: baja 6 px y aparece
            let intro = ease_out(g.t0.elapsed().as_secs_f32() / 0.45);
            if intro < 1.0 { animating = true; }
            let base = Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: -(1.0 - intro) * 6.0 };

            rt.BeginDraw();
            rt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
            rt.SetTransform(&base);
            popup::push_alpha(&rt, intro);

            // Sombra suave de arriba abajo para que el texto se lea sobre cualquier fondo
            let veil = p.vgrad(0.0, h, rgba(bk, bk, bk, 0.30 * (1.0 - bk) + 0.22 * bk), rgba(bk, bk, bk, 0.0));
            rt.FillRectangle(&rect(-1.0, -8.0, w + 1.0, h), &veil);

            // ============ IZQUIERDA: estilo menú de macOS (sin píldora, texto sobre la barra) ============
            // Logo de Windows suelto, en el lugar de la manzana
            let mut x = 14.0;
            let (sq, gap) = (6.3_f32, 1.4_f32);
            let logo_w = sq * 2.0 + gap;
            let l0 = p.snap(x + 6.0);
            let t0 = p.snap(cy - logo_w / 2.0);
            // Cada borde en un píxel físico exacto: los cuatro cuadrados iguales y nítidos
            for (dx, dy) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                let l = p.snap(l0 + dx * (sq + gap));
                let t = p.snap(t0 + dy * (sq + gap));
                let (r, b) = (p.snap(l + sq), p.snap(t + sq));
                rt.FillRoundedRectangle(&rrect(rect(l, t + 1.0, r, b + 1.0), 0.8), &shadow);
                rt.FillRoundedRectangle(&rrect(rect(l, t, r, b), 0.8), &white);
            }
            hits.push((rect(x - 2.0, sy, x + logo_w + 14.0, sb), Act::Logo));
            x += logo_w + 26.0;
            // Nombre de la app activa en negrita: abre su menú (ocultar, salir…).
            // Al cambiar de app, el nombre viejo se desvanece y el nuevo entra desde un poco más abajo.
            let app = g.app_name.clone();
            let aw = p.width(&g.f.app, &app);
            let k = ease_out(g.app_at.elapsed().as_secs_f32() / 0.22);
            if k < 1.0 {
                animating = true;
                let old = g.app_prev.clone();
                popup::push_alpha(&rt, 1.0 - k);
                rt.SetTransform(&Matrix3x2 { M32: base.M32 - k * 4.0, ..base });
                p.text(&g.f.app, &old, x, &white, sh);
                rt.PopLayer();
                rt.SetTransform(&Matrix3x2 { M32: base.M32 + (1.0 - k) * 4.0, ..base });
                popup::push_alpha(&rt, k);
                p.text(&g.f.app, &app, x, &white, sh);
                rt.PopLayer();
                rt.SetTransform(&base);
            } else {
                p.text(&g.f.app, &app, x, &white, sh);
            }
            hits.push((rect(x - 9.0, sy, x + aw + 9.0, sb), Act::AppMenu));
            let (white, soft, dim, shadow) = (isl_white.clone(), isl_soft.clone(), isl_dim.clone(), isl_shadow.clone());
            let sh = Some(&shadow);
            // ============ ISLA CENTRAL REACTIVA: música, temporizador y avisos (volumen, brillo, cargador) ============
            let ytm_red = p.brush(rgba(1.0, 0.0, 0.2, 1.0));
            let orange = p.brush(rgba(1.0, 0.62, 0.22, 1.0));
            let green = p.brush(rgba(0.30, 0.85, 0.45, 1.0));
            let red = p.brush(rgba(1.0, 0.36, 0.33, 1.0));
            // Logo de YouTube Music: círculo rojo, anillo blanco y triángulo de reproducir
            let yt_logo = |cx: f32, cyy: f32, r: f32| {
                let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cyy }, radiusX: r, radiusY: r };
                rt.FillEllipse(&e, &ytm_red);
                let ring = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cyy }, radiusX: r * 0.6, radiusY: r * 0.6 };
                rt.DrawEllipse(&ring, &white, r * 0.13, None);
                if let Ok(tri) = g.factory.CreatePathGeometry() {
                    if let Ok(sk) = tri.Open() {
                        sk.BeginFigure(Vector2 { X: cx - r * 0.2, Y: cyy - r * 0.3 }, D2D1_FIGURE_BEGIN_FILLED);
                        sk.AddLines(&[Vector2 { X: cx - r * 0.2, Y: cyy + r * 0.3 }, Vector2 { X: cx + r * 0.32, Y: cyy }]);
                        sk.EndFigure(D2D1_FIGURE_END_CLOSED);
                        let _ = sk.Close();
                        rt.FillGeometry(&tri, &white, None);
                    }
                }
            };
            let hud = live::current();
            // Miniatura de la última captura (se sube a la GPU una vez)
            if hud == Some(live::Hud::Shot) {
                let lf = live::LAST_FILE.lock().unwrap().clone();
                if let Some((_, Some(px), v)) = lf {
                    if g.hud_bmp.as_ref().map(|b| b.0) != Some(v) {
                        let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                        g.hud_bmp = rt.CreateBitmap(D2D_SIZE_U { width: ART_PX, height: ART_PX }, Some(px.as_ptr() as _), ART_PX * 4, &bp).ok().map(|b| (v, b));
                    }
                }
            }
            if let Some(hh) = hud { g.last_hud = Some(hh); }
            let timer = live::timer_left();
            // El temporizador repinta cada segundo mientras corre
            if timer.is_some() != g.count_on {
                g.count_on = timer.is_some();
                if g.count_on { SetTimer(Some(hwnd), TIMER_COUNT, 1000, None); } else { let _ = KillTimer(Some(hwnd), TIMER_COUNT); }
            }
            // Repintado justo cuando caduca el aviso
            if let Some(until) = (*live::HUD.lock().unwrap()).map(|x| x.1) {
                let ms = until.saturating_duration_since(now).as_millis() as u32 + 30;
                SetTimer(Some(hwnd), TIMER_HUD, ms, None);
            }
            let mode = match (hud, &media, timer) {
                (Some(hh), _, _) => Mode::Hud(hh),
                (None, Some(_), _) => Mode::Media,
                (None, None, Some(_)) => Mode::Timer,
                (None, None, None) if !live::SHELF.lock().unwrap().is_empty() => Mode::Shelf,
                _ => Mode::Idle,
            };
            if mode.key() != g.isl_mode.key() { g.isl_prev = Some(g.isl_mode); g.isl_mode_at = now; }
            g.isl_mode = mode;
            let mode_k = ease_out(g.isl_mode_at.elapsed().as_secs_f32() / 0.24);
            if mode_k >= 1.0 { g.isl_prev = None; } else { animating = true; }
            // Barra del aviso: se desliza hasta el valor nuevo
            let hud_target = match hud.or(g.last_hud) {
                Some(live::Hud::Volume(v, m)) => if m { 0.0 } else { v },
                Some(live::Hud::Bright(b)) => b,
                Some(live::Hud::Power(pc, _)) | Some(live::Hud::LowBat(pc)) | Some(live::Hud::Claude(pc)) | Some(live::Hud::Mem(pc)) | Some(live::Hud::Disk(pc)) => pc as f32 / 100.0,
                Some(live::Hud::Full) => 1.0,
                _ => 0.0,
            };
            if (g.hud_val - hud_target).abs() > 0.002 { g.hud_val += (hud_target - g.hud_val) * (1.0 - (-dt / 0.07).exp()); animating = true; } else { g.hud_val = hud_target; }
            // El color de la isla se funde suavemente hacia el de la carátula
            {
                let k = 1.0 - (-dt / 0.35).exp();
                let mut moving = false;
                for i in 0..3 { let d = g.art_tint[i] - g.tint_now[i]; if d.abs() > 0.004 { g.tint_now[i] += d * k; moving = true; } else { g.tint_now[i] = g.art_tint[i]; } }
                if moving { animating = true; }
            }
            // Pop de la carátula nueva y "hinchado" de la barra de volumen al cambiar
            // Reproducir/pausa: el botón da un pequeño "pop" al cambiar de estado
            let playing_now = media.as_ref().is_some_and(|m| m.playing);
            if g.play_state.0 != playing_now { g.play_state = (playing_now, now); }
            let play_pop = { let e = g.play_state.1.elapsed().as_secs_f32(); if e < 0.35 { animating = true; 0.6 + 0.4 * ease_back(e / 0.35) } else { 1.0 } };
            let art_pop = { let e = g.art_at.elapsed().as_secs_f32(); if e < 0.45 { animating = true; 0.72 + 0.28 * ease_back(e / 0.45) } else { 1.0 } };
            let bump = live::VOL_BUMP.lock().unwrap().map(|t| t.elapsed().as_secs_f32()).filter(|e| *e < 0.4).map(|e| { animating = true; 1.0 - ease_out(e / 0.4) }).unwrap_or(0.0);
            // Puntos de escritorios: la píldora activa se desliza a su sitio
            if let Some(live::Hud::Desk(i, _)) = hud {
                let tx = (i as f32 - 1.0) * 12.0;
                if g.desk_x < -0.5 { g.desk_x = (live::DESK_FROM.load(Ordering::Relaxed).max(1) as f32 - 1.0) * 12.0; }
                let d = tx - g.desk_x;
                if d.abs() > 0.2 { g.desk_x += d * (1.0 - (-dt / 0.06).exp()); animating = true; } else { g.desk_x = tx; }
            } else if g.isl_prev.is_none() { g.desk_x = -1.0; }

            // Anchos (compacta, desplegada) de cada modo
            let count = timer.map(|(l, _)| live::mmss(l)).unwrap_or_default();
            let count_w = if count.is_empty() { 0.0 } else { p.width(&g.f.title, &count) };
            let ctrl_w = 3.0 * 26.0;
            let (title, artist) = media.as_ref().map(|m| (trunc(&m.title, 34), trunc(&m.artist, 22))).unwrap_or_default();
            let media_full = {
                let tw = p.width(&g.f.title, &title);
                let aw2 = if artist.is_empty() { 0.0 } else { 8.0 + p.width(&g.f.small, &artist) };
                4.0 + 22.0 + 10.0 + tw + aw2 + 12.0 + ctrl_w + 4.0
            };
            let media_compact = if timer.is_some() { 4.0 + 22.0 + 12.0 + count_w + 13.0 } else { 4.0 + 22.0 + 12.0 + 22.0 + 13.0 };
            let timer_compact = 12.0 + 16.0 + 10.0 + count_w + 14.0;
            let pomo = live::POMO.load(Ordering::Relaxed);
            let timer_lbl = match pomo { 1 => i18n::t("Concentración"), 2 => i18n::t("Descanso"), _ => i18n::t("Temporizador") };
            let pomo_dots = if pomo != 0 { 4.0 * 7.0 + 6.0 } else { 0.0 };
            let timer_full = timer_compact + p.width(&g.f.small, timer_lbl) + 8.0 + pomo_dots + 30.0;
            const HUD_W: f32 = 250.0;
            // Bandeja de archivos: iconos subidos a la GPU solo cuando cambia
            let shelf_n = live::SHELF.lock().unwrap().len();
            let sv = live::SHELF_VER.load(Ordering::Relaxed);
            if g.shelf_bmps.0 != sv {
                let shelf = live::SHELF.lock().unwrap().clone();
                let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                g.shelf_bmps = (sv, shelf.iter().map(|(_, px)| px.as_ref().and_then(|px| rt.CreateBitmap(D2D_SIZE_U { width: ART_PX, height: ART_PX }, Some(px.as_ptr() as _), ART_PX * 4, &bp).ok())).collect());
            }
            let shelf_cnt = shelf_n.to_string();
            let shelf_compact = 12.0 + 16.0 + 8.0 + p.width(&g.f.title, &shelf_cnt) + 14.0;
            let shelf_full = 12.0 + 16.0 + 10.0 + shelf_n as f32 * 30.0 + 6.0 + 26.0 + 4.0 + 26.0 + 8.0;
            let mode_size = |m: Mode| -> (f32, f32) {
                match m { Mode::Idle => (0.0, 0.0), Mode::Shelf => (shelf_compact, shelf_full), Mode::Media => (media_compact, media_full), Mode::Timer => (timer_compact, timer_full), Mode::Hud(h) => {
                    let (_, l, bar) = hud_parts(h);
                    let dots = match h { live::Hud::Desk(_, t) => 14.0 + t as f32 * 12.0, live::Hud::Mem(_) => 26.0, _ => 0.0 };
                    // Con barra: texto + hueco + barra (84) + hueco + porcentaje, para que nunca se pisen
                    let w = if bar { (38.0 + p.width(&g.f.title, &l) + 16.0 + 84.0 + 10.0 + p.width(&g.f.title, "100%") + 14.0).max(HUD_W) } else { (38.0 + p.width(&g.f.title, &l) + 18.0 + dots).max(170.0) }; (w, w) } }
            };
            let (compact_w, full_w) = mode_size(mode);
            // El destello de carga necesita fotogramas mientras se ve
            if matches!(hud, Some(live::Hud::Power(_, true) | live::Hud::LowBat(_) | live::Hud::Mic(true) | live::Hud::Mem(_) | live::Hud::Net(false))) { animating = true; }
            // Pulsar la isla la hunde un pelín (respuesta táctil)
            let pressed_k = if g.pressed.is_some_and(|a| a.in_island()) && g.isl_hover { 0.965 } else { 1.0 };
            // Desplegada y sonando: se repinta cada segundo para que avance el progreso
            if mode == Mode::Media && (g.isl_hover || g.expand_until.is_some_and(|t| now < t)) && media.as_ref().is_some_and(|m| m.playing) {
                SetTimer(Some(hwnd), TIMER_PROG, 1000, None);
            }
            let expanded = match mode {
                Mode::Media => g.isl_hover || g.expand_until.is_some_and(|t| now < t),
                Mode::Timer | Mode::Shelf => g.isl_hover,
                Mode::Hud(_) => true,
                Mode::Idle => false,
            };
            g.isl_target = if expanded { full_w } else { compact_w };
            // Muelle suave: rápido al principio y frenando al llegar, como en macOS
            // Muelle con un pelín de rebote al llegar (como la Dynamic Island)
            let d = g.isl_target - g.isl_w;
            if d.abs() < 0.3 && g.isl_v.abs() < 6.0 { g.isl_w = g.isl_target; g.isl_v = 0.0; } else {
                let (k, c) = (380.0_f32, 2.0 * 380.0_f32.sqrt() * 0.68);
                g.isl_v += (k * d - c * g.isl_v) * dt;
                g.isl_w += g.isl_v * dt;
                animating = true;
            }
            // Sin música ni avisos la isla desaparece: se encoge hasta 0 y se desvanece
            let iw = g.isl_w.max(0.0);
            let pill_a = (iw / 26.0).clamp(0.0, 1.0);
            // Fin del temporizador: la isla se sacude como una alarma (cada 1,6 s mientras se ve)
            let shake = if hud == Some(live::Hud::TimerDone) {
                animating = true;
                let ph = g.isl_mode_at.elapsed().as_secs_f32() % 1.6;
                if ph < 0.55 { (ph * 42.0).sin() * 7.0 * (1.0 - ph / 0.55) } else { 0.0 }
            } else { 0.0 };
            let ix = ((w - iw) / 2.0).round() + shake;
            let ir = rect(ix, py, ix + iw, pb);
            g.isl_rect = ir;
            // 0 = compacta, 1 = desplegada del todo (según el ancho real de ahora)
            let open_t = |m: Mode| { let (c, f) = mode_size(m); if f > c + 1.0 { ((iw - c) / (f - c)).clamp(0.0, 1.0) } else { 0.0 } };
            // Mientras cambia de ancho se estira un poco a lo alto (como la Dynamic Island al transformarse)
            let sq = (g.isl_v.abs() / 2200.0).min(1.0) * 2.5;
            let ir_draw = rect(ir.left, ir.top - sq, ir.right, ir.bottom + sq);
            if pill_a > 0.01 {
                popup::push_alpha(&rt, pill_a);
                // Sonando: el borde de la isla brilla con el color de la carátula
                if mode == Mode::Media && media.as_ref().is_some_and(|m| m.playing) {
                    let tn = g.tint_now;
                    let te = p.vgrad(py, pb, rgba(tn[0], tn[1], tn[2], 0.42), rgba(tn[0], tn[1], tn[2], 0.08));
                    p.pill(ir_draw, &black, Some(&te));
                } else if let Some(col) = match mode {
                    Mode::Hud(live::Hud::LowBat(_) | live::Hud::Net(false) | live::Hud::Mic(true)) => Some((1.0, 0.36, 0.33)),
                    Mode::Hud(live::Hud::TimerDone | live::Hud::Mem(_) | live::Hud::Disk(_)) => Some((1.0, 0.62, 0.22)),
                    Mode::Timer if timer.is_some_and(|(l, _)| l <= 10) && live::POMO.load(Ordering::Relaxed) == 0 => Some((1.0, 0.36, 0.33)),
                    _ => None,
                } {
                    // Alertas: el borde respira con el color del aviso
                    animating = true;
                    let beat = 0.5 + 0.5 * (g.t0.elapsed().as_secs_f32() * 3.2).sin();
                    let eb = p.vgrad(py, pb, rgba(col.0, col.1, col.2, 0.30 + 0.40 * beat), rgba(col.0, col.1, col.2, 0.10 + 0.15 * beat));
                    p.pill(ir_draw, &black, Some(&eb));
                } else { p.pill(ir_draw, &black, Some(&isl_edge)); }
                rt.PopLayer();
            }
            rt.PushAxisAlignedClip(&rect(ix + 1.0, py, ix + iw - 1.0, pb), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let animating_cell = std::cell::Cell::new(false);
            let draw_mode = |m: Mode, a: f32, live: bool, hits: &mut Vec<(D2D_RECT_F, Act)>, eq_rect: &mut Option<D2D_RECT_F>| {
                if a <= 0.01 { return; }
                popup::push_alpha(&rt, a);
                // Como en la Dynamic Island: el contenido nuevo crece desde el centro con un rebote suave,
                // el que se va se encoge un poco mientras se desvanece
                let sc = (if live { 0.86 + 0.14 * ease_back(mode_k) } else { 1.0 - 0.10 * (1.0 - a) }) * pressed_k;
                let icx = ix + iw / 2.0;
                let cmat = Matrix3x2 { M11: sc, M12: 0.0, M21: 0.0, M22: sc, M31: icx * (1.0 - sc) + base.M31, M32: cy * (1.0 - sc) + base.M32 };
                rt.SetTransform(&cmat);
                match m {
                    // Sin nada que enseñar: la isla no está
                    Mode::Idle => {}
                    Mode::Media => if let Some(mi) = &media {
                        let t = open_t(Mode::Media);
                        let ax = ix + 4.0;
                        match &g.art {
                            // Carátula redonda (como un disco), que entra con su "pop" al cambiar de canción
                            Some((_, bmp)) => { let s = 22.0 * art_pop; p.art(bmp, ax + (22.0 - s) / 2.0, cy - s / 2.0, s, None) }
                            None => yt_logo(ax + 11.0, cy, 10.0),
                        }
                        // En compacta: ecualizador (o la cuenta atrás si hay temporizador); se desvanece al abrirse
                        if t < 0.5 {
                            let fade = 1.0 - t * 2.0;
                            if timer.is_some() {
                                let ob = p.brush(rgba(1.0, 0.62, 0.22, fade));
                                p.text(&g.f.title, &count, ix + iw - 13.0 - count_w, &ob, None);
                            } else {
                                let bx0 = ix + iw - 13.0 - 22.0;
                                // Ecualizador del color de la carátula (como la Dynamic Island)
                                let tn = g.tint_now;
                                let wb = p.brush(rgba(tn[0], tn[1], tn[2], 0.96 * fade));
                                draw_eq(&rt, &g.eq, bx0, cy, &wb);
                                if t == 0.0 && live && a >= 1.0 { *eq_rect = Some(rect(bx0 - 1.0, sy, bx0 + 24.0, sb)); }
                            }
                        }
                        // Desplegada: título, artista y controles (atenuados si la app no los permite)
                        if t > 0.35 {
                            let k = ((t - 0.35) / 0.65).clamp(0.0, 1.0);
                            let wa = p.brush(rgba(1.0, 1.0, 1.0, 0.96 * k));
                            let da = p.brush(rgba(1.0, 1.0, 1.0, 0.52 * k));
                            let off = p.brush(rgba(1.0, 1.0, 1.0, 0.25 * k));
                            let tx = ax + 22.0 + 10.0 - (1.0 - k) * 10.0;
                            p.text(&g.f.title, &title, tx, &wa, None);
                            let tw = p.width(&g.f.title, &title);
                            if !artist.is_empty() { p.text(&g.f.small, &artist, tx + tw + 8.0, &da, None); }
                            // Progreso de la canción: una línea finísima bajo el título
                            if let Some(fr) = media_progress(mi.playing) {
                                let (l, r) = (tx, ix + iw - 4.0 - ctrl_w - 12.0);
                                if r > l + 20.0 {
                                    let yb = pb - 5.0;
                                    let tb = p.brush(rgba(1.0, 1.0, 1.0, 0.16 * k));
                                    rt.FillRoundedRectangle(&rrect(rect(l, yb - 1.0, r, yb + 1.0), 1.0), &tb);
                                    rt.FillRoundedRectangle(&rrect(rect(l, yb - 1.0, l + (r - l) * fr, yb + 1.0), 1.0), &da);
                                }
                            }
                            let mut bx = ix + iw - 4.0 - ctrl_w;
                            let play = if mi.playing { "\u{E769}" } else { "\u{E768}" };
                            for (gl, act, en) in [("\u{E892}", Act::Prev, mi.can_prev), (play, Act::Toggle, true), ("\u{E893}", Act::Next, mi.can_next)] {
                                let pp = if act == Act::Toggle { play_pop } else { 1.0 };
                                if pp < 0.999 { let (pcx, pcy) = (bx + 13.0, cy); rt.SetTransform(&(Matrix3x2 { M11: pp, M12: 0.0, M21: 0.0, M22: pp, M31: pcx * (1.0 - pp), M32: pcy * (1.0 - pp) } * cmat)); }
                                p.glyph(&g.f.icon_s, gl, bx, 26.0, if en { &wa } else { &off }, None);
                                if pp < 0.999 { rt.SetTransform(&cmat); }
                                if t > 0.9 && live && en { hits.push((rect(bx + 1.0, sy + 1.0, bx + 25.0, sb - 1.0), act)); }
                                bx += 26.0;
                            }
                            // Carátula y título: traen el reproductor al frente
                            if t > 0.9 && live { hits.push((rect(ix, py, ix + iw - 4.0 - ctrl_w, pb), Act::OpenPlayer)); }
                        }
                        // En compacta, un clic pausa o reanuda
                        if live { hits.push((ir, Act::Island)); }
                    },
                    Mode::Timer => {
                        let t = open_t(Mode::Timer);
                        // Anillo de progreso (como el temporizador de la Dynamic Island): se vacía según pasa el tiempo
                        let (rcx, rr) = (ix + 20.0, 7.5);
                        // Descanso en verde; los últimos 10 s de un temporizador normal, en rojo
                        let last10 = pomo == 0 && timer.is_some_and(|(l, _)| l <= 10);
                        let tc = if pomo == 2 { (0.30, 0.85, 0.45) } else if last10 { (1.0, 0.36, 0.33) } else { (1.0, 0.62, 0.22) };
                        let orange = p.brush(rgba(tc.0, tc.1, tc.2, 1.0));
                        let ring_bg = p.brush(rgba(tc.0, tc.1, tc.2, 0.25));
                        rt.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: rcx, Y: cy }, radiusX: rr, radiusY: rr }, &ring_bg, 2.2, None);
                        if let Some((left, total)) = timer {
                            let fr = (left as f32 / total.max(1) as f32).clamp(0.0, 1.0);
                            if fr >= 0.999 {
                                rt.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: rcx, Y: cy }, radiusX: rr, radiusY: rr }, &orange, 2.2, None);
                            } else if fr > 0.0 {
                                if let Ok(geo) = g.factory.CreatePathGeometry() {
                                    if let Ok(sk) = geo.Open() {
                                        let a = fr * std::f32::consts::TAU;
                                        let end = Vector2 { X: rcx + rr * a.sin(), Y: cy - rr * a.cos() };
                                        sk.BeginFigure(Vector2 { X: rcx, Y: cy - rr }, D2D1_FIGURE_BEGIN_HOLLOW);
                                        sk.AddArc(&D2D1_ARC_SEGMENT { point: end, size: D2D_SIZE_F { width: rr, height: rr }, rotationAngle: 0.0, sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE, arcSize: if fr > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL } });
                                        sk.EndFigure(D2D1_FIGURE_END_OPEN);
                                        let _ = sk.Close();
                                        rt.DrawGeometry(&geo, &orange, 2.2, g.round_cap.as_ref());
                                    }
                                }
                            }
                        }
                        // Los segundos ruedan hacia arriba al cambiar, como en el temporizador del iPhone
                        if g.timer_roll.borrow_mut().draw(&p, &rt, &cmat, &g.f.title, &count, ix + iw - 14.0 - count_w, sy, sb, &orange, None) { animating_cell.set(true); }
                        if t > 0.3 {
                            let k = ((t - 0.3) / 0.7).clamp(0.0, 1.0);
                            let da = p.brush(rgba(1.0, 1.0, 1.0, 0.6 * k));
                            let lx = ix + 12.0 + 16.0 + 10.0;
                            p.text(&g.f.small, timer_lbl, lx, &da, None);
                            // Pomodoro: cuatro puntos, uno por bloque de concentración de la tanda
                            if pomo != 0 {
                                let done = live::POMO_DONE.load(Ordering::Relaxed) % 4;
                                let x0 = lx + p.width(&g.f.small, timer_lbl) + 10.0;
                                for j in 0..4u8 {
                                    let on = j < done || (j == done && pomo == 1);
                                    let al = if j == done && pomo == 1 { 0.55 + 0.45 * (g.t0.elapsed().as_secs_f32() * 2.0).sin().abs() } else if on { 1.0 } else { 0.22 };
                                    if j == done && pomo == 1 { animating_cell.set(true); }
                                    let e = D2D1_ELLIPSE { point: Vector2 { X: x0 + j as f32 * 7.0, Y: cy }, radiusX: 2.4, radiusY: 2.4 };
                                    rt.FillEllipse(&e, &p.brush(rgba(tc.0, tc.1, tc.2, al * k)));
                                }
                            }
                            let sx = ix + iw - 14.0 - count_w - 8.0 - 26.0;
                            let wa = p.brush(rgba(1.0, 1.0, 1.0, 0.96 * k));
                            p.glyph(&g.f.icon_s, "\u{E711}", sx, 26.0, &wa, None);
                            if t > 0.9 && live { hits.push((rect(sx + 1.0, sy + 1.0, sx + 25.0, sb - 1.0), Act::TimerStop)); }
                        }
                        if live { hits.push((ir, Act::TimerOpen)); }
                    }
                    Mode::Shelf => {
                        // Compacta: icono y número; al pasar el ratón, los archivos (clic = abrir) y una X para vaciar
                        let t = open_t(Mode::Shelf);
                        p.glyph(&g.f.icon_s, "\u{E8B7}", ix + 12.0, 16.0, &white, None);
                        if t < 0.5 {
                            let cb = p.brush(rgba(1.0, 1.0, 1.0, 0.96 * (1.0 - t * 2.0)));
                            p.text(&g.f.title, &shelf_cnt, ix + iw - 14.0 - p.width(&g.f.title, &shelf_cnt), &cb, None);
                        }
                        if t > 0.3 {
                            let k = ((t - 0.3) / 0.7).clamp(0.0, 1.0);
                            popup::push_alpha(&rt, k);
                            for (i, b) in g.shelf_bmps.1.iter().enumerate() {
                                let x0 = ix + 12.0 + 16.0 + 10.0 + i as f32 * 30.0;
                                match b {
                                    Some(b) => p.art(b, x0 + 4.0, cy - 10.0, 20.0, Some(4.0)),
                                    None => p.glyph(&g.f.icon_s, "\u{E8A5}", x0, 28.0, &white, None),
                                }
                                if t > 0.9 && live { hits.push((rect(x0, sy + 1.0, x0 + 28.0, sb - 1.0), Act::ShelfOpen(i as u8))); }
                            }
                            let xx = ix + iw - 8.0 - 26.0;
                            // Compartir (al móvil con Enlace Móvil, Uso compartido cercano…) y vaciar
                            let xs = xx - 4.0 - 26.0;
                            p.glyph(&g.f.icon_s, "\u{E72D}", xs, 26.0, &white, None);
                            if t > 0.9 && live { hits.push((rect(xs + 1.0, sy + 1.0, xs + 25.0, sb - 1.0), Act::ShelfShare)); }
                            p.glyph(&g.f.icon_s, "\u{E711}", xx, 26.0, &dim, None);
                            if t > 0.9 && live { hits.push((rect(xx + 1.0, sy + 1.0, xx + 25.0, sb - 1.0), Act::ShelfClear)); }
                            rt.PopLayer();
                        }
                    }
                    Mode::Hud(hh) => {
                        let (gl, lbl, has_bar) = hud_parts(hh);
                        let (icon_b, fill_b) = match hh {
                            live::Hud::TimerDone => (&orange, &orange),
                            live::Hud::Power(_, true) | live::Hud::Net(true) | live::Hud::Download | live::Hud::Usb(true) | live::Hud::Full | live::Hud::Wifi | live::Hud::CamOn => (&green, &green),
                            live::Hud::MicOn | live::Hud::Disk(_) | live::Hud::Saver(true) | live::Hud::Pomo(true, _) | live::Hud::Caffeine(true) => (&orange, &orange),
                            live::Hud::Pomo(false, _) => (&green, &green),
                            live::Hud::LowBat(_) | live::Hud::Net(false) | live::Hud::Mic(true) => (&red, &red),
                            live::Hud::Claude(pc) if pc >= 90 => (&orange, &orange),
                            live::Hud::Mem(_) => (&orange, &orange),
                            _ => (&white, &white),
                        };
                        // El icono entra con un "pop" (crece con un pequeño rebote)
                        let pop = if live { 0.55 + 0.45 * ease_back(mode_k) } else { 1.0 };
                        let (icx, icy) = (ix + 21.0, cy);
                        rt.SetTransform(&Matrix3x2 { M11: pop, M12: 0.0, M21: 0.0, M22: pop, M31: icx * (1.0 - pop) + base.M31, M32: icy * (1.0 - pop) + base.M32 });
                        // Captura: su miniatura en lugar del icono
                        let shot_bmp = if matches!(hh, live::Hud::Shot) { g.hud_bmp.as_ref().map(|b| b.1.clone()) } else { None };
                        if let Some(bmp) = shot_bmp {
                            rt.SetTransform(&cmat);
                            p.art(&bmp, ix + 8.0, cy - 11.0, 22.0, Some(5.0));
                        } else if let live::Hud::Color(c) = hh {
                            // Cuentagotas: una gota del color elegido con un aro fino
                            let (r, gg, b) = (((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0);
                            let e = D2D1_ELLIPSE { point: Vector2 { X: icx, Y: icy }, radiusX: 8.0, radiusY: 8.0 };
                            rt.FillEllipse(&e, &p.brush(rgba(r, gg, b, 1.0)));
                            rt.DrawEllipse(&e, &p.brush(rgba(1.0, 1.0, 1.0, 0.35)), 1.2, None);
                        } else if matches!(hh, live::Hud::Copied) {
                            // Copiado: un círculo verde y una marca que se dibuja de un trazo
                            let e = D2D1_ELLIPSE { point: Vector2 { X: icx, Y: icy }, radiusX: 8.5, radiusY: 8.5 };
                            rt.FillEllipse(&e, &green);
                            let k = if live { ((g.isl_mode_at.elapsed().as_secs_f32() - 0.08) / 0.30).clamp(0.0, 1.0) } else { 1.0 };
                            if k < 1.0 { animating_cell.set(true); }
                            let pts = [(icx - 4.0, icy + 0.2), (icx - 1.2, icy + 3.0), (icx + 4.2, icy - 3.0)];
                            let l1 = ((pts[1].0 - pts[0].0).powi(2) + (pts[1].1 - pts[0].1).powi(2)).sqrt();
                            let l2 = ((pts[2].0 - pts[1].0).powi(2) + (pts[2].1 - pts[1].1).powi(2)).sqrt();
                            let d = ease_out(k) * (l1 + l2);
                            let v = |a: (f32, f32)| Vector2 { X: a.0, Y: a.1 };
                            let lerp = |a: (f32, f32), b: (f32, f32), t: f32| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                            if d > 0.01 {
                                rt.DrawLine(v(pts[0]), v(lerp(pts[0], pts[1], (d / l1).min(1.0))), &black, 2.0, g.round_cap.as_ref());
                                if d > l1 { rt.DrawLine(v(pts[1]), v(lerp(pts[1], pts[2], ((d - l1) / l2).min(1.0))), &black, 2.0, g.round_cap.as_ref()); }
                            }
                        } else if matches!(hh, live::Hud::Claude(_)) {
                            if let Some(geo) = &g.claude_geo {
                                let k = 15.0 / 125.0;
                                let m0 = Matrix3x2 { M11: pop * k, M12: 0.0, M21: 0.0, M22: pop * k, M31: icx * (1.0 - pop) + base.M31 + pop * (icx - 63.5 * k), M32: icy * (1.0 - pop) + base.M32 + pop * (icy - 62.5 * k) };
                                rt.SetTransform(&m0);
                                rt.FillGeometry(geo, &white, None);
                            }
                        } else {
                            // Avisos de alerta: el icono late suavemente
                            let alert = matches!(hh, live::Hud::LowBat(_) | live::Hud::Mic(true) | live::Hud::Mem(_) | live::Hud::Net(false) | live::Hud::TimerDone);
                            if alert {
                                let beat = 0.55 + 0.45 * (g.t0.elapsed().as_secs_f32() * 5.5).cos().abs();
                                popup::push_alpha(&rt, beat);
                                p.glyph(&g.f.icon_s, gl, ix + 12.0, 18.0, icon_b, None);
                                rt.PopLayer();
                            } else { p.glyph(&g.f.icon_s, gl, ix + 12.0, 18.0, icon_b, None); }
                        }
                        rt.SetTransform(&cmat);
                        // El texto entra deslizándose un poco desde la izquierda
                        let slide = if live { (1.0 - mode_k) * -6.0 } else { 0.0 };
                        p.text(&g.f.title, &lbl, ix + 38.0 + slide, &white, None);
                        // Memoria: anillo radial pequeño que se llena hasta el % de uso (hud_val ya viene animado)
                        if matches!(hh, live::Hud::Mem(_)) {
                            let (rcx, rr, sw) = (ix + iw - 14.0 - 8.0, 7.0, 2.4);
                            let ring_bg = p.brush(rgba(1.0, 0.62, 0.22, 0.25));
                            rt.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: rcx, Y: cy }, radiusX: rr, radiusY: rr }, &ring_bg, sw, None);
                            let fr = g.hud_val.clamp(0.0, 1.0);
                            if fr >= 0.999 {
                                rt.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: rcx, Y: cy }, radiusX: rr, radiusY: rr }, &orange, sw, None);
                            } else if fr > 0.005 {
                                if let Ok(geo) = g.factory.CreatePathGeometry() {
                                    if let Ok(sk) = geo.Open() {
                                        let a = fr * std::f32::consts::TAU;
                                        sk.BeginFigure(Vector2 { X: rcx, Y: cy - rr }, D2D1_FIGURE_BEGIN_HOLLOW);
                                        sk.AddArc(&D2D1_ARC_SEGMENT { point: Vector2 { X: rcx + rr * a.sin(), Y: cy - rr * a.cos() }, size: D2D_SIZE_F { width: rr, height: rr }, rotationAngle: 0.0, sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE, arcSize: if fr > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL } });
                                        sk.EndFigure(D2D1_FIGURE_END_OPEN);
                                        let _ = sk.Close();
                                        rt.DrawGeometry(&geo, &orange, sw, g.round_cap.as_ref());
                                    }
                                }
                            }
                        }
                        if has_bar {
                            // El número acompaña a la barra mientras se desliza (cuenta en vez de saltar)
                            let pct = format!("{}%", (g.hud_val.clamp(0.0, 1.0) * 100.0).round() as i32);
                            let pw = p.width(&g.f.title, "100%");
                            let tr = ix + iw - 14.0;
                            p.text(&g.f.title, &pct, tr - p.width(&g.f.title, &pct), &white, None);
                            let (bl, br) = (tr - pw - 10.0 - 84.0, tr - pw - 10.0);
                            let track = p.brush(rgba(1.0, 1.0, 1.0, 0.18));
                            // Al cambiar el volumen la barra engorda un instante, como el HUD de macOS
                            let bh = 2.5 + if matches!(hh, live::Hud::Volume(..)) { 1.6 * bump } else { 0.0 };
                            rt.FillRoundedRectangle(&rrect(rect(bl, cy - bh, br, cy + bh), bh), &track);
                            let fx = bl + (br - bl) * g.hud_val.clamp(0.0, 1.0);
                            if fx > bl + 0.5 { rt.FillRoundedRectangle(&rrect(rect(bl, cy - bh, fx.max(bl + bh * 2.0), cy + bh), bh), fill_b); }
                            // Cargando: un destello recorre la barra, como la animación de carga del iPhone
                            if matches!(hh, live::Hud::Power(_, true)) && fx > bl + 8.0 {
                                let ph = (g.t0.elapsed().as_secs_f32() * 0.9).fract();
                                let sx = bl - 18.0 + (fx - bl + 36.0) * ph;
                                let sh2 = p.brush(rgba(1.0, 1.0, 1.0, 0.55));
                                rt.PushAxisAlignedClip(&rect(bl, cy - 3.0, fx, cy + 3.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                                rt.FillRoundedRectangle(&rrect(rect(sx - 6.0, cy - 2.5, sx + 6.0, cy + 2.5), 2.5), &sh2);
                                rt.PopAxisAlignedClip();
                            }
                        }
                        // Escritorios: un punto por escritorio y una píldora que se desliza hasta el actual
                        if let live::Hud::Desk(_, t) = hh {
                            let x0 = ix + iw - 16.0 - t as f32 * 12.0 + 6.0;
                            let faint = p.brush(rgba(1.0, 1.0, 1.0, 0.28));
                            for j in 0..t {
                                let e = D2D1_ELLIPSE { point: Vector2 { X: x0 + j as f32 * 12.0, Y: cy }, radiusX: 3.0, radiusY: 3.0 };
                                rt.FillEllipse(&e, &faint);
                            }
                            let ax = x0 + g.desk_x.max(0.0);
                            rt.FillRoundedRectangle(&rrect(rect(ax - 6.0, cy - 3.0, ax + 6.0, cy + 3.0), 3.0), &white);
                        }
                        if live { hits.push((ir, Act::HudDismiss)); }
                    }
                }
                rt.SetTransform(&base);
                rt.PopLayer();
            };
            // El contenido anterior se desvanece mientras entra el nuevo
            if let Some(pm) = g.isl_prev { draw_mode(pm, 1.0 - mode_k, false, &mut hits, &mut eq_rect); }
            draw_mode(mode, mode_k, true, &mut hits, &mut eq_rect);
            if animating_cell.get() { animating = true; }
            rt.PopAxisAlignedClip();
            // Puntos de privacidad (como en macOS), junto a la isla: verde = cámara, naranja = micrófono
            let mut dx = ir.right + 9.0;
            for (on, key, br) in [(live::CAM.load(Ordering::Relaxed), "cam", &green), (live::MIC.load(Ordering::Relaxed), "mic", &orange)] {
                if !on { continue; }
                let k = appear(key, &mut animating);
                let e = D2D1_ELLIPSE { point: Vector2 { X: dx, Y: cy }, radiusX: 3.2 * k, radiusY: 3.2 * k };
                rt.FillEllipse(&e, br);
                dx += 10.0;
            }
            let _ = (&glass, &edge, &soft, &dim, &sub_fill, &sub_edge);

            let (white, soft, dim, shadow) = (bar_white.clone(), bar_soft.clone(), bar_dim.clone(), bar_shadow.clone());
            let sh = Some(&shadow);
            // ============ DERECHA (de derecha a izquierda) ============
            let mut rx = w - 11.0;
            // Fecha y hora
            let clock = clock_text();
            let cw = p.width(&g.f.bold, &clock);
            rx -= cw;
            // Al cambiar el minuto, los caracteres que cambian ruedan: el viejo sube y se va, el nuevo entra desde abajo
            if g.clock_roll.draw(&p, &rt, &base, &g.f.bold, &clock, rx, sy, sb, &white, sh) { animating = true; }
            hits.push((rect(rx - 8.0, sy, rx + cw + 8.0, sb), Act::Clock));
            rx -= 26.0;
            // Usuario: avatar + nombre
            let user = g.user.clone();
            let uw = p.width(&g.f.mid, &user);
            rx -= uw;
            p.text(&g.f.mid, &user, rx, &white, sh);
            rx -= 28.0;
            // Foto de la cuenta de Windows; si no hay, círculo con inicial
            if g.avatar.is_none() {
                if let Some(px) = &g.avatar_px {
                    let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                    g.avatar = rt.CreateBitmap(D2D_SIZE_U { width: ART_PX, height: ART_PX }, Some(px.as_ptr() as _), ART_PX * 4, &bp).ok();
                }
            }
            let ay = p.snap(cy - 11.0);
            match &g.avatar {
                Some(bmp) => {
                    p.art(bmp, p.snap(rx), ay, 22.0, None);
                    let ring = D2D1_ELLIPSE { point: Vector2 { X: p.snap(rx) + 11.0, Y: ay + 11.0 }, radiusX: 11.0, radiusY: 11.0 };
                    let rb = p.brush(ic(0.22));
                    rt.DrawEllipse(&ring, &rb, 1.0 / scale, None);
                }
                None => {
                    let av = D2D1_ELLIPSE { point: Vector2 { X: rx + 11.0, Y: cy }, radiusX: 11.0, radiusY: 11.0 };
                    let av_b = p.dgrad(rx, cy - 11.0, rx + 22.0, cy + 11.0, rgba(0.98, 0.55, 0.30, 1.0), rgba(0.62, 0.30, 0.86, 1.0));
                    rt.FillEllipse(&av, &av_b);
                    let initial: String = user.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                    p.glyph(&g.f.title, &initial, rx, 22.0, &white, None);
                }
            }
            hits.push((rect(rx - 5.0, sy, rx + 28.0 + uw + 6.0, sb), Act::User));
            rx -= 22.0;
            // Tiempo: entra con un fundido y su hueco se abre poco a poco (sin saltos del resto)
            if let Some(wt) = *WEATHER.lock().unwrap() {
                let k = appear("weather", &mut animating);
                let temp = format!("{}°C", wt.temp.round() as i32);
                let tw = p.width(&g.f.bold, &temp);
                let chip_w = 9.0 + 14.0 + 5.0 + tw + 10.0;
                let right = rx;
                rx -= chip_w * k;
                // Mientras entra se va destapando desde la derecha (no pisa al avatar)
                rt.PushAxisAlignedClip(&rect(rx, 0.0, right, h), D2D1_ANTIALIAS_MODE_ALIASED);
                popup::push_alpha(&rt, k);
                let cx0 = right - chip_w;
                p.pill(rect(cx0, sy, right, sb), &sub_fill, Some(&sub_edge));
                weather::draw_icon(&rt, &p, &g.f.icon_s, cx0 + 8.0, 0.0, 16.0, h, wt.code, wt.is_day, &white);
                if g.temp_roll.draw(&p, &rt, &base, &g.f.bold, &temp, cx0 + 28.0, sy, sb, &white, None) { animating = true; }
                rt.PopLayer();
                rt.PopAxisAlignedClip();
                hits.push((rect(rx, sy, rx + chip_w, sb), Act::Weather));
                rx -= 8.0 * k;
            }
            // Iconos de estado (ranuras de 34), de derecha a izquierda: red, batería, iconos ocultos,
            // búsqueda, Claude, escritorio. Los inactivos (sin conexión, Claude cerrado) van atenuados.
            let off_b = p.brush(ic(0.45));
            let bat = battery();
            let bat_glyph = bat.map(|(pct, ch)| {
                let n = (pct as u32 / 10).min(10);
                let c = if ch { match n { 10 => 0xEA93, 9 => 0xE83E, _ => 0xE85A + n } } else if pct >= 95 { 0xE83F } else { 0xE850 + n.min(9) };
                char::from_u32(c).unwrap().to_string()
            });
            let net = *live::NET.lock().unwrap();
            let (net_g, net_ok) = live::net_glyph(&net);
            // De derecha a izquierda: escritorio, red, batería, iconos ocultos, búsqueda, Claude
            let mut icons: Vec<(String, Act, bool)> = vec![("\u{E7F4}".into(), Act::Desktop, true), (net_g.into(), Act::Wifi, net_ok)];
            if let Some(b) = bat_glyph { icons.push((b, Act::Battery, true)); }
            let tray_open = tray::is_open();
            icons.extend([
                // Flecha hacia abajo (el panel se abre hacia abajo); gira 180° al desplegarse
                ("\u{E70D}".to_string(), Act::Tray, true),
                ("\u{E721}".into(), Act::Search, true),
                (String::new(), Act::Claude, CLAUDE_UP.load(Ordering::Relaxed)),
            ]);
            // Cafeína encendida: una taza a la izquierda de todo (clic = apagarla)
            if live::caffeine_on() { icons.push(("\u{EC32}".into(), Act::Caffeine, true)); }
            // Uso semanal de Claude, en texto pegado a la derecha de su icono (p. ej. "36%")
            // El porcentaje "cuenta" hasta el valor nuevo en vez de saltar
            if let Some((pct, _)) = g.usage {
                let d = pct - g.usage_disp;
                if d.abs() < 0.5 { g.usage_disp = pct; } else { g.usage_disp += d * (1.0 - (-dt / 0.22).exp()); animating = true; }
            }
            let usage_txt = g.usage.map(|_| format!("{}%", g.usage_disp.round() as i32));
            for (gl, act, on) in icons {
                let ib = if on { &white } else { &off_b };
                rx -= 34.0;
                let mut hit_r = rx + 30.0;
                if act == Act::Claude {
                    if let Some(t) = &usage_txt {
                        let k = appear("usage", &mut animating);
                        let tw = p.width(&g.f.bold, t);
                        // El icono se corre a la izquierda para dejar sitio al porcentaje
                        rx -= (tw + 1.0) * k;
                        rt.PushAxisAlignedClip(&rect(rx + 30.0, 0.0, rx + 31.0 + tw * k + 2.0, h), D2D1_ANTIALIAS_MODE_ALIASED);
                        popup::push_alpha(&rt, k);
                        p.text(&g.f.bold, t, rx + 31.0, ib, sh);
                        rt.PopLayer();
                        rt.PopAxisAlignedClip();
                        hit_r = rx + 31.0 + tw * k + 4.0;
                    }
                    // Logotipo oficial de Claude (trazado del SVG), a 17 DIP
                    if g.claude_geo.is_none() { g.claude_geo = claude_geometry(&g.factory); }
                    if let Some(geo) = &g.claude_geo {
                        // Relleno y con picos: a 15,5 DIP pesa a la vista lo mismo que los glifos de 16
                        let size = ICON_PX - 0.5;
                        let k = size / 125.0;
                        let (cx, cyy) = (rx + 17.0, cy);
                        let tx = p.snap(cx - 63.5 * k);
                        let ty = p.snap(cyy - 62.5 * k);
                        let m = |dy: f32| Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: tx, M32: ty + dy + base.M32 };
                        rt.SetTransform(&m(1.0));
                        rt.FillGeometry(geo, &shadow, None);
                        rt.SetTransform(&m(0.0));
                        rt.FillGeometry(geo, ib, None);
                        rt.SetTransform(&base);
                    }
                } else {
                    // Pulsado: el icono se encoge un poco, como un botón físico
                    let mut sc = if g.pressed == Some(act) && g.hover == Some(act) { 0.86 } else { 1.0 };
                    // Mismo tamaño visible para todos: cada glifo de Segoe Fluent tiene su propia caja
                    // (la flecha es pequeña, la batería ancha…), así que se escala por su tinta real
                    sc *= icon_norm(&p, &g.f.icon, &gl);
                    // La taza de la cafeína entra creciendo con un pequeño rebote
                    if act == Act::Caffeine { let k = appear("caffeine", &mut animating); sc *= 0.4 + 0.6 * ease_back(k); }
                    // Si el icono cambia (batería que sube, señal Wi-Fi, sin conexión…), el viejo se funde
                    // y el nuevo entra con un pequeño "pop"
                    let mo = g.morph.entry(act).or_insert_with(|| Morph { prev: gl.clone(), cur: gl.clone(), at: now - Duration::from_secs(5) });
                    if mo.cur != gl { mo.prev = std::mem::replace(&mut mo.cur, gl.clone()); mo.at = now; }
                    let mk = (mo.at.elapsed().as_secs_f32() / 0.32).min(1.0);
                    let fading = if mk < 1.0 && mo.prev != mo.cur { animating = true; sc *= 0.78 + 0.22 * ease_back(mk); Some((mo.prev.clone(), mk)) } else { None };
                    let (pcx, pcy) = (rx + 17.0, cy);
                    // La flecha de iconos ocultos gira con suavidad al abrir y cerrar su panel
                    let ang = if act == Act::Tray {
                        let target = if tray_open { std::f32::consts::PI } else { 0.0 };
                        let d = target - g.tray_rot;
                        if d.abs() < 0.01 { g.tray_rot = target; } else { g.tray_rot += d * (1.0 - (-dt / 0.06).exp()); animating = true; }
                        g.tray_rot
                    } else { 0.0 };
                    let (cs, sn) = (ang.cos() * sc, ang.sin() * sc);
                    rt.SetTransform(&Matrix3x2 { M11: cs, M12: sn, M21: -sn, M22: cs, M31: pcx - cs * pcx + sn * pcy, M32: pcy - sn * pcx - cs * pcy + base.M32 });
                    match &fading {
                        Some((old, k)) => {
                            popup::push_alpha(&rt, ease_out(*k));
                            p.glyph(&g.f.icon, &gl, rx, 34.0, ib, sh);
                            rt.PopLayer();
                            rt.SetTransform(&base);
                            popup::push_alpha(&rt, 1.0 - ease_out(*k * 1.6));
                            p.glyph(&g.f.icon, old, rx, 34.0, ib, sh);
                            rt.PopLayer();
                        }
                        None => p.glyph(&g.f.icon, &gl, rx, 34.0, ib, sh),
                    }
                    rt.SetTransform(&base);
                    // Un icono oculto pidió atención (aviso nuevo): punto azul junto a la flecha
                    if act == Act::Tray && !tray_open && trayhost::needs_attention() {
                        let e = D2D1_ELLIPSE { point: Vector2 { X: pcx + 7.5, Y: pcy - 7.0 }, radiusX: 3.0, radiusY: 3.0 };
                        rt.FillEllipse(&e, &p.brush(crate::profiles::accent(1.0)));
                    }
                }
                hits.push((rect(rx + 4.0, sy, hit_r, sb), act));
            }
            let _ = &sep;
            let _ = &hover_b;

            // ---- Resaltados animados: ratón encima, pulsado y abierto (menú o panel)
            let cc_open = ccenter::is_open();
            let panel = popup::open_kind();
            let is_active = |a: Act| g.open == Some(a) || match a {
                Act::Logo => cc_open,
                Act::Tray => tray_open,
                Act::Clock => panel == Some(popup::Kind::Calendar),
                Act::Weather => panel == Some(popup::Kind::Weather),
                Act::Wifi => panel == Some(popup::Kind::Wifi),
                _ => false,
            };
            // El brillo de "ratón encima" es UNA píldora que se desliza de un botón a otro (muelle suave)
            // en vez de apagarse en uno y encenderse en el siguiente
            let hover_r = g.hover.filter(|a| !a.in_island()).and_then(|a| hits.iter().find(|(_, x)| *x == a).map(|(r, _)| *r));
            {
                let (gr, ga) = g.glide;
                let ta = if hover_r.is_some() { 0.13 } else { 0.0 };
                let tr = hover_r.unwrap_or(gr);
                // Si no se veía, aparece directamente en su sitio
                let mut nr = if ga < 0.01 || (tr.left - gr.left).abs() > 260.0 { tr } else { gr };
                let k = 1.0 - (-dt / 0.055).exp();
                let mut moved = false;
                for (c, t) in [(&mut nr.left, tr.left), (&mut nr.right, tr.right), (&mut nr.top, tr.top), (&mut nr.bottom, tr.bottom)] {
                    if (*c - t).abs() > 0.3 { *c += (t - *c) * k; moved = true; } else { *c = t; }
                }
                let tau = if ta > ga { 0.04 } else { 0.11 };
                let na = if (ga - ta).abs() < 0.004 { ta } else { moved = true; ga + (ta - ga) * (1.0 - (-dt / tau).exp()) };
                if moved { animating = true; }
                g.glide = (nr, na);
                if na > 0.004 {
                    let rr = rect(p.snap(nr.left), p.snap(nr.top), p.snap(nr.right), p.snap(nr.bottom));
                    let b = p.brush(ic(na));
                    rt.FillRoundedRectangle(&rrect(rr, (rr.bottom - rr.top) / 2.0), &b);
                }
            }
            let mut hl = std::mem::take(&mut g.hl);
            for (r, a) in hits.iter() {
                if a.in_island() { continue; }
                let hov = g.hover == Some(*a);
                let target = if g.pressed == Some(*a) && hov { 0.13 } else if is_active(*a) { if hov { 0.07 } else { 0.20 } } else { 0.0 };
                let cur = hl.entry(*a).or_insert(0.0);
                // Se enciende rápido y se apaga algo más despacio, como en macOS
                let tau = if target > *cur { 0.04 } else { 0.11 };
                if (*cur - target).abs() < 0.004 { *cur = target; } else { *cur += (target - *cur) * (1.0 - (-dt / tau).exp()); animating = true; }
                if *cur > 0.004 {
                    let rr = rect(p.snap(r.left), p.snap(r.top), p.snap(r.right), p.snap(r.bottom));
                    let rad = (rr.bottom - rr.top) / 2.0;
                    let b = p.brush(ic(*cur));
                    rt.FillRoundedRectangle(&rrect(rr, rad), &b);
                }
            }
            hl.retain(|a, _| hits.iter().any(|(_, x)| x == a));
            g.hl = hl;

            rt.PopLayer();
            rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 });
            // Lo que ya no se dibuja vuelve a entrar con fundido si reaparece
            born.retain(|k, _| seen.contains(k));
            g.born = born;
            g.hits = hits;
            g.eq_visible = eq_rect.is_some();
            g.eq_rect = eq_rect;
            if animating { SetTimer(Some(hwnd), TIMER_FX, 15, None); }
            if let Err(e) = rt.EndDraw(None, None) { log_draw_fail("barra", &e); g.target = None; g.art = None; g.avatar = None; }
        });
        let _ = EndPaint(hwnd, &ps);
    }
}

/// Avanza el ecualizador con el pico real del dispositivo de audio.
fn step_eq() {
    GFX.with(|g| {
        let mut g = g.borrow_mut();
        let Some(g) = g.as_mut() else { return };
        if g.meter.is_none() {
            g.meter = unsafe {
                CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .and_then(|en| en.GetDefaultAudioEndpoint(eRender, eConsole))
                    .and_then(|d| d.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None))
                    .ok()
            };
        }
        let peak = g.meter.as_ref().and_then(|m| unsafe { m.GetPeakValue().ok() });
        if peak.is_none() { g.meter = None; }
        let playing = MEDIA.lock().unwrap().as_ref().is_some_and(|m| m.playing);
        let demo = std::env::var_os("MACBAR_DEMO").is_some();
        let peak = if demo { Some(0.6) } else { peak };
        let level = if playing { peak.unwrap_or(0.5).powf(0.55) } else { 0.0 };
        let t = g.t0.elapsed().as_secs_f32();
        for (i, v) in g.eq.iter_mut().enumerate() {
            let fi = i as f32;
            let wave = ((t * (6.0 + fi * 1.7) + fi * 2.1).sin() * 0.5 + 0.5) * 0.6 + 0.4;
            let target = level * wave;
            *v += (target - *v) * 0.55;
        }
    });
}

/// Redibuja solo las barras del ecualizador. Devuelve false si hace falta un repintado completo.
fn paint_eq() -> bool {
    GFX.with(|g| {
        let mut g = g.borrow_mut();
        let Some(g) = g.as_mut() else { return false };
        if g.hover == Some(Act::Island) { return false; }
        let (Some(rt), Some(r)) = (g.target.clone(), g.eq_rect) else { return false };
        unsafe {
            let black: ID2D1Brush = rt.CreateSolidColorBrush(&rgba(0.0, 0.0, 0.0, 1.0), None).unwrap().into();
            // Mismo color que en el pintado completo (el de la carátula): antes este repintado rápido lo
            // dibujaba blanco y el ecualizador parpadeaba entre los dos colores
            let tn = g.tint_now;
            let bars: ID2D1Brush = rt.CreateSolidColorBrush(&rgba(tn[0], tn[1], tn[2], 0.96), None).unwrap().into();
            rt.BeginDraw();
            rt.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
            rt.FillRectangle(&r, &black);
            draw_eq(&rt, &g.eq, r.left + 1.0, BAR_H / 2.0, &bars);
            rt.PopAxisAlignedClip();
            if let Err(e) = rt.EndDraw(None, None) { log_draw_fail("barra (eq)", &e); g.target = None; return false; }
        }
        true
    })
}

/// Foto de la cuenta ya cargada por la barra (la usa también el centro de control).
fn avatar_pixels() -> Option<Vec<u8>> { GFX.with(|g| g.borrow().as_ref().and_then(|g| g.avatar_px.clone())) }

/// Si cambias la foto en Configuración > Cuentas, la barra la recoge sola.
fn refresh_avatar() {
    let path = sys::account_picture(64);
    // Ruta + fecha de modificación: UserImage.jpg mantiene el nombre aunque cambies la foto
    let key = path.as_ref().map(|p| {
        let t = std::fs::metadata(p).and_then(|m| m.modified()).ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        format!("{p}|{t}")
    });
    GFX.with(|g| {
        if let Some(g) = g.borrow_mut().as_mut() {
            if g.avatar_path != key {
                g.avatar_px = path.as_deref().and_then(load_file_square);
                g.avatar_path = key;
                g.avatar = None;
            }
        }
    });
}

fn refresh_foreground() {
    let fg = unsafe { GetForegroundWindow() };
    // La propia barra (al abrir un menú) no cuenta como app activa
    // Ninguna ventana de la propia barra (menús, centro de control) cuenta como app activa
    if fg.0.is_null() || sys::window_pid(fg) == std::process::id() { return; }
    LAST_FG.store(fg.0 as isize, Ordering::Relaxed);
    GFX.with(|g| {
        if let Some(g) = g.borrow_mut().as_mut() {
            if let Some(n) = app_name_of(fg) {
                // Cambio de app: el nombre viejo se queda para el fundido cruzado
                if n != g.app_name { g.app_prev = std::mem::replace(&mut g.app_name, n); g.app_at = Instant::now(); }
            }
        }
    });
}

/// ¿La ventana activa ocupa todo su monitor (vídeo, juego, F11)? El escritorio no cuenta.
fn real_fullscreen() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() || fg == bar_hwnd() || sys::is_shell_window(fg) { return false; }
        // Una ventana MAXIMIZADA normal (con barra de título) nunca es pantalla completa, aunque tape
        // todo el monitor: pasa justo cuando Windows olvida el hueco de la barra (p. ej. al suspender),
        // y tomarla por pantalla completa escondería la barra para siempre.
        let style = GetWindowLongW(fg, GWL_STYLE) as u32;
        if IsZoomed(fg).as_bool() && style & WS_CAPTION.0 == WS_CAPTION.0 { return false; }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_err() { return false; }
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(mon, &mut mi).as_bool() { return false; }
        let m = mi.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

/// Oculta la barra solo con una app de verdad a pantalla completa; la muestra en cuanto deja de haberla.
fn sync_fullscreen(hwnd: HWND) {
    let full = real_fullscreen();
    if FULLSCREEN.swap(full, Ordering::Relaxed) == full { return; }
    unsafe {
        if full {
            // Fuera del todo: oculta y sin ser "siempre encima", no tapa ni recibe clics
            let _ = KillTimer(Some(hwnd), TIMER_KEEP);
            let _ = SetWindowPos(hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_HIDEWINDOW);
        } else {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            // Al salir de pantalla completa (juegos que cambian la resolución…) el hueco puede haberse perdido
            if !reserved_ok(hwnd) { reserve(hwnd); }
        }
    }
}

/// Win+D y "mostrar escritorio" esconden las ventanas siempre-encima: la volvemos a mostrar.
fn keep_visible() {
    // Con una app a pantalla completa la barra se queda oculta: nunca competir por estar encima
    if FULLSCREEN.load(Ordering::Relaxed) { return; }
    // "Mostrar escritorio" también esconde los widgets: vuelven a su sitio sobre el fondo
    desk::restack();
    let bar = bar_hwnd();
    unsafe {
        let vis = IsWindowVisible(bar).as_bool();
        let ic = IsIconic(bar).as_bool();
        let mut cloak = 0u32;
        let _ = DwmGetWindowAttribute(bar, DWMWA_CLOAKED, &mut cloak as *mut _ as _, 4);
        if let Some(p) = std::env::var_os("MACBAR_LOG") {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = writeln!(f, "keep_visible: visible={vis} iconic={ic} cloaked={cloak}");
            }
        }
        if ic { let _ = ShowWindow(bar, SW_RESTORE); }
        if !vis { let _ = ShowWindow(bar, SW_SHOWNOACTIVATE); }
        let _ = SetWindowPos(bar, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

fn hit_at(lp: LPARAM, hwnd: HWND) -> Option<Act> {
    let s = scale_of(hwnd);
    let x = (lp.0 & 0xFFFF) as i16 as f32 / s;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / s;
    GFX.with(|g| {
        g.borrow().as_ref().and_then(|g| {
            g.hits.iter().find(|(r, _)| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom).map(|(_, a)| *a)
        })
    })
}

unsafe extern "system" fn win_event(_: HWINEVENTHOOK, event: u32, hwnd: HWND, id_object: i32, _: i32, _: u32, _: u32) {
    // Cambio de escritorio de seguridad (bloquear/desbloquear, UAC): al volver, se pide el hueco otra vez
    if event == EVENT_SYSTEM_DESKTOPSWITCH {
        unsafe { SetTimer(Some(bar_hwnd()), TIMER_RESERVE, 1200, None); }
        return;
    }
    if event == EVENT_OBJECT_NAMECHANGE && (id_object != 0 || hwnd != unsafe { GetForegroundWindow() }) { return; }
    if event == EVENT_SYSTEM_FOREGROUND { sync_fullscreen(bar_hwnd()); }
    if event == EVENT_SYSTEM_FOREGROUND && sys::is_shell_window(hwnd) {
        keep_visible();
        // Windows esconde las ventanas en varias fases: reafirmamos durante ~1 s
        KEEP_LEFT.store(8, Ordering::Relaxed);
        unsafe { SetTimer(Some(bar_hwnd()), TIMER_KEEP, 120, None); }
    }
    refresh_foreground();
    unsafe { let _ = InvalidateRect(Some(bar_hwnd()), None, false); }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => { paint(hwnd); LRESULT(0) }
            WM_ERASEBKGND => LRESULT(1),
            WM_TIMER => {
                // Un fotograma más de animación (el pintado vuelve a pedirlo mientras algo se mueva)
                if wp.0 == TIMER_COLLAPSE || wp.0 == TIMER_FX || wp.0 == TIMER_HUD || wp.0 == TIMER_PROG {
                    let _ = KillTimer(Some(hwnd), wp.0);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return LRESULT(0);
                }
                if wp.0 == TIMER_RESERVE { let _ = KillTimer(Some(hwnd), TIMER_RESERVE); sync_fullscreen(hwnd); if !FULLSCREEN.load(Ordering::Relaxed) { reserve(hwnd); } return LRESULT(0); }
                if wp.0 == TIMER_GUARD { guard(hwnd); return LRESULT(0); }
                if wp.0 == TIMER_COUNT {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return LRESULT(0);
                }
                if wp.0 == TIMER_FOCUS {
                    let _ = KillTimer(Some(hwnd), TIMER_FOCUS);
                    let fg = HWND(FOCUS_TARGET.load(Ordering::Relaxed) as *mut _);
                    if GetForegroundWindow() == hwnd {
                        let usable = !fg.0.is_null() && IsWindow(Some(fg)).as_bool() && IsWindowVisible(fg).as_bool() && !IsIconic(fg).as_bool();
                        let target = if usable { fg } else {
                            sys::app_windows().into_iter().find(|h| *h != hwnd && *h != fg && IsWindowVisible(*h).as_bool() && !IsIconic(*h).as_bool())
                                .unwrap_or_else(|| FindWindowW(w!("Progman"), None).unwrap_or_default())
                        };
                        if !target.0.is_null() { let _ = SetForegroundWindow(target); }
                    }
                    return LRESULT(0);
                }
                if wp.0 == TIMER_KEEP {
                    keep_visible();
                    if KEEP_LEFT.fetch_sub(1, Ordering::Relaxed) <= 1 { let _ = KillTimer(Some(hwnd), TIMER_KEEP); }
                    return LRESULT(0);
                }
                if wp.0 == TIMER_ANIM {
                    step_eq();
                    // Sin ecualizador a la vista (isla desplegada, aviso, temporizador): no hace falta repintar
                    if !GFX.with(|g| g.borrow().as_ref().is_some_and(|g| g.eq_visible)) { return LRESULT(0); }
                    if paint_eq() { return LRESULT(0); }
                } else {
                    refresh_foreground(); refresh_avatar(); sync_fullscreen(hwnd);
                    if !reserved_ok(hwnd) { reserve(hwnd); }
                    // Batería baja: aviso en la isla al bajar del 20 %, 10 % y 5 % (sin cargador)
                    if let Some((pct, ac)) = battery() {
                        let warn = GFX.with(|g| g.borrow_mut().as_mut().and_then(|g| {
                            if ac { g.low_warned = 100; return None; }
                            let th = [5u8, 10, 20].into_iter().find(|t| pct <= *t && g.low_warned > *t)?;
                            g.low_warned = th;
                            Some(pct)
                        }));
                        if let Some(p) = warn { live::show(live::Hud::LowBat(p), 4500); }
                        // Llena con el cargador puesto: se avisa una vez (vuelve a avisar tras desenchufar)
                        let full = GFX.with(|g| g.borrow_mut().as_mut().is_some_and(|g| {
                            if !ac { g.full_warned = false; return false; }
                            if pct >= 100 && !g.full_warned { g.full_warned = true; return true; }
                            false
                        }));
                        if full { live::show(live::Hud::Full, 3000); }
                    }
                    // Ahorro de batería que se enciende o se apaga
                    let saver = { let mut s = SYSTEM_POWER_STATUS::default(); unsafe { GetSystemPowerStatus(&mut s).is_ok() }.then_some(s.SystemStatusFlag == 1) };
                    if let Some(on) = saver {
                        let ch = GFX.with(|g| g.borrow_mut().as_mut().map(|g| { let c = g.saver.is_some_and(|l| l != on); g.saver = Some(on); c }).unwrap_or(false));
                        if ch { live::show(live::Hud::Saver(on), 2400); }
                    }
                    // Disco del sistema casi lleno: una vez por sesión (con la barra de cuánto)
                    let d = ccenter::disk_used();
                    let disk = GFX.with(|g| g.borrow_mut().as_mut().is_some_and(|g| { if d >= 95.0 && !g.disk_warned { g.disk_warned = true; true } else { false } }));
                    if disk { live::show(live::Hud::Disk(d.round() as u8), 4000); }
                    let u = current_usage();
                    GFX.with(|g| if let Some(g) = g.borrow_mut().as_mut() { g.usage = u; });
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_MEDIA => {
                let playing = MEDIA.lock().unwrap().as_ref().is_some_and(|m| m.playing);
                // Canción nueva: la isla se despliega 3,5 s para enseñarla y vuelve a encogerse
                let key = MEDIA.lock().unwrap().as_ref().map(|m| format!("{}\u{1}{}", m.title, m.artist)).unwrap_or_default();
                let fresh = GFX.with(|g| g.borrow_mut().as_mut().map(|g| {
                    let f = !key.is_empty() && g.track_key != key;
                    g.track_key = key.clone();
                    if f { g.expand_until = Some(Instant::now() + Duration::from_millis(3500)); }
                    f
                }).unwrap_or(false));
                if fresh { SetTimer(Some(hwnd), TIMER_COLLAPSE, 3600, None); }
                // La animación solo corre mientras suena algo (10 fps: suave y casi sin gasto)
                if playing { SetTimer(Some(hwnd), TIMER_ANIM, 100, None); } else {
                    let _ = KillTimer(Some(hwnd), TIMER_ANIM);
                    GFX.with(|g| if let Some(g) = g.borrow_mut().as_mut() { g.eq = [0.0; 5]; });
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_DROPFILES => {
                // Archivos soltados sobre la barra: a la bandeja del notch
                let hd = HDROP(wp.0 as _);
                let n = DragQueryFileW(hd, u32::MAX, None);
                let mut paths = Vec::new();
                for i in 0..n {
                    let mut buf = [0u16; 1024];
                    let l = DragQueryFileW(hd, i, Some(&mut buf));
                    paths.push(String::from_utf16_lossy(&buf[..l as usize]));
                }
                DragFinish(hd);
                live::shelf_add(paths);
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                // Rueda sobre la zona central (el notch): volumen
                let x = (lp.0 & 0xFFFF) as i16 as f32 / scale_of(hwnd);
                let w = GetSystemMetrics(SM_CXSCREEN) as f32 / scale_of(hwnd);
                if (x - w / 2.0).abs() < 280.0 {
                    let d = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
                    live::nudge_volume(if d > 0 { 1 } else { -1 } * (d.abs() / 120).max(1));
                }
                LRESULT(0)
            }
            0x031D /* WM_CLIPBOARDUPDATE */ => { live::clipboard_changed(hwnd); LRESULT(0) }
            WM_DEVICECHANGE => {
                // Memoria USB conectada o quitada (solo unidades de disco locales, no CD ni red)
                if (wp.0 == 0x8000 || wp.0 == 0x8004) && lp.0 != 0 {
                    let p = lp.0 as *const u8;
                    let kind = *(p.add(4) as *const u32);
                    if kind == 2 {
                        let mask = *(p.add(12) as *const u32);
                        let flags = *(p.add(16) as *const u16);
                        if flags == 0 && mask != 0 {
                            live::USB_LETTER.store(b'A' + mask.trailing_zeros() as u8, Ordering::Relaxed);
                            live::show(live::Hud::Usb(wp.0 == 0x8000), 2600);
                        }
                    }
                }
                LRESULT(1)
            }
            WM_WEATHER | live::WM_HUD | live::WM_NET => { let _ = InvalidateRect(Some(hwnd), None, false); LRESULT(0) }
            WM_POWERBROADCAST if wp.0 == 0x12 /* PBT_APMRESUMEAUTOMATIC */ || wp.0 == 7 /* PBT_APMRESUMESUSPEND */ => {
                // Al volver de suspensión Windows rehace las pantallas: la barra vuelve a mostrarse y pide el hueco otra vez (ahora y en 2 s)
                FULLSCREEN.store(false, Ordering::Relaxed);
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                reserve(hwnd);
                SetTimer(Some(hwnd), TIMER_RESERVE, 2000, None);
                LRESULT(1)
            }
            m if m == RegisterWindowMessageW(w!("TaskbarCreated")) => { reserve(hwnd); LRESULT(0) }
            WM_POWERBROADCAST => {
                // Cargador conectado o desconectado: la isla lo enseña un momento
                if wp.0 == PBT_APMPOWERSTATUSCHANGE as usize {
                    if let Some((pct, ac)) = battery() {
                        let changed = GFX.with(|g| g.borrow_mut().as_mut().map(|g| { let c = g.last_ac.is_some_and(|l| l != ac); g.last_ac = Some(ac); c }).unwrap_or(false));
                        if changed { live::show(live::Hud::Power(pct, ac), 2600); }
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(1)
            }
            WM_USAGE => {
                let u = current_usage();
                // Al cruzar el 50, 75 o 90 % del uso semanal, la isla lo avisa
                let crossed = GFX.with(|g| g.borrow_mut().as_mut().and_then(|g| {
                    let old = g.usage.map(|x| x.0).unwrap_or(0.0);
                    g.usage = u;
                    let new = u?.0;
                    [90.0f32, 75.0, 50.0].into_iter().find(|t| old > 0.0 && old < *t && new >= *t).map(|_| new)
                }));
                if let Some(p) = crossed { live::show(live::Hud::Claude(p.round() as u8), 3500); }
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let h = hit_at(lp, hwnd);
                let (mx, my) = ((lp.0 & 0xFFFF) as i16 as f32 / scale_of(hwnd), ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / scale_of(hwnd));
                let over = GFX.with(|g| g.borrow_mut().as_mut().map(|g| {
                    let r = g.isl_rect;
                    let o = mx >= r.left - 4.0 && mx <= r.right + 4.0 && my >= 0.0 && my <= BAR_H;
                    let c = g.isl_hover != o; g.isl_hover = o; c
                }).unwrap_or(false));
                if over { let _ = InvalidateRect(Some(hwnd), None, false); }
                let changed = GFX.with(|g| g.borrow_mut().as_mut().map(|g| { let c = g.hover != h; g.hover = h; c }).unwrap_or(false));
                if changed { let _ = InvalidateRect(Some(hwnd), None, false); }
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                GFX.with(|g| if let Some(g) = g.borrow_mut().as_mut() { g.hover = None; g.isl_hover = false; g.pressed = None; });
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                // Estado pulsado al instante; menús y paneles se abren al pulsar (como en macOS), el resto al soltar
                let a = hit_at(lp, hwnd);
                GFX.with(|g| if let Some(g) = g.borrow_mut().as_mut() { g.pressed = a; });
                let _ = InvalidateRect(Some(hwnd), None, false);
                let _ = UpdateWindow(hwnd);
                if let Some(a) = a.filter(|a| a.on_press()) { click(hwnd, a); }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let pressed = GFX.with(|g| g.borrow_mut().as_mut().and_then(|g| g.pressed.take()));
                // Solo cuenta si se suelta sobre el mismo botón que se pulsó
                if let Some(a) = hit_at(lp, hwnd).filter(|a| !a.on_press() && Some(*a) == pressed) { click(hwnd, a); }
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                if reserved_ok(hwnd) { position_appbar(hwnd); fix_maximized(hwnd); } else { reserve(hwnd); }
                // Pantallas y DPI tardan en asentarse: se repite cuando Windows ha terminado de recolocarlo todo
                // (no con WM_SETTINGCHANGE: reservar el hueco lo emite y entraríamos en bucle)
                if msg != WM_SETTINGCHANGE { SetTimer(Some(hwnd), TIMER_RESERVE, 1500, None); }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_WINDOWPOSCHANGED => {
                let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: hwnd, ..Default::default() };
                appbar(ABM_WINDOWPOSCHANGED, &mut abd);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_APPBAR => {
                // App a pantalla completa (vídeo, juegos): la barra se esconde detrás
                // Windows también lo avisa cuando solo queda el escritorio: no nos fiamos y lo comprobamos
                if wp.0 as u32 == ABN_FULLSCREENAPP { sync_fullscreen(hwnd); }
                LRESULT(0)
            }
            WM_RBUTTONUP => {
                // Menú de la propia barra: inicio con Windows, configuración, salir
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let none: Box<dyn Fn(POINT) -> Option<usize>> = Box::new(|_| None);
                // Sobre la isla: controles de música; en el resto: menú de la barra
                let in_isl = { let s = scale_of(hwnd); let (mx, my) = ((lp.0 & 0xFFFF) as i16 as f32 / s, ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / s); GFX.with(|g| g.borrow().as_ref().is_some_and(|g| { let r = g.isl_rect; mx >= r.left && mx <= r.right && my >= r.top && my <= r.bottom })) };
                let island = MEDIA.lock().unwrap().clone().filter(|_| in_isl);
                let menu = match &island {
                    Some(m) => menus::island_menu(&m.title, &m.artist, m.playing),
                    None => menus::bar_menu(),
                };
                let fg = last_fg();
                if let menus::Outcome::Picked(a) = menus::track(hwnd, menu, pt.x, pt.y, false, usize::MAX, none) {
                    if let Some(id) = menus::perform(hwnd, fg, a) { bar_action(hwnd, id); }
                }
                give_back_focus(hwnd, fg);
                LRESULT(0)
            }
            WM_DESTROY => {
                if bar_bottom() { windows_taskbar(true); }
                let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: hwnd, ..Default::default() };
                appbar(ABM_REMOVE, &mut abd);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// Modo línea de estado de Claude Code: lee el JSON de stdin, guarda el uso semanal
/// para la barra e imprime "Semana N %". Sin ventana, sin mutex: termina al momento.
fn statusline_mode() {
    use std::io::{Read, Write};
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    // Solo el bloque seven_day dentro de rate_limits (no confundir con context_window)
    let week = input.find("\"rate_limits\"").and_then(|i| {
        let rl = &input[i..];
        let s = &rl[rl.find("\"seven_day\"")?..];
        let s = &s[..s.find('}').map(|e| e + 1).unwrap_or(s.len())];
        Some((json_num(s, "used_percentage")?, json_num(s, "resets_at").unwrap_or(0.0)))
    });
    if let Some((pct, resets)) = week {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = std::fs::create_dir_all(sys::config_dir());
        // Escritura atómica: la barra nunca lee un archivo a medias
        let tmp = sys::config_dir() + "\\claude_usage.tmp";
        if std::fs::write(&tmp, format!("{pct}\n{resets}\n{now}\n")).is_ok() {
            let _ = std::fs::rename(&tmp, usage_path());
        }
        let _ = writeln!(std::io::stdout(), "{}", i18n::tf("Semana {} %", &[&(pct.round() as i64)]));
    }
}

fn usage_path() -> String { sys::config_dir() + "\\claude_usage.txt" }

/// Último uso semanal leído de la barra inferior de Claude Desktop, con su hora de lectura.
static USAGE_UIA: Mutex<Option<(f32, Instant)>> = Mutex::new(None);
const WM_USAGE: u32 = WM_USER + 4;

/// Ventana principal de la app de escritorio de Claude (no las de Claude Code, que no tienen ventana).
fn claude_window() -> Option<HWND> {
    sys::app_windows().into_iter().find(|h| {
        let exe = sys::process_path(sys::real_pid(*h)).unwrap_or_default().to_lowercase();
        exe.ends_with("\\claude.exe") && !exe.contains("claude-code")
    })
}

/// Busca en el árbol de accesibilidad de Claude Desktop el texto "Semana N % · …" de su barra inferior.
/// Solo acepta un elemento corto que EMPIECE así, para no confundirlo con texto de una conversación.
fn uia_weekly(uia: &IUIAutomation, h: HWND) -> Option<f32> {
    unsafe {
        let root = uia.ElementFromHandle(h).ok()?;
        let cond = uia.CreateTrueCondition().ok()?;
        let all = root.FindAll(TreeScope_Descendants, &cond).ok()?;
        for i in 0..all.Length().ok()? {
            let Ok(el) = all.GetElement(i) else { continue };
            let Ok(name) = el.CurrentName() else { continue };
            let name = name.to_string();
            if name.chars().count() > 40 { continue; }
            let rest = name.strip_prefix("Semana ").or_else(|| name.strip_prefix("Week "));
            let Some(rest) = rest else { continue };
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
            if digits.is_empty() || !rest[digits.len()..].trim_start().starts_with('%') { continue; }
            if let Ok(v) = digits.replace(',', ".").parse::<f32>() { return Some(v); }
        }
        None
    }
}

/// Hilo que lee el uso semanal de Claude Desktop una vez por minuto (solo si la app está abierta).
fn usage_thread() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else { return };
        loop {
            let mut wait = 60;
            let win = claude_window();
            if CLAUDE_UP.swap(win.is_some(), Ordering::Relaxed) != win.is_some() {
                let _ = PostMessageW(Some(bar_hwnd()), WM_USAGE, WPARAM(0), LPARAM(0));
            }
            if let Some(h) = win {
                let mut v = uia_weekly(&uia, h);
                // La primera consulta despierta la accesibilidad de Electron: se reintenta enseguida
                if v.is_none() { std::thread::sleep(Duration::from_secs(3)); v = uia_weekly(&uia, h); }
                if let Some(pct) = v {
                    *USAGE_UIA.lock().unwrap() = Some((pct, Instant::now()));
                    let _ = PostMessageW(Some(bar_hwnd()), WM_USAGE, WPARAM(0), LPARAM(0));
                } else { wait = 20; }
            } else { wait = 15; }
            std::thread::sleep(Duration::from_secs(wait));
        }
    }
}

/// Uso semanal a mostrar: el de Claude Desktop si es reciente (< 10 min); si no, el de la línea de estado.
fn current_usage() -> Option<(f32, u64)> {
    let uia = USAGE_UIA.lock().unwrap().filter(|(_, t)| t.elapsed() < Duration::from_secs(600));
    match uia { Some((p, _)) => Some((p, 0)), None => read_usage() }
}

/// Uso semanal guardado por la línea de estado: (porcentaje, reinicio en segundos Unix).
/// None si no hay dato o si la semana ya se reinició (el dato viejo ya no vale).
fn read_usage() -> Option<(f32, u64)> {
    let txt = std::fs::read_to_string(usage_path()).ok()?;
    let mut l = txt.lines();
    let pct: f32 = l.next()?.trim().parse().ok()?;
    let resets: f64 = l.next()?.trim().parse().ok()?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    if resets > 0.0 && (resets as u64) < now { return None; }
    Some((pct, resets as u64))
}

/// ¿Ya falló 3 veces en el último minuto? (mira las horas de las últimas líneas de crash.log)
fn restart_storm() -> bool {
    let Ok(txt) = std::fs::read_to_string(sys::config_dir() + "\\crash.log") else { return false };
    let now = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let now_s = |h: u16, m: u16, s: u16| h as i32 * 3600 + m as i32 * 60 + s as i32;
    let cur = now_s(now.wHour, now.wMinute, now.wSecond);
    let today = format!("{}-{:02}-{:02}", now.wYear, now.wMonth, now.wDay);
    txt.lines().rev().take(5).filter(|l| {
        let Some(rest) = l.strip_prefix(&today) else { return false };
        let p: Vec<i32> = rest.trim().get(..8).unwrap_or("").split(':').filter_map(|x| x.parse().ok()).collect();
        p.len() == 3 && (cur - (p[0] * 3600 + p[1] * 60 + p[2])).abs() <= 60
    }).count() >= 3
}

fn main() {
    if std::env::args().any(|a| a == "--statusline") { statusline_mode(); return; }
    // Prueba del cuentagotas: `macbar --picker` lo abre y escribe el color elegido (nada si se cancela)
    if std::env::args().any(|a| a == "--picker") { if let Some((r, g, b)) = picker::pick() { println!("#{r:02X}{g:02X}{b:02X}"); } return; }
    // Si algo falla (panic), queda escrito dónde, antes de que el proceso se cierre
    std::panic::set_hook(Box::new(|info| {
        let loc = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        let hilo = std::thread::current().name().unwrap_or("?").to_string();
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        let line = format!("{}-{:02}-{:02} {:02}:{:02}:{:02} hilo {hilo}: {msg} ({loc})\n", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
        let _ = std::fs::create_dir_all(sys::config_dir());
        // Si cae con la barra abajo, la de Windows no puede quedarse escondida
        if bar_bottom() { windows_taskbar(true); }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(sys::config_dir() + "\\crash.log") { let _ = f.write_all(line.as_bytes()); }
        // Y se vuelve a abrir sola (salvo que ya haya fallado 3 veces en el último minuto: así no entra en bucle)
        if !restart_storm() {
            if let Ok(exe) = std::env::current_exe() {
                use std::os::windows::process::CommandExt;
                let _ = std::process::Command::new(exe).arg("--tras-fallo").creation_flags(0x0000_0008 /* DETACHED_PROCESS */).spawn();
            }
        }
    }));
    // Prueba del relanzado: MACBAR_FALLO_PRUEBA=1 provoca un fallo a los 3 s (solo esa vez)
    if std::env::var_os("MACBAR_FALLO_PRUEBA").is_some() && !std::env::args().any(|a| a == "--tras-fallo") {
        std::thread::spawn(|| { std::thread::sleep(Duration::from_secs(3)); panic!("fallo de prueba"); });
    }
    if let Ok(q) = std::env::var("MACBAR_SEARCHTEST") { search::selftest(&q); return; }
    if std::env::var("MACBAR_PHONETEST").is_ok() { phone::selftest(); return; }
    unsafe {
        // Una sola instancia
        let mut _mutex = CreateMutexW(None, true, w!("macbar-single-instance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            // Relanzada tras un fallo: la instancia que cae aún está cerrándose; se espera a que suelte el sitio
            if !std::env::args().any(|a| a == "--tras-fallo" || a == "--reinicio") { return; }
            let mut ok = false;
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(250));
                if let Ok(h) = _mutex.as_ref() { let _ = CloseHandle(*h); }
                _mutex = CreateMutexW(None, true, w!("macbar-single-instance"));
                if GetLastError() != ERROR_ALREADY_EXISTS { ok = true; break; }
            }
            if !ok { return; }
        }
        // Si cae por otra causa (no un panic), Windows la vuelve a abrir (tras llevar abierta al menos un minuto)
        let _ = windows::Win32::System::Recovery::RegisterApplicationRestart(w!("--tras-fallo"), windows::Win32::System::Recovery::REGISTER_APPLICATION_RESTART_FLAGS(0));

        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        profiles::load_accent();
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst = GetModuleHandleW(None).unwrap();
        let class = w!("MacBarWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap(),
            ..Default::default()
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            class, w!("macbar"), WS_POPUP,
            0, 0, GetSystemMetrics(SM_CXSCREEN), BAR_H as i32,
            None, None, Some(hinst.into()), None,
        ).unwrap();
        BAR_HWND.store(hwnd.0 as isize, Ordering::Relaxed);

        // Ventana totalmente transparente: solo se ven las píldoras que dibujamos
        let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
        let corner: i32 = 1; // sin esquinas redondeadas
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as _, 4);
        let no_border: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &no_border as *const _ as _, 4);

        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).unwrap();
        let round_cap = factory.CreateStrokeStyle(&D2D1_STROKE_STYLE_PROPERTIES { startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, ..Default::default() }, None).ok();
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();
        let f = Fonts {
            // Tamaños medidos en la referencia (1 px = 1 DIP)
            app: make_format(&dwrite, FONT, 13.5, DWRITE_FONT_WEIGHT_BOLD),      // nombre de la app (como en macOS)
            small: make_format(&dwrite, FONT, 11.5, DWRITE_FONT_WEIGHT_NORMAL),  // artista
            mid: make_format(&dwrite, FONT, 12.5, DWRITE_FONT_WEIGHT_MEDIUM),     // usuario
            bold: make_format(&dwrite, FONT, 12.5, DWRITE_FONT_WEIGHT_SEMI_BOLD), // app, fecha, idioma, temperatura
            title: make_format(&dwrite, FONT, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),// título de la canción
            icon: make_format(&dwrite, ICONS, 14.5, DWRITE_FONT_WEIGHT_NORMAL),
            icon_s: make_format(&dwrite, ICONS, 12.5, DWRITE_FONT_WEIGHT_NORMAL),
        };
        let user = if sys::demo() { "alex".into() } else { std::env::var("USERNAME").unwrap_or_default().to_lowercase() };
        menus::enable_dark_menus();
        // Si macbar se cerró con el panel de Windows transparente, se deja como estaba
        tray::repair();
        // Foto de la cuenta de Windows (a 2x para que se vea nítida en pantallas con escala)
        let avatar_path = sys::account_picture(64);
        let avatar_px = avatar_path.as_deref().and_then(load_file_square);
        let fg = GetForegroundWindow();
        if !fg.0.is_null() && fg != hwnd { LAST_FG.store(fg.0 as isize, Ordering::Relaxed); }
        GFX.with(|g| *g.borrow_mut() = Some(Gfx {
            factory, dwrite, target: None, f,
            app_name: app_name_of(fg).unwrap_or_else(|| i18n::t("Escritorio").into()),
            user, hits: Vec::new(), hover: None,
            open: None, avatar_px, avatar_path, usage: current_usage(), claude_geo: None, isl_w: 0.0, isl_target: 0.0, isl_hover: false, isl_rect: D2D_RECT_F::default(), expand_until: None, track_key: String::new(), avatar: None, art: None, meter: None,
            eq: [0.0; 5], eq_rect: None, wcache: RefCell::new(Default::default()), t0: Instant::now(),
            last_frame: Instant::now(), hl: HashMap::new(), pressed: None, born: HashMap::new(),
            app_prev: String::new(), app_at: Instant::now() - Duration::from_secs(5),
            isl_mode: Mode::Idle, isl_prev: None, isl_mode_at: Instant::now() - Duration::from_secs(5),
            last_hud: None, hud_val: 0.0, count_on: false, eq_visible: false, last_ac: battery().map(|b| b.1), isl_v: 0.0, tray_rot: 0.0, shelf_bmps: (u64::MAX, Vec::new()), hud_bmp: None, usage_disp: 0.0, low_warned: 100, full_warned: true, saver: None, disk_warned: false,
            art_tint: [1.0; 3], tint_now: [1.0; 3], art_at: Instant::now() - Duration::from_secs(5), desk_x: -1.0, glide: (D2D_RECT_F::default(), 0.0), clock_roll: Roll::default(), timer_roll: Default::default(), temp_roll: Roll::default(), morph: HashMap::new(), play_state: (false, Instant::now() - Duration::from_secs(5)),
            round_cap,
        }));

        let mut abd = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, hWnd: hwnd, uCallbackMessage: WM_APPBAR, ..Default::default() };
        appbar(ABM_NEW, &mut abd);
        position_appbar(hwnd);
        // Soltar archivos en la barra (bandeja del notch) y avisos al copiar
        DragAcceptFiles(hwnd, true);
        let _ = windows::Win32::System::DataExchange::AddClipboardFormatListener(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        // Eventos del sistema (sin polling): ventana activa y cambios de título
        let _h1 = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(win_event), 0, 0, WINEVENT_OUTOFCONTEXT);
        let _h2 = SetWinEventHook(EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_NAMECHANGE, None, Some(win_event), 0, 0, WINEVENT_OUTOFCONTEXT);
        let _h3 = SetWinEventHook(EVENT_SYSTEM_DESKTOPSWITCH, EVENT_SYSTEM_DESKTOPSWITCH, None, Some(win_event), 0, 0, WINEVENT_OUTOFCONTEXT);
        let (tx, rx) = channel();
        let _ = CMD_TX.set(tx);
        std::thread::spawn(move || media_thread(rx));
        std::thread::spawn(weather_thread);
        std::thread::spawn(usage_thread);
        // Iconos de bandeja reales, avisos de la isla y estado de la red
        trayhost::start();
        live::start();
        // Paneles creados de antemano (ocultos): se abren sin esperar
        popup::prewarm();
        ccenter::prewarm();
        tray::prewarm();
        // Widgets del escritorio (si están activados)
        desk::start();
        bar_ink_thread();
        hang_watch();
        search_extra::clip_start();
        // Prueba visual de los avisos de la isla: MACBAR_HUDTEST=1 los enseña uno tras otro (3 s cada uno)
        if std::env::var_os("MACBAR_HUDTEST").is_some() {
            std::thread::spawn(|| {
                *live::PHONE_NOTE.lock().unwrap() = ("WhatsApp".into(), "Mamá: ¿vienes a cenar?".into());
                *live::WIFI_NAME.lock().unwrap() = "MiCasa_5G".into();
                *live::CLIP.lock().unwrap() = "Hola mundo".into();
                for h in [live::Hud::Copied, live::Hud::Color(0x4F8CFF), live::Hud::Pomo(true, 25), live::Hud::Pomo(false, 5), live::Hud::LowBat(9), live::Hud::Volume(0.4, false)] {
                    std::thread::sleep(Duration::from_secs(4));
                    live::show(h, 3000);
                }
                std::thread::sleep(Duration::from_secs(4));
                live::caffeine_set(true, None);
                std::thread::sleep(Duration::from_secs(4));
                live::pomodoro_start();
                std::thread::sleep(Duration::from_secs(6));
                live::timer_start(14);
                std::thread::sleep(Duration::from_secs(8));
                live::timer_stop();
                live::caffeine_set(false, None);
            });
        }
        // Reloj, batería e idioma: un timer lento
        SetTimer(Some(hwnd), TIMER_TICK, 10_000, None);
        SetTimer(Some(hwnd), TIMER_GUARD, 2_000, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
