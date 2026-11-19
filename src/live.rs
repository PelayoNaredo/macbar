//! "Actividades en vivo" de la isla (como la Dynamic Island): volumen, brillo, cargador y temporizador,
//! más el estado de la red para el icono de la barra. Todo por eventos o con sondeos muy baratos.

use super::*;
use windows::core::BSTR;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::System::Wmi::*;

pub const WM_HUD: u32 = WM_USER + 5;
pub const WM_NET: u32 = WM_USER + 6;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Hud { Volume(f32, bool), Bright(f32), Power(u8, bool), TimerDone, Net(bool), LowBat(u8), Caps(bool), Audio, Claude(u8), Download, Shot, Usb(bool), Copied, Shelf,
    Mic(bool), Num(bool), Lang, Desk(u8, u8), Mem(u8), Profile(u8), Present(bool),
    /// Notificación nueva del móvil, cámara o micrófono que empiezan a usarse, batería llena, ahorro de
    /// batería, red Wi-Fi a la que se conecta y disco casi lleno
    Phone, CamOn, MicOn, Full, Saver(bool), Wifi, Disk(u8) }

/// Última notificación del móvil que se enseñó: (app, título)
pub static PHONE_NOTE: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));
/// Red Wi-Fi a la que se acaba de conectar
pub static WIFI_NAME: Mutex<String> = Mutex::new(String::new());

/// Nombre del idioma de teclado recién elegido (para Hud::Lang).
pub static LANG: Mutex<String> = Mutex::new(String::new());
/// Nombre del escritorio virtual al que se acaba de cambiar.
pub static DESK_NAME: Mutex<String> = Mutex::new(String::new());
/// Escritorio del que se viene (la píldora de los puntos se desliza desde ahí).
pub static DESK_FROM: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);
/// Momento del último cambio de volumen (la barra "se hincha" un instante).
pub static VOL_BUMP: Mutex<Option<Instant>> = Mutex::new(None);

/// Último archivo avisado (descarga o captura): ruta, miniatura y versión (para la caché de la isla).
pub static LAST_FILE: Mutex<Option<(String, Option<Arc<Vec<u8>>>, u64)>> = Mutex::new(None);
/// Letra de la última unidad USB conectada o quitada.
pub static USB_LETTER: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(b'?');

/// Nombre de la salida de audio nueva (para el aviso Hud::Audio).
pub static AUDIO_NAME: Mutex<String> = Mutex::new(String::new());

/// Aviso que la isla muestra ahora mismo y hasta cuándo.
pub static HUD: Mutex<Option<(Hud, Instant)>> = Mutex::new(None);

pub fn show(h: Hud, ms: u64) {
    // En modo presentación solo pasa lo imprescindible
    if crate::profiles::presenting() && !matches!(h, Hud::TimerDone | Hud::LowBat(_) | Hud::Present(_)) { return; }
    *HUD.lock().unwrap() = Some((h, Instant::now() + Duration::from_millis(ms)));
    unsafe { let _ = PostMessageW(Some(bar_hwnd()), WM_HUD, WPARAM(0), LPARAM(0)); }
}

pub fn current() -> Option<Hud> {
    let mut g = HUD.lock().unwrap();
    match *g {
        Some((h, until)) if Instant::now() < until => Some(h),
        Some(_) => { *g = None; None }
        None => None,
    }
}

pub fn dismiss() { *HUD.lock().unwrap() = None; }

// ---------------------------------------------------------------- temporizador
/// (fin, duración total en segundos)
static TIMER: Mutex<Option<(Instant, u32)>> = Mutex::new(None);

pub fn timer_start(secs: u32) {
    *TIMER.lock().unwrap() = Some((Instant::now() + Duration::from_secs(secs as u64), secs));
    unsafe { let _ = PostMessageW(Some(bar_hwnd()), WM_HUD, WPARAM(1), LPARAM(0)); }
}

