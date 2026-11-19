//! Panel de Wi-Fi: interruptor, redes cercanas con su señal y conexión de un clic a las ya guardadas.

use super::*;
use popup::{Content, Re, Ui};
use std::sync::mpsc::{channel as wf_channel, Sender as WfSender};
use windows::Win32::NetworkManagement::WiFi::*;

const W: f32 = 320.0;
const PAD: f32 = 14.0;
const ROW: f32 = 44.0;
const MAX_ROWS: usize = 7;
const ID_TOGGLE: u32 = 1;
const ID_SETTINGS: u32 = 2;
const ID_MORE: u32 = 3;
const ID_NET0: u32 = 100;

#[derive(Clone)]
struct NetItem { ssid: String, profile: String, quality: u32, secure: bool, connected: bool, known: bool }

#[derive(Clone, Default)]
struct State { radio: Option<bool>, nets: Vec<NetItem>, scanning: bool, connecting: Option<String> }
static ST: Mutex<State> = Mutex::new(State { radio: None, nets: Vec::new(), scanning: false, connecting: None });

enum Job { Scan, Connect(String), Radio(bool) }
static JOBS: OnceLock<WfSender<Job>> = OnceLock::new();

fn wstr(s: &[u16]) -> String { let n = s.iter().position(|&c| c == 0).unwrap_or(s.len()); String::from_utf16_lossy(&s[..n]) }

/// Primera interfaz Wi-Fi y el manejador de la WLAN API.
fn open() -> Option<(HANDLE, windows::core::GUID)> {
    unsafe {
        let (mut ver, mut h) = (0u32, HANDLE::default());
        if WlanOpenHandle(2, None, &mut ver, &mut h) != 0 { return None; }
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let mut g = None;
        if WlanEnumInterfaces(h, None, &mut list) == 0 && !list.is_null() {
            let l = &*list;
            g = std::slice::from_raw_parts(l.InterfaceInfo.as_ptr(), l.dwNumberOfItems as usize).first().map(|i| i.InterfaceGuid);
            WlanFreeMemory(list as _);
        }
        match g { Some(g) => Some((h, g)), None => { let _ = WlanCloseHandle(h, None); None } }
    }
}

fn networks(h: HANDLE, g: &windows::core::GUID) -> Vec<NetItem> {
    let mut out: Vec<NetItem> = Vec::new();
    if sys::demo() {
        let fake = |s: &str, q, known, connected| NetItem { ssid: s.into(), profile: s.into(), quality: q, secure: true, connected, known };
        return vec![fake("Casa", 96, true, true), fake("Oficina 5G", 78, true, false), fake("Café Central", 64, false, false),
            fake("Biblioteca", 47, false, false), fake("Invitados", 30, false, false)];
    }
    unsafe {
        let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
        if WlanGetAvailableNetworkList(h, g, 0, None, &mut list) != 0 || list.is_null() { return out; }
        let l = &*list;
        for n in std::slice::from_raw_parts(l.Network.as_ptr(), l.dwNumberOfItems as usize) {
            let ssid = String::from_utf8_lossy(&n.dot11Ssid.ucSSID[..n.dot11Ssid.uSSIDLength as usize]).to_string();
            if ssid.is_empty() { continue; }
            let it = NetItem {
                profile: wstr(&n.strProfileName), quality: n.wlanSignalQuality, secure: n.bSecurityEnabled.as_bool(),
                connected: n.dwFlags & 1 != 0, known: n.dwFlags & 2 != 0, ssid,
            };
            // La misma red sale repetida (una por perfil/seguridad): se queda la mejor
            match out.iter_mut().find(|o| o.ssid == it.ssid) {
                Some(o) => { if it.connected || (it.known && !o.known) || it.quality > o.quality && it.known == o.known { *o = it; } }
                None => out.push(it),
            }
        }
        WlanFreeMemory(list as _);
    }
    // Conectada primero, luego guardadas, luego por señal
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.known.cmp(&a.known)).then(b.quality.cmp(&a.quality)));
    out
}

fn radio_on() -> Option<bool> {
    use windows::Devices::Radios::{Radio, RadioKind, RadioState};
    let l = Radio::GetRadiosAsync().and_then(|o| o.join()).ok()?;
    (0..l.Size().unwrap_or(0)).filter_map(|i| l.GetAt(i).ok()).find(|r| r.Kind().ok() == Some(RadioKind::WiFi)).and_then(|r| r.State().ok()).map(|s| s == RadioState::On)
}

