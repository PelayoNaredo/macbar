//! Datos de los widgets del escritorio, cada uno en su hilo: sistema (cada segundo), discos y papelera,
//! proyectos (carpetas con git y repos de GitHub vía `gh`) y letras sincronizadas (LRCLIB).

use super::*;
use std::os::windows::process::CommandExt;
use std::sync::atomic::AtomicBool;

const NO_WINDOW: u32 = 0x0800_0000;
pub const HIST: usize = 60;

// ---------------------------------------------------------------- sistema
#[derive(Clone, Default)]
pub struct SysData {
    pub cpu: Vec<f32>,
    pub ram: Vec<f32>,
    pub ram_used: u64,
    pub ram_total: u64,
    /// Bytes por segundo de bajada y subida
    pub down: Vec<f32>,
    pub up: Vec<f32>,
    /// Apps que más CPU gastan: (nombre, %)
    pub top: Vec<(String, f32)>,
    pub uptime: u64,
    /// Cuándo llegó la última muestra (para desplazar la gráfica con suavidad)
    pub at: Option<Instant>,
}
pub static SYS: Mutex<Option<SysData>> = Mutex::new(None);

// ---------------------------------------------------------------- discos y papelera
#[derive(Clone)]
pub struct Drive { pub root: String, pub label: String, pub total: u64, pub free: u64 }
#[derive(Clone, Default)]
pub struct StoreData { pub drives: Vec<Drive>, pub bin_size: u64, pub bin_items: u64, pub bin_big: Vec<(String, u64)> }
pub static STORE: Mutex<Option<StoreData>> = Mutex::new(None);
pub static STORE_NOW: AtomicBool = AtomicBool::new(false);

// ---------------------------------------------------------------- proyectos
#[derive(Clone)]
pub struct Repo { pub name: String, pub path: String, pub branch: String, pub dirty: Option<usize>, pub lang: &'static str, pub color: [f32; 3], pub modified: u64, pub remote: Option<String> }
#[derive(Clone)]
pub struct GhRepo { pub name: String, pub private: bool, pub stars: u32, pub pushed: u64, pub url: String, pub lang: String, pub color: [f32; 3] }
#[derive(Clone, Default)]
pub struct Projects { pub local: Vec<Repo>, pub gh: Vec<GhRepo>, pub gh_ok: Option<bool>, pub scanned: bool }
pub static PROJ: Mutex<Projects> = Mutex::new(Projects { local: Vec::new(), gh: Vec::new(), gh_ok: None, scanned: false });

// ---------------------------------------------------------------- cola de YouTube Music
/// Temas de la cola tal como los muestra YouTube Music: (título, artistas, duración "m:ss").
#[derive(Clone)]
pub struct QItem { pub title: String, pub artists: String, pub dur: String }
pub static QUEUE: Mutex<Vec<QItem>> = Mutex::new(Vec::new());
/// Tema de la cola que se pidió reproducir desde el widget (lo atiende el hilo de la cola).
pub static PLAY_REQ: Mutex<Option<String>> = Mutex::new(None);

static STARTED: AtomicBool = AtomicBool::new(false);
/// ¿Se ve algún widget? (lo actualiza la interfaz). Tapados, los hilos solo toman lo barato.
pub static VISIBLE: AtomicBool = AtomicBool::new(true);

pub fn start() {
    if STARTED.swap(true, Ordering::Relaxed) { return; }
    std::thread::spawn(sys_thread);
    std::thread::spawn(store_thread);
    std::thread::spawn(proj_thread);
    std::thread::spawn(queue_thread);
}

pub static YTM_OPEN: AtomicBool = AtomicBool::new(false);
/// ¿Hay una ventana de YouTube Music? (lo actualiza el hilo de la cola)
pub fn ytm_open() -> bool { YTM_OPEN.load(Ordering::Relaxed) }

/// Ventana de YouTube Music: la app instalada ("YouTube Music") o una pestaña del navegador.
fn ytm_window() -> Option<HWND> {
    sys::app_windows().into_iter().find(|h| {
        let mut buf = [0u16; 256];
        let n = unsafe { GetWindowTextW(*h, &mut buf) } as usize;
        String::from_utf16_lossy(&buf[..n]).contains("YouTube Music")
    })
}

/// Lee la cola de la ventana con la accesibilidad de Windows (UI Automation). Cada tema aparece como
/// título (texto), artistas (grupo) y duración "m:ss" (texto): se trocea por las duraciones.
fn read_queue(uia: &windows::Win32::UI::Accessibility::IUIAutomation, h: HWND) -> Option<Vec<QItem>> {
    use windows::Win32::UI::Accessibility::*;
    unsafe {
        let root = uia.ElementFromHandle(h).ok()?;
        let cond = uia.CreateTrueCondition().ok()?;
        let cr = uia.CreateCacheRequest().ok()?;
        cr.AddProperty(UIA_NamePropertyId).ok()?;
        cr.AddProperty(UIA_ControlTypePropertyId).ok()?;
        let all = root.FindAllBuildCache(TreeScope_Descendants, &cond, &cr).ok()?;
        let n = all.Length().ok()?;
        let mut els: Vec<(i32, String)> = Vec::with_capacity(n as usize);
        for i in 0..n {
            let Ok(e) = all.GetElement(i) else { continue };
            let name = e.CachedName().map(|b| b.to_string()).unwrap_or_default();
            let ty = e.CachedControlType().map(|t| t.0).unwrap_or(0);
            els.push((ty, name));
        }
        let is_dur = |s: &str| { let b = s.as_bytes(); (b.len() == 4 || b.len() == 5) && s.contains(':') && s.chars().all(|c| c.is_ascii_digit() || c == ':') };
        let mut out = Vec::new();
        let mut start = 0usize;
        for (i, (ty, name)) in els.iter().enumerate() {
            if *ty != UIA_TextControlTypeId.0 || !is_dur(name) { continue; }
            let seg = &els[start..i];
            start = i + 1;
            // Título: el texto que va justo detrás del botón "Reproducir…" del tema (así no se cogen rótulos
            // como "Reproduciendo de" o "Reproducción automática"); si no hay botón, el primer texto
            let after_btn = (1..seg.len()).find(|&j| seg[j].0 == UIA_TextControlTypeId.0 && !seg[j].1.trim().is_empty() && seg[j - 1].0 == UIA_ButtonControlTypeId.0);
            let Some(ti) = after_btn.or_else(|| seg.iter().position(|(t, n)| *t == UIA_TextControlTypeId.0 && !n.trim().is_empty())) else { continue };
            let title = seg[ti].1.clone();
            // Artistas: los textos que siguen al título ("DUKI", ", ", "KHEA"…) hasta el menú del tema,
            // sin etiquetas como "Contenido explícito" ni la duración dicha en palabras
            let mut artists = String::new();
            for (t, n) in &seg[ti + 1..] {
                let l = n.to_lowercase();
                if l.contains("menú de acciones") || l.contains("action menu") { break; }
                if *t != UIA_TextControlTypeId.0 || n.is_empty() { continue; }
                if l.contains("contenido explícito") || l.contains("explicit") || l.contains("minuto") || l.contains("segundo") { continue; }
                artists.push_str(n);
            }
            let artists = artists.trim().to_string();
            if title.chars().count() > 120 { continue; }
            out.push(QItem { title, artists, dur: name.clone() });
        }
        Some(out)
    }
}

/// Busca el botón "Reproducir <título>" de la cola y lo pulsa (patrón Invoke de UI Automation).
fn play_in_queue(uia: &windows::Win32::UI::Accessibility::IUIAutomation, h: HWND, title: &str) {
    use windows::Win32::UI::Accessibility::*;
    unsafe {
        let Ok(root) = uia.ElementFromHandle(h) else { return };
        let Ok(cond) = uia.CreatePropertyCondition(UIA_ControlTypePropertyId, &windows::Win32::System::Variant::VARIANT::from(UIA_ButtonControlTypeId.0)) else { return };
        let Ok(all) = root.FindAll(TreeScope_Descendants, &cond) else { return };
        let want = title.trim().to_lowercase();
        for i in 0..all.Length().unwrap_or(0) {
            let Ok(e) = all.GetElement(i) else { continue };
            let name = e.CurrentName().map(|b| b.to_string()).unwrap_or_default().to_lowercase();
            // "Reproducir X" (o "Play X" con el idioma en inglés)
            let Some(rest) = name.strip_prefix("reproducir ").or_else(|| name.strip_prefix("play ")) else { continue };
            if rest.trim() != want { continue; }
            if let Ok(p) = e.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) { let _ = p.Invoke(); }
            return;
        }
    }
}

