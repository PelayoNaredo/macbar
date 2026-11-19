//! Móvil de Enlace Móvil: si la app está en marcha y las notificaciones del móvil que Enlace Móvil pone
//! en Windows (sus bases de datos van cifradas: lo único legible son esos avisos). Prueba: MACBAR_PHONETEST=1.

use super::*;
use std::sync::atomic::AtomicBool;
use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};

#[derive(Clone, Default, PartialEq)]
pub struct PNotif { pub id: u32, pub title: String, pub body: String, pub when: u64, pub app: String }

/// Foto o vídeo reciente del móvil: ruta, miniatura (THUMB × THUMB, BGRA) y si es vídeo.
#[derive(Clone)]
pub struct PPhoto { pub path: String, pub thumb: Option<Arc<Vec<u8>>>, pub video: bool }
pub const THUMB: u32 = 192;

/// Fondo del móvil: proporción de pantalla de móvil (≈ 9:19).
pub const WALL_W: u32 = 132;
pub const WALL_H: u32 = 276;

/// `battery`: carga y cuándo se vio (solo se puede leer con la ventana de Enlace Móvil abierta).
#[derive(Clone, Default)]
pub struct PhoneState { pub ready: bool, pub access: bool, pub running: bool, pub notifs: Vec<PNotif>, pub photos: Vec<PPhoto>,
    pub name: String, pub battery: Option<(u8, u64)>, pub wall: Option<Arc<Vec<u8>>>,
    /// Contador del icono de Enlace Móvil (avisos sin leer), aunque los avisos no lleguen a Windows
    pub unread: u32 }

pub static PHONE: Mutex<PhoneState> = Mutex::new(PhoneState { ready: false, access: false, running: false, notifs: Vec::new(), photos: Vec::new(),
    name: String::new(), battery: None, wall: None, unread: 0 });
/// Pedir una lectura ya (al quitar un aviso, al volver a la vista…).
pub static PHONE_NOW: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);

const AUMID: &str = "Microsoft.YourPhone_8wekyb3d8bbwe";

/// Arranca (una vez) el hilo que lee el estado cada pocos segundos mientras los widgets se ven.
pub fn start() {
    if STARTED.swap(true, Ordering::Relaxed) { return; }
    std::thread::spawn(|| {
        unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
        let wic: Option<IWICImagingFactory> = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok() };
        let mut thumbs: HashMap<String, Option<Arc<Vec<u8>>>> = HashMap::new();
        let mut photos: Vec<PPhoto> = Vec::new();
        let mut round = 0u32;
        // Avisos ya vistos: solo los que llegan después del arranque salen en la isla
        let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut battery: Option<(u8, u64)> = None;
        let mut wall: (String, Option<Arc<Vec<u8>>>) = (String::new(), None);
        let name = if sys::demo() { "Mi móvil".to_string() } else { device_name() };
        loop {
            let mut st = if sys::demo() { demo() } else { read() };
            st.name = name.clone();
            let fresh: Vec<&PNotif> = st.notifs.iter().filter(|n| !seen.contains(&n.id)).collect();
            if round > 0 && !sys::demo() {
                if let Some(n) = fresh.iter().max_by_key(|n| n.when) {
                    *live::PHONE_NOTE.lock().unwrap() = (n.app.clone(), if n.title.is_empty() { n.body.clone() } else { n.title.clone() });
                    live::show(live::Hud::Phone, 4000);
                }
            }
            for n in &st.notifs { seen.insert(n.id); }
            // La batería solo la enseña la ventana de Enlace Móvil: se lee si está abierta y se recuerda
            if st.running && round % 3 == 0 { if let Some(b) = read_battery() { battery = Some((b, deskdata::now_unix())); } }
            st.battery = if sys::demo() { Some((72, deskdata::now_unix())) } else { battery };
            // El fondo lo elige el usuario (Enlace Móvil no lo deja en ningún archivo legible)
            let wp = if sys::demo() { "demo".into() } else { sys::config_value("fondo_movil").unwrap_or_default() };
            if wp == "demo" && wall.1.is_none() { wall = (wp.clone(), Some(Arc::new(gradient(WALL_W, WALL_H, [0.55, 0.70, 0.95], [0.12, 0.16, 0.32])))); }
            else if wp != wall.0 { wall = (wp.clone(), wic.as_ref().and_then(|w| load_wall(w, &wp)).map(Arc::new)); }
            st.wall = wall.1.clone();
            // Las fotos, cada minuto (listar es al momento; la miniatura puede traer el archivo del móvil)
            if sys::demo() {
                if photos.is_empty() { photos = demo_photos(); }
            } else if round % 15 == 0 {
                if let Some(w) = &wic { photos = recent_photos(w, &mut thumbs); }
            }
            round += 1;
            st.photos = photos.clone();
            // El widget lo recoge en su repintado de cada segundo
            *PHONE.lock().unwrap() = st;
            // Cada 4 s; en pausa mientras no se ve el escritorio
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(100));
                if PHONE_NOW.swap(false, Ordering::Relaxed) { break; }
            }
            while !deskdata::VISIBLE.load(Ordering::Relaxed) && !PHONE_NOW.load(Ordering::Relaxed) { std::thread::sleep(Duration::from_millis(1000)); }
        }
    });
}

