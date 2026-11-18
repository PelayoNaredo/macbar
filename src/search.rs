//! Buscador propio: apps del menú Inicio (las mismas que lista Windows, también las de la Store),
//! archivos del índice de Windows Search, páginas de Configuración, calculadora y búsqueda web.
//! Se maneja entero con el teclado: escribir, ↑/↓, Intro (Ctrl+Intro abre la carpeta del archivo).

use super::*;
use popup::{Content, Re, Ui};
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{channel as s_channel, Sender as SSender};
use windows::core::{Interface, BSTR, GUID};
use windows::Win32::System::Variant::VARIANT;

pub const W: f32 = 600.0;
const PAD: f32 = 12.0;
const INPUT_H: f32 = 50.0;
const HDR: f32 = 24.0;
const ROW: f32 = 38.0;
const BEST: f32 = 58.0;
const ICON_PX: u32 = 64;
const ID_ROW0: u32 = 100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cat { App, File, Folder, Setting, Web, Calc }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sec { Calc, Best, Recent, Apps, Settings, Files, Web }

impl Sec {
    fn label(self) -> &'static str {
        match self {
            Sec::Calc => "Calculadora", Sec::Best => "Mejor resultado", Sec::Recent => "Recientes",
            Sec::Apps => "Aplicaciones", Sec::Settings => "Configuración", Sec::Files => "Archivos y carpetas", Sec::Web => "Web",
        }
    }
}

#[derive(Clone)]
struct Item { cat: Cat, title: String, sub: String, target: String, icon: Option<Arc<Vec<u8>>>, score: i32 }

/// Lo que va encontrando el hilo de búsqueda para la consulta `ver`.
#[derive(Default)]
struct Out { ver: u64, apps: Vec<Item>, settings: Vec<Item>, files: Vec<Item>, recent: Vec<Item>, files_done: bool }
static OUT: Mutex<Out> = Mutex::new(Out { ver: 0, apps: Vec::new(), settings: Vec::new(), files: Vec::new(), recent: Vec::new(), files_done: true });
/// Última consulta pedida: el hilo abandona las que ya no interesan.
static LATEST: AtomicU64 = AtomicU64::new(0);

enum Job { Query(u64, String) }
static JOBS: OnceLock<SSender<Job>> = OnceLock::new();
fn job(j: Job) {
    let tx = JOBS.get_or_init(|| { let (tx, rx) = s_channel(); std::thread::spawn(move || worker(rx)); tx });
    let _ = tx.send(j);
}

/// Abre o cierra el buscador, centrado bajo la barra.
pub fn toggle() {
    let s = scale_of(bar_hwnd());
    let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let wpx = (W * s) as i32;
    popup::toggle(popup::Kind::Search, sw / 2 + wpx / 2);
}

// ---------------------------------------------------------------- texto
/// Minúsculas y sin tildes, para comparar como lo haría una persona.
fn fold(s: &str) -> String {
    s.chars().flat_map(|c| c.to_lowercase()).map(|c| match c {
        'á' | 'à' | 'ä' | 'â' => 'a', 'é' | 'è' | 'ë' | 'ê' => 'e', 'í' | 'ì' | 'ï' | 'î' => 'i',
        'ó' | 'ò' | 'ö' | 'ô' => 'o', 'ú' | 'ù' | 'ü' | 'û' => 'u', 'ñ' => 'n', 'ç' => 'c', c => c,
    }).collect()
}

/// Cuánto se parece `name` a lo escrito (ambos ya pasados por `fold`). None = nada que ver.
fn score(q: &str, name: &str) -> Option<i32> {
    if q.is_empty() { return None; }
    if name == q { return Some(1000); }
    if name.starts_with(q) { return Some(900 - (name.len() as i32 - q.len() as i32).min(100)); }
    // Sin guiones ni espacios: "wifi" → "Wi-Fi", "onedrive" → "One Drive"
    let compact = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let (cq, cn) = (compact(q), compact(name));
    if !cq.is_empty() && cn.starts_with(&cq) { return Some(880 - (cn.len() as i32 - cq.len() as i32).min(100)); }
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if words.iter().any(|w| w.starts_with(q)) { return Some(760); }
    let qw: Vec<&str> = q.split_whitespace().collect();
    if qw.len() > 1 && qw.iter().all(|x| words.iter().any(|w| w.starts_with(x))) { return Some(720); }
    let initials: String = words.iter().filter_map(|w| w.chars().next()).collect();
    if q.len() >= 2 && initials.starts_with(q) { return Some(650); }
    if name.contains(q) { return Some(500); }
    // Letras en orden aunque falten otras ("vscode" → "Visual Studio Code")
    if q.chars().count() >= 3 {
        let mut it = name.chars();
        if q.chars().filter(|c| !c.is_whitespace()).all(|c| it.any(|n| n == c)) { return Some(200); }
    }
    None
}

/// Recorta por la izquierda (rutas: lo importante está al final).
fn ltrunc(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max { s.to_string() } else { "…".to_string() + &s.chars().skip(n - max + 1).collect::<String>() }
}

