//! Centro de control (menú del logo de Windows): panel flotante con desenfoque acrílico.
//! Interruptores reales (Wi-Fi, Bluetooth, modo oscuro, silencio), brillo y volumen, uso de CPU,
//! RAM y disco, y botones de energía.

use super::*;
use std::sync::mpsc::{channel as cc_channel, Sender as CcSender};
use windows::core::BSTR;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::System::Wmi::*;

const W_DIP: f32 = 320.0;
const PAD: f32 = 14.0;
const WM_CC_STATE: u32 = WM_USER + 20;
const TIMER_STATS: usize = 1;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Hit { User, Settings, Wifi, Bt, Dark, Mute, Bright, Vol, Lock, Sleep, Restart, Shutdown }

/// Estado que lee el hilo de trabajo (radios y brillo tardan; no pueden bloquear la interfaz).
#[derive(Clone, Default)]
struct SysState { wifi: Option<bool>, ssid: String, bt: Option<bool>, bright: Option<f32> }
static SYS: Mutex<SysState> = Mutex::new(SysState { wifi: None, ssid: String::new(), bt: None, bright: None });

enum Job { Refresh, Radio(bool, bool), Bright(f32) }
static JOBS: OnceLock<CcSender<Job>> = OnceLock::new();

struct Cc {
    hwnd: HWND,
    target: Option<ID2D1HwndRenderTarget>,
    dw: IDWriteFactory,
    cache: RefCell<Caches>,
    f_name: IDWriteTextFormat, f_sub: IDWriteTextFormat, f_lbl: IDWriteTextFormat, f_val: IDWriteTextFormat,
    f_icon: IDWriteTextFormat, f_icon_l: IDWriteTextFormat,
    hits: Vec<(D2D_RECT_F, Hit)>,
    hover: Option<Hit>,
    drag: Option<Hit>,
    avatar_px: Option<Vec<u8>>,
    avatar: Option<ID2D1Bitmap>,
    title: String,
    subtitle: String,
    // Valores locales (rápidos)
    vol: f32, mute: bool, dark: bool,
    cpu: f32, ram: f32, disk: f32,
    cpu_prev: Option<(u64, u64)>,
    // Valor de brillo mientras se arrastra (para no esperar al hilo)
    bright_drag: Option<f32>,
    height: f32,
    hidden_at: Option<Instant>,
    /// Valores que se mueven con suavidad hacia su objetivo (brillos, colores, deslizadores, cifras).
    anim: HashMap<String, f32>,
    last_paint: Instant,
    /// Al abrir: los bloques entran en cascada (como el panel del tiempo).
    opened: Instant,
}

const TIMER_ANIM_CC: usize = 93;

/// Lleva `key` hacia `target` (aproximación exponencial con constante `tau`) y devuelve el valor actual.
fn ease_to(m: &mut HashMap<String, f32>, key: &str, target: f32, tau: f32, dt: f32, moving: &mut bool) -> f32 {
    let v = m.entry(key.to_string()).or_insert(target);
    let eps = (target.abs() * 0.002).max(0.002);
    if (*v - target).abs() < eps { *v = target; } else { *v += (target - *v) * (1.0 - (-dt / tau).exp()); *moving = true; }
    *v
}

thread_local! { static CC: RefCell<Option<Cc>> = const { RefCell::new(None) }; }

// ---------------------------------------------------------------- sistema: audio, tema, estadísticas
fn endpoint_volume() -> Option<IAudioEndpointVolume> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok()
    }
}

fn read_audio() -> (f32, bool) {
    endpoint_volume().map(|v| unsafe { (v.GetMasterVolumeLevelScalar().unwrap_or(0.0), v.GetMute().map(|b| b.as_bool()).unwrap_or(false)) }).unwrap_or((0.0, false))
}

fn set_volume(x: f32) { if let Some(v) = endpoint_volume() { unsafe { let _ = v.SetMasterVolumeLevelScalar(x.clamp(0.0, 1.0), std::ptr::null()); if x > 0.0 { let _ = v.SetMute(false, std::ptr::null()); } } } }
fn set_mute(m: bool) { if let Some(v) = endpoint_volume() { unsafe { let _ = v.SetMute(m, std::ptr::null()); } } }

const THEME_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";

fn read_dark() -> bool {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut v = 1u32; let mut sz = 4u32;
        let _ = RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(THEME_KEY), w!("AppsUseLightTheme"), RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as _), Some(&mut sz));
        v == 0
    }
}

/// Cambia el tema de apps y sistema y avisa a Windows para que lo aplique al momento.
fn set_dark(dark: bool) {
    use windows::Win32::System::Registry::*;
    unsafe {
        let v: u32 = if dark { 0 } else { 1 };
        for name in [w!("AppsUseLightTheme"), w!("SystemUsesLightTheme")] {
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(THEME_KEY), name, REG_DWORD.0, Some(&v as *const _ as _), 4);
        }
        let msg = HSTRING::from("ImmersiveColorSet");
        let mut res = 0usize;
        let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(msg.as_ptr() as isize), SMTO_ABORTIFHUNG, 200, Some(&mut res));
    }
}

fn ft(f: FILETIME) -> u64 { ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64 }

fn cpu_times() -> Option<(u64, u64)> {
    use windows::Win32::System::Threading::GetSystemTimes;
    let (mut i, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)).ok()?; }
    Some((ft(i), ft(k) + ft(u)))
}