/// Carpetas del móvil que Windows enseña en CrossDevice (Enlace Móvil → acceso a archivos).
fn phone_roots() -> Vec<std::path::PathBuf> {
    let base = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("CrossDevice");
    std::fs::read_dir(&base).map(|rd| rd.flatten().map(|e| e.path().join("storage")).filter(|p| p.is_dir()).collect()).unwrap_or_default()
}

/// Las 3 fotos o vídeos más recientes de la cámara, capturas e Imágenes del móvil. Solo se leen fechas
/// (no se descarga nada); la miniatura la hace Windows, y se guarda para no volver a pedirla.
fn recent_photos(wic: &IWICImagingFactory, cache: &mut HashMap<String, Option<Arc<Vec<u8>>>>) -> Vec<PPhoto> {
    const IMG: [&str; 6] = ["jpg", "jpeg", "png", "heic", "webp", "gif"];
    const VID: [&str; 3] = ["mp4", "mov", "3gp"];
    let mut found: Vec<(std::time::SystemTime, std::path::PathBuf, bool)> = Vec::new();
    fn walk(dir: &std::path::Path, depth: u32, out: &mut Vec<(std::time::SystemTime, std::path::PathBuf, bool)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten().take(4000) {
            let p = e.path();
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() { if depth > 0 && !e.file_name().to_string_lossy().starts_with('.') { walk(&p, depth - 1, out); } continue; }
            let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
            let video = VID.contains(&ext.as_str());
            if !video && !IMG.contains(&ext.as_str()) { continue; }
            if let Ok(t) = m.modified() { out.push((t, p, video)); }
        }
    }
    for root in phone_roots() {
        walk(&root.join("DCIM"), 1, &mut found);
        walk(&root.join("Pictures"), 2, &mut found);
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let out: Vec<PPhoto> = found.into_iter().take(3).map(|(_, p, video)| {
        let path = p.display().to_string();
        let thumb = cache.entry(path.clone()).or_insert_with(|| thumb_of(wic, &path).map(Arc::new)).clone();
        PPhoto { path, thumb, video }
    }).collect();
    if cache.len() > 40 { cache.retain(|k, _| out.iter().any(|p| p.path == *k)); }
    out
}

fn thumb_of(wic: &IWICImagingFactory, path: &str) -> Option<Vec<u8>> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
        let f: IShellItemImageFactory = windows::core::Interface::cast(&item).ok()?;
        let hb = f.GetImage(SIZE { cx: THUMB as i32 * 2, cy: THUMB as i32 * 2 }, SIIGBF_BIGGERSIZEOK).ok()?;
        let b = wic.CreateBitmapFromHBITMAP(hb, HPALETTE::default(), WICBitmapIgnoreAlpha).ok();
        let _ = DeleteObject(HGDIOBJ(hb.0));
        wic_square_n(wic, &b?.into(), THUMB)
    }
}