// ---------------------------------------------------------------- calculadora
fn calc(q: &str) -> Option<String> {
    let s: String = q.chars().filter(|c| !c.is_whitespace()).collect::<String>().replace(',', ".").replace(['x', 'X', '×'], "*").replace('÷', "/");
    if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit() || "+-*/^().%".contains(c)) { return None; }
    // Hace falta una operación de verdad entre números ("12" o "-3" solos no cuentan)
    if !s.chars().skip(1).any(|c| "+-*/^%".contains(c)) || !s.chars().any(|c| c.is_ascii_digit()) { return None; }
    let b = s.as_bytes();
    let mut i = 0;
    let v = expr(b, &mut i)?;
    if i != b.len() || !v.is_finite() { return None; }
    let txt = if (v - v.round()).abs() < 1e-9 && v.abs() < 1e15 { format!("{}", v.round() as i64) } else {
        let t = format!("{v:.10}");
        t.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    Some(txt.replace('.', ","))
}
fn expr(b: &[u8], i: &mut usize) -> Option<f64> {
    let mut v = term(b, i)?;
    while *i < b.len() && (b[*i] == b'+' || b[*i] == b'-') { let op = b[*i]; *i += 1; let r = term(b, i)?; if op == b'+' { v += r } else { v -= r } }
    Some(v)
}
fn term(b: &[u8], i: &mut usize) -> Option<f64> {
    let mut v = power(b, i)?;
    while *i < b.len() && (b[*i] == b'*' || b[*i] == b'/') { let op = b[*i]; *i += 1; let r = power(b, i)?; if op == b'*' { v *= r } else { v /= r } }
    Some(v)
}
fn power(b: &[u8], i: &mut usize) -> Option<f64> {
    let base = unary(b, i)?;
    if *i < b.len() && b[*i] == b'^' { *i += 1; let e = power(b, i)?; return Some(base.powf(e)); }
    Some(base)
}
fn unary(b: &[u8], i: &mut usize) -> Option<f64> {
    if *i < b.len() && b[*i] == b'-' { *i += 1; return Some(-unary(b, i)?); }
    let mut v = if *i < b.len() && b[*i] == b'(' {
        *i += 1;
        let v = expr(b, i)?;
        if *i >= b.len() || b[*i] != b')' { return None; }
        *i += 1;
        v
    } else {
        let st = *i;
        while *i < b.len() && (b[*i].is_ascii_digit() || b[*i] == b'.') { *i += 1; }
        std::str::from_utf8(&b[st..*i]).ok()?.parse::<f64>().ok()?
    };
    while *i < b.len() && b[*i] == b'%' { *i += 1; v /= 100.0; }
    Some(v)
}

// ---------------------------------------------------------------- configuración
const SETTINGS: &[(&str, &str, &str)] = &[
    ("Wi-Fi", "ms-settings:network-wifi", "wifi red internet inalambrica"),
    ("Red e Internet", "ms-settings:network", "red ethernet proxy conexion"),
    ("VPN", "ms-settings:network-vpn", "vpn"),
    ("Modo avión", "ms-settings:network-airplanemode", "avion"),
    ("Bluetooth y dispositivos", "ms-settings:bluetooth", "bluetooth dispositivos auriculares"),
    ("Ratón", "ms-settings:mousetouchpad", "raton mouse cursor puntero"),
    ("Panel táctil", "ms-settings:devices-touchpad", "touchpad"),
    ("Impresoras y escáneres", "ms-settings:printers", "impresora escaner"),
    ("Pantalla", "ms-settings:display", "pantalla resolucion brillo escala monitor"),
    ("Luz nocturna", "ms-settings:nightlight", "noche luz azul"),
    ("Sonido", "ms-settings:sound", "sonido audio volumen altavoces microfono"),
    ("Notificaciones", "ms-settings:notifications", "avisos no molestar"),
    ("Energía y batería", "ms-settings:powersleep", "energia bateria suspension apagado"),
    ("Almacenamiento", "ms-settings:storagesense", "almacenamiento disco espacio liberar"),
    ("Multitarea", "ms-settings:multitasking", "ventanas ajustar escritorios"),
    ("Portapapeles", "ms-settings:clipboard", "historial copiar"),
    ("Personalización", "ms-settings:personalization", "personalizar apariencia"),
    ("Fondo de escritorio", "ms-settings:personalization-background", "fondo pantalla wallpaper imagen"),
    ("Colores", "ms-settings:colors", "modo oscuro claro color acento tema"),
    ("Temas", "ms-settings:themes", "tema"),
    ("Barra de tareas", "ms-settings:taskbar", "taskbar"),
    ("Aplicaciones instaladas", "ms-settings:appsfeatures", "apps programas desinstalar"),
    ("Aplicaciones predeterminadas", "ms-settings:defaultapps", "navegador predeterminado abrir con"),
    ("Aplicaciones de inicio", "ms-settings:startupapps", "arranque inicio automatico"),
    ("Tu información", "ms-settings:yourinfo", "cuenta usuario perfil"),
    ("Opciones de inicio de sesión", "ms-settings:signinoptions", "contrasena pin hello huella"),
    ("Fecha y hora", "ms-settings:dateandtime", "hora reloj zona horaria"),
    ("Idioma y región", "ms-settings:regionlanguage", "idioma teclado region"),
    ("Juegos", "ms-settings:gaming-gamebar", "game bar juegos captura"),
    ("Accesibilidad", "ms-settings:easeofaccess", "lupa narrador contraste"),
    ("Privacidad y seguridad", "ms-settings:privacy", "permisos camara microfono ubicacion"),
    ("Windows Update", "ms-settings:windowsupdate", "actualizaciones actualizar update"),
    ("Seguridad de Windows", "windowsdefender:", "antivirus defender firewall"),
    ("Acerca de", "ms-settings:about", "sistema equipo nombre especificaciones"),
    ("Configuración", "ms-settings:", "ajustes configuracion"),
];

fn settings_for(q: &str) -> Vec<Item> {
    let mut v: Vec<Item> = SETTINGS.iter().filter_map(|(n, uri, kw)| {
        let s = score(q, &fold(n)).or_else(|| {
            // Palabras clave: valen menos que el nombre
            let words: Vec<&str> = kw.split(' ').collect();
            q.split_whitespace().all(|x| words.iter().any(|w| w.starts_with(x))).then_some(420)
        })?;
        // Las coincidencias sueltas de letras en ajustes solo meten ruido
        if s < 400 { return None; }
        Some(Item { cat: Cat::Setting, title: n.to_string(), sub: "Configuración".into(), target: uri.to_string(), icon: None, score: s - 30 })
    }).collect();
    v.sort_by(|a, b| b.score.cmp(&a.score));
    v.truncate(3);
    v
}