pub fn timer_stop() {
    *TIMER.lock().unwrap() = None;
    unsafe { let _ = PostMessageW(Some(bar_hwnd()), WM_HUD, WPARAM(1), LPARAM(0)); }
}

/// Segundos que quedan y duración total. Si acaba de terminar, lo avisa y lo quita.
pub fn timer_left() -> Option<(u32, u32)> {
    let mut t = TIMER.lock().unwrap();
    let (end, total) = (*t)?;
    let now = Instant::now();
    if now >= end {
        *t = None;
        drop(t);
        show(Hud::TimerDone, 8000);
        unsafe { let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(MB_ICONASTERISK); }
        return None;
    }
    Some(((end - now).as_secs_f32().ceil() as u32, total))
}

pub fn mmss(s: u32) -> String { if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) } }

// ---------------------------------------------------------------- volumen
fn endpoint() -> Option<IAudioEndpointVolume> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()
    }
}

/// Mira el volumen 5 veces por segundo (una lectura cuesta microsegundos) y avisa si cambia.
/// Id y nombre de la salida de audio por defecto.
fn default_device() -> Option<(String, String)> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let id = dev.GetId().ok()?;
        let ids = id.to_string().unwrap_or_default();
        CoTaskMemFree(Some(id.0 as _));
        let name = dev.OpenPropertyStore(STGM_READ).ok().and_then(|st| st.GetValue(&PKEY_Device_FriendlyName).ok())
            .map(|v| v.to_string()).unwrap_or_default();
        Some((ids, name))
    }
}

/// Mira el volumen 5 veces por segundo (una lectura cuesta microsegundos) y avisa si cambia.
/// Cada segundo comprueba además si cambió la salida de audio (auriculares, altavoz Bluetooth…).
fn capture_endpoint() -> Option<IAudioEndpointVolume> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        en.GetDefaultAudioEndpoint(windows::Win32::Media::Audio::eCapture, windows::Win32::Media::Audio::eCommunications).ok()?.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()
    }
}

fn volume_thread() {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let mut ep = None;
    let mut last: Option<(f32, bool)> = None;
    let mut mic_ep: Option<IAudioEndpointVolume> = None;
    let mut mic_last: Option<bool> = None;
    let mut dev_id: Option<String> = None;
    let mut n = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(200));
        n += 1;
        if n % 5 == 0 {
            if let Some((id, name)) = default_device() {
                if dev_id.as_ref().is_some_and(|d| *d != id) {
                    // Salida nueva: su volumen distinto no es un cambio de volumen
                    ep = endpoint(); last = None;
                    *AUDIO_NAME.lock().unwrap() = name;
                    show(Hud::Audio, 2600);
                }
                dev_id = Some(id);
            }
            // Micrófono: silenciado / activado (cada segundo; el micro por defecto puede cambiar)
            if n % 25 == 0 { mic_ep = None; }
            if mic_ep.is_none() { mic_ep = capture_endpoint(); mic_last = None; }
            if let Some(m) = &mic_ep {
                match unsafe { m.GetMute() } {
                    Ok(b) => { let b = b.as_bool(); if mic_last.is_some_and(|l| l != b) { show(Hud::Mic(b), 1800); } mic_last = Some(b); }
                    Err(_) => mic_ep = None,
                }
            }
        }
        if ep.is_none() { ep = endpoint(); }
        let Some(v) = &ep else { continue };
        let cur = unsafe { (v.GetMasterVolumeLevelScalar().unwrap_or(0.0), v.GetMute().map(|b| b.as_bool()).unwrap_or(false)) };
        if let Some(l) = last {
            if (l.0 - cur.0).abs() > 0.004 || l.1 != cur.1 { *VOL_BUMP.lock().unwrap() = Some(Instant::now()); show(Hud::Volume(cur.0, cur.1), 1600); }
        }
        last = Some(cur);
    }
}