/// Nombre que se le dio al móvil al vincularlo ("S24 Ultra de …"), del archivo de Enlace Móvil.
fn device_name() -> String {
    let p = std::env::var("LOCALAPPDATA").unwrap_or_default() + "\\Packages\\Microsoft.YourPhone_8wekyb3d8bbwe\\LocalCache\\DeviceMetadataStorage.json";
    let Ok(txt) = std::fs::read_to_string(p) else { return String::new() };
    let Some(i) = txt.find("\"DisplayName\"") else { return String::new() };
    let rest = &txt[i + 13..];
    let Some(a) = rest.find('"') else { return String::new() };
    let rest = &rest[a + 1..];
    rest.find('"').map(|b| rest[..b].to_string()).unwrap_or_default()
}

/// Imagen elegida como fondo, recortada al centro con forma de pantalla de móvil.
fn load_wall(wic: &IWICImagingFactory, path: &str) -> Option<Vec<u8>> {
    if path.is_empty() { return None; }
    unsafe {
        let dec = wic.CreateDecoderFromFilename(&HSTRING::from(path), None, windows::Win32::Foundation::GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let frame: IWICBitmapSource = dec.GetFrame(0).ok()?.into();
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        let want = WALL_W as f32 / WALL_H as f32;
        let (cw, ch) = if w as f32 / h as f32 > want { ((h as f32 * want) as u32, h) } else { (w, (w as f32 / want) as u32) };
        let clip = wic.CreateBitmapClipper().ok()?;
        clip.Initialize(&frame, &WICRect { X: ((w - cw) / 2) as i32, Y: ((h - ch) / 2) as i32, Width: cw as i32, Height: ch as i32 }).ok()?;
        let sc = wic.CreateBitmapScaler().ok()?;
        sc.Initialize(&clip, WALL_W, WALL_H, WICBitmapInterpolationModeHighQualityCubic).ok()?;
        let cv = wic.CreateFormatConverter().ok()?;
        cv.Initialize(&sc, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let mut buf = vec![0u8; (WALL_W * WALL_H * 4) as usize];
        cv.CopyPixels(std::ptr::null(), WALL_W * 4, &mut buf).ok()?;
        Some(buf)
    }
}

/// Elegir el fondo del móvil (clic en el dibujo del móvil): se guarda en config.ini como `fondo_movil`.
pub fn pick_wallpaper() {
    std::thread::spawn(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(dlg) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) else { return };
        use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
        let imgs = HSTRING::from(i18n::t("Imágenes"));
        let spec = [COMDLG_FILTERSPEC { pszName: PCWSTR(imgs.as_ptr()), pszSpec: w!("*.jpg;*.jpeg;*.png;*.bmp;*.webp;*.heic") }];
        let _ = dlg.SetFileTypes(&spec);
        let _ = dlg.SetTitle(&HSTRING::from(i18n::t("Fondo del móvil")));
        if dlg.Show(None).is_err() { return; }
        let Ok(item) = dlg.GetResult() else { return };
        let Ok(p) = item.GetDisplayName(SIGDN_FILESYSPATH) else { return };
        let path = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        if path.is_empty() || path.contains(['\n', '\r']) { return; }
        let cfg = sys::config_path();
        let old = std::fs::read_to_string(&cfg).unwrap_or_default();
        let mut lines: Vec<String> = old.lines().filter(|l| !l.trim_start().starts_with("fondo_movil")).map(str::to_string).collect();
        lines.push(format!("fondo_movil = {path}"));
        let _ = std::fs::create_dir_all(sys::config_dir());
        let _ = std::fs::write(&cfg, lines.join("\n") + "\n");
        PHONE_NOW.store(true, Ordering::Relaxed);
    });
}

/// Batería del móvil leída de la ventana de Enlace Móvil (su cabecera dice "60 %"). Sin ventana, nada.
fn read_battery() -> Option<u8> {
    let pid = phone_pid()?;
    unsafe {
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let cond = uia.CreatePropertyCondition(UIA_ProcessIdPropertyId, &windows::Win32::System::Variant::VARIANT::from(pid as i32)).ok()?;
        let wins = uia.GetRootElement().ok()?.FindAll(TreeScope_Children, &cond).ok()?;
        let any = uia.CreateTrueCondition().ok()?;
        for i in 0..wins.Length().unwrap_or(0) {
            let Ok(w) = wins.GetElement(i) else { continue };
            let Ok(all) = w.FindAll(TreeScope_Descendants, &any) else { continue };
            for j in 0..all.Length().unwrap_or(0).min(600) {
                // Lo dice en un grupo: "La batería está al 59 %" (o "Battery is at 59%")
                let n = all.GetElement(j).and_then(|e| e.CurrentName()).map(|b| b.to_string()).unwrap_or_default();
                let l = n.to_lowercase();
                if !(l.contains("bater") || l.contains("battery")) { continue; }
                let Some(pc) = n.find('%') else { continue };
                let digits: String = n[..pc].trim_end().chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
                if let Ok(v) = digits.parse::<u8>() { if v <= 100 { return Some(v); } }
            }
        }
        None
    }
}