fn queue_thread() {
    use windows::Win32::UI::Accessibility::*;
    if sys::demo() {
        // Cola inventada (la canción de la demo y lo que viene detrás)
        let q = |t: &str, a: &str, d: &str| QItem { title: t.into(), artists: a.into(), dur: d.into() };
        *QUEUE.lock().unwrap() = vec![
            q("Tumblr Girls (feat. Christoph Andersson)", "G-Eazy", "4:01"), q("Midnight City", "M83", "4:03"),
            q("Electric Feel", "MGMT", "3:49"), q("Breezeblocks", "alt-J", "3:47"), q("Sweater Weather", "The Neighbourhood", "4:00"),
        ];
        YTM_OPEN.store(true, Ordering::Relaxed);
        return;
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(uia) = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) else { return };
        let mut woke: Option<isize> = None;
        let mut key = String::new();
        let mut last = Instant::now() - Duration::from_secs(60);
        loop {
            std::thread::sleep(Duration::from_millis(250));
            // Clic en un tema de la cola: se pulsa su botón "Reproducir …" en YouTube Music
            if let Some(title) = PLAY_REQ.lock().unwrap().take() {
                if let Some(h) = ytm_window() { play_in_queue(&uia, h, &title); last = Instant::now() - Duration::from_secs(60); }
                continue;
            }
            if last.elapsed() < Duration::from_millis(1000) { continue; }
            if !VISIBLE.load(Ordering::Relaxed) { continue; }
            let k = MEDIA.lock().unwrap().as_ref().map(|m| format!("{}\u{1}{}", m.title, m.artist)).unwrap_or_default();
            // Cada 6 s, o enseguida si cambió la canción
            if k == key && last.elapsed() < Duration::from_secs(6) { continue; }
            key = k;
            last = Instant::now();
            let w = ytm_window();
            YTM_OPEN.store(w.is_some(), Ordering::Relaxed);
            let Some(h) = w else { QUEUE.lock().unwrap().clear(); continue };
            if woke != Some(h.0 as isize) {
                // Chromium solo construye el árbol de la página cuando alguien lo pide por accesibilidad:
                // esta petición lo "despierta" (también con la ventana minimizada)
                let mut obj: *mut core::ffi::c_void = std::ptr::null_mut();
                let iid = windows::core::GUID::from_u128(0x618736e0_3c3d_11cf_810c_00aa00389b71);
                let _ = AccessibleObjectFromWindow(h, 0xFFFF_FFFC /* OBJID_CLIENT */, &iid, &mut obj);
                if !obj.is_null() { let _: windows::core::IUnknown = windows::core::Interface::from_raw(obj); }
                woke = Some(h.0 as isize);
                std::thread::sleep(Duration::from_millis(2500));
            }
            if let Some(q) = read_queue(&uia, h) { *QUEUE.lock().unwrap() = q; }
        }
    }
}