// ---------------------------------------------------------------- bloq mayús (gancho de teclado en su propio hilo)
static CAPS_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static CAPS_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static NUM_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static NUM_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "system" fn kb_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
            if k.vkCode == VK_NUMLOCK.0 as u32 {
                let m = wp.0 as u32;
                if m == WM_KEYDOWN || m == WM_SYSKEYDOWN {
                    if !NUM_DOWN.swap(true, Ordering::Relaxed) {
                        let on = !NUM_ON.fetch_xor(true, Ordering::Relaxed);
                        show(Hud::Num(on), 1400);
                    }
                } else { NUM_DOWN.store(false, Ordering::Relaxed); }
            }
            if k.vkCode == VK_CAPITAL.0 as u32 {
                let m = wp.0 as u32;
                if m == WM_KEYDOWN || m == WM_SYSKEYDOWN {
                    // Solo la primera pulsación (al mantenerla, Windows repite el "abajo")
                    if !CAPS_DOWN.swap(true, Ordering::Relaxed) {
                        let on = !CAPS_ON.fetch_xor(true, Ordering::Relaxed);
                        show(Hud::Caps(on), 1400);
                    }
                } else { CAPS_DOWN.store(false, Ordering::Relaxed); }
            }
        }
        CallNextHookEx(None, code, wp, lp)
    }
}
fn keyboard_thread() {
    unsafe {
        CAPS_ON.store(GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0, Ordering::Relaxed);
        NUM_ON.store(GetKeyState(VK_NUMLOCK.0 as i32) & 1 != 0, Ordering::Relaxed);
        let Ok(_h) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_hook), None, 0) else { return };
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() { let _ = TranslateMessage(&msg); DispatchMessageW(&msg); }    }
}
// ---------------------------------------------------------------- brillo (eventos WMI, sin sondeo)
fn brightness_thread() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(loc) = CoCreateInstance::<_, IWbemLocator>(&WbemLocator, None, CLSCTX_INPROC_SERVER) else { return };
        let Ok(svc) = loc.ConnectServer(&BSTR::from("ROOT\\WMI"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None) else { return };
        let _ = CoSetProxyBlanket(&svc, 10, 0, PCWSTR::null(), RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE);
        let Ok(en) = svc.ExecNotificationQuery(&BSTR::from("WQL"), &BSTR::from("SELECT * FROM WmiMonitorBrightnessEvent"), WBEM_FLAG_RETURN_IMMEDIATELY | WBEM_FLAG_FORWARD_ONLY, None) else { return };
        let _ = CoSetProxyBlanket(&en, 10, 0, PCWSTR::null(), RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE);
        loop {
            let mut objs = [None];
            let mut n = 0u32;
            if en.Next(WBEM_INFINITE, &mut objs, &mut n).is_err() || n == 0 { std::thread::sleep(Duration::from_secs(5)); continue; }
            let Some(o) = objs[0].take() else { continue };
            use windows::Win32::System::Variant::{VariantClear, VariantToInt32, VARIANT};
            let mut v = VARIANT::default();
            if o.Get(w!("Brightness"), 0, &mut v, None, None).is_ok() {
                if let Ok(b) = VariantToInt32(&v) { show(Hud::Bright(b as f32 / 100.0), 1600); }
            }
            let _ = VariantClear(&mut v);
        }
    }
}

// ---------------------------------------------------------------- red (icono de la barra)
#[derive(Clone, Copy, PartialEq, Default)]
pub struct Net { pub wifi_quality: Option<u32>, pub online: bool, pub radio_off: bool }
pub static NET: Mutex<Net> = Mutex::new(Net { wifi_quality: None, online: true, radio_off: false });