/// Avisos de la lista «Notificaciones» de la ventana de Enlace Móvil (si está abierta). Cada aviso trae app,
/// título, texto y hora ("8:06").
fn read_window_notifs() -> Option<Vec<PNotif>> {
    let pid = phone_pid()?;
    unsafe {
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let cond = uia.CreatePropertyCondition(UIA_ProcessIdPropertyId, &windows::Win32::System::Variant::VARIANT::from(pid as i32)).ok()?;
        let wins = uia.GetRootElement().ok()?.FindAll(TreeScope_Children, &cond).ok()?;
        let by_id = |id: &str| uia.CreatePropertyCondition(UIA_AutomationIdPropertyId, &windows::Win32::System::Variant::VARIANT::from(id)).ok();
        let items_c = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &windows::Win32::System::Variant::VARIANT::from(UIA_ListItemControlTypeId.0)).ok()?;
        for i in 0..wins.Length().unwrap_or(0) {
            let Ok(w) = wins.GetElement(i) else { continue };
            let Some(hc) = by_id("NotificationsListScrollHost") else { continue };
            let Ok(host) = w.FindFirst(TreeScope_Descendants, &hc) else { continue };
            let Ok(items) = host.FindAll(TreeScope_Descendants, &items_c) else { continue };
            let text_of = |e: &IUIAutomationElement, id: &str| -> String {
                by_id(id).and_then(|c| e.FindFirst(TreeScope_Descendants, &c).ok()).and_then(|x| x.CurrentName().ok())
                    .map(|b| b.to_string().chars().filter(|c| *c != '\u{200e}' && *c != '\u{200f}').collect::<String>().trim().to_string())
                    .unwrap_or_default()
            };
            let mut out = Vec::new();
            for j in 0..items.Length().unwrap_or(0).min(20) {
                let Ok(it) = items.GetElement(j) else { continue };
                let app = text_of(&it, "AppNameTextBlock");
                let title = text_of(&it, "CompactModeTitleTextBlock");
                let body = text_of(&it, "NotificationDescriptionText");
                if title.is_empty() && body.is_empty() { continue; }
                let when = clock_to_unix(&text_of(&it, "HoverDateReceivedTextBlock"));
                // Sin identificador propio: uno estable a partir del contenido
                let id = { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new(); (&app, &title, &body).hash(&mut h); h.finish() as u32 };
                let (title, body) = if title.is_empty() { (body, String::new()) } else { (title, body) };
                out.push(PNotif { id, title, body, when, app });
            }
            return Some(out);
        }
        None
    }
}

/// "8:06" (hoy) → hora unix; si no es una hora, ahora.
fn clock_to_unix(s: &str) -> u64 {
    let now = deskdata::now_unix();
    let p: Vec<u32> = s.split(':').filter_map(|x| x.trim().parse().ok()).collect();
    if p.len() != 2 { return now; }
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let secs_now = t.wHour as i64 * 3600 + t.wMinute as i64 * 60 + t.wSecond as i64;
    let mut d = secs_now - (p[0] as i64 * 3600 + p[1] as i64 * 60);
    if d < 0 { d += 86_400; } // de ayer
    now.saturating_sub(d as u64)
}

/// Lo último visto en la ventana de Enlace Móvil (se sigue enseñando al cerrarla, mientras su contador diga
/// que hay avisos).
static WINDOW_CACHE: Mutex<Vec<PNotif>> = Mutex::new(Vec::new());

fn running() -> bool { phone_pid().is_some() }