fn ft(f: FILETIME) -> u64 { ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64 }

fn sys_times() -> Option<(u64, u64)> {
    let (mut i, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)).ok()?; }
    Some((ft(i), ft(k) + ft(u)))
}

fn net_bytes() -> (u64, u64) {
    use windows::Win32::NetworkManagement::IpHelper::*;
    let (mut i, mut o) = (0u64, 0u64);
    unsafe {
        let mut t: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut t).is_ok() && !t.is_null() {
            let tb = &*t;
            for r in std::slice::from_raw_parts(tb.Table.as_ptr(), tb.NumEntries as usize) {
                // Solo adaptadores físicos conectados (los virtuales repetirían el mismo tráfico)
                if r.InterfaceAndOperStatusFlags._bitfield & 1 == 0 || r.OperStatus.0 != 1 || r.Type == 24 { continue; }
                i += r.InOctets;
                o += r.OutOctets;
            }
            FreeMibTable(t as _);
        }
    }
    (i, o)
}

/// Tiempo de CPU acumulado por proceso (100 ns), con su nombre.
fn proc_times() -> HashMap<u32, (String, u64)> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    let mut out = HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let pid = e.th32ProcessID;
            if pid > 4 {
                if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                    let (mut c, mut x, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
                    if GetProcessTimes(h, &mut c, &mut x, &mut k, &mut u).is_ok() {
                        let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                        out.insert(pid, (String::from_utf16_lossy(&e.szExeFile[..n]), ft(k) + ft(u)));
                    }
                    let _ = CloseHandle(h);
                }
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// "msedgewebview2.exe" → "Msedgewebview2"; unos cuantos nombres conocidos, más bonitos.
fn pretty_exe(exe: &str) -> String {
    let stem = exe.trim_end_matches(".exe").trim_end_matches(".EXE");
    let known = [("msedge", "Edge"), ("chrome", "Chrome"), ("brave", "Brave"), ("firefox", "Firefox"), ("code", "VS Code"), ("explorer", "Explorador"),
        ("claude", "Claude"), ("discord", "Discord"), ("spotify", "Spotify"), ("steam", "Steam"), ("steamwebhelper", "Steam"), ("dwm", "Escritorio (DWM)"),
        ("msmpeng", "Defender"), ("macbar", "macbar"), ("studio64", "Android Studio"), ("java", "Java"), ("node", "Node.js"), ("cargo", "Cargo"), ("rustc", "Rust"),
        ("unity", "Unity"), ("powershell", "PowerShell"), ("pwsh", "PowerShell"), ("windowsterminal", "Terminal"), ("searchhost", "Búsqueda"), ("audiodg", "Audio")];
    let l = stem.to_lowercase();
    if let Some((_, n)) = known.iter().find(|(k, _)| *k == l) { return i18n::t(*n).to_string(); }
    let mut c = stem.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn push(v: &mut Vec<f32>, x: f32) { v.push(x); if v.len() > HIST { v.remove(0); } }

fn sys_thread() {
    use windows::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut d = SysData::default();
    if sys::demo() {
        // Un minuto de historia inventada, para que la gráfica no salga vacía al arrancar
        for i in 0..HIST {
            let x = i as f32;
            push(&mut d.cpu, 14.0 + 9.0 * (x * 0.31).sin() + 6.0 * (x * 0.83).sin().max(0.0));
            push(&mut d.ram, 82.0 + 1.5 * (x * 0.12).sin());
            push(&mut d.down, 180_000.0 * ((x * 0.4).sin() + 1.2).max(0.0));
            push(&mut d.up, 40_000.0 * ((x * 0.55 + 1.0).sin() + 1.1).max(0.0));
        }
    }
    let mut last_cpu = sys_times();
    let mut last_net = net_bytes();
    let mut last_t = Instant::now();
    let mut procs = proc_times();
    let mut procs_cpu = sys_times();
    let mut n = 0u32;
    let mut paused = false;
    loop {
        std::thread::sleep(Duration::from_millis(1000));
        if !VISIBLE.load(Ordering::Relaxed) { paused = true; continue; }
        if paused {
            // Al volver a verse se toman referencias nuevas (sin picos falsos de red o CPU)
            paused = false;
            last_cpu = sys_times();
            last_net = net_bytes();
            last_t = Instant::now();
            procs = proc_times();
            procs_cpu = sys_times();
            continue;
        }
        n += 1;
        let now = sys_times();
        let cpu = match (last_cpu, now) {
            (Some((i0, t0)), Some((i1, t1))) if t1 > t0 => (1.0 - (i1 - i0) as f32 / (t1 - t0) as f32).clamp(0.0, 1.0) * 100.0,
            _ => d.cpu.last().copied().unwrap_or(0.0),
        };
        last_cpu = now;
        let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        unsafe { let _ = GlobalMemoryStatusEx(&mut m); }
        let net = net_bytes();
        let secs = last_t.elapsed().as_secs_f32().max(0.2);
        last_t = Instant::now();
        let down = net.0.saturating_sub(last_net.0) as f32 / secs;
        let up = net.1.saturating_sub(last_net.1) as f32 / secs;
        last_net = net;
        push(&mut d.cpu, cpu);
        push(&mut d.ram, m.dwMemoryLoad as f32);
        push(&mut d.down, down);
        push(&mut d.up, up);
        d.ram_used = m.ullTotalPhys - m.ullAvailPhys;
        d.ram_total = m.ullTotalPhys;
        d.uptime = unsafe { GetTickCount64() } / 1000;
        // Las apps que más gastan, cada 3 s (abrir cada proceso cuesta algo)
        if n % 3 == 0 {
            let p2 = proc_times();
            let c2 = sys_times();
            if let (Some((_, t0)), Some((_, t1))) = (procs_cpu, c2) {
                let total = (t1 - t0).max(1) as f32;
                let mut by: HashMap<String, f32> = HashMap::new();
                for (pid, (name, t)) in &p2 {
                    if let Some((_, old)) = procs.get(pid) {
                        let l = name.to_lowercase();
                        if l == "system" || l == "idle" || l == "registry" || l == "memory compression" { continue; }
                        *by.entry(pretty_exe(name)).or_default() += t.saturating_sub(*old) as f32 / total * 100.0;
                    }
                }
                let mut v: Vec<(String, f32)> = by.into_iter().collect();
                v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                v.truncate(3);
                d.top = v;
            }
            procs = p2;
            procs_cpu = c2;
        }
        d.at = Some(Instant::now());
        if sys::demo() { d.top = vec![("Navegador".into(), 6.2), ("Visual Studio Code".into(), 3.1), ("Spotify".into(), 1.4)]; }
        *SYS.lock().unwrap() = Some(d.clone());
    }
}

// ---------------------------------------------------------------- discos y papelera
fn drives() -> Vec<Drive> {
    use windows::Win32::Storage::FileSystem::*;
    let mut out = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 { continue; }
        let root = format!("{}:\\", (b'A' + i as u8) as char);
        let wr = HSTRING::from(root.as_str());
        let ty = unsafe { GetDriveTypeW(&wr) };
        if ty != 3 /* DRIVE_FIXED */ && ty != 2 /* DRIVE_REMOVABLE */ { continue; }
        let (mut free, mut total) = (0u64, 0u64);
        if unsafe { GetDiskFreeSpaceExW(&wr, None, Some(&mut total), Some(&mut free)) }.is_err() || total == 0 { continue; }
        let mut name = [0u16; 128];
        let label = unsafe {
            if GetVolumeInformationW(&wr, Some(&mut name), None, None, None, None).is_ok() {
                let n = name.iter().position(|&c| c == 0).unwrap_or(0);
                String::from_utf16_lossy(&name[..n])
            } else { String::new() }
        };
        out.push(Drive { root, label, total, free });
    }
    out
}

/// Lo más grande de la papelera: los ficheros `$I…` guardan el tamaño y la ruta original.
fn bin_biggest(drives: &[Drive]) -> Vec<(String, u64)> {
    let Some(sid) = sys::user_sid() else { return Vec::new() };
    let mut v = Vec::new();
    for d in drives {
        let dir = format!("{}$Recycle.Bin\\{sid}", d.root);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("$I") { continue; }
            let Ok(b) = std::fs::read(e.path()) else { continue };
            if b.len() < 28 { continue; }
            let ver = u64::from_le_bytes(b[0..8].try_into().unwrap());
            let size = u64::from_le_bytes(b[8..16].try_into().unwrap());
            let (start, len) = if ver >= 2 { (28, u32::from_le_bytes(b[24..28].try_into().unwrap()) as usize) } else { (24, 260) };
            let chars: Vec<u16> = b[start.min(b.len())..].chunks_exact(2).take(len).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
            let path = String::from_utf16_lossy(&chars);
            let file = path.rsplit('\\').next().unwrap_or(&path).to_string();
            if !file.is_empty() { v.push((file, size)); }
        }
    }
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v.truncate(3);
    v
}