fn worker(rx: std::sync::mpsc::Receiver<Job>) {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let notify = || popup::notify(popup::Kind::Wifi);
    while let Ok(job) = rx.recv() {
        match job {
            Job::Scan => {
                let radio = radio_on();
                { let mut s = ST.lock().unwrap(); s.radio = radio; s.scanning = true; }
                if let Some((h, g)) = open() {
                    // Lo que Windows ya conoce, al instante; tras el escaneo, la lista fresca
                    ST.lock().unwrap().nets = networks(h, &g);
                    notify();
                    unsafe { let _ = WlanScan(h, &g, None, None, None); }
                    std::thread::sleep(Duration::from_millis(2500));
                    ST.lock().unwrap().nets = networks(h, &g);
                    unsafe { let _ = WlanCloseHandle(h, None); }
                }
                ST.lock().unwrap().scanning = false;
            }
            Job::Connect(profile) => {
                if let Some((h, g)) = open() {
                    let p = HSTRING::from(profile.as_str());
                    let params = WLAN_CONNECTION_PARAMETERS {
                        wlanConnectionMode: wlan_connection_mode_profile, strProfile: PCWSTR(p.as_ptr()),
                        pDot11Ssid: std::ptr::null_mut(), pDesiredBssidList: std::ptr::null_mut(), dot11BssType: dot11_BSS_type_any, dwFlags: 0,
                    };
                    unsafe { let _ = WlanConnect(h, &g, &params, None); }
                    // Se espera a que conecte (o no) para refrescar la lista
                    for _ in 0..16 {
                        std::thread::sleep(Duration::from_millis(500));
                        let nets = networks(h, &g);
                        let done = nets.iter().any(|n| n.profile == profile && n.connected);
                        ST.lock().unwrap().nets = nets;
                        notify();
                        if done { break; }
                    }
                    unsafe { let _ = WlanCloseHandle(h, None); }
                }
                ST.lock().unwrap().connecting = None;
            }
            Job::Radio(on) => {
                use windows::Devices::Radios::{Radio, RadioKind, RadioState};
                if let Ok(l) = Radio::GetRadiosAsync().and_then(|o| o.join()) {
                    if let Some(r) = (0..l.Size().unwrap_or(0)).filter_map(|i| l.GetAt(i).ok()).find(|r| r.Kind().ok() == Some(RadioKind::WiFi)) {
                        let _ = r.SetStateAsync(if on { RadioState::On } else { RadioState::Off }).and_then(|o| o.join());
                    }
                }
                std::thread::sleep(Duration::from_millis(600));
                ST.lock().unwrap().radio = radio_on();
                if on { notify(); std::thread::sleep(Duration::from_millis(1500)); if let Some(tx) = JOBS.get() { let _ = tx.send(Job::Scan); } }
                else { ST.lock().unwrap().nets.clear(); }
            }
        }
        notify();
    }
}

fn job(j: Job) {
    let tx = JOBS.get_or_init(|| { let (tx, rx) = wf_channel(); std::thread::spawn(move || worker(rx)); tx });
    let _ = tx.send(j);
}

/// `knob`: posición animada del interruptor (0 apagado … 1 encendido); `born`: cuándo apareció la lista.
pub struct WifiPanel { st: State, knob: f32, last: Instant, born: Instant, rows_seen: usize }

impl WifiPanel { pub fn new() -> Self { WifiPanel { st: State::default(), knob: -1.0, last: Instant::now(), born: Instant::now(), rows_seen: 0 } } }

/// Mezcla el gris de "apagado" con el azul de "encendido".
fn onoff(k: f32) -> D2D1_COLOR_F {
    let l = |a: f32, b: f32| a + (b - a) * k;
    rgba(l(1.0, 0.23), l(1.0, 0.51), l(1.0, 0.96), l(0.13, 1.0))
}

/// Puntos suspensivos que se van llenando ("Buscando redes.", "..", "...").
fn dots(base: &str) -> String {
    let n = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) / 380 % 4) as usize;
    format!("{base}{}", ".".repeat(n))
}

fn bars(q: u32) -> &'static str { match q { 0..=30 => "\u{E872}", 31..=55 => "\u{E873}", 56..=78 => "\u{E874}", _ => "\u{E701}" } }