fn phone_pid() -> Option<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return None };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut found = None;
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let n = String::from_utf16_lossy(&e.szExeFile[..e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)]);
                if n.eq_ignore_ascii_case("PhoneExperienceHost.exe") { found = Some(e.th32ProcessID); break; }
                if Process32NextW(snap, &mut e).is_err() { break; }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

fn texts(n: &UserNotification) -> Vec<String> {
    n.Notification().and_then(|x| x.Visual())
        .and_then(|v| v.GetBinding(&KnownNotificationBindings::ToastGeneric()?))
        .and_then(|b| b.GetTextElements())
        .map(|t| t.into_iter().filter_map(|e| e.Text().ok().map(|h| h.to_string())).filter(|s| !s.trim().is_empty()).collect())
        .unwrap_or_default()
}

fn read() -> PhoneState {
    let mut st = PhoneState { ready: true, running: running(), ..Default::default() };
    // Los avisos se leen de la base de datos del centro de notificaciones de Windows (solo lectura):
    // la API de "leer notificaciones" dice que sí a una app sin paquete y luego devuelve la lista vacía
    match wpn_phone_notifs() {
        Some((v, badge)) => { st.access = true; st.notifs = v; st.unread = badge; }
        None => {
            // Sin base de datos legible: la API, por si acaso (con paquete sí funciona)
            let Ok(l) = UserNotificationListener::Current() else { return st };
            st.access = l.GetAccessStatus().ok() == Some(UserNotificationListenerAccessStatus::Allowed);
            if !st.access { return st; }
            let Ok(v) = l.GetNotificationsAsync(NotificationKinds::Toast).and_then(|a| a.join()) else { return st };
            for n in v {
                let app = n.AppInfo().and_then(|a| a.AppUserModelId()).map(|h| h.to_string()).unwrap_or_default();
                if !app.starts_with(AUMID) { continue; }
                let t = texts(&n);
                let Some(title) = t.first().cloned() else { continue };
                let when = n.CreationTime().map(|d| (d.UniversalTime / 10_000_000 - 11_644_473_600).max(0) as u64).unwrap_or(0);
                st.notifs.push(PNotif { id: n.Id().unwrap_or(0), title, body: t[1..].join(" · "), when, app: app_name(&app) });
            }
        }
    }
    // Si a Windows no llegan (banners apagados), los de la ventana de Enlace Móvil
    if st.notifs.is_empty() && st.running {
        if let Some(v) = read_window_notifs() { *WINDOW_CACHE.lock().unwrap() = v; }
        if st.unread == 0 && st.access { WINDOW_CACHE.lock().unwrap().clear(); }
        st.notifs = WINDOW_CACHE.lock().unwrap().clone();
    }
    let hidden = HIDDEN.lock().unwrap();
    st.notifs.retain(|n| !hidden.contains(&n.id));
    st.notifs.sort_by(|a, b| b.when.cmp(&a.when));
    st.notifs.truncate(12);
    st
}