/// Calidad (0..100) de la red Wi-Fi conectada.
pub fn wifi_quality() -> Option<u32> {
    use windows::Win32::NetworkManagement::WiFi::*;
    unsafe {
        let (mut ver, mut h) = (0u32, HANDLE::default());
        if WlanOpenHandle(2, None, &mut ver, &mut h) != 0 { return None; }
        let mut out = None;
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if WlanEnumInterfaces(h, None, &mut list) == 0 && !list.is_null() {
            let l = &*list;
            for i in std::slice::from_raw_parts(l.InterfaceInfo.as_ptr(), l.dwNumberOfItems as usize) {
                if i.isState != wlan_interface_state_connected { continue; }
                let (mut size, mut data) = (0u32, std::ptr::null_mut::<core::ffi::c_void>());
                if WlanQueryInterface(h, &i.InterfaceGuid, wlan_intf_opcode_current_connection, None, &mut size, &mut data, None) == 0 && !data.is_null() {
                    out = Some((*(data as *const WLAN_CONNECTION_ATTRIBUTES)).wlanAssociationAttributes.wlanSignalQuality);
                    WlanFreeMemory(data);
                    break;
                }
            }
            WlanFreeMemory(list as _);
        }
        let _ = WlanCloseHandle(h, None);
        out
    }
}

fn online() -> bool {
    use windows::Win32::Networking::NetworkListManager::*;
    unsafe {
        let Ok(nlm) = CoCreateInstance::<_, INetworkListManager>(&NetworkListManager, None, CLSCTX_ALL) else { return true };
        nlm.GetConnectivity().map(|c| c.0 & (NLM_CONNECTIVITY_IPV4_INTERNET.0 | NLM_CONNECTIVITY_IPV6_INTERNET.0) != 0).unwrap_or(true)
    }
}

fn net_thread() {
    use windows::Devices::Radios::{Radio, RadioKind, RadioState};
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let mut n = 0u32;
    let mut was_online: Option<bool> = None;
    let mut last_ssid: Option<String> = None;
    loop {
        // Micrófono y cámara en uso: cada 2 s (leer el registro cuesta microsegundos)
        let (mic, cam) = (in_use("microphone"), in_use("webcam"));
        let pc = MIC.swap(mic, Ordering::Relaxed) != mic;
        let cc = CAM.swap(cam, Ordering::Relaxed) != cam;
        // Al empezar a usarse (no al soltarlos) la isla lo dice un momento, como el aviso de privacidad de iOS
        if n > 0 && cc && cam { show(Hud::CamOn, 2200); } else if n > 0 && pc && mic { show(Hud::MicOn, 2200); }
        if pc || cc { unsafe { let _ = PostMessageW(Some(bar_hwnd()), WM_NET, WPARAM(0), LPARAM(0)); } }
        // Red: cada 4 s; la radio Wi-Fi, cada ~30 s
        if n % 2 == 0 {
            let radio_off = if n % 15 == 0 || NET.lock().unwrap().radio_off {
                Radio::GetRadiosAsync().and_then(|o| o.join()).ok().and_then(|l| {
                    (0..l.Size().unwrap_or(0)).filter_map(|i| l.GetAt(i).ok()).find(|r| r.Kind().ok() == Some(RadioKind::WiFi)).and_then(|r| r.State().ok())
                }).is_some_and(|s| s != RadioState::On)
            } else { NET.lock().unwrap().radio_off };
            let st = Net { wifi_quality: wifi_quality(), online: online(), radio_off };
            let changed = { let mut g = NET.lock().unwrap(); let c = *g != st; *g = st; c };
            // Se conecta o se pierde la conexión: la isla lo cuenta
            // Wi-Fi nueva (cambio de red o reconexión): se dice a cuál
            let ssid = if sys::demo() { String::new() } else { crate::ccenter::wifi_ssid() };
            let joined = !ssid.is_empty() && last_ssid.as_ref().is_some_and(|l| *l != ssid);
            last_ssid = Some(ssid.clone());
            if joined { *WIFI_NAME.lock().unwrap() = ssid; show(Hud::Wifi, 2400); }
            else if was_online.is_some_and(|w| w != st.online) { show(Hud::Net(st.online), 2400); }
            was_online = Some(st.online);
            if changed { unsafe { let _ = PostMessageW(Some(bar_hwnd()), WM_NET, WPARAM(0), LPARAM(0)); } }
        }
        n += 1;
        std::thread::sleep(Duration::from_secs(2));
    }
}

