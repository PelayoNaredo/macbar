//! Bluetooth: icono de la barra, panel de dispositivos emparejados y aviso en la isla al conectar o
//! desconectar uno.
//!
//! - La radio se lee y se cambia con Windows.Devices.Radios (lo mismo que el Centro de control).
//! - Los dispositivos emparejados (clásicos y de baja energía) salen de Windows.Devices.Enumeration;
//!   un DeviceWatcher por cada tipo avisa de cualquier cambio y el hilo vuelve a leer la lista.
//! - La batería es la propiedad PnP DEVPKEY_Bluetooth_Battery del nodo del dispositivo (la misma que
//!   enseña Configuración), unida al dispositivo por su contenedor.
//! - Conectar o desconectar un dispositivo de audio clásico no tiene API pública fiable en Windows: el clic
//!   en un dispositivo abre Configuración > Bluetooth (donde sí están esos botones) en vez de fingirlo.

use super::*;
use popup::{Content, Re, Ui};
use std::collections::HashMap;
use std::sync::atomic::AtomicU8;
use std::sync::mpsc::{channel as bt_channel, Receiver, RecvTimeoutError, Sender};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind, DeviceWatcher, DeviceWatcherStatus};
use windows::Foundation::{IPropertyValue, PropertyType, TypedEventHandler};

// ---------------------------------------------------------------- tipos de dispositivo
/// Tipo de dispositivo (por su clase Bluetooth, su «Appearance» de BLE o, si no hay, su nombre).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Tipo { Otro = 0, Auriculares = 1, Altavoz = 2, Raton = 3, Teclado = 4, Mando = 5, Movil = 6, Ordenador = 7, Reloj = 8 }

impl Tipo {
    pub fn from_u8(v: u8) -> Tipo {
        [Tipo::Otro, Tipo::Auriculares, Tipo::Altavoz, Tipo::Raton, Tipo::Teclado, Tipo::Mando, Tipo::Movil, Tipo::Ordenador, Tipo::Reloj]
            .get(v as usize).copied().unwrap_or(Tipo::Otro)
    }
    /// Glifo de Segoe Fluent / MDL2.
    pub fn glyph(self) -> &'static str {
        match self {
            Tipo::Auriculares => "\u{E7F6}",
            Tipo::Altavoz => "\u{E7F5}",
            Tipo::Raton => "\u{E962}",
            Tipo::Teclado => "\u{E765}",
            Tipo::Mando => "\u{E7FC}",
            Tipo::Movil => "\u{E8EA}",
            Tipo::Ordenador => "\u{E7F8}",
            Tipo::Reloj => "\u{E916}",
            Tipo::Otro => "\u{E702}",
        }
    }
}

/// Glifo del aviso de la isla para el tipo guardado en `Hud::Bt`.
pub fn glyph_of(t: u8) -> &'static str { Tipo::from_u8(t).glyph() }

/// Tipo por la clase de dispositivo clásica (Class of Device): clase mayor y menor.
pub fn tipo_cod(major: u32, minor: u32) -> Tipo {
    match major {
        1 => Tipo::Ordenador,
        2 => Tipo::Movil,
        // Audio/vídeo: 1 auricular con micro, 2 manos libres, 6 cascos; 5 altavoz, 7 audio portátil,
        // 8 coche, 10 equipo de música
        4 => match minor { 1 | 2 | 6 => Tipo::Auriculares, 5 | 7 | 8 | 10 => Tipo::Altavoz, _ => Tipo::Auriculares },
        // Periféricos: los dos bits altos dicen teclado (1), puntero (2) o ambos (3); los bajos, mando…
        5 => match ((minor >> 4) & 3, minor & 0xF) {
            (1, _) | (3, _) => Tipo::Teclado,
            (2, _) => Tipo::Raton,
            (_, 1) | (_, 2) => Tipo::Mando,
            _ => Tipo::Otro,
        },
        7 => Tipo::Reloj,
        _ => Tipo::Otro,
    }
}

/// Tipo por el «Appearance» de Bluetooth de baja energía (categoría = valor >> 6).
pub fn tipo_ble(appearance: u32) -> Tipo {
    match (appearance >> 6, appearance & 0x3F) {
        (1, _) => Tipo::Movil,
        (2, _) => Tipo::Ordenador,
        (3, _) => Tipo::Reloj,
        // HID: 1 teclado, 2 ratón, 3 joystick, 4 mando
        (15, 1) => Tipo::Teclado,
        (15, 2) => Tipo::Raton,
        (15, 3) | (15, 4) => Tipo::Mando,
        // Receptor de audio (altavoces) y audio que se lleva puesto (auriculares, cascos)
        (33, _) => Tipo::Altavoz,
        (37, _) => Tipo::Auriculares,
        _ => Tipo::Otro,
    }
}