/// Avisos quitados desde el widget (Windows no deja a otra app borrarlos: se ocultan aquí).
static HIDDEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Nombre de la app del móvil a partir del identificador que le da Enlace Móvil
/// ("…!YourPhoneNotifications_com.whatsapp" → "WhatsApp").
fn app_name(aumid: &str) -> String {
    if aumid.contains("!YourPhoneMessages") { return i18n::t("Mensajes").to_string(); }
    let pkg = aumid.rsplit("YourPhoneNotifications_").next().unwrap_or("").to_lowercase();
    let known = [
        ("com.whatsapp", "WhatsApp"), ("com.instagram.android", "Instagram"), ("com.google.android.gm", "Gmail"),
        ("com.google.android.youtube", "YouTube"), ("com.zhiliaoapp.musically", "TikTok"), ("org.telegram.messenger", "Telegram"),
        ("com.discord", "Discord"), ("com.valvesoftware.android.steam.community", "Steam"), ("com.spotify.music", "Spotify"),
        ("com.twitter.android", "X"), ("com.google.android.apps.walletnfcrel", "Wallet"), ("com.android.bluetooth", "Bluetooth"),
        ("com.samsung.android.app.routines", "Rutinas"), ("com.sec.spp.push", "Samsung"), ("com.osp.app.signin", "Samsung"),
        ("com.google.android.calendar", "Calendar"), ("com.facebook.orca", "Messenger"), ("com.facebook.katana", "Facebook"),
        ("com.amazon.mshop.android.shopping", "Amazon"), ("com.netflix.mediaclient", "Netflix"), ("com.reddit.frontpage", "Reddit"),
    ];
    if let Some((_, n)) = known.iter().find(|(k, _)| *k == pkg) { return n.to_string(); }
    // Si no se conoce, la última parte del paquete con mayúscula ("com.foo.bar" → "Bar")
    let last = pkg.rsplit('.').find(|s| !["android", "app", "apps", "mobile"].contains(s)).unwrap_or("");
    let mut c = last.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

// ---- Base de datos del centro de notificaciones (SQLite de Windows: winsqlite3.dll, sin dependencias)
type Open = unsafe extern "C" fn(*const u8, *mut *mut core::ffi::c_void, i32, *const u8) -> i32;
type Prepare = unsafe extern "C" fn(*mut core::ffi::c_void, *const u8, i32, *mut *mut core::ffi::c_void, *mut *const u8) -> i32;
type Step = unsafe extern "C" fn(*mut core::ffi::c_void) -> i32;
type ColBlob = unsafe extern "C" fn(*mut core::ffi::c_void, i32) -> *const u8;
type ColBytes = unsafe extern "C" fn(*mut core::ffi::c_void, i32) -> i32;
type ColI64 = unsafe extern "C" fn(*mut core::ffi::c_void, i32) -> i64;
type Fin = unsafe extern "C" fn(*mut core::ffi::c_void) -> i32;

fn wpn_phone_notifs() -> Option<(Vec<PNotif>, u32)> {
    unsafe {
        let lib = windows::Win32::System::LibraryLoader::LoadLibraryW(w!("winsqlite3.dll")).ok()?;
        let f = |n: windows::core::PCSTR| windows::Win32::System::LibraryLoader::GetProcAddress(lib, n);
        let open: Open = std::mem::transmute(f(windows::core::s!("sqlite3_open_v2"))?);
        let prepare: Prepare = std::mem::transmute(f(windows::core::s!("sqlite3_prepare_v2"))?);
        let step: Step = std::mem::transmute(f(windows::core::s!("sqlite3_step"))?);
        let blob: ColBlob = std::mem::transmute(f(windows::core::s!("sqlite3_column_blob"))?);
        let text: ColBlob = std::mem::transmute(f(windows::core::s!("sqlite3_column_text"))?);
        let bytes: ColBytes = std::mem::transmute(f(windows::core::s!("sqlite3_column_bytes"))?);
        let i64_: ColI64 = std::mem::transmute(f(windows::core::s!("sqlite3_column_int64"))?);
        let finalize: Fin = std::mem::transmute(f(windows::core::s!("sqlite3_finalize"))?);
        let close: Fin = std::mem::transmute(f(windows::core::s!("sqlite3_close"))?);
        let path = std::env::var("LOCALAPPDATA").ok()? + "\\Microsoft\\Windows\\Notifications\\wpndatabase.db\0";
        let mut db = std::ptr::null_mut();
        // 1 = SQLITE_OPEN_READONLY: nunca se escribe en ella
        if open(path.as_ptr(), &mut db, 1, std::ptr::null()) != 0 || db.is_null() { if !db.is_null() { close(db); } return None; }
        let sql = "SELECT n.Id, n.ArrivalTime, n.Payload, h.PrimaryId FROM Notification n JOIN NotificationHandler h ON n.HandlerId = h.RecordId \
                   WHERE h.PrimaryId LIKE 'Microsoft.YourPhone_8wekyb3d8bbwe!YourPhone%' AND n.Type = 'toast' ORDER BY n.ArrivalTime DESC LIMIT 20\0";
        let mut stmt = std::ptr::null_mut();
        if prepare(db, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut()) != 0 { close(db); return None; }
        let mut out = Vec::new();
        // 100 = SQLITE_ROW
        while step(stmt) == 100 {
            let id = i64_(stmt, 0) as u32;
            let ft = i64_(stmt, 1);
            let when = if ft > 0 { (ft / 10_000_000 - 11_644_473_600).max(0) as u64 } else { 0 };
            let (p, n) = (blob(stmt, 2), bytes(stmt, 2));
            if p.is_null() || n <= 0 { continue; }
            let xml = String::from_utf8_lossy(std::slice::from_raw_parts(p, n as usize)).to_string();
            let (a, an) = (text(stmt, 3), bytes(stmt, 3));
            let aumid = if a.is_null() { String::new() } else { String::from_utf8_lossy(std::slice::from_raw_parts(a, an as usize)).to_string() };
            let t = toast_texts(&xml);
            let Some(title) = t.first().cloned() else { continue };
            out.push(PNotif { id, title, body: t[1..].join(" · "), when, app: app_name(&aumid) });
        }
        finalize(stmt);
        // El contador del icono de Enlace Móvil: <badge value="4"/>
        let mut badge = 0u32;
        let sql2 = "SELECT n.Payload FROM Notification n JOIN NotificationHandler h ON n.HandlerId = h.RecordId WHERE h.PrimaryId LIKE 'Microsoft.YourPhone_8wekyb3d8bbwe!%' AND n.Type = 'badge' LIMIT 1\0";
        let mut st2 = std::ptr::null_mut();
        if prepare(db, sql2.as_ptr(), -1, &mut st2, std::ptr::null_mut()) == 0 {
            if step(st2) == 100 {
                let (p, n) = (blob(st2, 0), bytes(st2, 0));
                if !p.is_null() && n > 0 {
                    let x = String::from_utf8_lossy(std::slice::from_raw_parts(p, n as usize)).to_string();
                    badge = x.split("value=\"").nth(1).and_then(|r| r.split('"').next()).and_then(|v| v.parse().ok()).unwrap_or(0);
                }
            }
            finalize(st2);
        }
        close(db);
        Some((out, badge))
    }
}

/// Textos de un aviso (los `<text>` de su XML, sin la atribución), con las entidades ya resueltas.
fn toast_texts(xml: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<text") {
        rest = &rest[i + 5..];
        let Some(gt) = rest.find('>') else { break };
        let attrs = &rest[..gt];
        if attrs.ends_with('/') { rest = &rest[gt + 1..]; continue; }
        let body = &rest[gt + 1..];
        let Some(end) = body.find("</text>") else { break };
        let s = unescape(&body[..end]);
        if !attrs.contains("attribution") && !s.trim().is_empty() { v.push(s.trim().to_string()); }
        rest = &body[end..];
    }
    v
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.split('&');
    if let Some(first) = it.next() { out.push_str(first); }
    for part in it {
        let Some(semi) = part.find(';') else { out.push('&'); out.push_str(part); continue };
        let ent = &part[..semi];
        let ch = match ent {
            "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'), "quot" => Some('"'), "apos" => Some('\''),
            e if e.starts_with("#x") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch { Some(c) => { out.push(c); out.push_str(&part[semi + 1..]); } None => { out.push('&'); out.push_str(part); } }
    }
    out
}
/// Imagen de degradado vertical con unas "colinas" (para el modo demo).
fn gradient(w: u32, h: u32, top: [f32; 3], bottom: [f32; 3]) -> Vec<u8> {
    let mut v = vec![0u8; (w * h * 4) as usize];
    for y in 0..h { for x in 0..w {
        let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
        let hill = 0.62 + 0.08 * (fx * 6.0).sin() + 0.04 * (fx * 13.0 + 1.0).sin();
        let k = if fy > hill { 0.35 } else { 1.0 };
        let i = ((y * w + x) * 4) as usize;
        for c in 0..3 { let val = (top[c] + (bottom[c] - top[c]) * fy) * k; v[i + 2 - c] = (val.clamp(0.0, 1.0) * 255.0) as u8; }
        v[i + 3] = 255;
    } }
    v
}