pub fn read_store() -> StoreData {
    if sys::demo() {
        let gb = 1u64 << 30;
        return StoreData { drives: vec![Drive { root: "C:\\".into(), label: "Windows".into(), total: 952 * gb, free: 318 * gb }, Drive { root: "D:\\".into(), label: "Datos".into(), total: 1863 * gb, free: 1204 * gb }],
            bin_size: 2 * gb + 380 * (1 << 20), bin_items: 14, bin_big: vec![("vacaciones-2025.mp4".into(), 1_400 << 20), ("instalador-antiguo.exe".into(), 640 << 20), ("borrador-final.psd".into(), 212 << 20)] };
    }
    let drives = drives();
    let mut q = SHQUERYRBINFO { cbSize: size_of::<SHQUERYRBINFO>() as u32, ..Default::default() };
    unsafe { let _ = SHQueryRecycleBinW(PCWSTR::null(), &mut q); }
    let bin_big = bin_biggest(&drives);
    StoreData { drives, bin_size: q.i64Size.max(0) as u64, bin_items: q.i64NumItems.max(0) as u64, bin_big }
}

fn store_thread() {
    loop {
        let s = read_store();
        *STORE.lock().unwrap() = Some(s);
        for _ in 0..20 {
            if STORE_NOW.swap(false, Ordering::Relaxed) { break; }
            std::thread::sleep(Duration::from_millis(1000));
        }
        while !VISIBLE.load(Ordering::Relaxed) && !STORE_NOW.load(Ordering::Relaxed) { std::thread::sleep(Duration::from_millis(1000)); }
    }
}