// ---------------------------------------------------------------- privacidad (punto naranja / verde)
pub static MIC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static CAM: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// ¿Alguna app usa ahora el micrófono o la cámara? Windows lo apunta en el registro
/// (LastUsedTimeStop = 0 mientras se usa), tanto para apps de la Tienda como de escritorio.
fn in_use(cap: &str) -> bool {
    use windows::Win32::System::Registry::*;
    fn scan(root: HKEY, path: &str, deep: bool) -> bool {
        unsafe {
            let mut k = HKEY::default();
            if RegOpenKeyExW(root, &HSTRING::from(path), Some(0), KEY_READ, &mut k).is_err() { return false; }
            let mut i = 0u32;
            let mut hit = false;
            loop {
                let mut name = [0u16; 512];
                let mut len = name.len() as u32;
                if RegEnumKeyExW(k, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None).is_err() { break; }
                i += 1;
                let sub = String::from_utf16_lossy(&name[..len as usize]);
                if sub == "NonPackaged" {
                    if deep && scan(root, &format!("{path}\\NonPackaged"), false) { hit = true; break; }
                    continue;
                }
                let q = |v: PCWSTR| -> Option<u64> {
                    let (mut x, mut sz) = (0u64, 8u32);
                    RegGetValueW(k, &HSTRING::from(sub.as_str()), v, RRF_RT_REG_QWORD, None, Some(&mut x as *mut _ as _), Some(&mut sz)).ok().ok()?;
                    Some(x)
                };
                if q(w!("LastUsedTimeStop")) == Some(0) && q(w!("LastUsedTimeStart")).is_some_and(|s| s > 0) { hit = true; break; }
            }
            let _ = RegCloseKey(k);
            hit
        }
    }
    scan(HKEY_CURRENT_USER, &format!("Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\{cap}"), true)
}
/// Glifo del icono de red según la señal real.
pub fn net_glyph(n: &Net) -> (&'static str, bool) {
    match n.wifi_quality {
        Some(q) => (match q { 0..=30 => "\u{E872}", 31..=55 => "\u{E873}", 56..=78 => "\u{E874}", _ => "\u{E701}" }, n.online),
        None if n.online => ("\u{E839}", true),        // por cable
        None => ("\u{F384}", false),                     // sin conexión
    }
}

// ---------------------------------------------------------------- idioma del teclado, escritorios virtuales y memoria
fn reg_bin(path: &str, value: &str) -> Option<Vec<u8>> {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut sz = 0u32;
        RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(path), &HSTRING::from(value), RRF_RT_REG_BINARY, None, None, Some(&mut sz)).ok().ok()?;
        let mut buf = vec![0u8; sz as usize];
        RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(path), &HSTRING::from(value), RRF_RT_REG_BINARY, None, Some(buf.as_mut_ptr() as _), Some(&mut sz)).ok().ok()?;
        buf.truncate(sz as usize);
        Some(buf)
    }
}

fn reg_sz(path: &str, value: &str) -> Option<String> {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut buf = [0u16; 260];
        let mut sz = (buf.len() * 2) as u32;
        RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(path), &HSTRING::from(value), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut sz)).ok().ok()?;
        let n = (sz as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&buf[..n.min(buf.len())]))
    }
}

fn guid_of(b: &[u8]) -> String {
    let g = windows::core::GUID::from_values(
        u32::from_le_bytes([b[0], b[1], b[2], b[3]]), u16::from_le_bytes([b[4], b[5]]), u16::from_le_bytes([b[6], b[7]]),
        [b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]);
    format!("{{{g:?}}}")
}