impl Content for WifiPanel {
    fn size(&self) -> (f32, f32) {
        let rows = if self.st.radio == Some(true) { self.st.nets.len().clamp(1, MAX_ROWS) as f32 } else { 0.0 };
        let list = if rows > 0.0 { rows * ROW + 8.0 + 12.0 } else { 0.0 };
        (W, PAD + 54.0 + 12.0 + list + 34.0 + PAD)
    }
    fn shown(&mut self) {
        self.st = ST.lock().unwrap().clone();
        self.knob = -1.0;
        self.born = Instant::now();
        self.rows_seen = 0;
        job(Job::Scan);
    }
    fn data(&mut self) { self.st = ST.lock().unwrap().clone(); }
    fn animating(&self) -> bool {
        let target = if self.st.radio == Some(true) { 1.0 } else { 0.0 };
        (self.knob - target).abs() > 0.002 || self.st.scanning || self.st.connecting.is_some() || self.born.elapsed().as_secs_f32() < 0.6
    }

    fn paint(&mut self, ui: &mut Ui) {
        let dt = self.last.elapsed().as_secs_f32().min(1.0 / 30.0);
        self.last = Instant::now();
        let st = &self.st;
        let mut y = PAD;
        // ---- Interruptor
        let r = rect(PAD, y, W - PAD, y + 54.0);
        ui.card_k(r, ui.heat(ID_TOGGLE));
        let on = st.radio == Some(true);
        let target = if on { 1.0 } else { 0.0 };
        if self.knob < 0.0 { self.knob = target; }
        self.knob += (target - self.knob) * (1.0 - (-dt / 0.07).exp());
        let k = self.knob;
        let on_b = ui.brush(onoff(k));
        // El icono "respira" mientras busca redes
        let breathe = if st.scanning && on { 0.5 + 0.5 * (self.born.elapsed().as_secs_f32() * 4.0).cos().abs() } else { 1.0 };
        let e = D2D1_ELLIPSE { point: Vector2 { X: PAD + 25.0, Y: y + 27.0 }, radiusX: 15.0, radiusY: 15.0 };
        unsafe { ui.rt.FillEllipse(&e, &on_b); }
        ui.glyph(&ui.f.icon, "\u{E701}", PAD + 10.0, y + 12.0, 30.0, 30.0, &ui.wa(breathe));
        ui.text(&ui.f.lbl, "Wi-Fi", PAD + 50.0, y + 9.0, 18.0, &ui.white);
        let sub = match st.radio {
            None => i18n::t("Comprobando…").to_string(),
            Some(false) => i18n::t("Desactivado").into(),
            Some(true) => st.nets.iter().find(|n| n.connected).map(|n| n.ssid.clone()).unwrap_or_else(|| i18n::t("Sin conectar").into()),
        };
        ui.text(&ui.f.sub, &trunc(&sub, 30), PAD + 50.0, y + 27.0, 17.0, &ui.dim);
        // Interruptor tipo macOS a la derecha
        let (sx, sy) = (W - PAD - 52.0, y + 17.0);
        unsafe {
            ui.rt.FillRoundedRectangle(&rrect(rect(sx, sy, sx + 38.0, sy + 20.0), 10.0), &on_b);
            // El pomo se desliza y se estira un poco a mitad de camino, como en macOS
            let kx = sx + 10.0 + 18.0 * k;
            let stretch = 3.0 * (k * std::f32::consts::PI).sin();
            ui.rt.FillRoundedRectangle(&rrect(rect(kx - 8.0 - stretch / 2.0, sy + 2.0, kx + 8.0 + stretch / 2.0, sy + 18.0), 8.0), &ui.white);
        }
        if st.radio.is_some() { ui.hit(r, ID_TOGGLE); }
        y += 54.0 + 12.0;

        // ---- Redes
        if on {
            let rows = st.nets.len().clamp(1, MAX_ROWS);
            let h = rows as f32 * ROW + 8.0;
            ui.sep(y - 6.0);
            if st.nets.is_empty() {
                let msg = if st.scanning { dots(i18n::t("Buscando redes")) } else { i18n::t("No hay redes a la vista").into() };
                ui.text(&ui.f.sub, &msg, PAD + 14.0, y + 4.0, ROW, &ui.dim);
            }
            // Filas nuevas entran en cascada: suben 8 px y aparecen con fundido
            if st.nets.len() > self.rows_seen && self.rows_seen == 0 { self.born = Instant::now(); }
            self.rows_seen = st.nets.len();
            let since = self.born.elapsed().as_secs_f32();
            for (i, n) in st.nets.iter().take(MAX_ROWS).enumerate() {
                let p = popup::ease_out(((since - i as f32 * 0.035) / 0.28).clamp(0.0, 1.0));
                let ry = y + 4.0 + i as f32 * ROW + 8.0 * (1.0 - p);
                popup::push_alpha(ui.rt, p);
                let rr = rect(PAD + 4.0, ry, W - PAD - 4.0, ry + ROW);
                let id = ID_NET0 + i as u32;
                let k = ui.heat(id); if k > 0.01 && !n.connected { unsafe { ui.rt.FillRoundedRectangle(&rrect(rr, 9.0), &ui.wa(0.07 * k)); } }
                ui.glyph(&ui.f.icon, bars(n.quality), PAD + 8.0, ry, 30.0, ROW, if n.connected { &ui.white } else { &ui.dim });
                let busy = st.connecting.as_deref() == Some(n.profile.as_str()) && !n.profile.is_empty();
                let note = if n.connected { i18n::t("Conectada").to_string() } else if busy { dots(i18n::t("Conectando")) } else if n.known { i18n::t("Guardada").into() } else if n.secure { i18n::t("Protegida").into() } else { i18n::t("Abierta").into() };
                ui.text(&ui.f.lbl, &trunc(&n.ssid, 26), PAD + 44.0, ry + 5.0, 18.0, &ui.white);
                ui.text(&ui.f.sub, &note, PAD + 44.0, ry + 22.0, 16.0, &ui.dim);
                if busy {
                    // Spinner: arco que gira a la derecha
                    let (cx, cy, rad) = (W - PAD - 22.0, ry + ROW / 2.0, 7.0);
                    let a0 = since * 7.0;
                    unsafe {
                        ui.rt.DrawEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: rad, radiusY: rad }, &ui.track, 2.0, None);
                        for s in 0..10 {
                            let a = a0 + s as f32 * 0.16;
                            let (x0, y0) = (cx + rad * a.cos(), cy + rad * a.sin());
                            let (x1, y1) = (cx + rad * (a + 0.17).cos(), cy + rad * (a + 0.17).sin());
                            ui.rt.DrawLine(Vector2 { X: x0, Y: y0 }, Vector2 { X: x1, Y: y1 }, &ui.white, 2.0, None);
                        }
                    }
                }
                else if n.connected { ui.glyph(&ui.f.icon, "\u{E73E}", W - PAD - 34.0, ry, 24.0, ROW, &ui.white); }
                else if n.secure { ui.glyph(&ui.f.icon, "\u{E72E}", W - PAD - 34.0, ry, 24.0, ROW, &ui.faint); }
                unsafe { ui.rt.PopLayer(); }
                ui.hit(rr, id);
            }
            y += h + 12.0;
        }

        // ---- Pie
        let bw = (W - PAD * 2.0 - 8.0) / 2.0;
        ui.button(rect(PAD, y, PAD + bw, y + 34.0), None, i18n::t("Más redes…"), ID_MORE, false);
        ui.button(rect(PAD + bw + 8.0, y, W - PAD, y + 34.0), Some("\u{E713}"), i18n::t("Configuración"), ID_SETTINGS, false);
    }

    fn click(&mut self, id: u32, _right: bool) -> Re {
        match id {
            ID_TOGGLE => {
                if let Some(on) = self.st.radio {
                    self.st.radio = Some(!on);
                    ST.lock().unwrap().radio = Some(!on);
                    job(Job::Radio(!on));
                }
                Re::Stay
            }
            ID_MORE => Re::CloseRun(Box::new(|| sys::open("ms-availablenetworks:"))),
            ID_SETTINGS => Re::CloseRun(Box::new(|| sys::open("ms-settings:network-wifi"))),
            i if i >= ID_NET0 => {
                let Some(n) = self.st.nets.get((i - ID_NET0) as usize).cloned() else { return Re::Stay };
                if n.connected { return Re::CloseRun(Box::new(|| sys::open("ms-settings:network-wifi"))); }
                if n.known && !n.profile.is_empty() {
                    self.st.connecting = Some(n.profile.clone());
                    ST.lock().unwrap().connecting = Some(n.profile.clone());
                    job(Job::Connect(n.profile));
                    Re::Stay
                } else {
                    // Red nueva: la contraseña la pide el panel de Windows
                    Re::CloseRun(Box::new(|| sys::open("ms-availablenetworks:")))
                }
            }
            _ => Re::Stay,
        }
    }
}