/// Último recurso: por el nombre («AirPods», «Keyboard», «MX Master»…).
pub fn tipo_nombre(name: &str) -> Tipo {
    let n = name.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| n.contains(w));
    if has(&["airpods", "buds", "headphone", "headset", "auricular", "cascos", "wh-", "wf-", "earbud", "beats"]) { Tipo::Auriculares }
    else if has(&["keyboard", "teclado", "k380", "k780", "keys"]) { Tipo::Teclado }
    else if has(&["mouse", "ratón", "raton", "mx master", "mx anywhere", "trackpad"]) { Tipo::Raton }
    else if has(&["controller", "gamepad", "mando", "dualsense", "dualshock", "joy-con"]) { Tipo::Mando }
    else if has(&["speaker", "altavoz", "jbl", "soundlink", "boom", "flip", "charge", "soundbar"]) { Tipo::Altavoz }
    else if has(&["iphone", "galaxy", "pixel", "phone", "móvil", "movil"]) { Tipo::Movil }
    else if has(&["watch", "reloj"]) { Tipo::Reloj }
    else { Tipo::Otro }
}

/// «AirPods Pro · 80 %» (con batería) o solo el nombre. En inglés, «80%».
pub fn con_bateria(name: &str, batt: Option<u8>, en: bool) -> String {
    match batt {
        Some(b) if en => format!("{name} · {b}%"),
        Some(b) => format!("{name} · {b} %"),
        None => name.to_string(),
    }
}

// ---------------------------------------------------------------- agrupado de rebotes
/// Tiempo que un cambio de conexión debe mantenerse antes de avisar (un conecta-desconecta más rápido
/// no se cuenta).
pub const HOLD: Duration = Duration::from_secs(2);

/// Decide qué cambios de conexión se avisan: el primer estado (al arrancar macbar) no se avisa, y un
/// cambio solo cuenta si dura `HOLD`; si vuelve atrás antes, no pasa nada.
#[derive(Default)]
pub struct Rebotes { base: Option<HashMap<String, bool>>, pend: HashMap<String, (bool, Instant)> }

impl Rebotes {
    /// Estado actual de cada dispositivo → cambios ya firmes que hay que avisar (id, conectado).
    pub fn step(&mut self, now: Instant, cur: &[(String, bool)]) -> Vec<(String, bool)> {
        let Some(base) = self.base.as_mut() else {
            self.base = Some(cur.iter().cloned().collect());
            return Vec::new();
        };
        let mut out = Vec::new();
        for (id, c) in cur {
            let b = base.get(id).copied().unwrap_or(false);
            if *c == b { self.pend.remove(id); continue; }
            match self.pend.get(id) {
                Some((s, t)) if s == c => {
                    if now.saturating_duration_since(*t) >= HOLD {
                        out.push((id.clone(), *c));
                        base.insert(id.clone(), *c);
                        self.pend.remove(id);
                    }
                }
                _ => { self.pend.insert(id.clone(), (*c, now)); }
            }
        }
        // Los que dejan de estar emparejados se olvidan
        base.retain(|id, _| cur.iter().any(|(x, _)| x == id));
        self.pend.retain(|id, _| cur.iter().any(|(x, _)| x == id));
        out
    }
    /// ¿Hay algún cambio esperando a ver si se mantiene?
    pub fn pending(&self) -> bool { !self.pend.is_empty() }
}

// ---------------------------------------------------------------- estado compartido
#[derive(Clone, Debug, PartialEq)]
pub struct Dev { pub id: String, pub name: String, pub tipo: Tipo, pub connected: bool, pub battery: Option<u8> }

#[derive(Clone, Default)]
struct State {
    /// None = sin radio Bluetooth (o aún sin leer); Some(encendida)
    radio: Option<bool>,
    /// Ya se buscó la radio al menos una vez
    checked: bool,
    devs: Vec<Dev>,
    /// Cambiando la radio (el interruptor ya se ve en su sitio nuevo)
    busy: bool,
}
static ST: Mutex<State> = Mutex::new(State { radio: None, checked: false, devs: Vec::new(), busy: false });

/// Texto del último aviso de la isla («AirPods Pro · 80 %»).
pub static NOTE: Mutex<String> = Mutex::new(String::new());

enum Msg { Wake, Radio(bool) }
static TX: OnceLock<Sender<Msg>> = OnceLock::new();

fn send(m: Msg) { if let Some(tx) = TX.get() { let _ = tx.send(m); } }

/// Arranca el hilo que vigila la radio y los dispositivos.
pub fn start() {
    let (tx, rx) = bt_channel();
    if TX.set(tx).is_err() { return; }
    std::thread::spawn(move || worker(rx));
}

// ---------------------------------------------------------------- opciones (config.ini)
/// 0 = sin leer, 1 = sí, 2 = no. Se guardan en memoria: la barra las mira en cada fotograma.
static BAR_OPT: AtomicU8 = AtomicU8::new(0);
static HUD_OPT: AtomicU8 = AtomicU8::new(0);