/// (índice desde 1, total) del escritorio virtual actual; None con un solo escritorio.
fn desktop_now() -> Option<(u8, u8, String)> {
    const VD: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VirtualDesktops";
    let cur = reg_bin(VD, "CurrentVirtualDesktop")?;
    let ids = reg_bin(VD, "VirtualDesktopIDs")?;
    if cur.len() != 16 || ids.len() < 32 { return None; }
    let total = ids.len() / 16;
    let i = ids.chunks(16).position(|c| c == cur.as_slice())?;
    let name = reg_sz(&format!("{VD}\\Desktops\\{}", guid_of(&cur)), "Name").filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::i18n::tf("Escritorio {}", &[&(i + 1)]));
    Some(((i + 1) as u8, total as u8, name))
}

/// Nombre ("Español") y código corto ("ES") de una distribución de teclado.
fn lang_name(hkl: isize) -> String {
    use windows::Win32::Globalization::*;
    unsafe {
        let lcid = (hkl & 0xFFFF) as u32;
        let mut loc = [0u16; 85];
        let n = LCIDToLocaleName(lcid, Some(&mut loc), 0);
        if n <= 1 { return String::new(); }
        let locname = String::from_utf16_lossy(&loc[..n as usize - 1]);
        let mut nm = [0u16; 128];
        let m = GetLocaleInfoEx(PCWSTR(loc.as_ptr()), LOCALE_SNATIVELANGUAGENAME, Some(&mut nm));
        let mut name = if m > 1 { String::from_utf16_lossy(&nm[..m as usize - 1]) } else { locname.clone() };
        if let Some(c) = name.chars().next() { name = c.to_uppercase().collect::<String>() + &name[c.len_utf8()..]; }
        let code = locname.split('-').next().unwrap_or("").to_uppercase();
        format!("{name} · {code}")
    }
}

fn context_thread() {
    let mut last_fg = HWND::default();
    let mut last_hkl = 0isize;
    let mut last_desk: Option<u8> = None;
    let mut mem_warned = false;
    let mut n = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(250));
        n += 1;
        unsafe {
            // Idioma: solo cuenta si cambia dentro de la MISMA ventana (Win+Espacio, Alt+Mayús)
            let fg = GetForegroundWindow();
            if !fg.is_invalid() && fg != bar_hwnd() {
                let tid = GetWindowThreadProcessId(fg, None);
                let hkl = GetKeyboardLayout(tid).0 as isize;
                if fg == last_fg && hkl != last_hkl && last_hkl != 0 && hkl != 0 {
                    *LANG.lock().unwrap() = lang_name(hkl);
                    show(Hud::Lang, 1600);
                }
                last_fg = fg; last_hkl = hkl;
            }
        }
        // Escritorios virtuales (Win+Ctrl+flechas)
        let d = desktop_now();
        let idx = d.as_ref().map(|x| x.0);
        if let Some((i, t, name)) = d {
            if let Some(l) = last_desk.filter(|l| *l != i) {
                *DESK_NAME.lock().unwrap() = name;
                DESK_FROM.store(l, Ordering::Relaxed);
                show(Hud::Desk(i, t), 1600);
            }
        }
        last_desk = idx;
        // Memoria: avisa una vez al pasar del 90 % y se rearma al bajar del 80 %
        if n % 8 == 0 {
            let mut ms = windows::Win32::System::SystemInformation::MEMORYSTATUSEX { dwLength: std::mem::size_of::<windows::Win32::System::SystemInformation::MEMORYSTATUSEX>() as u32, ..Default::default() };
            if unsafe { windows::Win32::System::SystemInformation::GlobalMemoryStatusEx(&mut ms) }.is_ok() {
                let l = ms.dwMemoryLoad;
                if l >= 90 && !mem_warned { mem_warned = true; show(Hud::Mem(l as u8), 3500); }
                if l < 80 { mem_warned = false; }
            }
        }
    }
}