// ---------------------------------------------------------------- recientes
fn recent_path() -> String { sys::config_dir() + "\\search_recent.txt" }

fn cat_code(c: Cat) -> char { match c { Cat::App => 'a', Cat::File => 'f', Cat::Folder => 'd', Cat::Setting => 's', Cat::Web => 'w', Cat::Calc => 'c' } }
fn code_cat(c: &str) -> Option<Cat> { Some(match c { "a" => Cat::App, "f" => Cat::File, "d" => Cat::Folder, "s" => Cat::Setting, _ => return None }) }

fn load_recent() -> Vec<(Cat, String, String)> {
    std::fs::read_to_string(recent_path()).unwrap_or_default().lines().filter_map(|l| {
        let mut p = l.splitn(3, '\t');
        Some((code_cat(p.next()?)?, p.next()?.to_string(), p.next()?.to_string()))
    }).collect()
}

fn remember(it: &Item) {
    if matches!(it.cat, Cat::Web | Cat::Calc) { return; }
    let mut v = load_recent();
    v.retain(|(_, _, t)| *t != it.target);
    v.insert(0, (it.cat, it.title.clone(), it.target.clone()));
    v.truncate(8);
    let txt: String = v.iter().map(|(c, n, t)| format!("{}\t{}\t{}\n", cat_code(*c), n.replace(['\t', '\n'], " "), t)).collect();
    let _ = std::fs::create_dir_all(sys::config_dir());
    let _ = std::fs::write(recent_path(), txt);
}

// ---------------------------------------------------------------- hilo de búsqueda
struct App { name: String, key: String, parsing: String, item: IShellItem }

/// Las apps de "Todas las aplicaciones" de Inicio (escritorio y Store), como las enumera el Explorador.
fn load_apps() -> Vec<App> {
    let mut out = Vec::new();
    unsafe {
        let Ok(folder) = SHGetKnownFolderItem::<IShellItem>(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) else { return out };
        let Ok(en) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else { return out };
        loop {
            let mut buf = [None];
            let mut got = 0u32;
            if en.Next(&mut buf, Some(&mut got)).is_err() || got == 0 { break; }
            let Some(item) = buf[0].take() else { break };
            let name = shell_name(&item, SIGDN_NORMALDISPLAY);
            let parsing = shell_name(&item, SIGDN_PARENTRELATIVEPARSING);
            if name.is_empty() || parsing.is_empty() { continue; }
            let key = fold(&name);
            // Desinstaladores, ayudas y léeme no son apps que nadie quiera abrir desde aquí
            if ["uninstall", "desinstal", "readme", "leeme", "release notes"].iter().any(|b| key.contains(b)) { continue; }
            if [".url", ".txt", ".chm", ".html", ".htm", ".pdf"].iter().any(|e| parsing.to_lowercase().ends_with(e)) { continue; }
            out.push(App { name, key, parsing, item });
        }
    }
    out
}

fn shell_name(item: &IShellItem, kind: SIGDN) -> String {
    unsafe {
        match item.GetDisplayName(kind) {
            Ok(p) => { let s = p.to_string().unwrap_or_default(); CoTaskMemFree(Some(p.0 as _)); s }
            Err(_) => String::new(),
        }
    }
}

/// Icono del shell a 64 px (el mismo que enseña Inicio / el Explorador).
fn shell_icon(wic: &IWICImagingFactory, item: &IShellItem) -> Option<Vec<u8>> {
    unsafe {
        let f: IShellItemImageFactory = item.cast().ok()?;
        let hb = f.GetImage(SIZE { cx: ICON_PX as i32, cy: ICON_PX as i32 }, SIIGBF(SIIGBF_ICONONLY.0 | SIIGBF_BIGGERSIZEOK.0)).ok()?;
        let b = wic.CreateBitmapFromHBITMAP(hb, HPALETTE::default(), WICBitmapUsePremultipliedAlpha).ok();
        let _ = DeleteObject(HGDIOBJ(hb.0));
        wic_square_n(wic, &b?.into(), ICON_PX)
    }
}

fn path_icon(wic: &IWICImagingFactory, path: &str) -> Option<Vec<u8>> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
        shell_icon(wic, &item)
    }
}

// ---- índice de Windows Search (OLE DB a través de ADO, por IDispatch)
unsafe fn disp(d: &IDispatch, name: &str, flags: DISPATCH_FLAGS, args: &[VARIANT]) -> Option<VARIANT> {
    unsafe {
        let n = HSTRING::from(name);
        let names = [PCWSTR(n.as_ptr())];
        let mut id = 0i32;
        d.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, 0x0400, &mut id).ok()?;
        // DISPPARAMS lleva los argumentos al revés
        let mut rev: Vec<VARIANT> = args.iter().rev().cloned().collect();
        let dp = DISPPARAMS { rgvarg: if rev.is_empty() { std::ptr::null_mut() } else { rev.as_mut_ptr() }, rgdispidNamedArgs: std::ptr::null_mut(), cArgs: rev.len() as u32, cNamedArgs: 0 };
        let mut out = VARIANT::default();
        d.Invoke(id, &GUID::zeroed(), 0x0400, flags, &dp, Some(&mut out), None, None).ok()?;
        Some(out)
    }
}
fn get(d: &IDispatch, name: &str) -> Option<VARIANT> { unsafe { disp(d, name, DISPATCH_PROPERTYGET, &[]) } }
fn call(d: &IDispatch, name: &str, args: &[VARIANT]) -> Option<VARIANT> { unsafe { disp(d, name, DISPATCH_METHOD, args) } }

fn connect() -> Option<IDispatch> {
    unsafe {
        let clsid = CLSIDFromProgID(w!("ADODB.Connection")).ok()?;
        let c: IDispatch = CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER).ok()?;
        call(&c, "Open", &[VARIANT::from("Provider=Search.CollatorDSO;Extended Properties='Application=Windows';")])?;
        Some(c)
    }
}