fn opt(cache: &AtomicU8, key: &str) -> bool {
    let v = cache.load(Ordering::Relaxed);
    if v != 0 { return v == 1; }
    let on = !sys::config_value(key).is_some_and(|v| v.eq_ignore_ascii_case("no"));
    cache.store(if on { 1 } else { 2 }, Ordering::Relaxed);
    on
}
fn set_opt(cache: &AtomicU8, key: &str, on: bool) {
    cache.store(if on { 1 } else { 2 }, Ordering::Relaxed);
    sys::set_config_value(key, if on { "si" } else { "no" });
}

/// «Mostrar Bluetooth en la barra» (activado si no se dice lo contrario).
pub fn bar_opt() -> bool { opt(&BAR_OPT, "bluetooth_barra") }
pub fn set_bar_opt(on: bool) {
    set_opt(&BAR_OPT, "bluetooth_barra", on);
    unsafe { let _ = InvalidateRect(Some(bar_hwnd()), None, false); }
}
/// «Avisos de Bluetooth» en la isla.
pub fn hud_opt() -> bool { opt(&HUD_OPT, "avisos_bluetooth") }
pub fn set_hud_opt(on: bool) { set_opt(&HUD_OPT, "avisos_bluetooth", on); }

/// Lo que pinta la barra: None = no se enseña (sin radio, oculto en Configuración o aún sin leer);
/// Some((radio encendida, algún dispositivo conectado)).
pub fn bar() -> Option<(bool, bool)> {
    if !bar_opt() { return None; }
    let s = ST.lock().unwrap();
    let on = s.radio?;
    Some((on, on && s.devs.iter().any(|d| d.connected)))
}

// ---------------------------------------------------------------- lectura de Windows
const P_CONN: &str = "System.Devices.Aep.IsConnected";
const P_CONT: &str = "System.Devices.Aep.ContainerId";
const P_MAJOR: &str = "System.Devices.Aep.Bluetooth.Cod.Major";
const P_MINOR: &str = "System.Devices.Aep.Bluetooth.Cod.Minor";
const P_APPEAR: &str = "System.Devices.Aep.Bluetooth.Le.Appearance";
/// DEVPKEY_Bluetooth_Battery (porcentaje, en el nodo PnP del dispositivo)
const P_BATT: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
const P_DEV_CONT: &str = "System.Devices.ContainerId";

fn props(list: &[&str]) -> windows_collections::IIterable<HSTRING> {
    windows_collections::IIterable::<HSTRING>::from(list.iter().map(|s| HSTRING::from(*s)).collect::<Vec<_>>())
}

fn pv(d: &DeviceInformation, k: &str) -> Option<IPropertyValue> {
    let m = d.Properties().ok()?;
    let v = m.Lookup(&HSTRING::from(k)).ok()?;
    windows::core::Interface::cast::<IPropertyValue>(&v).ok()
}
fn pv_num(d: &DeviceInformation, k: &str) -> Option<u32> {
    let v = pv(d, k)?;
    match v.Type().ok()? {
        PropertyType::UInt8 => v.GetUInt8().ok().map(u32::from),
        PropertyType::UInt16 => v.GetUInt16().ok().map(u32::from),
        PropertyType::UInt32 => v.GetUInt32().ok(),
        PropertyType::Int16 => v.GetInt16().ok().map(|x| x as u32),
        PropertyType::Int32 => v.GetInt32().ok().map(|x| x as u32),
        _ => None,
    }
}
fn pv_bool(d: &DeviceInformation, k: &str) -> Option<bool> { pv(d, k)?.GetBoolean().ok() }
fn pv_guid(d: &DeviceInformation, k: &str) -> Option<String> { pv(d, k)?.GetGuid().ok().map(|g| format!("{g:?}").to_lowercase()) }

fn find(aqs: &HSTRING, list: &[&str], kind: DeviceInformationKind) -> Vec<DeviceInformation> {
    let Ok(op) = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(aqs, &props(list), kind) else { return Vec::new() };
    let Ok(c) = op.join() else { return Vec::new() };
    (0..c.Size().unwrap_or(0)).filter_map(|i| c.GetAt(i).ok()).collect()
}

fn selectors() -> Vec<HSTRING> {
    use windows::Devices::Bluetooth::{BluetoothDevice, BluetoothLEDevice};
    [BluetoothDevice::GetDeviceSelectorFromPairingState(true), BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)].into_iter().filter_map(|s| s.ok()).collect()
}

const AEP_PROPS: [&str; 5] = [P_CONN, P_CONT, P_MAJOR, P_MINOR, P_APPEAR];

/// Batería de un dispositivo: la lleva alguno de los nodos PnP de su contenedor (en los auriculares,
/// el de manos libres). Solo se pregunta por los conectados.
fn battery(cont: &str) -> Option<u8> {
    let aqs = HSTRING::from(format!("{P_DEV_CONT}:=\"{{{cont}}}\""));
    find(&aqs, &[P_BATT], DeviceInformationKind::Device).iter().filter_map(|d| pv_num(d, P_BATT)).filter(|b| *b <= 100).max().map(|b| b as u8)
}