fn ram_load() -> f32 {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { if GlobalMemoryStatusEx(&mut m).is_ok() { m.dwMemoryLoad as f32 } else { 0.0 } }
}

fn disk_used() -> f32 {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let (mut free, mut total) = (0u64, 0u64);
    unsafe {
        if GetDiskFreeSpaceExW(w!("C:\\"), None, Some(&mut total), Some(&mut free)).is_ok() && total > 0 {
            return (1.0 - free as f32 / total as f32) * 100.0;
        }
    }
    0.0
}

// ---------------------------------------------------------------- hilo de trabajo: radios, Wi-Fi y brillo (WMI)
struct Wmi { svc: IWbemServices }

impl Wmi {
    fn new() -> Option<Self> {
        unsafe {
            let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
            let svc = loc.ConnectServer(&BSTR::from("ROOT\\WMI"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None).ok()?;
            // Sin suplantación WMI rechaza leer y cambiar el brillo
            let _ = CoSetProxyBlanket(&svc, 10 /* WINNT */, 0 /* sin autorización */, PCWSTR::null(),
                RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE);
            Some(Wmi { svc })
        }
    }
    fn first(&self, q: &str) -> Option<IWbemClassObject> {
        unsafe {
            let en = self.svc.ExecQuery(&BSTR::from("WQL"), &BSTR::from(q), WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None).ok()?;
            let mut objs = [None];
            let mut n = 0u32;
            let _ = en.Next(WBEM_INFINITE, &mut objs, &mut n);
            objs[0].take()
        }
    }
    fn brightness(&self) -> Option<f32> {
        use windows::Win32::System::Variant::{VariantClear, VariantToInt32, VARIANT};
        unsafe {
            let o = self.first("SELECT CurrentBrightness FROM WmiMonitorBrightness")?;
            let mut v = VARIANT::default();
            o.Get(w!("CurrentBrightness"), 0, &mut v, None, None).ok()?;
            let r = VariantToInt32(&v).ok();
            let _ = VariantClear(&mut v);
            Some(r? as f32 / 100.0)
        }
    }
    fn set_brightness(&self, x: f32) -> Option<()> {
        use windows::Win32::System::Variant::*;
        unsafe {
            let o = self.first("SELECT * FROM WmiMonitorBrightnessMethods")?;
            let mut pv = VARIANT::default();
            o.Get(w!("__PATH"), 0, &mut pv, None, None).ok()?;
            let path = BSTR::from((*pv.Anonymous.Anonymous).Anonymous.bstrVal.to_string());
            let _ = VariantClear(&mut pv);
            // VARIANT a mano: la versión del crate no trae conversiones
            let mut vb = VARIANT::default();
            { let i = &mut *vb.Anonymous.Anonymous; i.vt = VT_UI1; i.Anonymous.bVal = (x.clamp(0.0, 1.0) * 100.0).round() as u8; }
            let mut vt = VARIANT::default();
            { let i = &mut *vt.Anonymous.Anonymous; i.vt = VT_I4; i.Anonymous.lVal = 0; } // WMI quiere los uint32 como VT_I4
            let mut cls = None;
            self.svc.GetObject(&BSTR::from("WmiMonitorBrightnessMethods"), WBEM_GENERIC_FLAG_TYPE(0), None, Some(&mut cls), None).ok()?;
            let mut inp = None;
            cls?.GetMethod(w!("WmiSetBrightness"), 0, &mut inp, std::ptr::null_mut()).ok()?;
            let inst = inp?.SpawnInstance(0).ok()?;
            inst.Put(w!("Brightness"), 0, &vb, 0).ok()?;
            inst.Put(w!("Timeout"), 0, &vt, 0).ok()?;
            self.svc.ExecMethod(&path, &BSTR::from("WmiSetBrightness"), WBEM_GENERIC_FLAG_TYPE(0), None, &inst, None, None).ok()?;
            Some(())
        }
    }
}

fn radios() -> Vec<windows::Devices::Radios::Radio> {
    use windows::Devices::Radios::Radio;
    let Ok(list) = Radio::GetRadiosAsync().and_then(|o| o.join()) else { return Vec::new() };
    (0..list.Size().unwrap_or(0)).filter_map(|i| list.GetAt(i).ok()).collect()
}

fn radio_state(kind: windows::Devices::Radios::RadioKind) -> Option<bool> {
    use windows::Devices::Radios::RadioState;
    radios().into_iter().find(|r| r.Kind().ok() == Some(kind)).and_then(|r| r.State().ok()).map(|s| s == RadioState::On)
}

/// SSID de la red Wi-Fi conectada (WLAN API).
fn wifi_ssid() -> String {
    use windows::Win32::NetworkManagement::WiFi::*;
    if sys::demo() { return "Casa".into(); }
    unsafe {
        let mut ver = 0u32;
        let mut h = HANDLE::default();
        if WlanOpenHandle(2, None, &mut ver, &mut h) != 0 { return String::new(); }
        let mut out = String::new();
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if WlanEnumInterfaces(h, None, &mut list) == 0 && !list.is_null() {
            let l = &*list;
            let ifs = std::slice::from_raw_parts(l.InterfaceInfo.as_ptr(), l.dwNumberOfItems as usize);
            for i in ifs {
                if i.isState != wlan_interface_state_connected { continue; }
                let mut size = 0u32;
                let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
                if WlanQueryInterface(h, &i.InterfaceGuid, wlan_intf_opcode_current_connection, None, &mut size, &mut data, None) == 0 && !data.is_null() {
                    let a = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                    let s = &a.wlanAssociationAttributes.dot11Ssid;
                    out = String::from_utf8_lossy(&s.ucSSID[..s.uSSIDLength as usize]).to_string();
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

fn worker(rx: std::sync::mpsc::Receiver<Job>) {
    use windows::Devices::Radios::{RadioKind, RadioState};
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let wmi = Wmi::new();
    let post = |hwnd: isize| unsafe { let _ = PostMessageW(Some(HWND(hwnd as _)), WM_CC_STATE, WPARAM(0), LPARAM(0)); };
    let cc_hwnd = || CC_HWND.load(Ordering::Relaxed);
    while let Ok(job) = rx.recv() {
        match job {
            Job::Refresh => {
                let st = SysState {
                    wifi: radio_state(RadioKind::WiFi),
                    ssid: wifi_ssid(),
                    bt: radio_state(RadioKind::Bluetooth),
                    bright: wmi.as_ref().and_then(|w| w.brightness()),
                };
                *SYS.lock().unwrap() = st;
            }
            Job::Radio(wifi, on) => {
                let kind = if wifi { RadioKind::WiFi } else { RadioKind::Bluetooth };
                if let Some(r) = radios().into_iter().find(|r| r.Kind().ok() == Some(kind)) {
                    let _ = r.SetStateAsync(if on { RadioState::On } else { RadioState::Off }).and_then(|o| o.join());
                }
                std::thread::sleep(Duration::from_millis(400));
                let mut s = SYS.lock().unwrap();
                if wifi { s.wifi = radio_state(RadioKind::WiFi); } else { s.bt = radio_state(RadioKind::Bluetooth); }
                drop(s);
                if wifi { std::thread::sleep(Duration::from_millis(1500)); SYS.lock().unwrap().ssid = wifi_ssid(); }
            }
            Job::Bright(x) => {
                // Solo el último valor pedido: al arrastrar llegan muchos seguidos
                let mut last = x;
                while let Ok(Job::Bright(y)) = rx.try_recv() { last = y; }
                if let Some(w) = &wmi { if w.set_brightness(last).is_some() { SYS.lock().unwrap().bright = Some(last); } }
            }
        }
        post(cc_hwnd());
    }
}

static CC_HWND: AtomicIsize = AtomicIsize::new(0);
fn job(j: Job) { if let Some(tx) = JOBS.get() { let _ = tx.send(j); } }

// ---------------------------------------------------------------- ventana
pub fn toggle(bar: HWND) {
    let now_visible = is_open();
    if now_visible { hide(); return; }
    // Un clic en el logo justo después de cerrarse (al perder el foco) no debe reabrirlo
    let just = CC.with(|c| c.borrow().as_ref().and_then(|c| c.hidden_at).is_some_and(|t| t.elapsed() < Duration::from_millis(350)));
    if just { return; }
    if CC.with(|c| c.borrow().is_none()) { create(); }
    show(bar);
}

pub fn is_open() -> bool { CC.with(|c| c.borrow().as_ref().is_some_and(|c| popup::shown(c.hwnd))) }
pub fn close() { hide(); }

/// Crea el panel por adelantado (oculto) con los datos que no cambian: el primer clic es instantáneo.
pub fn prewarm() {
    if CC.with(|c| c.borrow().is_none()) { create(); }
    let title = sys::display_name();
    let dv = read_reg_sz(windows::Win32::System::Registry::HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", "DisplayVersion").unwrap_or_default();
    let pc = if sys::demo() { "DESKTOP".into() } else { std::env::var("COMPUTERNAME").unwrap_or_default() };
    CC.with(|c| if let Some(c) = c.borrow_mut().as_mut() {
        c.title = title;
        c.subtitle = format!("{} · Windows 11 {}", title_case(&pc), dv).trim().to_string();
    });
    job(Job::Refresh);
}

/// Oculta el panel. Las llamadas a Windows van FUERA del préstamo del estado: ShowWindow manda
/// WM_ACTIVATE al momento, que vuelve a llamar a hide(), y un doble préstamo abortaría el programa.
fn hide() {
    let hwnd = CC.with(|c| c.borrow_mut().as_mut().map(|c| {
        c.hidden_at = Some(Instant::now());
        c.drag = None;
        c.hwnd
    }));
    let Some(hwnd) = hwnd else { return };
    unsafe {
        if !popup::shown(hwnd) { return; }
        let _ = KillTimer(Some(hwnd), TIMER_STATS);
        let _ = ReleaseCapture();
        popup::dismiss(hwnd);
    }
}

fn create() {
    unsafe {
        // Vidrio con desenfoque y esquinas redondeadas de Windows 11 (ver popup.rs)
        let hwnd = popup::create_window(w!("MacBarControlCenter"), w!("Centro de control"), Some(wndproc));
        CC_HWND.store(hwnd.0 as isize, Ordering::Relaxed);

        let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap();
        let mk = |fam, size, wt| make_format(&dw, fam, size, wt);
        let cc = Cc {
            hwnd, target: None,
            f_name: mk(FONT, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            f_sub: mk(FONT, 11.5, DWRITE_FONT_WEIGHT_NORMAL),
            f_lbl: mk(FONT, 12.5, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            f_val: mk(FONT, 15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            f_icon: mk(ICONS, 14.0, DWRITE_FONT_WEIGHT_NORMAL),
            f_icon_l: mk(ICONS, 16.0, DWRITE_FONT_WEIGHT_NORMAL),
            dw, cache: RefCell::new(Caches::default()),
            hits: Vec::new(), hover: None, drag: None,
            avatar_px: None, avatar: None, title: String::new(), subtitle: String::new(),
            vol: 0.0, mute: false, dark: true, cpu: 0.0, ram: 0.0, disk: 0.0, cpu_prev: None, bright_drag: None,
            height: 400.0, hidden_at: None, anim: HashMap::new(), last_paint: Instant::now(), opened: Instant::now(),
        };
        CC.with(|c| *c.borrow_mut() = Some(cc));
        // El canal existe ANTES de lanzar el hilo: así el primer trabajo nunca se pierde
        let (tx, rx) = cc_channel::<Job>();
        let _ = JOBS.set(tx);
        std::thread::spawn(move || worker(rx));
    }
}

fn show(bar: HWND) {
    let avatar = avatar_pixels();
    unsafe {
        let s = scale_of(bar);
        CC.with(|c| {
            let mut c = c.borrow_mut();
            let Some(c) = c.as_mut() else { return };
            // La foto ya la tiene la barra (y la vigila): sin volver a leer el archivo
            if avatar.is_some() && avatar != c.avatar_px { c.avatar_px = avatar; c.avatar = None; }
            let (v, m) = read_audio();
            c.vol = v; c.mute = m; c.dark = read_dark();
            c.ram = ram_load(); c.disk = disk_used();
            c.cpu_prev = cpu_times();
            c.bright_drag = None;
            // Al abrir, las cifras del equipo cuentan desde 0
            c.anim.clear();
            for k in ["CPU", "RAM", "Disco"] { c.anim.insert(k.to_string(), 0.0); }
            c.last_paint = Instant::now();
            c.opened = Instant::now();
            let has_bright = SYS.lock().unwrap().bright.is_some();
            c.height = layout_height(has_bright);
            c.target = None;
        });
        // Mostrar y dar el foco fuera del préstamo (mandan mensajes al panel al momento)
        let Some((hwnd, hh)) = CC.with(|c| c.borrow().as_ref().map(|c| (c.hwnd, c.height))) else { return };
        let (wpx, hpx) = ((W_DIP * s) as i32, (hh * s) as i32);
        popup::begin(hwnd, (8.0 * s) as i32, ((BAR_H + 4.0) * s) as i32, wpx, hpx);
        let _ = SetForegroundWindow(hwnd);
        SetTimer(Some(hwnd), TIMER_STATS, 1000, None);
        job(Job::Refresh);
    }
}

fn read_reg_sz(root: windows::Win32::System::Registry::HKEY, key: &str, val: &str) -> Option<String> {
    use windows::Win32::System::Registry::*;
    unsafe {
        let mut buf = vec![0u16; 256];
        let mut size = (buf.len() * 2) as u32;
        RegGetValueW(root, &HSTRING::from(key), &HSTRING::from(val), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut size)).ok().ok()?;
        Some(String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]))
    }
}

fn title_case(s: &str) -> String {
    // "OMEN-PELAYO" → "Omen-Pelayo"
    let mut out = String::new();
    let mut up = true;
    for ch in s.chars() {
        if up { out.extend(ch.to_uppercase()); } else { out.extend(ch.to_lowercase()); }
        up = !ch.is_alphanumeric();
    }
    out
}

// ---------------------------------------------------------------- composición
const HEADER_H: f32 = 44.0;
const CARD_H: f32 = 54.0;
const SLIDER_H: f32 = 52.0;
const STATS_H: f32 = 56.0;
const POWER_H: f32 = 44.0;
const GAP: f32 = 8.0;

fn layout_height(has_bright: bool) -> f32 {
    let sliders = if has_bright { 2.0 } else { 1.0 };
    PAD + HEADER_H + 12.0 + CARD_H * 2.0 + GAP + 12.0 + sliders * SLIDER_H + (sliders - 1.0) * GAP + 12.0 + STATS_H + 12.0 + POWER_H + PAD
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        BeginPaint(hwnd, &mut ps);
        CC.with(|cc| {
            let mut cc = cc.borrow_mut();
            let Some(c) = cc.as_mut() else { return };
            let s = scale_of(hwnd);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            if c.target.is_none() {
                let Ok(fac) = D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) else { return };
                let props = D2D1_RENDER_TARGET_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, ..Default::default() };
                let hp = D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd, pixelSize: D2D_SIZE_U { width: (rc.right - rc.left) as u32, height: (rc.bottom - rc.top) as u32 }, presentOptions: D2D1_PRESENT_OPTIONS_NONE };
                c.target = fac.CreateHwndRenderTarget(&props, &hp).ok();
                if let Some(t) = &c.target { t.SetDpi(96.0 * s, 96.0 * s); t.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE); }
                c.avatar = None;
            }
            let Some(rt) = c.target.clone() else { return };
            if c.avatar.is_none() {
                if let Some(px) = &c.avatar_px {
                    let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                    c.avatar = rt.CreateBitmap(D2D_SIZE_U { width: ART_PX, height: ART_PX }, Some(px.as_ptr() as _), ART_PX * 4, &bp).ok();
                }
            }
            let st = SYS.lock().unwrap().clone();
            let w = W_DIP;
            let ph = c.height;
            // Altura de referencia fija: el centrado vertical se cachea y no debe depender del alto del panel
            let h = 100.0;
            let p = Painter { rt: &rt, dw: &c.dw, cache: &c.cache, h, s };
            let white = p.brush(rgba(1.0, 1.0, 1.0, 0.95));
            let dim = p.brush(rgba(1.0, 1.0, 1.0, 0.55));
            let card = p.brush(rgba(1.0, 1.0, 1.0, 0.075));
            let card_edge = p.brush(rgba(1.0, 1.0, 1.0, 0.09));
            let off = p.brush(rgba(1.0, 1.0, 1.0, 0.13));
            let on = p.brush(rgba(0.23, 0.51, 0.96, 1.0));
            let track = p.brush(rgba(1.0, 1.0, 1.0, 0.14));
            let hov = p.brush(rgba(1.0, 1.0, 1.0, 0.07));
            let mut hits: Vec<(D2D_RECT_F, Hit)> = Vec::new();
            let dt = c.last_paint.elapsed().as_secs_f32().min(1.0 / 30.0);
            c.last_paint = Instant::now();
            let mut moving = false;
            let mut anim = std::mem::take(&mut c.anim);
            // Brillo de "ratón encima" de cada zona, con fundido (rápido al entrar, más lento al salir)
            let mut glow = |anim: &mut HashMap<String, f32>, h: Hit, moving: &mut bool| -> f32 {
                let t = if c.hover == Some(h) { 1.0 } else { 0.0 };
                let key = format!("h{h:?}");
                let cur = anim.get(&key).copied().unwrap_or(0.0);
                ease_to(anim, &key, t, if t > cur { 0.05 } else { 0.14 }, dt, moving)
            };

            rt.BeginDraw();
            rt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
            // Tinte oscuro encima del acrílico (el desenfoque lo pone Windows)
            let tint = p.brush(popup::TINT);
            rt.FillRectangle(&rect(0.0, 0.0, w, ph), &tint); popup::glass_edge(&rt, w, ph, s); rt.SetTransform(&popup::content_matrix(hwnd, w));
            // Entrada: el contenido aparece con un fundido mientras el panel baja a su sitio
            popup::push_reveal(&rt, hwnd, ph, popup::alpha(hwnd));

            // Texto en una caja [top, top+box]: centrado vertical por mayúscula, como en la barra
            let text_in = |f: &IDWriteTextFormat, t: &str, x: f32, top: f32, box_h: f32, b: &ID2D1Brush| {
                let cap = p.text_top(f) - h / 2.0; // desplazamiento relativo calculado para la altura h
                p.draw_at(f, t, x, top + box_h / 2.0 + cap, b, None);
            };
            let glyph_in = |f: &IDWriteTextFormat, g: &str, x: f32, top: f32, bw: f32, bh: f32, b: &ID2D1Brush| {
                let [l, t, iw, ih] = p.ink(f, g);
                p.draw_at(f, g, x + (bw - iw) / 2.0 - l, p.snap(top + bh / 2.0 - ih / 2.0 - t), b, None);
            };
            // Sin cajas: el contenido va directamente sobre el vidrio; solo brilla al pasar el ratón
            let card_at = |r: D2D_RECT_F, k: f32| {
                if k > 0.01 { rt.FillRoundedRectangle(&rrect(r, 10.0), &p.brush(rgba(1.0, 1.0, 1.0, 0.07 * k))); }
            };
            let _ = &hov;
            let sep_at = |y: f32| { let px = 1.0 / s; rt.FillRectangle(&rect(PAD + 4.0, y, w - PAD - 4.0, y + px), &card_edge); };

            // Cascada de entrada: cada bloque sube y aparece con un pequeño retraso
            let since = c.opened.elapsed().as_secs_f32();
            if since < 0.9 { moving = true; }
            let enter = |delay: f32| popup::rise(&rt, popup::stagger(since, delay, 0.32));

            // ---- Cabecera: avatar, nombre, equipo + Windows; engranaje a la derecha
            let b0 = enter(0.0);
            let mut y = PAD;
            let av = 36.0;
            match &c.avatar {
                Some(b) => p.art(b, PAD, y + (HEADER_H - av) / 2.0, av, None),
                None => { let e = D2D1_ELLIPSE { point: Vector2 { X: PAD + av / 2.0, Y: y + HEADER_H / 2.0 }, radiusX: av / 2.0, radiusY: av / 2.0 }; rt.FillEllipse(&e, &off); }
            }
            text_in(&c.f_name, &c.title, PAD + av + 10.0, y + 3.0, 20.0, &white);
            text_in(&c.f_sub, &c.subtitle, PAD + av + 10.0, y + 22.0, 18.0, &dim);
            let gear = rect(w - PAD - 30.0, y + 7.0, w - PAD, y + 37.0);
            let gk = glow(&mut anim, Hit::Settings, &mut moving);
            if gk > 0.01 { rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: (gear.left + gear.right) / 2.0, Y: (gear.top + gear.bottom) / 2.0 }, radiusX: 15.0, radiusY: 15.0 }, &p.brush(rgba(1.0, 1.0, 1.0, 0.07 * gk))); }
            // El engranaje gira un poco al pasar el ratón
            let ang = gk * std::f32::consts::FRAC_PI_3;
            let (gcx, gcy) = ((gear.left + gear.right) / 2.0, (gear.top + gear.bottom) / 2.0);
            // La transformación actual (incluye el desplazamiento de la cascada)
            let mut cm = Matrix3x2::default();
            rt.GetTransform(&mut cm);
            if ang > 0.001 {
                let (cs, sn) = (ang.cos(), ang.sin());
                let rot = Matrix3x2 { M11: cs, M12: sn, M21: -sn, M22: cs, M31: gcx - cs * gcx + sn * gcy, M32: gcy - sn * gcx - cs * gcy };
                rt.SetTransform(&(rot * cm));
            }
            glyph_in(&c.f_icon_l, "\u{E713}", gear.left, gear.top, 30.0, 30.0, &white);
            rt.SetTransform(&cm);
            hits.push((gear, Hit::Settings));
            hits.push((rect(PAD, y, gear.left - 6.0, y + HEADER_H), Hit::User));
            popup::rise_end(&rt, b0);
            y += HEADER_H + 12.0;
            sep_at(y - 6.0);

            // ---- Interruptores 2×2
            let cw = (w - PAD * 2.0 - GAP) / 2.0;
            let wifi_sub = match st.wifi { Some(true) if !st.ssid.is_empty() => st.ssid.clone(), Some(true) => "Activado".into(), Some(false) => "Desactivado".into(), None => "No disponible".into() };
            let bt_sub = match st.bt { Some(true) => "Activado".into(), Some(false) => "Desactivado".into(), None => "No disponible".into() };
            let tiles: [(&str, &str, String, bool, Hit); 4] = [
                ("\u{E701}", "Wi-Fi", wifi_sub, st.wifi == Some(true), Hit::Wifi),
                ("\u{E702}", "Bluetooth", bt_sub, st.bt == Some(true), Hit::Bt),
                ("\u{E793}", "Modo oscuro", if c.dark { "Activado".into() } else { "Desactivado".into() }, c.dark, Hit::Dark),
                (if c.mute { "\u{E74F}" } else { "\u{E767}" }, "Silencio", if c.mute { "Activado".into() } else { "Desactivado".into() }, c.mute, Hit::Mute),
            ];
            for (i, (gl, lbl, sub, active, hit)) in tiles.iter().enumerate() {
                let cx = PAD + (i % 2) as f32 * (cw + GAP);
                let cy0 = y + (i / 2) as f32 * (CARD_H + GAP);
                let r = rect(cx, cy0, cx + cw, cy0 + CARD_H);
                let bt = enter(0.06 + i as f32 * 0.04);
                card_at(r, glow(&mut anim, *hit, &mut moving));
                // El círculo pasa de gris a azul (y al revés) con fundido, y da un pequeño "pop" al cambiar
                let ak = ease_to(&mut anim, &format!("a{hit:?}"), if *active { 1.0 } else { 0.0 }, 0.09, dt, &mut moving);
                let pop = 1.0 + 0.12 * (ak * std::f32::consts::PI).sin();
                let cr = 15.0 * pop;
                let ccx = cx + 10.0 + 15.0;
                let e = D2D1_ELLIPSE { point: Vector2 { X: ccx, Y: cy0 + CARD_H / 2.0 }, radiusX: cr, radiusY: cr };
                let mix = |a: f32, b: f32| a + (b - a) * ak;
                rt.FillEllipse(&e, &p.brush(rgba(mix(1.0, 0.23), mix(1.0, 0.51), mix(1.0, 0.96), mix(0.13, 1.0))));
                let _ = (&on, &off);
                let cr = 15.0;
                glyph_in(&c.f_icon, gl, ccx - cr, cy0 + CARD_H / 2.0 - cr, cr * 2.0, cr * 2.0, &white);
                let tx = cx + 10.0 + cr * 2.0 + 9.0;
                text_in(&c.f_lbl, lbl, tx, cy0 + 9.0, 18.0, &white);
                text_in(&c.f_sub, &trunc(sub, 14), tx, cy0 + 27.0, 17.0, &dim);
                popup::rise_end(&rt, bt);
                hits.push((r, *hit));
            }
            y += CARD_H * 2.0 + GAP + 12.0;
            sep_at(y - 6.0);

            // ---- Deslizadores
            let drag = c.drag;
            let mut slider = |y: &mut f32, gl: &str, lbl: &str, val: f32, hit: Hit, anim: &mut HashMap<String, f32>, moving: &mut bool| {
                let r = rect(PAD, *y, w - PAD, *y + SLIDER_H);
                card_at(r, 0.0);
                // Arrastrando: sigue al dedo al instante; si cambia desde fuera, se desliza
                let dragging = drag == Some(hit);
                let key = format!("v{hit:?}");
                let val = if dragging { anim.insert(key, val); val } else { ease_to(anim, &key, val, 0.08, dt, moving) };
                // El pomo crece un poco mientras se arrastra o al pasar por encima
                let kk = ease_to(anim, &format!("k{hit:?}"), if dragging { 1.0 } else if c.hover == Some(hit) { 0.5 } else { 0.0 }, 0.06, dt, moving);
                text_in(&c.f_sub, lbl, PAD + 12.0, *y + 7.0, 16.0, &dim);
                glyph_in(&c.f_icon, gl, PAD + 8.0, *y + 24.0, 22.0, 20.0, &white);
                let tl = PAD + 36.0;
                let tr = w - PAD - 14.0;
                let ty = *y + 34.0;
                let th = 8.0;
                rt.FillRoundedRectangle(&rrect(rect(tl, ty - th / 2.0, tr, ty + th / 2.0), th / 2.0), &track);
                let fx = tl + (tr - tl) * val.clamp(0.0, 1.0);
                if fx > tl + 1.0 { rt.FillRoundedRectangle(&rrect(rect(tl, ty - th / 2.0, fx.max(tl + th), ty + th / 2.0), th / 2.0), &white); }
                let kr = 7.0 + 2.5 * kk;
                let knob = D2D1_ELLIPSE { point: Vector2 { X: fx.clamp(tl + 7.0, tr - 7.0), Y: ty }, radiusX: kr, radiusY: kr };
                rt.FillEllipse(&knob, &white);
                // La zona de arrastre es la pista entera, algo más alta para acertar fácil
                hits.push((rect(tl - 6.0, *y + 20.0, tr + 6.0, *y + SLIDER_H), hit));
                *y += SLIDER_H + GAP;
            };
            if let Some(b) = c.bright_drag.or(st.bright) {
                let bs = enter(0.22);
                slider(&mut y, "\u{E706}", "Brillo", b, Hit::Bright, &mut anim, &mut moving);
                popup::rise_end(&rt, bs);
            }
            let bs = enter(0.26);
            slider(&mut y, if c.mute || c.vol == 0.0 { "\u{E74F}" } else { "\u{E995}" }, "Volumen", if c.mute { 0.0 } else { c.vol }, Hit::Vol, &mut anim, &mut moving);
            popup::rise_end(&rt, bs);
            y += 12.0 - GAP;
            sep_at(y - 6.0);

            // ---- Estado del equipo
            let r = rect(PAD, y, w - PAD, y + STATS_H);
            card_at(r, 0.0);
            let colw = (w - PAD * 2.0) / 3.0;
            for (i, (v, l)) in [(c.cpu, "CPU"), (c.ram, "RAM"), (c.disk, "Disco")].iter().enumerate() {
                let x0 = PAD + i as f32 * colw;
                let bc = enter(0.32 + i as f32 * 0.03);
                // Las cifras cuentan hasta su valor nuevo
                let v = ease_to(&mut anim, l, *v, 0.25, dt, &mut moving);
                let vt = format!("{}%", v.round() as i32);
                let vw = p.width(&c.f_val, &vt);
                text_in(&c.f_val, &vt, x0 + (colw - vw) / 2.0, y + 8.0, 22.0, &white);
                let lw = p.width(&c.f_sub, l);
                text_in(&c.f_sub, l, x0 + (colw - lw) / 2.0, y + 31.0, 16.0, &dim);
                popup::rise_end(&rt, bc);
            }
            y += STATS_H + 12.0;
            sep_at(y - 6.0);

            // ---- Energía: bloquear, suspender, reiniciar, apagar
            let pw = (w - PAD * 2.0) / 4.0;
            for (i, (gl, hit)) in [("\u{E72E}", Hit::Lock), ("\u{E708}", Hit::Sleep), ("\u{E72C}", Hit::Restart), ("\u{E7E8}", Hit::Shutdown)].iter().enumerate() {
                let cx = PAD + i as f32 * pw + pw / 2.0;
                let bp = enter(0.40 + i as f32 * 0.035);
                let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: y + POWER_H / 2.0 }, radiusX: 19.0, radiusY: 19.0 };
                let hk = glow(&mut anim, *hit, &mut moving);
                rt.FillEllipse(&e, &p.brush(rgba(1.0, 1.0, 1.0, 0.075 + 0.055 * hk)));
                let _ = &card;
                glyph_in(&c.f_icon_l, gl, cx - 19.0, y + POWER_H / 2.0 - 19.0, 38.0, 38.0, &white);
                popup::rise_end(&rt, bp);
                hits.push((rect(cx - 20.0, y + 2.0, cx + 20.0, y + POWER_H - 2.0), *hit));
            }