/// Archivos y carpetas del perfil cuyo nombre empieza por las palabras escritas: (ruta, nombre).
fn index_files(conn: &IDispatch, q: &str) -> Vec<(String, String)> {
    // Solo letras, números y . _ - : nada que pueda romper la consulta
    let words: Vec<String> = q.split_whitespace()
        .map(|w| w.chars().filter(|c| c.is_alphanumeric() || "._-".contains(*c)).collect::<String>())
        .filter(|w| !w.is_empty()).take(4).collect();
    if words.is_empty() { return Vec::new(); }
    let cond = words.iter().map(|w| format!("CONTAINS(System.FileName, '\"{w}*\"')")).collect::<Vec<_>>().join(" AND ");
    let scope = std::env::var("USERPROFILE").unwrap_or_default().replace('\\', "/");
    let sql = format!("SELECT TOP 60 System.ItemUrl, System.ItemNameDisplay FROM SystemIndex WHERE SCOPE='file:{scope}' AND {cond} ORDER BY System.Search.Rank DESC");
    let mut out = Vec::new();
    let Some(rs) = call(conn, "Execute", &[VARIANT::from(sql.as_str())]).and_then(|v| IDispatch::try_from(&v).ok()) else { return out };
    for _ in 0..60 {
        if get(&rs, "EOF").and_then(|v| bool::try_from(&v).ok()).unwrap_or(true) { break; }
        let field = |i: i32| -> String {
            get(&rs, "Fields").and_then(|f| IDispatch::try_from(&f).ok())
                .and_then(|f| unsafe { disp(&f, "Item", DISPATCH_PROPERTYGET | DISPATCH_METHOD, &[VARIANT::from(i)]) })
                .and_then(|x| IDispatch::try_from(&x).ok())
                .and_then(|x| get(&x, "Value"))
                .and_then(|v| BSTR::try_from(&v).ok()).map(|b| b.to_string()).unwrap_or_default()
        };
        let url = field(0);
        let name = field(1);
        if let Some(p) = url.strip_prefix("file:") {
            let p = p.replace('/', "\\");
            if !name.is_empty() && !junk(&p) { out.push((p, name)); }
        }
        if call(&rs, "MoveNext", &[]).is_none() { break; }
    }
    let _ = call(&rs, "Close", &[]);
    // Lo que una persona busca suele estar cerca de la superficie (Escritorio, Documentos…), no en lo hondo
    out.sort_by_key(|(p, _)| p.matches('\\').count());
    out
}

/// Carpetas de herramientas, cachés y compilaciones: llenan el índice pero nadie las busca.
fn junk(p: &str) -> bool {
    let l = p.to_lowercase();
    if l.split('\\').skip(3).any(|seg| seg.starts_with('.') || seg.starts_with('$')) { return true; }
    ["\\appdata\\", "\\node_modules\\", "\\build\\", "\\target\\", "\\dist\\", "\\out\\", "\\bin\\", "\\obj\\", "\\vendor\\",
     "\\site-packages\\", "\\__pycache__\\", "\\library\\", "\\temp\\", "\\cache\\", "\\worktrees\\", "\\intermediates\\", "\\packages\\"]
        .iter().any(|x| l.contains(x))
}