// ---------------------------------------------------------------- proyectos
fn mtime(p: &std::path::Path) -> u64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn now_unix() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

/// Lenguaje por los ficheros de la raíz, con el color de GitHub.
fn guess_lang(p: &std::path::Path) -> (&'static str, [f32; 3]) {
    let has = |f: &str| p.join(f).exists();
    let hex = |h: u32| [((h >> 16) & 255) as f32 / 255.0, ((h >> 8) & 255) as f32 / 255.0, (h & 255) as f32 / 255.0];
    if has("Cargo.toml") { return ("Rust", hex(0xdea584)); }
    if has("build.gradle.kts") || has("settings.gradle.kts") || has("build.gradle") { return ("Kotlin", hex(0xa97bff)); }
    if has("ProjectSettings") || has("Assets") { return ("Unity · C#", hex(0x178600)); }
    if has("tsconfig.json") { return ("TypeScript", hex(0x3178c6)); }
    if has("package.json") { return ("JavaScript", hex(0xf1e05a)); }
    if has("pyproject.toml") || has("requirements.txt") || has("setup.py") { return ("Python", hex(0x3572a5)); }
    if has("go.mod") { return ("Go", hex(0x00add8)); }
    if has("CMakeLists.txt") { return ("C++", hex(0xf34b7d)); }
    if std::fs::read_dir(p).map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().ends_with(".sln") || e.file_name().to_string_lossy().ends_with(".csproj"))).unwrap_or(false) {
        return ("C#", hex(0x178600));
    }
    ("", [0.7, 0.7, 0.7])
}

pub fn lang_color(lang: &str) -> [f32; 3] {
    let hex = |h: u32| [((h >> 16) & 255) as f32 / 255.0, ((h >> 8) & 255) as f32 / 255.0, (h & 255) as f32 / 255.0];
    match lang {
        "Rust" => hex(0xdea584), "Kotlin" => hex(0xa97bff), "C#" => hex(0x178600), "TypeScript" => hex(0x3178c6), "JavaScript" => hex(0xf1e05a),
        "Python" => hex(0x3572a5), "Go" => hex(0x00add8), "C++" => hex(0xf34b7d), "Java" => hex(0xb07219), "HTML" => hex(0xe34c26), "CSS" => hex(0x663399),
        "Shell" => hex(0x89e051), "PowerShell" => hex(0x012456), "Dart" => hex(0x00b4ab), "Swift" => hex(0xf05138), "Lua" => hex(0x000080),
        _ => [0.7, 0.7, 0.7],
    }
}

fn git_remote(git: &std::path::Path) -> Option<String> {
    let cfg = std::fs::read_to_string(git.join("config")).ok()?;
    let url = cfg.lines().map(str::trim).find_map(|l| l.strip_prefix("url"))?.trim().trim_start_matches('=').trim();
    let path = url.strip_prefix("https://github.com/").or_else(|| url.strip_prefix("git@github.com:"))?;
    Some(format!("https://github.com/{}", path.trim_end_matches(".git")))
}

fn repo_at(p: &std::path::Path) -> Option<Repo> {
    let git = p.join(".git");
    if !git.is_dir() { return None; }
    let head = std::fs::read_to_string(git.join("HEAD")).unwrap_or_default();
    let branch = head.trim().strip_prefix("ref: refs/heads/").map(str::to_string).unwrap_or_else(|| head.trim().chars().take(7).collect());
    let modified = [git.join("index"), git.join("logs").join("HEAD"), git.join("HEAD")].iter().map(|f| mtime(f)).max().unwrap_or(0);
    let (lang, color) = guess_lang(p);
    Some(Repo { name: p.file_name()?.to_string_lossy().to_string(), path: p.to_string_lossy().to_string(), branch, dirty: None, lang, color, modified, remote: git_remote(&git) })
}