/// Dispositivos emparejados (clásicos y BLE), uno por contenedor: conectados primero y luego por nombre.
fn devices() -> Vec<Dev> {
    // Contenedor → (dispositivo, prioridad); un mismo aparato puede salir como clásico y como BLE
    let mut by: Vec<(String, Dev)> = Vec::new();
    for (i, sel) in selectors().iter().enumerate() {
        let le = i == 1;
        for d in find(sel, &AEP_PROPS, DeviceInformationKind::AssociationEndpoint) {
            let id = d.Id().map(|s| s.to_string()).unwrap_or_default();
            let name = d.Name().map(|s| s.to_string()).unwrap_or_default();
            if name.trim().is_empty() { continue; }
            let cont = pv_guid(&d, P_CONT).unwrap_or_else(|| id.clone());
            let connected = pv_bool(&d, P_CONN).unwrap_or(false);
            let mut tipo = if le { pv_num(&d, P_APPEAR).map(tipo_ble).unwrap_or(Tipo::Otro) } else {
                match (pv_num(&d, P_MAJOR), pv_num(&d, P_MINOR)) { (Some(a), Some(b)) => tipo_cod(a, b), _ => Tipo::Otro }
            };
            if tipo == Tipo::Otro { tipo = tipo_nombre(&name); }
            let dev = Dev { id: cont.clone(), name, tipo, connected, battery: None };
            match by.iter_mut().find(|(c, _)| *c == cont) {
                Some((_, o)) => {
                    // Se queda el que está conectado; y del otro, lo que este no sepa
                    if dev.connected && !o.connected { o.connected = true; }
                    if o.tipo == Tipo::Otro { o.tipo = dev.tipo; }
                }
                None => by.push((cont, dev)),
            }
        }
    }
    let mut v: Vec<Dev> = by.into_iter().map(|x| x.1).collect();
    for d in v.iter_mut().filter(|d| d.connected) { d.battery = battery(&d.id); }
    ordenar(&mut v);
    v
}