fn demo_photos() -> Vec<PPhoto> {
    let mk = |a: [f32; 3], b: [f32; 3], video| PPhoto { path: String::new(), thumb: Some(Arc::new(gradient(THUMB, THUMB, a, b))), video };
    vec![mk([0.98, 0.72, 0.45], [0.55, 0.25, 0.35], false), mk([0.45, 0.80, 0.75], [0.10, 0.30, 0.35], true), mk([0.70, 0.75, 0.95], [0.25, 0.25, 0.45], false)]
}

fn demo() -> PhoneState {
    let now = deskdata::now_unix();
    let n = |id, t: &str, b: &str, m: u64| PNotif { id, title: t.into(), body: b.into(), when: now - m * 60, app: ["WhatsApp", "WhatsApp", "Gmail", "Calendario"][(id as usize - 1) % 4].into() };
    PhoneState { ready: true, access: true, running: true, photos: Vec::new(), name: "Móvil de demostración".into(), battery: Some((72, now)), wall: None, unread: 0, notifs: vec![
        n(1, "Lucía", "¿Quedamos a las ocho en el sitio de siempre?", 2),
        n(2, "Grupo Familia", "Mamá: Las fotos de ayer ya están en el álbum", 18),
        n(3, "Correos", "Tu envío llega hoy entre las 10:00 y las 14:00", 64),
        n(4, "Calendario", "Revisión con el equipo · 16:30", 130),
    ] }
}

