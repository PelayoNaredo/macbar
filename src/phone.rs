//! Móvil de Enlace Móvil: si la app está en marcha y las notificaciones del móvil que Enlace Móvil pone
//! en Windows (sus bases de datos van cifradas: lo único legible son esos avisos). Prueba: MACBAR_PHONETEST=1.

use super::*;
use std::sync::atomic::AtomicBool;
use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};

#[derive(Clone, Default, PartialEq)]
pub struct PNotif { pub id: u32, pub title: String, pub body: String, pub when: u64 }

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
    pub name: String, pub battery: Option<(u8, u64)>, pub wall: Option<Arc<Vec<u8>>> }

pub static PHONE: Mutex<PhoneState> = Mutex::new(PhoneState { ready: false, access: false, running: false, notifs: Vec::new(), photos: Vec::new(),
    name: String::new(), battery: None, wall: None });
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
        let mut battery: Option<(u8, u64)> = None;
        let mut wall: (String, Option<Arc<Vec<u8>>>) = (String::new(), None);
        let name = device_name();
        loop {
            let mut st = if sys::demo() { demo() } else { read() };
            st.name = name.clone();
            // La batería solo la enseña la ventana de Enlace Móvil: se lee si está abierta y se recuerda
            if st.running && round % 3 == 0 { if let Some(b) = read_battery() { battery = Some((b, deskdata::now_unix())); } }
            st.battery = battery;
            // El fondo lo elige el usuario (Enlace Móvil no lo deja en ningún archivo legible)
            let wp = sys::config_value("fondo_movil").unwrap_or_default();
            if wp != wall.0 { wall = (wp.clone(), wic.as_ref().and_then(|w| load_wall(w, &wp)).map(Arc::new)); }
            st.wall = wall.1.clone();
            // Las fotos, cada minuto (listar es al momento; la miniatura puede traer el archivo del móvil)
            if round % 15 == 0 && !sys::demo() {
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
        let spec = [COMDLG_FILTERSPEC { pszName: w!("Imágenes"), pszSpec: w!("*.jpg;*.jpeg;*.png;*.bmp;*.webp;*.heic") }];
        let _ = dlg.SetFileTypes(&spec);
        let _ = dlg.SetTitle(w!("Fondo del móvil"));
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
    let Ok(l) = UserNotificationListener::Current() else { return st };
    st.access = l.GetAccessStatus().ok() == Some(UserNotificationListenerAccessStatus::Allowed);
    if !st.access { return st; }
    let Ok(v) = l.GetNotificationsAsync(NotificationKinds::Toast).and_then(|a| a.join()) else { return st };
    for n in v {
        let app = n.AppInfo().and_then(|a| a.AppUserModelId()).map(|h| h.to_string()).unwrap_or_default();
        if !app.starts_with(AUMID) { continue; }
        let t = texts(&n);
        let Some(title) = t.first().cloned() else { continue };
        // DateTime: intervalos de 100 ns desde 1601
        let when = n.CreationTime().map(|d| (d.UniversalTime / 10_000_000 - 11_644_473_600).max(0) as u64).unwrap_or(0);
        st.notifs.push(PNotif { id: n.Id().unwrap_or(0), title, body: t[1..].join(" · "), when });
    }
    st.notifs.sort_by(|a, b| b.when.cmp(&a.when));
    st.notifs.truncate(12);
    st
}

fn demo() -> PhoneState {
    let now = deskdata::now_unix();
    let n = |id, t: &str, b: &str, m: u64| PNotif { id, title: t.into(), body: b.into(), when: now - m * 60 };
    PhoneState { ready: true, access: true, running: true, photos: Vec::new(), name: "Móvil de demostración".into(), battery: Some((72, now)), wall: None, notifs: vec![
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
    let t = Instant::now();
    if let Ok(w) = unsafe { CoCreateInstance::<_, IWICImagingFactory>(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) } {
        for p in recent_photos(&w, &mut HashMap::new()) { s += &format!("foto vídeo={} miniatura={:?} bytes\n", p.video, p.thumb.map(|v| v.len())); }
    }
    s += &format!("fotos en {} ms\n", t.elapsed().as_millis());
    s += &format!("nombre=«{}» batería={:?}\n", device_name(), read_battery());
    let _ = std::fs::write(std::env::temp_dir().join("macbar_phone.txt"), s);
}