/// Carpetas donde buscar proyectos: `proyectos = ruta; ruta` en config.ini o las habituales.
fn roots() -> Vec<std::path::PathBuf> {
    if let Some(v) = sys::config_value("proyectos") {
        return v.split(';').map(|s| std::path::PathBuf::from(s.trim())).filter(|p| p.is_dir()).collect();
    }
    let home = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
    ["Desktop", "OneDrive\\Desktop", "OneDrive\\Escritorio", "source\\repos", "Projects", "Proyectos", "Documents\\GitHub", "dev", "code", "repos"]
        .iter().map(|r| home.join(r)).filter(|p| p.is_dir()).collect()
}

/// ¿Es seguro preguntarle a git por este repo? Un repo que viene de fuera (p. ej. sacado de un zip) trae
/// su propio .git/config, y git obedece cosas de ahí que ejecutan programas durante `status` (filtros,
/// fsmonitor…). En vez de buscar lo peligroso (siempre se escapa algo), solo se acepta lo conocido:
/// las secciones y claves de un repo normal, sin ficheros de configuración extra ni filtros en sus
/// atributos. Si no, el proyecto sale sin el recuento de cambios.
fn git_safe(repo: &std::path::Path) -> bool {
    let git = repo.join(".git");
    // Configuración por rama de trabajo o atributos propios: se descarta sin mirar más
    // (Un filtro solo se ejecuta si su programa está definido en la configuración: con la lista de
    // permitidos de abajo, ninguna sección [filter] o [include] pasa. Lo que no se puede revisar desde
    // aquí, la configuración compartida de otra copia de trabajo, se rechaza entero.)
    if git.join("config.worktree").exists() || git.join("commondir").exists() || git.join("gitdir").exists() { return false; }
    for attrs in [git.join("info").join("attributes"), repo.join(".gitattributes")] {
        if std::fs::read_to_string(&attrs).is_ok_and(|a| a.to_lowercase().contains("filter")) { return false; }
    }
    let Ok(cfg) = std::fs::read_to_string(git.join("config")) else { return false };
    const SECTIONS: [&str; 8] = ["core", "remote", "branch", "user", "pull", "push", "fetch", "init"];
    const CORE: [&str; 12] = ["repositoryformatversion", "filemode", "bare", "logallrefupdates", "symlinks", "ignorecase",
        "autocrlf", "safecrlf", "eol", "longpaths", "precomposeunicode", "checkstat"];
    // git se salta la BOM de UTF-8 al principio: aquí también, o "\u{feff}[filter …]" colaría como clave suelta
    let cfg = cfg.trim_start_matches('\u{feff}');
    // Una clave de git: letra y luego letras, números o guiones. Cualquier otra cosa no se entiende → se rechaza
    let key_ok = |k: &str| k.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let mut section = String::new();
    for line in cfg.lines() {
        let l = line.trim();
        // Caracteres invisibles o de control a la izquierda (otra BOM, NUL…): git podría leer ahí una cabecera
        if l.starts_with(|c: char| c.is_control() || c == '\u{feff}' || c == '\u{200b}') { return false; }
        if l.is_empty() || l.starts_with('#') || l.starts_with(';') { continue; }
        let mut l = l;
        if let Some(h) = l.strip_prefix('[') {
            section = h.split([']', ' ', '"']).next().unwrap_or("").to_lowercase();
            if !SECTIONS.contains(&section.as_str()) { return false; }
            // git admite una clave en la misma línea que la cabecera ("[core] fsmonitor = …"): también se revisa
            let Some(i) = h.find(']') else { return false };
            l = h[i + 1..].trim();
            if l.is_empty() || l.starts_with('#') || l.starts_with(';') { continue; }
        }
        let key = l.split('=').next().unwrap_or("").trim().to_lowercase();
        // Una línea que no es cabecera ni "clave = valor" (o fuera de toda sección): no se sabe qué hará git con ella
        if section.is_empty() || !key_ok(&key) { return false; }
        if section == "core" && !CORE.contains(&key.as_str()) { return false; }
        // En remote/branch/pull… solo lo de siempre (url, fetch, merge, rebase…), nada que lance un programa
        if ["uploadpack", "receivepack", "proxy", "vcs", "sshcommand"].iter().any(|k| key.contains(k)) { return false; }
    }
    true
}

fn scan_local() -> Vec<Repo> {
    let mut v: Vec<Repo> = Vec::new();
    for root in roots() {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() { continue; }
            if let Some(r) = repo_at(&p) { v.push(r); continue; }
            // Un nivel más (carpetas que agrupan varios proyectos)
            if let Ok(rd2) = std::fs::read_dir(&p) {
                for e2 in rd2.flatten().take(60) {
                    if let Some(r) = repo_at(&e2.path()) { v.push(r); }
                }
            }
        }
    }
    v.sort_by(|a, b| b.modified.cmp(&a.modified));
    v.dedup_by(|a, b| a.path.eq_ignore_ascii_case(&b.path));
    v.truncate(20);
    // Cambios sin guardar solo de los que se ven (git status cuesta)
    for r in v.iter_mut().take(8) {
        // Un repo que viene de fuera (p. ej. sacado de un zip) trae su propio .git/config, que puede mandar
        // a git ejecutar programas durante `status` (filtros, fsmonitor, hooks…). Si su configuración
        // define cualquier cosa así, no se le pregunta a git: se queda sin el recuento de cambios.
        if !git_safe(std::path::Path::new(&r.path)) { continue; }
        if let Ok(o) = std::process::Command::new("git")
            // Sin entrar en submódulos: cada uno tiene su propia configuración, que aquí no se revisa
            .args(["--no-optional-locks", "-c", "core.fsmonitor=false", "-c", "core.untrackedCache=false", "-c", "core.hooksPath=NUL", "-c", "core.attributesFile=NUL",
                "-c", "submodule.recurse=false", "-c", "status.submoduleSummary=false", "-C", &r.path, "status", "--porcelain", "--ignore-submodules=all"])
            .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_ATTR_NOSYSTEM", "1").creation_flags(NO_WINDOW).output() {
            if o.status.success() { r.dirty = Some(String::from_utf8_lossy(&o.stdout).lines().filter(|l| !l.trim().is_empty()).count()); }
        }
    }
    v
}