            c.hits = hits;
            c.anim = anim;
            if moving { SetTimer(Some(hwnd), TIMER_ANIM_CC, 15, None); }
            rt.PopLayer();
            rt.SetTransform(&popup::IDENTITY);
            if rt.EndDraw(None, None).is_err() { c.target = None; c.avatar = None; }
        });
        let _ = EndPaint(hwnd, &ps);
    }
}

// ---------------------------------------------------------------- interacción
fn hit_at(hwnd: HWND, lp: LPARAM) -> Option<(Hit, f32)> {
    let s = scale_of(hwnd);
    let x = (lp.0 & 0xFFFF) as i16 as f32 / s;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / s;
    CC.with(|c| c.borrow().as_ref().and_then(|c| c.hits.iter().find(|(r, _)| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom).map(|(_, h)| (*h, x))))
}

/// Valor 0..1 de un deslizador según la X del ratón (misma pista que se dibuja).
fn slider_value(x: f32) -> f32 { let (tl, tr) = (PAD + 36.0, W_DIP - PAD - 14.0); ((x - tl) / (tr - tl)).clamp(0.0, 1.0) }

fn apply_slider(hit: Hit, x: f32) {
    let v = slider_value(x);
    CC.with(|c| if let Some(c) = c.borrow_mut().as_mut() {
        match hit {
            Hit::Vol => { c.vol = v; c.mute = false; set_volume(v); }
            Hit::Bright => { c.bright_drag = Some(v); job(Job::Bright(v)); }
            _ => {}
        }
    });
}