pub fn start() {
    std::thread::spawn(context_thread);
    std::thread::spawn(volume_thread);
    std::thread::spawn(brightness_thread);
    std::thread::spawn(net_thread);
    std::thread::spawn(keyboard_thread);
    std::thread::spawn(watch_thread);
}

// ---------------------------------------------------------------- descargas y capturas (sin sondeo: avisos del sistema de archivos)
fn known_folder(id: &windows::core::GUID) -> Option<String> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

fn list(dir: &str) -> std::collections::HashSet<String> {
    std::fs::read_dir(dir).map(|r| r.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect()).unwrap_or_default()
}

/// Archivos a medio descargar o temporales: no cuentan
fn partial(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with('.') || n.starts_with('~') || [".crdownload", ".part", ".partial", ".tmp", ".download", ".opdownload"].iter().any(|e| n.ends_with(e))
}

fn watch_thread() {
    use windows::Win32::Storage::FileSystem::*;
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let dirs: Vec<(String, Hud)> = [(known_folder(&FOLDERID_Downloads), Hud::Download), (known_folder(&FOLDERID_Screenshots), Hud::Shot)]
        .into_iter().filter_map(|(d, k)| d.map(|d| (d, k))).collect();
    let mut seen: Vec<std::collections::HashSet<String>> = dirs.iter().map(|(d, _)| list(d)).collect();
    let handles: Vec<HANDLE> = dirs.iter().filter_map(|(d, _)| unsafe { FindFirstChangeNotificationW(&HSTRING::from(d.as_str()), false, FILE_NOTIFY_CHANGE_FILE_NAME).ok() }).collect();
    if handles.len() != dirs.len() || handles.is_empty() { return; }
    let mut ver = 0u64;
    loop {
        let r = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        let i = (r.0 as usize).wrapping_sub(WAIT_OBJECT_0.0 as usize);
        if i >= handles.len() { std::thread::sleep(Duration::from_secs(1)); continue; }
        // Se deja terminar de escribir antes de mirar
        std::thread::sleep(Duration::from_millis(600));
        let (dir, kind) = &dirs[i];
        let now = list(dir);
        let fresh: Vec<String> = now.iter().filter(|n| !seen[i].contains(*n) && !partial(n)).cloned().collect();
        seen[i] = now;
        if let Some(name) = fresh.last() {
            let path = format!("{dir}\\{name}");
            // Tamaño estable: el archivo ya está completo
            let size = || std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let mut s0 = size();
            for _ in 0..10 { std::thread::sleep(Duration::from_millis(400)); let s1 = size(); if s1 == s0 && s1 > 0 { break; } s0 = s1; }
            if std::path::Path::new(&path).is_file() {
                let thumb = if *kind == Hud::Shot { load_file_square(&path).map(Arc::new) } else { None };
                ver += 1;
                *LAST_FILE.lock().unwrap() = Some((path, thumb, ver));
                show(*kind, 4000);
            }
        }
        unsafe { let _ = FindNextChangeNotification(handles[i]); }
    }
}
// ---------------------------------------------------------------- rueda sobre el notch = volumen
/// Sube o baja el volumen (paso de 2 %) y lo enseña al momento en la isla.
pub fn nudge_volume(steps: i32) {
    if let Some(v) = endpoint() {
        unsafe {
            let cur = v.GetMasterVolumeLevelScalar().unwrap_or(0.0);
            let n = (cur + steps as f32 * 0.02).clamp(0.0, 1.0);
            let _ = v.SetMasterVolumeLevelScalar(n, std::ptr::null());
            if n > 0.0 { let _ = v.SetMute(false, std::ptr::null()); }
            show(Hud::Volume(n, false), 1600);
        }
    }
}