/// Conectados primero; después por nombre.
pub fn ordenar(v: &mut [Dev]) {
    v.sort_by(|a, b| b.connected.cmp(&a.connected).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

fn bt_radio() -> Option<windows::Devices::Radios::Radio> {
    use windows::Devices::Radios::{Radio, RadioKind};
    let l = Radio::GetRadiosAsync().and_then(|o| o.join()).ok()?;
    (0..l.Size().unwrap_or(0)).filter_map(|i| l.GetAt(i).ok()).find(|r| r.Kind().ok() == Some(RadioKind::Bluetooth))
}

/// Dispositivos inventados del modo demostración (MACBAR_DEMO=1).
pub fn demo_devs() -> Vec<Dev> {
    let d = |id: &str, n: &str, t, c, b| Dev { id: id.into(), name: n.into(), tipo: t, connected: c, battery: b };
    vec![
        d("demo-airpods", "AirPods Pro", Tipo::Auriculares, true, Some(80)),
        d("demo-mx", "MX Master 3S", Tipo::Raton, true, Some(45)),
        d("demo-k380", "Teclado K380", Tipo::Teclado, true, None),
        d("demo-jbl", "JBL Flip 6", Tipo::Altavoz, false, None),
    ]
}

fn notify_all() {
    popup::notify(popup::Kind::Bluetooth);
    unsafe { let _ = PostMessageW(Some(bar_hwnd()), live::WM_NET, WPARAM(0), LPARAM(0)); }
}

/// Avisa en la isla de un cambio de conexión (si los avisos de Bluetooth están activados).
fn announce(d: &Dev, connected: bool) {
    if !hud_opt() { return; }
    let txt = if connected { con_bateria(&trunc(&d.name, 28), d.battery, i18n::en()) } else { i18n::tf("{} · desconectado", &[&trunc(&d.name, 28)]) };
    *NOTE.lock().unwrap() = txt;
    live::show(live::Hud::Bt(connected, d.tipo as u8), 2600);
}

/// Prueba visual del aviso (MACBAR_BTTEST=aviso_on|aviso_off): los AirPods del modo demostración.
pub fn test_hud(connected: bool) {
    let d = demo_devs().remove(0);
    let txt = if connected { con_bateria(&d.name, d.battery, i18n::en()) } else { i18n::tf("{} · desconectado", &[&d.name]) };
    *NOTE.lock().unwrap() = txt;
    live::show(live::Hud::Bt(connected, d.tipo as u8), 8000);
}

fn worker(rx: Receiver<Msg>) {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    if sys::demo() {
        { let mut s = ST.lock().unwrap(); s.radio = Some(true); s.checked = true; s.devs = demo_devs(); }
        notify_all();
        // El interruptor solo cambia el estado inventado: nunca toca la radio de verdad
        while let Ok(m) = rx.recv() {
            if let Msg::Radio(on) = m { let mut s = ST.lock().unwrap(); s.radio = Some(on); s.busy = false; }
            notify_all();
        }
        return;
    }
    let radio = bt_radio();
    {
        let mut s = ST.lock().unwrap();
        s.checked = true;
        s.radio = radio.as_ref().and_then(|r| r.State().ok()).map(|st| st == windows::Devices::Radios::RadioState::On);
    }
    // Sin radio Bluetooth no hay nada que vigilar (ni icono en la barra)
    let Some(radio) = radio else { notify_all(); return };
    let tx = TX.get().cloned();
    let wake = move || { if let Some(t) = &tx { let _ = t.send(Msg::Wake); } };
    { let w = wake.clone(); let _ = radio.StateChanged(&TypedEventHandler::new(move |_, _| { w(); Ok(()) })); }
    let make_watchers = || -> Vec<DeviceWatcher> {
        selectors().iter().filter_map(|sel| {
            let w = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(sel, &props(&AEP_PROPS), DeviceInformationKind::AssociationEndpoint).ok()?;
            let (a, b, c) = (wake.clone(), wake.clone(), wake.clone());
            let _ = w.Added(&TypedEventHandler::new(move |_, _| { a(); Ok(()) }));
            let _ = w.Updated(&TypedEventHandler::new(move |_, _| { b(); Ok(()) }));
            let _ = w.Removed(&TypedEventHandler::new(move |_, _| { c(); Ok(()) }));
            w.Start().ok()?;
            Some(w)
        }).collect()
    };
    let mut watchers = make_watchers();
    let mut reb = Rebotes::default();
    let mut last_read = Instant::now() - Duration::from_secs(60);
    loop {
        // Lectura (cada aviso del vigilante llega en ráfagas: se espera un poco y se juntan)
        let now = Instant::now();
        if now.duration_since(last_read) >= Duration::from_millis(400) {
            last_read = now;
            let on = radio.State().ok().map(|st| st == windows::Devices::Radios::RadioState::On);
            let devs = if on == Some(true) { devices() } else { Vec::new() };
            let cur: Vec<(String, bool)> = devs.iter().map(|d| (d.id.clone(), d.connected)).collect();
            // Con la radio apagada no se cuenta como desconexión de cada uno (ya lo dice el interruptor)
            let firm = if on == Some(true) { reb.step(now, &cur) } else { reb = Rebotes::default(); Vec::new() };
            let changed = {
                let mut s = ST.lock().unwrap();
                let c = s.radio != on || s.devs != devs;
                s.radio = on;
                s.devs = devs.clone();
                c
            };
            if let Some((id, c)) = firm.last() {
                if let Some(d) = devs.iter().find(|d| d.id == *id) { announce(d, *c); }
            }
            if changed { notify_all(); }
            // Un vigilante parado (p. ej. al apagar y encender la radio) se rehace
            if watchers.iter().any(|w| matches!(w.Status(), Ok(DeviceWatcherStatus::Aborted) | Ok(DeviceWatcherStatus::Stopped))) {
                for w in &watchers { let _ = w.Stop(); }
                watchers = make_watchers();
            }
        }
        let wait = if reb.pending() { HOLD + Duration::from_millis(100) } else { Duration::from_secs(30) };
        match rx.recv_timeout(wait) {
            Ok(Msg::Wake) => { std::thread::sleep(Duration::from_millis(250)); while rx.try_recv().is_ok() {} }
            Ok(Msg::Radio(on)) => {
                use windows::Devices::Radios::RadioState;
                let _ = radio.SetStateAsync(if on { RadioState::On } else { RadioState::Off }).and_then(|o| o.join());
                std::thread::sleep(Duration::from_millis(500));
                ST.lock().unwrap().busy = false;
                last_read = Instant::now() - Duration::from_secs(60);
            }
            Err(RecvTimeoutError::Timeout) => last_read = Instant::now() - Duration::from_secs(60),
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Diagnóstico de solo lectura (MACBAR_BTLIST=ruta): escribe lo que Windows dice de la radio y de los
/// dispositivos emparejados, sin cambiar nada.
pub fn dump(path: &str) -> i32 {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let mut o = String::new();
    let r = bt_radio();
    o += &format!("radio: {:?}\n", r.as_ref().map(|r| r.State().map(|s| s.0)));
    for (i, sel) in selectors().iter().enumerate() {
        o += &format!("\n== {} ==\n{sel}\n", if i == 0 { "clásicos" } else { "BLE" });
        for d in find(sel, &AEP_PROPS, DeviceInformationKind::AssociationEndpoint) {
            o += &format!("- {:?} conn={:?} cont={:?} major={:?} minor={:?} appear={:?}\n", d.Name().map(|s| s.to_string()).unwrap_or_default(),
                pv_bool(&d, P_CONN), pv_guid(&d, P_CONT), pv_num(&d, P_MAJOR), pv_num(&d, P_MINOR), pv_num(&d, P_APPEAR));
        }
    }
    o += "\n== resultado ==\n";
    for d in devices() { o += &format!("{d:?}\n"); }
    let _ = std::fs::write(path, o);
    0
}

// ---------------------------------------------------------------- panel
const W: f32 = 320.0;
const PAD: f32 = 14.0;
const ROW: f32 = 44.0;
const CAP: f32 = 22.0;
const MAX_ROWS: usize = 7;
const ID_TOGGLE: u32 = 1;
const ID_SETTINGS: u32 = 2;
const ID_ADD: u32 = 3;
const ID_DEV0: u32 = 100;

/// `knob`: posición animada del interruptor (0 apagado … 1 encendido); `born`: cuándo apareció la lista.
pub struct BtPanel { st: State, knob: f32, last: Instant, born: Instant }

impl BtPanel { pub fn new() -> Self { BtPanel { st: State::default(), knob: -1.0, last: Instant::now(), born: Instant::now() } } }

/// Mezcla el gris de "apagado" con el azul de "encendido" (el mismo del Wi-Fi).
fn onoff(k: f32) -> D2D1_COLOR_F {
    let l = |a: f32, b: f32| a + (b - a) * k;
    rgba(l(1.0, 0.23), l(1.0, 0.51), l(1.0, 0.96), l(0.13, 1.0))
}

/// Subtítulo de la cabecera.
fn subtitle(st: &State) -> String {
    match st.radio {
        None => i18n::t("Comprobando…").to_string(),
        Some(false) => i18n::t("Desactivado").into(),
        Some(true) => {
            let c: Vec<&Dev> = st.devs.iter().filter(|d| d.connected).collect();
            match c.len() {
                0 => i18n::t("Activado").into(),
                1 => i18n::tf("Conectado a {}", &[&c[0].name]),
                n => i18n::tf("{} dispositivos conectados", &[&n]),
            }
        }
    }
}

impl Content for BtPanel {
    fn size(&self) -> (f32, f32) {
        let rows = if self.st.radio == Some(true) { self.st.devs.len().clamp(1, MAX_ROWS) as f32 } else { 0.0 };
        let list = if rows > 0.0 { CAP + rows * ROW + 8.0 + 12.0 } else { 0.0 };
        (W, PAD + 54.0 + 12.0 + list + 34.0 + PAD)
    }
    fn shown(&mut self) {
        self.st = ST.lock().unwrap().clone();
        self.knob = -1.0;
        self.born = Instant::now();
        send(Msg::Wake);
    }
    fn data(&mut self) {
        let busy = self.st.busy;
        let n = ST.lock().unwrap().clone();
        // Mientras cambia la radio, el interruptor se queda donde se dejó
        if busy && n.busy { self.st.devs = n.devs; } else { self.st = n; }
    }
    fn animating(&self) -> bool {
        let target = if self.st.radio == Some(true) { 1.0 } else { 0.0 };
        (self.knob - target).abs() > 0.002 || self.born.elapsed().as_secs_f32() < 0.6
    }

    fn paint(&mut self, ui: &mut Ui) {
        let dt = self.last.elapsed().as_secs_f32().min(1.0 / 30.0);
        self.last = Instant::now();
        let st = &self.st;
        let mut y = PAD;
        // ---- Cabecera con el interruptor de la radio
        let r = rect(PAD, y, W - PAD, y + 54.0);
        ui.card_k(r, ui.heat(ID_TOGGLE));
        let on = st.radio == Some(true);
        let target = if on { 1.0 } else { 0.0 };
        if self.knob < 0.0 { self.knob = target; }
        self.knob += (target - self.knob) * (1.0 - (-dt / 0.07).exp());
        let k = self.knob;
        let on_b = ui.brush(onoff(k));
        let e = D2D1_ELLIPSE { point: Vector2 { X: PAD + 25.0, Y: y + 27.0 }, radiusX: 15.0, radiusY: 15.0 };
        unsafe { ui.rt.FillEllipse(&e, &on_b); }
        ui.glyph(&ui.f.icon, "\u{E702}", PAD + 10.0, y + 12.0, 30.0, 30.0, &ui.white);
        ui.text(&ui.f.lbl, "Bluetooth", PAD + 50.0, y + 9.0, 18.0, &ui.white);
        ui.text(&ui.f.sub, &trunc(&subtitle(st), 30), PAD + 50.0, y + 27.0, 17.0, &ui.dim);
        let (sx, sy) = (W - PAD - 52.0, y + 17.0);
        unsafe {
            ui.rt.FillRoundedRectangle(&rrect(rect(sx, sy, sx + 38.0, sy + 20.0), 10.0), &on_b);
            let kx = sx + 10.0 + 18.0 * k;
            let stretch = 3.0 * (k * std::f32::consts::PI).sin();
            ui.rt.FillRoundedRectangle(&rrect(rect(kx - 8.0 - stretch / 2.0, sy + 2.0, kx + 8.0 + stretch / 2.0, sy + 18.0), 8.0), &ui.white);
        }
        if st.radio.is_some() { ui.hit(r, ID_TOGGLE); }
        y += 54.0 + 12.0;

        // ---- Dispositivos emparejados
        if on {
            ui.sep(y - 6.0);
            ui.text(&ui.f.small, i18n::t("Dispositivos"), PAD + 8.0, y, CAP, &ui.faint);
            y += CAP;
            let rows = st.devs.len().clamp(1, MAX_ROWS);
            if st.devs.is_empty() {
                ui.text(&ui.f.sub, i18n::t("No hay dispositivos emparejados"), PAD + 14.0, y + 4.0, ROW, &ui.dim);
            }
            let since = self.born.elapsed().as_secs_f32();
            for (i, d) in st.devs.iter().take(MAX_ROWS).enumerate() {
                let p = popup::ease_out(((since - i as f32 * 0.035) / 0.28).clamp(0.0, 1.0));
                let ry = y + 4.0 + i as f32 * ROW + 8.0 * (1.0 - p);
                popup::push_alpha(ui.rt, p);
                let rr = rect(PAD + 4.0, ry, W - PAD - 4.0, ry + ROW);
                let id = ID_DEV0 + i as u32;
                let hk = ui.heat(id);
                if hk > 0.01 { unsafe { ui.rt.FillRoundedRectangle(&rrect(rr, 9.0), &ui.wa(0.07 * hk)); } }
                // Icono del tipo en un círculo: relleno si está conectado (como en macOS), tenue si no
                let c = D2D1_ELLIPSE { point: Vector2 { X: PAD + 23.0, Y: ry + ROW / 2.0 }, radiusX: 13.0, radiusY: 13.0 };
                unsafe { ui.rt.FillEllipse(&c, &if d.connected { ui.brush(onoff(1.0)) } else { ui.wa(0.10) }); }
                let gb = if d.connected { ui.brush(rgba(1.0, 1.0, 1.0, 0.97)) } else { ui.dim.clone() };
                ui.glyph(&ui.f.icon, d.tipo.glyph(), PAD + 10.0, ry, 26.0, ROW, &gb);
                let note = if d.connected { i18n::t("Conectado").to_string() } else { i18n::t("No conectado").into() };
                let note = match d.battery { Some(b) if d.connected => con_bateria(&note, Some(b), i18n::en()), _ => note };
                ui.text(&ui.f.lbl, &trunc(&d.name, 26), PAD + 44.0, ry + 5.0, 18.0, &ui.white);
                ui.text(&ui.f.sub, &note, PAD + 44.0, ry + 22.0, 16.0, &ui.dim);
                // A la derecha, la flecha de «abre Configuración» (solo al pasar el ratón)
                if hk > 0.01 {
                    let ab = ui.wa(0.55 * hk);
                    ui.glyph(&ui.f.icon, "\u{E76C}", W - PAD - 34.0, ry, 24.0, ROW, &ab);
                }
                unsafe { ui.rt.PopLayer(); }
                ui.hit(rr, id);
            }
            y += rows as f32 * ROW + 8.0 + 12.0;
        }

        // ---- Pie
        let bw = (W - PAD * 2.0 - 8.0) / 2.0;
        ui.button(rect(PAD, y, PAD + bw, y + 34.0), None, i18n::t("Añadir dispositivo…"), ID_ADD, false);
        ui.button(rect(PAD + bw + 8.0, y, W - PAD, y + 34.0), Some("\u{E713}"), i18n::t("Configuración"), ID_SETTINGS, false);
    }

    fn click(&mut self, id: u32, _right: bool) -> Re {
        match id {
            ID_TOGGLE => {
                if let Some(on) = self.st.radio {
                    self.st.radio = Some(!on);
                    self.st.busy = true;
                    { let mut s = ST.lock().unwrap(); s.radio = Some(!on); s.busy = true; }
                    send(Msg::Radio(!on));
                }
                Re::Stay
            }
            ID_ADD => Re::CloseRun(Box::new(|| sys::open("ms-settings-connectabledevices:devicediscovery"))),
            ID_SETTINGS => Re::CloseRun(Box::new(|| sys::open("ms-settings:bluetooth"))),
            // Conectar o desconectar: en Configuración (Windows no tiene API pública fiable para el audio)
            i if i >= ID_DEV0 => Re::CloseRun(Box::new(|| sys::open("ms-settings:bluetooth"))),
            _ => Re::Stay,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tipos_por_clase_y_apariencia() {
        // Clásicos: cascos, manos libres, altavoz, teclado, ratón, combo, mando, móvil, ordenador
        assert_eq!(tipo_cod(4, 6), Tipo::Auriculares);
        assert_eq!(tipo_cod(4, 1), Tipo::Auriculares);
        assert_eq!(tipo_cod(4, 5), Tipo::Altavoz);
        assert_eq!(tipo_cod(4, 7), Tipo::Altavoz);
        assert_eq!(tipo_cod(5, 0x10), Tipo::Teclado);
        assert_eq!(tipo_cod(5, 0x20), Tipo::Raton);
        assert_eq!(tipo_cod(5, 0x30), Tipo::Teclado);
        assert_eq!(tipo_cod(5, 0x02), Tipo::Mando);
        assert_eq!(tipo_cod(2, 3), Tipo::Movil);
        assert_eq!(tipo_cod(1, 3), Tipo::Ordenador);
        assert_eq!(tipo_cod(31, 0), Tipo::Otro);
        // BLE: teclado 0x3C1, ratón 0x3C2, mando 0x3C4, altavoz 0x841, auriculares 0x941, reloj 0x0C1
        assert_eq!(tipo_ble(0x3C1), Tipo::Teclado);
        assert_eq!(tipo_ble(0x3C2), Tipo::Raton);
        assert_eq!(tipo_ble(0x3C4), Tipo::Mando);
        assert_eq!(tipo_ble(0x841), Tipo::Altavoz);
        assert_eq!(tipo_ble(0x941), Tipo::Auriculares);
        assert_eq!(tipo_ble(0x0C1), Tipo::Reloj);
        assert_eq!(tipo_ble(0x040), Tipo::Movil);
        assert_eq!(tipo_ble(0), Tipo::Otro);
        // Por nombre, cuando Windows no da clase
        assert_eq!(tipo_nombre("AirPods Pro"), Tipo::Auriculares);
        assert_eq!(tipo_nombre("MX Master 3S"), Tipo::Raton);
        assert_eq!(tipo_nombre("Keyboard K380"), Tipo::Teclado);
        assert_eq!(tipo_nombre("JBL Flip 6"), Tipo::Altavoz);
        assert_eq!(tipo_nombre("DualSense Wireless Controller"), Tipo::Mando);
        assert_eq!(tipo_nombre("XYZ-123"), Tipo::Otro);
        // Ida y vuelta por el u8 del aviso
        for t in [Tipo::Otro, Tipo::Auriculares, Tipo::Altavoz, Tipo::Raton, Tipo::Teclado, Tipo::Mando, Tipo::Movil, Tipo::Ordenador, Tipo::Reloj] {
            assert_eq!(Tipo::from_u8(t as u8), t);
            assert!(!t.glyph().is_empty());
        }
        assert_eq!(Tipo::from_u8(200), Tipo::Otro);
    }

    #[test]
    fn rebotes() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let st = |v: &[(&str, bool)]| v.iter().map(|(a, b)| (a.to_string(), *b)).collect::<Vec<_>>();
        let mut r = Rebotes::default();
        // Al arrancar no se avisa de los que ya estaban conectados
        assert!(r.step(at(0), &st(&[("a", true), ("b", false)])).is_empty());
        // b conecta: no se avisa hasta que dura 2 s
        assert!(r.step(at(100), &st(&[("a", true), ("b", true)])).is_empty());
        assert!(r.pending());
        assert!(r.step(at(1500), &st(&[("a", true), ("b", true)])).is_empty());
        assert_eq!(r.step(at(2200), &st(&[("a", true), ("b", true)])), st(&[("b", true)]));
        assert!(!r.pending());
        // Rebote: a se desconecta y vuelve antes de 2 s → nada
        assert!(r.step(at(3000), &st(&[("a", false), ("b", true)])).is_empty());
        assert!(r.step(at(3800), &st(&[("a", true), ("b", true)])).is_empty());
        assert!(!r.pending());
        assert!(r.step(at(9000), &st(&[("a", true), ("b", true)])).is_empty());
        // Desconexión que se mantiene
        assert!(r.step(at(10_000), &st(&[("a", false), ("b", true)])).is_empty());
        assert_eq!(r.step(at(12_100), &st(&[("a", false), ("b", true)])), st(&[("a", false)]));
        // Uno nuevo (recién emparejado) que entra conectado sí se avisa
        assert!(r.step(at(13_000), &st(&[("a", false), ("b", true), ("c", true)])).is_empty());
        assert_eq!(r.step(at(15_100), &st(&[("a", false), ("b", true), ("c", true)])), st(&[("c", true)]));
        // Quitado de los emparejados: se olvida sin avisar
        assert!(r.step(at(16_000), &st(&[("a", false)])).is_empty());
        assert!(!r.pending());
    }

    #[test]
    fn formato_y_orden() {
        assert_eq!(con_bateria("AirPods Pro", Some(80), false), "AirPods Pro · 80 %");
        assert_eq!(con_bateria("AirPods Pro", Some(80), true), "AirPods Pro · 80%");
        assert_eq!(con_bateria("Teclado K380", None, false), "Teclado K380");
        let mut v = demo_devs();
        v.reverse();
        ordenar(&mut v);
        let names: Vec<&str> = v.iter().map(|d| d.name.as_str()).collect();
        // Conectados primero (por nombre) y luego el resto
        assert_eq!(names, ["AirPods Pro", "MX Master 3S", "Teclado K380", "JBL Flip 6"]);
    }
}