fn click(hit: Hit) {
    match hit {
        Hit::User => { hide(); sys::open("ms-settings:yourinfo"); }
        Hit::Settings => { hide(); sys::open("ms-settings:"); }
        Hit::Wifi => { let on = SYS.lock().unwrap().wifi; if let Some(on) = on { SYS.lock().unwrap().wifi = Some(!on); job(Job::Radio(true, !on)); } }
        Hit::Bt => { let on = SYS.lock().unwrap().bt; if let Some(on) = on { SYS.lock().unwrap().bt = Some(!on); job(Job::Radio(false, !on)); } }
        Hit::Dark => { let d = CC.with(|c| c.borrow_mut().as_mut().map(|c| { c.dark = !c.dark; c.dark })).unwrap_or(true); set_dark(d); }
        Hit::Mute => { let m = CC.with(|c| c.borrow_mut().as_mut().map(|c| { c.mute = !c.mute; c.mute })).unwrap_or(false); set_mute(m); }
        Hit::Lock => { hide(); sys::lock(); }
        Hit::Sleep => { hide(); sys::sleep(); }
        Hit::Restart => { hide(); if sys::ask(bar_hwnd(), "¿Reiniciar el equipo ahora?") { sys::restart(); } }
        Hit::Shutdown => { hide(); if sys::ask(bar_hwnd(), "¿Apagar el equipo ahora?") { sys::shutdown(); } }
        Hit::Bright | Hit::Vol => {}
    }
}