fn worker(rx: std::sync::mpsc::Receiver<Job>) {
    unsafe { let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); }
    let Ok(wic) = (unsafe { CoCreateInstance::<_, IWICImagingFactory>(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }) else { return };
    let mut apps = load_apps();
    let mut loaded = Instant::now();
    let mut icons: HashMap<String, Option<Arc<Vec<u8>>>> = HashMap::new();
    let mut conn: Option<IDispatch> = None;
    let icon_of = |icons: &mut HashMap<String, Option<Arc<Vec<u8>>>>, key: &str, f: &dyn Fn() -> Option<Vec<u8>>| -> Option<Arc<Vec<u8>>> {
        if icons.len() > 600 { icons.clear(); }
        icons.entry(key.to_string()).or_insert_with(|| f().map(Arc::new)).clone()
    };
    while let Ok(mut j) = rx.recv() {
        // Solo cuenta la última consulta
        while let Ok(n) = rx.try_recv() { j = n; }
        let Job::Query(ver, q) = j;
        if LATEST.load(Ordering::Relaxed) != ver { continue; }
        // Apps nuevas instaladas: se vuelve a leer la lista de vez en cuando
        if loaded.elapsed() > Duration::from_secs(300) { apps = load_apps(); loaded = Instant::now(); }
        let recent = load_recent();
        let fq = fold(q.trim());
        if fq.is_empty() {
            let items: Vec<Item> = recent.iter().filter_map(|(c, n, t)| {
                let icon = match c {
                    Cat::App => apps.iter().find(|a| a.parsing == *t).and_then(|a| icon_of(&mut icons, t, &|| shell_icon(&wic, &a.item))),
                    Cat::File | Cat::Folder => { if !std::path::Path::new(t).exists() { return None; } icon_of(&mut icons, t, &|| path_icon(&wic, t)) }
                    _ => None,
                };
                let sub = match c { Cat::App => "Aplicación".into(), Cat::Setting => "Configuración".into(), _ => ltrunc(&parent_of(t), 46) };
                Some(Item { cat: *c, title: n.clone(), sub, target: t.clone(), icon, score: 0 })
            }).collect();
            let mut o = OUT.lock().unwrap();
            *o = Out { ver, recent: items, files_done: true, ..Default::default() };
            drop(o);
            popup::notify(popup::Kind::Search);
            continue;
        }
        // 1) Apps y configuración: al momento
        let mut found: Vec<(i32, usize)> = apps.iter().enumerate().filter_map(|(i, a)| {
            let s = score(&fq, &a.key)?;
            let boost = if recent.iter().any(|(_, _, t)| *t == a.parsing) { 120 } else { 0 };
            Some((s + boost, i))
        }).collect();
        found.sort_by(|a, b| b.0.cmp(&a.0).then(apps[a.1].name.len().cmp(&apps[b.1].name.len())));
        // Las coincidencias difusas solo si no hay nada mejor
        if found.first().is_some_and(|f| f.0 >= 500) { found.retain(|f| f.0 >= 500); }
        found.truncate(5);
        let app_items: Vec<Item> = found.iter().map(|(s, i)| {
            let a = &apps[*i];
            let icon = icon_of(&mut icons, &a.parsing, &|| shell_icon(&wic, &a.item));
            Item { cat: Cat::App, title: a.name.clone(), sub: "Aplicación".into(), target: a.parsing.clone(), icon, score: *s }
        }).collect();
        let settings = settings_for(&fq);
        // Una cuenta ("12*3") no es un nombre de archivo
        let want_files = fq.chars().count() >= 2 && calc(&q).is_none();
        {
            let mut o = OUT.lock().unwrap();
            let keep = if o.ver == ver { std::mem::take(&mut o.files) } else { Vec::new() };
            *o = Out { ver, apps: app_items, settings, files: keep, recent: Vec::new(), files_done: !want_files };
        }
        popup::notify(popup::Kind::Search);
        if !want_files { continue; }
        // 2) Archivos del índice: tardan algo más; si ya se escribió otra cosa, no se busca
        std::thread::sleep(Duration::from_millis(60));
        if LATEST.load(Ordering::Relaxed) != ver { continue; }
        if conn.is_none() { conn = connect(); }
        let files = match &conn { Some(c) => index_files(c, q.trim()), None => Vec::new() };
        if files.is_empty() && conn.is_some() && LATEST.load(Ordering::Relaxed) == ver {
            // La conexión pudo caerse (servicio reiniciado): se rehace la próxima vez
            conn = None;
        }
        if LATEST.load(Ordering::Relaxed) != ver { continue; }
        let file_items: Vec<Item> = files.into_iter().take(5).map(|(p, n)| {
            let dir = std::fs::metadata(&p).map(|m| m.is_dir()).unwrap_or(false);
            let icon = icon_of(&mut icons, &p, &|| path_icon(&wic, &p));
            Item { cat: if dir { Cat::Folder } else { Cat::File }, title: n, sub: ltrunc(&parent_of(&p), 46), target: p, icon, score: 0 }
        }).collect();
        let mut o = OUT.lock().unwrap();
        if o.ver == ver { o.files = file_items; o.files_done = true; }
        drop(o);
        popup::notify(popup::Kind::Search);
    }
}

/// MACBAR_SEARCHTEST="consulta": escribe lo que encontraría el buscador en %TEMP%\macbar_search.txt.
pub fn selftest(q: &str) {
    unsafe { let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED); }
    if q == "__teclas" {
        // Teclear "winrar" y borrarlo letra a letra
        let mut p = SearchPanel::new();
        p.shown();
        let mut s = String::new();
        for c in "winrar".chars().chain(std::iter::repeat_n('\u{8}', 6)) {
            p.chr(c);
            std::thread::sleep(Duration::from_millis(120));
            p.data();
            s += &format!("{:?} -> «{}» filas={} alto={}\n", c, p.query, p.rows.len(), p.size().1);
        }
        let _ = std::fs::write(std::env::temp_dir().join("macbar_search.txt"), s);
        return;
    }
    let t = Instant::now();
    let apps = load_apps();
    let mut s = format!("apps cargadas: {} en {} ms\n", apps.len(), t.elapsed().as_millis());
    let fq = fold(q);
    let mut f: Vec<(i32, &App)> = apps.iter().filter_map(|a| Some((score(&fq, &a.key)?, a))).collect();
    f.sort_by(|a, b| b.0.cmp(&a.0));
    for (sc, a) in f.iter().take(5) { s += &format!("APP {sc} {} | {}\n", a.name, a.parsing); }
    for it in settings_for(&fq) { s += &format!("SET {} {} | {}\n", it.score, it.title, it.target); }
    let t = Instant::now();
    match connect() {
        Some(c) => { for (p, n) in index_files(&c, q) { s += &format!("FILE {n} | {p}\n"); } s += &format!("índice: {} ms\n", t.elapsed().as_millis()); }
        None => s += "índice: sin conexión\n",
    }
    s += &format!("calc: {:?}\n", calc(q));
    if let Ok(wic) = unsafe { CoCreateInstance::<_, IWICImagingFactory>(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) } {
        if let Some((_, a)) = f.first() { s += &format!("icono app: {:?} bytes\n", shell_icon(&wic, &a.item).map(|v| v.len())); }
    }
    let _ = std::fs::write(std::env::temp_dir().join("macbar_search.txt"), s);
}

fn parent_of(p: &str) -> String {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let dir = std::path::Path::new(p).parent().map(|d| d.display().to_string()).unwrap_or_default();
    match dir.strip_prefix(&home) { Some(r) => format!("~{r}"), None => dir }
}

fn url_encode(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        b' ' => "+".into(),
        _ => format!("%{b:02X}"),
    }).collect()
}

fn read_clipboard() -> Option<String> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    unsafe {
        OpenClipboard(None).ok()?;
        let mut out = None;
        if let Ok(h) = GetClipboardData(13 /* CF_UNICODETEXT */) {
            let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
            if !p.is_null() {
                let mut n = 0; while *p.add(n) != 0 && n < 400 { n += 1; }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, n)));
                let _ = GlobalUnlock(HGLOBAL(h.0));
            }
        }
        let _ = CloseClipboard();
        out
    }
}

// ---------------------------------------------------------------- panel
struct Row { it: Item, sec: Sec, born: Instant, delay: f32 }