/// Quita un aviso (de Windows y, con él, del widget).
pub fn dismiss(id: u32) {
    std::thread::spawn(move || {
        if let Ok(l) = UserNotificationListener::Current() { let _ = l.RemoveNotification(id); }
        // Por si Windows no deja quitarlo (app sin paquete): al menos desaparece del widget
        HIDDEN.lock().unwrap().push(id);
        PHONE_NOW.store(true, Ordering::Relaxed);
    });
}

/// Pide el permiso de leer notificaciones (Windows enseña su diálogo).
pub fn ask_access() {
    std::thread::spawn(|| {
        if let Ok(l) = UserNotificationListener::Current() {
            if l.RequestAccessAsync().and_then(|a| a.join()).ok() != Some(UserNotificationListenerAccessStatus::Allowed) {
                sys::open("ms-settings:privacy-notifications");
            }
        }
        PHONE_NOW.store(true, Ordering::Relaxed);
    });
}

pub fn selftest() {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let st = read();
    let mut s = format!("en marcha={} acceso={} avisos del móvil={}\n", st.running, st.access, st.notifs.len());
    // Qué apps tienen avisos ahora (solo su identificador y cuántos, sin contenido)
    if let Ok(l) = UserNotificationListener::Current() {
        if let Ok(v) = l.GetNotificationsAsync(NotificationKinds::Toast).and_then(|a| a.join()) {
            let mut by: HashMap<String, usize> = HashMap::new();
            for n in v { *by.entry(n.AppInfo().and_then(|a| a.AppUserModelId()).map(|h| h.to_string()).unwrap_or_else(|_| "?".into())).or_default() += 1; }
            s += &format!("apps con avisos: {by:?}\n");
        }
    }
    let t = Instant::now();
    if let Ok(w) = unsafe { CoCreateInstance::<_, IWICImagingFactory>(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) } {
        for p in recent_photos(&w, &mut HashMap::new()) { s += &format!("foto vídeo={} miniatura={:?} bytes\n", p.video, p.thumb.map(|v| v.len())); }
    }
    s += &format!("fotos en {} ms\n", t.elapsed().as_millis());
    match wpn_phone_notifs() { Some((v, b)) => { s += &format!("base de avisos: {} del móvil, contador {b}\n", v.len()); for n in v.iter().take(5) { s += &format!("  app={} textos={} hace={}s\n", n.app, 1 + n.body.split(" · ").filter(|x| !x.is_empty()).count(), deskdata::now_unix().saturating_sub(n.when)); } } None => s += "base de avisos: no legible\n" }
    s += &format!("nombre=«{}» batería={:?}\n", device_name(), read_battery());
    match read_window_notifs() { Some(v) => { s += &format!("ventana: {} avisos\n", v.len()); for n in &v { s += &format!("  app={} titulo={}c texto={}c hace={}s\n", n.app, n.title.chars().count(), n.body.chars().count(), deskdata::now_unix().saturating_sub(n.when)); } } None => s += "ventana: cerrada\n" }
    let _ = std::fs::write(std::env::temp_dir().join("macbar_phone.txt"), s);
}