/// "2026-11-18T08:29:57Z" → segundos Unix.
pub fn iso_to_unix(s: &str) -> u64 {
    let n = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    let (y, m, d, hh, mm, ss) = (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19));
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    (days * 86400 + hh * 3600 + mm * 60 + ss).max(0) as u64
}

/// Valor de un campo de texto JSON en un trozo (sin anidamiento).
fn jstr(obj: &str, key: &str) -> Option<String> {
    let k = format!("\"{key}\":");
    let i = obj.find(&k)? + k.len();
    let rest = obj[i..].trim_start();
    if !rest.starts_with('"') { return None; }
    Some(json_unescape(&rest[1..]).0)
}

fn jnum(obj: &str, key: &str) -> Option<f64> {
    let k = format!("\"{key}\":");
    let i = obj.find(&k)? + k.len();
    let rest = obj[i..].trim_start();
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Lee una cadena JSON hasta su comilla de cierre. Devuelve (texto, bytes consumidos).
fn json_unescape(s: &str) -> (String, usize) {
    let mut out = String::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut pending: Option<u32> = None;
    while i < b.len() {
        let c = b[i];
        if c == '"' { break; }
        if c == '\\' && i + 1 < b.len() {
            i += 1;
            match b[i] {
                'n' => out.push('\n'), 't' => out.push('\t'), 'r' => {}, '/' => out.push('/'), '\\' => out.push('\\'), '"' => out.push('"'),
                'u' if i + 4 < b.len() => {
                    let hex: String = b[i + 1..i + 5].iter().collect();
                    let v = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD);
                    i += 4;
                    if (0xD800..0xDC00).contains(&v) { pending = Some(v); }
                    else if (0xDC00..0xE000).contains(&v) {
                        if let Some(h) = pending.take() { if let Some(ch) = char::from_u32(0x10000 + ((h - 0xD800) << 10) + (v - 0xDC00)) { out.push(ch); } }
                    } else if let Some(ch) = char::from_u32(v) { out.push(ch); }
                }
                o => out.push(o),
            }
        } else { out.push(c); }
        i += 1;
    }
    (out, i)
}

/// Trocea un array JSON de objetos (respetando llaves y cadenas).
fn json_objects(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start, mut in_str, mut esc) = (0i32, 0usize, false, false);
    for (i, c) in s.char_indices() {
        if in_str { if esc { esc = false; } else if c == '\\' { esc = true; } else if c == '"' { in_str = false; } continue; }
        match c {
            '"' => in_str = true,
            '{' => { if depth == 0 { start = i; } depth += 1; }
            '}' => { depth -= 1; if depth == 0 { out.push(&s[start..=i]); } }
            _ => {}
        }
    }
    out
}

fn scan_github() -> Option<Vec<GhRepo>> {
    let o = std::process::Command::new("gh").args(["repo", "list", "--limit", "20", "--json", "name,isPrivate,stargazerCount,pushedAt,url,primaryLanguage"])
        .creation_flags(NO_WINDOW).output().ok()?;
    if !o.status.success() { return None; }
    let body = String::from_utf8_lossy(&o.stdout).to_string();
    let mut v: Vec<GhRepo> = json_objects(&body).iter().filter_map(|ob| {
        let lang = ob.find("\"primaryLanguage\":{").and_then(|i| jstr(&ob[i..], "name")).unwrap_or_default();
        Some(GhRepo {
            name: jstr(ob, "name")?, private: ob.contains("\"isPrivate\":true"), stars: jnum(ob, "stargazerCount").unwrap_or(0.0) as u32,
            pushed: jstr(ob, "pushedAt").map(|s| iso_to_unix(&s)).unwrap_or(0), url: jstr(ob, "url")?, color: lang_color(&lang), lang,
        })
    }).collect();
    // "name" aparece también dentro de primaryLanguage: el del repo es el primero que no es el lenguaje
    v.sort_by(|a, b| b.pushed.cmp(&a.pushed));
    Some(v)
}

pub static PROJ_NOW: AtomicBool = AtomicBool::new(false);