pub struct SearchPanel {
    query: String,
    /// Ctrl+A: lo siguiente que se escriba sustituye a todo
    all: bool,
    ver: u64,
    rows: Vec<Row>,
    sel: usize,
    files_pending: bool,
    asked: Instant,
    opened: Instant,
    last: Instant,
    /// Resaltado de la fila elegida: se desliza hasta ella
    sel_y: f32, sel_h: f32,
    hover_prev: Option<u32>,
    recent: Vec<Item>,
    calc: Option<String>,
    bmps: HashMap<String, ID2D1Bitmap>,
    bmp_rt: usize,
}

impl SearchPanel {
    pub fn new() -> Self {
        SearchPanel { query: String::new(), all: false, ver: 0, rows: Vec::new(), sel: 0, files_pending: false, asked: Instant::now(),
            opened: Instant::now(), last: Instant::now(), sel_y: -1.0, sel_h: ROW, hover_prev: None, recent: Vec::new(), calc: None, bmps: HashMap::new(), bmp_rt: 0 }
    }

    /// Lo escrito cambió: se pide la búsqueda y se rehace la lista con lo que ya se sabe.
    fn changed(&mut self) {
        static GEN: AtomicU64 = AtomicU64::new(0);
        self.ver = GEN.fetch_add(1, Ordering::Relaxed) + 1;
        LATEST.store(self.ver, Ordering::Relaxed);
        self.asked = Instant::now();
        self.sel = 0;
        self.calc = calc(&self.query);
        job(Job::Query(self.ver, self.query.clone()));
        self.rebuild();
    }

    fn rebuild(&mut self) {
        let o = OUT.lock().unwrap();
        let q = self.query.trim().to_string();
        let mut list: Vec<(Item, Sec)> = Vec::new();
        if let Some(c) = &self.calc {
            list.push((Item { cat: Cat::Calc, title: format!("= {c}"), sub: "Intro para copiar el resultado".into(), target: c.clone(), icon: None, score: 0 }, Sec::Calc));
        }
        // Los recientes se guardan: al borrarlo todo aparecen ya, sin encoger y volver a crecer
        if o.ver == self.ver && q.is_empty() { self.recent = o.recent.clone(); }
        if q.is_empty() {
            for it in &self.recent { list.push((it.clone(), Sec::Recent)); }
        } else {
            let mut apps = o.apps.clone();
            let mut settings = o.settings.clone();
            let mut files = o.files.clone();
            // Mejor resultado: lo que más se parece entre apps y ajustes; si nada se parece bastante, el primer archivo
            let ta = apps.first().map(|x| x.score).unwrap_or(-1);
            let ts = settings.first().map(|x| x.score).unwrap_or(-1);
            let best = if ta >= 600 && ta >= ts { Some(apps.remove(0)) }
                else if ts >= 600 { Some(settings.remove(0)) }
                else if !files.is_empty() && ta < 500 && ts < 500 { Some(files.remove(0)) }
                else if ta >= ts && ta >= 0 { Some(apps.remove(0)) }
                else if ts >= 0 { Some(settings.remove(0)) }
                else { None };
            if self.calc.is_none() { if let Some(b) = best { list.push((b, Sec::Best)); } }
            else if let Some(b) = best { apps.insert(0, b); }
            for it in apps.into_iter().take(4) { list.push((it, Sec::Apps)); }
            for it in settings.into_iter().take(3) { list.push((it, Sec::Settings)); }
            for it in files.into_iter().take(5) { list.push((it, Sec::Files)); }
            list.push((Item { cat: Cat::Web, title: format!("Buscar «{}» en la web", trunc(&q, 40)), sub: "Google".into(), target: q.clone(), icon: None, score: 0 }, Sec::Web));
        }
        self.files_pending = !q.is_empty() && !(o.ver == self.ver && o.files_done);
        drop(o);
        // Las filas que ya estaban no vuelven a entrar; las nuevas, en cascada
        let first_open = self.opened.elapsed().as_secs_f32() < 0.2;
        let now = Instant::now();
        let mut k = 0.0;
        let rows: Vec<Row> = list.into_iter().map(|(it, sec)| {
            if let Some(r) = self.rows.iter().find(|r| r.it.target == it.target && r.it.cat == it.cat && r.sec == sec) {
                return Row { it, sec, born: r.born, delay: r.delay };
            }
            let d = if first_open { 0.06 + k * 0.03 } else { k * 0.022 };
            k += 1.0;
            Row { it, sec, born: now, delay: d }
        }).collect();
        // Si la fila elegida sigue, se queda elegida
        let keep = self.rows.get(self.sel).map(|r| r.it.target.clone());
        self.rows = rows;
        if let Some(t) = keep { if let Some(i) = self.rows.iter().position(|r| r.it.target == t) { self.sel = i; } }
        if self.sel >= self.rows.len() { self.sel = 0; }
    }

    /// Posición de cada fila: (cabecera de sección si empieza una, índice, y, alto).
    fn layout(&self) -> (Vec<(Option<f32>, usize, f32, f32)>, f32) {
        let mut y = PAD + INPUT_H + 6.0;
        let mut out = Vec::new();
        let mut prev: Option<Sec> = None;
        for (i, r) in self.rows.iter().enumerate() {
            let mut hy = None;
            if prev != Some(r.sec) {
                if prev.is_some() { y += 4.0; }
                hy = Some(y);
                y += HDR;
                prev = Some(r.sec);
            }
            let h = if matches!(r.sec, Sec::Best | Sec::Calc) { BEST } else { ROW };
            out.push((hy, i, y, h));
            y += h;
        }
        let total = if self.rows.is_empty() { PAD + INPUT_H + if self.query.trim().is_empty() { 34.0 } else { 0.0 } + PAD } else { y + 6.0 + PAD };
        (out, total)
    }