fn redraw(hwnd: HWND) { unsafe { let _ = InvalidateRect(Some(hwnd), None, false); } }

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => { paint(hwnd); LRESULT(0) }
            WM_ERASEBKGND => LRESULT(1),
            WM_ACTIVATE => {
                // Al hacer clic fuera se cierra, como un menú
                if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE { hide(); }
                LRESULT(0)
            }
            WM_KEYDOWN => { if wp.0 as u16 == VK_ESCAPE.0 { hide(); } LRESULT(0) }
            WM_TIMER => {
                if wp.0 == popup::TIMER_REVEAL { popup::step(hwnd); return LRESULT(0); }
                if wp.0 == TIMER_ANIM_CC { let _ = KillTimer(Some(hwnd), TIMER_ANIM_CC); redraw(hwnd); return LRESULT(0); }
                CC.with(|c| if let Some(c) = c.borrow_mut().as_mut() {
                    if let (Some((i0, b0)), Some((i1, b1))) = (c.cpu_prev, cpu_times()) {
                        let busy = (b1 - b0) as f32;
                        let idle = (i1 - i0) as f32;
                        if busy > 0.0 { c.cpu = ((busy - idle) / busy * 100.0).clamp(0.0, 100.0); }
                    }
                    c.cpu_prev = cpu_times();
                    c.ram = ram_load();
                    if c.drag.is_none() { let (v, m) = read_audio(); c.vol = v; c.mute = m; }
                });
                redraw(hwnd);
                LRESULT(0)
            }
            WM_CC_STATE => {
                // Si el brillo ya se aplicó, deja de mostrar el valor provisional del arrastre
                CC.with(|c| if let Some(c) = c.borrow_mut().as_mut() { if c.drag.is_none() { c.bright_drag = None; } });
                // La altura cambia si aparece o desaparece el brillo
                let has_b = SYS.lock().unwrap().bright.is_some();
                let resize = CC.with(|c| c.borrow_mut().as_mut().map(|c| { let nh = layout_height(has_b); let ch = c.height != nh; c.height = nh; ch }).unwrap_or(false));
                if resize {
                    let s = scale_of(hwnd);
                    let hh = CC.with(|c| c.borrow().as_ref().map(|c| c.height).unwrap_or(400.0));
                    let _ = SetWindowPos(hwnd, None, 0, 0, (W_DIP * s) as i32, (hh * s) as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
                    CC.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.target = None; });
                }
                redraw(hwnd);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let drag = CC.with(|c| c.borrow().as_ref().and_then(|c| c.drag));
                if let Some(d) = drag {
                    let x = (lp.0 & 0xFFFF) as i16 as f32 / scale_of(hwnd);
                    apply_slider(d, x);
                } else {
                    let h = hit_at(hwnd, lp).map(|(h, _)| h);
                    let changed = CC.with(|c| c.borrow_mut().as_mut().map(|c| { let ch = c.hover != h; c.hover = h; ch }).unwrap_or(false));
                    if changed { redraw(hwnd); }
                    let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut tme);
                    return LRESULT(0);
                }
                redraw(hwnd);
                LRESULT(0)
            }
            0x02A3 => { CC.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.hover = None; }); redraw(hwnd); LRESULT(0) }
            WM_LBUTTONDOWN => {
                if let Some((h, x)) = hit_at(hwnd, lp) {
                    if matches!(h, Hit::Bright | Hit::Vol) {
                        CC.with(|c| if let Some(c) = c.borrow_mut().as_mut() { c.drag = Some(h); });
                        SetCapture(hwnd);
                        apply_slider(h, x);
                        redraw(hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let drag = CC.with(|c| c.borrow_mut().as_mut().and_then(|c| c.drag.take()));
                if drag.is_some() {
                    let _ = ReleaseCapture();
                } else if let Some((h, _)) = hit_at(hwnd, lp) {
                    click(h);
                }
                redraw(hwnd);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