fn proj_thread() {
    let mut n = 0u32;
    if sys::demo() {
        let now = now_unix();
        let r = |name: &str, br: &str, dirty, lang, color, h: u64| Repo { name: name.into(), path: format!("C:\\Proyectos\\{name}"), branch: br.into(), dirty, lang, color, modified: now - h * 3600, remote: None };
        let g = |name: &str, private, stars, h: u64, lang: &str, color| GhRepo { name: name.into(), private, stars, pushed: now - h * 3600, url: format!("https://github.com/alex/{name}"), lang: lang.into(), color };
        let mut p = PROJ.lock().unwrap();
        p.local = vec![
            r("macbar", "main", Some(3), "Rust", [0.87, 0.65, 0.52], 1), r("portfolio-web", "main", Some(0), "TypeScript", [0.19, 0.47, 0.78], 20),
            r("api-tareas", "dev", Some(7), "Go", [0.0, 0.68, 0.85], 52), r("juego-plataformas", "main", Some(0), "C#", [0.4, 0.75, 0.3], 140),
            r("dotfiles", "main", Some(1), "Shell", [0.54, 0.88, 0.32], 300), r("notas", "main", Some(0), "Markdown", [0.5, 0.5, 0.5], 600),
        ];
        p.gh = vec![
            g("macbar", false, 128, 1, "Rust", [0.87, 0.65, 0.52]), g("portfolio-web", false, 12, 20, "TypeScript", [0.19, 0.47, 0.78]),
            g("api-tareas", true, 0, 52, "Go", [0.0, 0.68, 0.85]), g("dotfiles", false, 4, 300, "Shell", [0.54, 0.88, 0.32]),
        ];
        p.gh_ok = Some(true);
        p.scanned = true;
        return;
    }
    loop {
        let local = scan_local();
        { let mut p = PROJ.lock().unwrap(); p.local = local; p.scanned = true; }
        if n % 10 == 0 {
            let gh = scan_github();
            let mut p = PROJ.lock().unwrap();
            p.gh_ok = Some(gh.is_some());
            if let Some(g) = gh { p.gh = g; }
        }
        n += 1;
        for _ in 0..60 {
            if PROJ_NOW.swap(false, Ordering::Relaxed) { n = 0; break; }
            std::thread::sleep(Duration::from_millis(1000));
        }
        while !VISIBLE.load(Ordering::Relaxed) && !PROJ_NOW.load(Ordering::Relaxed) { std::thread::sleep(Duration::from_millis(1000)); }
    }
}

// ---------------------------------------------------------------- formato
pub fn fmt_bytes(b: u64) -> String {
    let (v, u) = if b >= 1 << 40 { (b as f64 / (1u64 << 40) as f64, "TB") } else if b >= 1 << 30 { (b as f64 / (1u64 << 30) as f64, "GB") }
        else if b >= 1 << 20 { (b as f64 / (1u64 << 20) as f64, "MB") } else if b >= 1 << 10 { (b as f64 / 1024.0, "KB") } else { (b as f64, "B") };
    let s = if v >= 100.0 || u == "B" { format!("{v:.0}") } else { format!("{v:.1}") };
    // Coma decimal en español, punto en inglés
    if i18n::en() { format!("{s} {u}") } else { format!("{} {u}", s.replace('.', ",")) }
}

pub fn fmt_rate(bps: f32) -> String {
    if bps >= 1e6 { let s = format!("{:.1} MB/s", bps / 1e6); if i18n::en() { s } else { s.replace('.', ",") } } else if bps >= 1e3 { format!("{:.0} KB/s", bps / 1e3) } else { format!("{:.0} B/s", bps) }
}

pub fn ago(unix: u64) -> String {
    if unix == 0 { return String::new(); }
    let d = now_unix().saturating_sub(unix);
    match d {
        0..=59 => i18n::t("ahora").into(),
        60..=3599 => i18n::tf("hace {} min", &[&(d / 60)]),
        3600..=86399 => i18n::tf("hace {} h", &[&(d / 3600)]),
        86400..=172799 => i18n::t("ayer").into(),
        172800..=604799 => i18n::tf("hace {} días", &[&(d / 86400)]),
        604800..=2591999 => { let w = d / 604800; if w == 1 { i18n::t("hace 1 semana").into() } else { i18n::tf("hace {} semanas", &[&w]) } }
        _ => { let m = d / 2592000; if m <= 1 { i18n::t("hace 1 mes").into() } else if m < 12 { i18n::tf("hace {} meses", &[&m]) } else { i18n::tf("hace {} años", &[&(m / 12)]) } }
    }
}

#[cfg(test)]
mod tests {
    use super::git_safe;
    fn repo(cfg: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("macbar-gs-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join(".git").join("config"), cfg).unwrap();
        d
    }
    #[test]
    fn normal_repo_passes() {
        assert!(git_safe(&repo("[core]\n\trepositoryformatversion = 0\n\tbare = false\n[remote \"origin\"]\n\turl = https://github.com/a/b.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n[branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n")));
    }
    #[test]
    fn bom_filter_rejected() {
        assert!(!git_safe(&repo("\u{feff}[filter \"x\"]\n\tclean = calc.exe\n")));
        assert!(!git_safe(&repo("[core]\n\tbare = false\n\u{feff}[filter \"x\"]\n\tclean = calc.exe\n")));
    }
    #[test]
    fn junk_lines_rejected() {
        assert!(!git_safe(&repo("clean = calc.exe\n")));
        assert!(!git_safe(&repo("[core]\n\tbare = false\n\t[weird stuff\n")));
        assert!(!git_safe(&repo("[filter \"x\"]\n\tclean = calc.exe\n")));
        assert!(!git_safe(&repo("[core]\n\tfsmonitor = calc.exe\n")));
    }
}