    fn launch(&self, i: usize, alt: bool) -> Re {
        let Some(r) = self.rows.get(i) else { return Re::Stay };
        let it = r.it.clone();
        Re::CloseRun(Box::new(move || {
            remember(&it);
            match it.cat {
                Cat::App => sys::open(&format!("shell:AppsFolder\\{}", it.target)),
                Cat::File | Cat::Folder if alt => sys::open_args("explorer.exe", &format!("/select,\"{}\"", it.target)),
                Cat::File | Cat::Folder | Cat::Setting => sys::open(&it.target),
                Cat::Web => sys::open(&format!("https://www.google.com/search?q={}", url_encode(&it.target))),
                Cat::Calc => deskw::copy_text(&it.target),
            }
        }))
    }

    fn bitmap(&mut self, rt: &ID2D1RenderTarget, key: &str, px: &Arc<Vec<u8>>) -> Option<ID2D1Bitmap> {
        let raw = rt.as_raw() as usize;
        if raw != self.bmp_rt { self.bmps.clear(); self.bmp_rt = raw; }
        if self.bmps.len() > 80 { self.bmps.clear(); }
        if let Some(b) = self.bmps.get(key) { return Some(b.clone()); }
        let b = deskw::bitmap(rt, px, ICON_PX)?;
        self.bmps.insert(key.to_string(), b.clone());
        Some(b)
    }
}

fn glyph_of(c: Cat) -> &'static str {
    match c { Cat::App => "\u{E737}", Cat::File => "\u{E8A5}", Cat::Folder => "\u{E8B7}", Cat::Setting => "\u{E713}", Cat::Web => "\u{E774}", Cat::Calc => "\u{E8EF}" }
}

impl Content for SearchPanel {
    fn size(&self) -> (f32, f32) { (W, self.layout().1) }

    fn shown(&mut self) {
        self.query.clear();
        self.all = false;
        self.opened = Instant::now();
        self.last = Instant::now();
        self.rows.clear();
        self.sel_y = -1.0;
        self.hover_prev = None;
        self.changed();
    }

    fn data(&mut self) { self.rebuild(); }

    fn animating(&self) -> bool {
        self.opened.elapsed().as_secs_f32() < 0.9
            || self.rows.iter().any(|r| r.born.elapsed().as_secs_f32() < r.delay + 0.3)
            || self.layout().0.get(self.sel).is_some_and(|s| (s.2 - self.sel_y).abs() > 0.3 || (s.3 - self.sel_h).abs() > 0.3)
            || (self.files_pending && self.asked.elapsed().as_secs_f32() < 4.0)
    }