// ---------------------------------------------------------------- bandeja de archivos (soltar en el notch)
/// Archivos guardados en la bandeja: ruta y su icono (64 px) ya convertido.
pub static SHELF: Mutex<Vec<(String, Option<Arc<Vec<u8>>>)>> = Mutex::new(Vec::new());
pub static SHELF_VER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Icono del explorador para un archivo (el mismo que se ve en las carpetas).
pub fn file_icon(path: &str) -> Option<Vec<u8>> {
    unsafe {
        let mut fi = SHFILEINFOW::default();
        let r = SHGetFileInfoW(&HSTRING::from(path), windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0), Some(&mut fi), size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_LARGEICON);
        if r == 0 || fi.hIcon.is_invalid() { return None; }
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let bmp = wic.CreateBitmapFromHICON(fi.hIcon).ok();
        let _ = DestroyIcon(fi.hIcon);
        wic_square(&wic, &bmp?.into())
    }
}

pub fn shelf_add(paths: Vec<String>) {
    let n = {
        let mut s = SHELF.lock().unwrap();
        for p in paths {
            if s.iter().any(|(x, _)| *x == p) { continue; }
            let ic = file_icon(&p).map(Arc::new);
            s.push((p, ic));
        }
        while s.len() > 8 { s.remove(0); }
        s.len()
    };
    SHELF_VER.fetch_add(1, Ordering::Relaxed);
    if n > 0 { show(Hud::Shelf, 1800); }
}

pub fn shelf_clear() { SHELF.lock().unwrap().clear(); SHELF_VER.fetch_add(1, Ordering::Relaxed); unsafe { let _ = InvalidateRect(Some(bar_hwnd()), None, false); } }

// ---------------------------------------------------------------- portapapeles
/// Vista previa de lo último copiado (texto recortado, "Imagen" o "N archivos").
pub static CLIP: Mutex<String> = Mutex::new(String::new());

/// Se llama con WM_CLIPBOARDUPDATE: lee QUÉ se copió (solo para el aviso) y lo enseña.
pub fn clipboard_changed(hwnd: HWND) {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    unsafe {
        let mut desc = String::new();
        for _ in 0..3 {
            if OpenClipboard(Some(hwnd)).is_ok() {
                // Gestores de contraseñas y apps sensibles marcan lo copiado como privado: ni se lee ni se avisa
                let private = [
                    windows::core::w!("ExcludeClipboardContentFromMonitorProcessing"),
                    windows::core::w!("CanIncludeInClipboardHistory"),
                    windows::core::w!("Clipboard Viewer Ignore"),
                ]
                .iter()
                .any(|f| { let id = RegisterClipboardFormatW(*f); id != 0 && IsClipboardFormatAvailable(id).is_ok() });
                if private { let _ = CloseClipboard(); return; }
                if let Ok(h) = GetClipboardData(13 /* CF_UNICODETEXT */) {
                    let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
                    if !p.is_null() {
                        let mut n = 0; while *p.add(n) != 0 && n < 400 { n += 1; }
                        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                        let _ = GlobalUnlock(HGLOBAL(h.0));
                        let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
                        desc = format!("“{}”", trunc(&one, 28));
                    }
                } else if IsClipboardFormatAvailable(15 /* CF_HDROP */).is_ok() {
                    if let Ok(h) = GetClipboardData(15) {
                        let n = DragQueryFileW(HDROP(h.0), u32::MAX, None);
                        desc = if n == 1 { crate::i18n::t("1 archivo").into() } else { crate::i18n::tf("{} archivos", &[&n]) };
                    }
                } else if IsClipboardFormatAvailable(8 /* CF_DIB */).is_ok() {
                    desc = crate::i18n::t("Imagen").into();
                }
                let _ = CloseClipboard();
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        if desc.is_empty() { return; }
        *CLIP.lock().unwrap() = desc;
        show(Hud::Copied, 1500);
    }
}