    fn paint(&mut self, ui: &mut Ui) {
        let rt = ui.rt;
        let since = self.opened.elapsed().as_secs_f32();
        let dt = self.last.elapsed().as_secs_f32().min(1.0 / 30.0);
        self.last = Instant::now();
        // El ratón elige fila al pasar por encima (como las flechas)
        if ui.hover != self.hover_prev {
            if let Some(h) = ui.hover { if h >= ID_ROW0 && ((h - ID_ROW0) as usize) < self.rows.len() { self.sel = (h - ID_ROW0) as usize; } }
            self.hover_prev = ui.hover;
        }

        // ---- Campo de búsqueda
        let b0 = popup::rise(rt, popup::stagger(since, 0.0, 0.3));
        ui.glyph(&ui.f.icon_l, "\u{E721}", PAD + 6.0, PAD, 30.0, INPUT_H, &ui.dim);
        let tx = PAD + 44.0;
        if self.query.is_empty() {
            ui.text(&ui.f.q, "Buscar apps, archivos, ajustes y web", tx, PAD, INPUT_H, &ui.faint);
        } else {
            let tw = ui.p.width(&ui.f.q, &self.query);
            if self.all {
                unsafe { rt.FillRoundedRectangle(&rrect(rect(tx - 2.0, PAD + INPUT_H / 2.0 - 13.0, tx + tw + 2.0, PAD + INPUT_H / 2.0 + 13.0), 4.0), &ui.brush(rgba(0.23, 0.51, 0.96, 0.55))); }
            }
            ui.text(&ui.f.q, &self.query, tx, PAD, INPUT_H, &ui.white);
        }
        // Cursor: parpadea suave (late mientras se busca, fijo si no)
        let cx = tx + if self.query.is_empty() { 0.0 } else { ui.p.width(&ui.f.q, &self.query) + 1.0 };
        if !self.all {
            let blink = if self.animating() { 0.55 + 0.45 * (since * 5.0).cos().abs() } else { 1.0 };
            unsafe { rt.FillRectangle(&rect(cx, PAD + INPUT_H / 2.0 - 11.0, cx + 1.6, PAD + INPUT_H / 2.0 + 11.0), &ui.brush(rgba(0.23, 0.51, 0.96, blink))); }
        }
        // Buscando en el índice: tres puntos que laten
        if self.files_pending && self.asked.elapsed().as_secs_f32() > 0.15 {
            let t = self.asked.elapsed().as_secs_f32();
            for i in 0..3 {
                let a = 0.25 + 0.6 * (((t * 6.0 - i as f32 * 0.9).sin() + 1.0) / 2.0);
                let e = D2D1_ELLIPSE { point: Vector2 { X: W - PAD - 34.0 + i as f32 * 9.0, Y: PAD + INPUT_H / 2.0 }, radiusX: 2.2, radiusY: 2.2 };
                unsafe { rt.FillEllipse(&e, &ui.wa(a)); }
            }
        }
        popup::rise_end(rt, b0);

        let (slots, _) = self.layout();
        if self.rows.is_empty() {
            if self.query.trim().is_empty() {
                let b = popup::rise(rt, popup::stagger(since, 0.08, 0.3));
                ui.text(&ui.f.sub, "Escribe para buscar · ↑↓ para elegir · Intro para abrir", tx, PAD + INPUT_H, 30.0, &ui.faint);
                popup::rise_end(rt, b);
            }
            return;
        }
        ui.sep(PAD + INPUT_H + 1.0);

        // ---- Resaltado de la fila elegida (se desliza)
        if let Some(&(_, _, sy, sh)) = slots.get(self.sel) {
            if self.sel_y < 0.0 { self.sel_y = sy; self.sel_h = sh; }
            let k = 1.0 - (-dt / 0.055).exp();
            self.sel_y += (sy - self.sel_y) * k;
            self.sel_h += (sh - self.sel_h) * k;
            if (sy - self.sel_y).abs() < 0.3 { self.sel_y = sy; }
            if (sh - self.sel_h).abs() < 0.3 { self.sel_h = sh; }
            let ka = popup::stagger(self.rows[self.sel].born.elapsed().as_secs_f32(), self.rows[self.sel].delay, 0.24);
            unsafe { rt.FillRoundedRectangle(&rrect(rect(PAD, self.sel_y, W - PAD, self.sel_y + self.sel_h), 9.0), &ui.wa(0.11 * ka)); }
        }

        // ---- Filas
        for (hy, i, y, h) in slots {
            let (born, delay) = (self.rows[i].born, self.rows[i].delay);
            let b = popup::rise(rt, popup::stagger(born.elapsed().as_secs_f32(), delay, 0.24));
            if let Some(hy) = hy {
                ui.text(&ui.f.small, self.rows[i].sec.label(), PAD + 10.0, hy, HDR, &ui.faint);
            }
            let it = self.rows[i].it.clone();
            let big = h >= BEST;
            let isz = if big { 38.0 } else { 22.0 };
            let ix = PAD + 10.0;
            let iy = y + (h - isz) / 2.0;
            let bmp = it.icon.as_ref().and_then(|px| self.bitmap(rt, &it.target, px));
            match bmp {
                Some(bm) => deskw::draw_art(rt, &bm, ICON_PX, ix, iy, isz, 0.0),
                None => {
                    let e = D2D1_ELLIPSE { point: Vector2 { X: ix + isz / 2.0, Y: iy + isz / 2.0 }, radiusX: isz / 2.0, radiusY: isz / 2.0 };
                    let fill = if it.cat == Cat::Calc { ui.on.clone() } else { ui.wa(0.10) };
                    unsafe { rt.FillEllipse(&e, &fill); }
                    ui.glyph(if big { &ui.f.icon_l } else { &ui.f.icon }, glyph_of(it.cat), ix, iy, isz, isz, &ui.white);
                }
            }
            let tx = ix + isz + 12.0;
            if big {
                let f = if it.cat == Cat::Calc { &ui.f.big } else { &ui.f.name };
                ui.text(f, &trunc(&it.title, 52), tx, y + 7.0, if it.cat == Cat::Calc { 30.0 } else { 22.0 }, &ui.white);
                if it.cat != Cat::Calc { ui.text(&ui.f.sub, &it.sub, tx, y + 30.0, 18.0, &ui.dim); }
                else { ui.text_r(&ui.f.sub, &it.sub, W - PAD - 14.0, y, h, &ui.dim); }
            } else {
                let title = trunc(&it.title, 56);
                ui.text(&ui.f.lbl, &title, tx, y, h, &ui.white);
                let tw = ui.p.width(&ui.f.lbl, &title);
                let room = (W - PAD - 14.0) - (tx + tw + 18.0);
                if room > 40.0 && !it.sub.is_empty() {
                    let max = (room / 6.0) as usize;
                    let sub = if matches!(it.cat, Cat::File | Cat::Folder) { ltrunc(&it.sub, max) } else { trunc(&it.sub, max) };
                    ui.text_r(&ui.f.sub, &sub, W - PAD - 14.0, y, h, &ui.dim);
                }
            }
            ui.hit(rect(PAD, y, W - PAD, y + h), ID_ROW0 + i as u32);
            popup::rise_end(rt, b);
        }
        let _ = dt;
    }

    fn click(&mut self, id: u32, right: bool) -> Re {
        if id >= ID_ROW0 { return self.launch((id - ID_ROW0) as usize, right); }
        Re::Stay
    }

    fn key(&mut self, vk: u16) -> Option<Re> {
        let n = self.rows.len();
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        match vk {
            v if v == VK_DOWN.0 || (v == VK_TAB.0 && unsafe { GetKeyState(VK_SHIFT.0 as i32) } >= 0) => { if n > 0 { self.sel = (self.sel + 1) % n; } Some(Re::Stay) }
            v if v == VK_UP.0 || v == VK_TAB.0 => { if n > 0 { self.sel = (self.sel + n - 1) % n; } Some(Re::Stay) }
            v if v == VK_RETURN.0 => Some(if n > 0 { self.launch(self.sel, ctrl) } else { Re::Stay }),
            v if v == VK_ESCAPE.0 && !self.query.is_empty() => { self.query.clear(); self.all = false; self.changed(); Some(Re::Stay) }
            _ => None,
        }
    }

    fn chr(&mut self, c: char) -> bool {
        match c {
            '\u{8}' => { if self.all { self.query.clear(); } else { self.query.pop(); } }
            // Ctrl+Retroceso: borra la última palabra
            '\u{7f}' => {
                let t = self.query.trim_end().to_string();
                let cut = t.rfind(' ').map(|i| i + 1).unwrap_or(0);
                self.query.truncate(if self.all { 0 } else { cut });
            }
            '\u{1}' => { self.all = !self.query.is_empty(); return true; }
            '\u{16}' => {
                let Some(p) = read_clipboard() else { return false };
                let one: String = p.split_whitespace().collect::<Vec<_>>().join(" ");
                if self.all { self.query.clear(); }
                self.query.push_str(&one);
            }
            c if c.is_control() => return false,
            c => { if self.all { self.query.clear(); } self.query.push(c); }
        }
        if self.query.chars().count() > 120 { self.query = self.query.chars().take(120).collect(); }
        self.all = false;
        self.changed();
        true
    }
}
