//! Widget de noticias: titulares de uno o varios feeds RSS/Atom (por defecto, Google News en español de
//! España). Las fuentes van en widgets.ini (`noticias_fuentes = url1|url2`). Un hilo los pide cada 15 min
//! mientras el widget se ve y los guarda en `%APPDATA%\macbar\noticias_cache.txt`, para arrancar con
//! datos aunque no haya red. Los titulares pasan por páginas con un deslizamiento suave; un clic abre la
//! noticia en el navegador.

use super::*;
use desk::{size, DFonts, Kind};
use deskw::{header, Widget};
use popup::Ui;
use std::sync::atomic::{AtomicBool, AtomicI64};

pub(crate) const DEFAULT_FEED: &str = "https://news.google.com/rss?hl=es&gl=ES&ceid=ES:es";
const KEY: &str = "noticias_fuentes";
/// Cada cuánto se piden (s) y, si falló, cada cuánto se reintenta.
const REFRESH: i64 = 15 * 60;
const RETRY: i64 = 2 * 60;
/// Titulares que se guardan (los más recientes de todas las fuentes).
const KEEP: usize = 16;

const PAD: f32 = 20.0;
const LIST_TOP: f32 = 54.0;
const ROW: f32 = 64.0;
const ROWS: usize = 4;
/// Segundos que se queda cada página y lo que dura el paso a la siguiente.
const TURN_S: f32 = 11.0;
const SLIDE_S: f32 = 0.6;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item { pub title: String, pub source: String, pub link: String, pub ts: i64 }

// ================================================================ utilidades comunes (también de la bolsa)
pub(crate) fn now() -> i64 { deskdata::now_unix() as i64 }

/// Abre widgets.ini en el Bloc de notas; si falta la clave, la escribe antes con un ejemplo.
pub(crate) fn edit_ini(key: &str, example: &str) {
    if desk::ini_value(key).is_none() { desk::set_ini_value(key, example); }
    let p = sys::config_dir() + "\\widgets.ini";
    sys::open_args("notepad.exe", &format!("\"{p}\""));
}

/// «ahora», «hace 12 min», «hace 3 h», «ayer», «hace 4 días».
pub(crate) fn fmt_ago(secs: i64) -> String {
    let d = secs.max(0);
    match d {
        0..=59 => i18n::t("ahora").into(),
        60..=3599 => i18n::tf("hace {} min", &[&(d / 60)]),
        3600..=86399 => i18n::tf("hace {} h", &[&(d / 3600)]),
        86400..=172799 => i18n::t("ayer").into(),
        _ => i18n::tf("hace {} días", &[&(d / 86400)]),
    }
}

/// Texto recortado con «…» para que quepa en `maxw`.
pub(crate) fn fit(ui: &Ui, f: &IDWriteTextFormat, s: &str, maxw: f32) -> String {
    if ui.p.width(f, s) <= maxw { return s.to_string(); }
    let mut cs: Vec<char> = s.chars().collect();
    while !cs.is_empty() {
        cs.pop();
        let t = cs.iter().collect::<String>().trim_end().to_string() + "…";
        if ui.p.width(f, &t) <= maxw { return t; }
    }
    "…".into()
}

/// Avance suave hacia `target`. true mientras se mueve.
pub(crate) fn approach(x: &mut f32, target: f32, dt: f32, tau: f32) -> bool {
    if (target - *x).abs() < 0.002 { *x = target; return false; }
    *x += (target - *x) * (1.0 - (-dt / tau).exp());
    true
}

pub(crate) fn ez(t: f32, delay: f32, dur: f32) -> f32 { popup::ease_out(((t - delay) / dur).clamp(0.0, 1.0)) }

/// Puntos de página (el de ahora es una píldora que se desliza). Zonas `10 + i`.
pub(crate) fn page_dots(ui: &mut Ui, n: usize, dot: f32, cx: f32, cy: f32) {
    if n < 2 { return; }
    let gap = 12.0;
    let x_of = |i: f32| cx - (n as f32 - 1.0) * gap / 2.0 + i * gap;
    unsafe {
        for i in 0..n {
            let id = 10 + i as u32;
            let x = x_of(i as f32);
            ui.rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: x, Y: cy }, radiusX: 2.6, radiusY: 2.6 }, &ui.wa(0.28 + 0.3 * ui.heat(id)));
            ui.hit(rect(x - gap / 2.0, cy - 9.0, x + gap / 2.0, cy + 9.0), id);
        }
        let px = x_of(dot);
        ui.rt.FillRoundedRectangle(&rrect(rect(px - 6.5, cy - 2.6, px + 6.5, cy + 2.6), 2.6), &ui.wa(0.9));
    }
}

// ================================================================ fechas
/// Segundos desde 1970 de una fecha y hora en UTC.
fn unix_of(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Option<i64> {
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 { return None; }
    Some(deskw3::days_from_civil(y, mo, d) * 86400 + (h * 3600 + mi * 60 + s) as i64)
}

/// Desfase de zona («+0100», «-05:00», «GMT», «Z», «EST»…) en segundos.
fn zone_secs(z: &str) -> Option<i64> {
    let z = z.trim();
    match z.to_ascii_uppercase().as_str() {
        "" | "GMT" | "UT" | "UTC" | "Z" => return Some(0),
        "EST" => return Some(-5 * 3600), "EDT" => return Some(-4 * 3600),
        "CST" => return Some(-6 * 3600), "CDT" => return Some(-5 * 3600),
        "MST" => return Some(-7 * 3600), "MDT" => return Some(-6 * 3600),
        "PST" => return Some(-8 * 3600), "PDT" => return Some(-7 * 3600),
        "CET" => return Some(3600), "CEST" => return Some(7200),
        _ => {}
    }
    let sign = match z.as_bytes().first()? { b'+' => 1, b'-' => -1, _ => return None };
    let digits: String = z[1..].chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 4 { return None; }
    let (h, m): (i64, i64) = (digits[..2].parse().ok()?, digits[2..].parse().ok()?);
    Some(sign * (h * 3600 + m * 60))
}

/// Fecha de RSS (RFC 822): «Sun, 22 Nov 2026 04:00:06 GMT», «22 Nov 26 10:00 +0100».
pub(crate) fn parse_rfc822(s: &str) -> Option<i64> {
    let s = s.trim();
    let s = s.split_once(',').map(|(_, r)| r).unwrap_or(s);
    let mut it = s.split_whitespace();
    let d: u32 = it.next()?.parse().ok()?;
    let mon = it.next()?.to_ascii_lowercase();
    const EN: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    const ES: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sep", "oct", "nov", "dic"];
    let m3: String = mon.chars().take(3).collect();
    let mo = EN.iter().position(|m| *m == m3).or_else(|| ES.iter().position(|m| *m == m3))? as u32 + 1;
    let y: i32 = it.next()?.parse().ok()?;
    let y = if y < 100 { y + if y < 70 { 2000 } else { 1900 } } else { y };
    let hms: Vec<u32> = it.next()?.split(':').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let (h, mi, sec) = match hms.as_slice() { [h, m] => (*h, *m, 0), [h, m, s] => (*h, *m, *s), _ => return None };
    let off = zone_secs(it.next().unwrap_or(""))?;
    Some(unix_of(y, mo, d, h, mi, sec)? - off)
}

/// Fecha de Atom (ISO 8601): «2026-11-22T10:00:00Z», «2026-11-22T10:00:00.5+01:00».
pub(crate) fn parse_iso8601(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 || !s.is_char_boundary(19) { return None; }
    let (dt, zone) = s.split_at(19);
    let b = dt.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b' ') || b[13] != b':' || b[16] != b':' { return None; }
    let n = |a: usize, z: usize| dt[a..z].parse::<u32>().ok();
    let zone = zone.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let off = zone_secs(zone)?;
    Some(unix_of(n(0, 4)? as i32, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?, n(17, 19)?)? - off)
}

// ================================================================ XML mínimo (lo justo para RSS y Atom)
/// Entidades de XML y HTML más comunes (`&amp;`, `&#39;`, `&#x27;`, `&nbsp;`…).
pub(crate) fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        o.push_str(&rest[..i]);
        rest = &rest[i..];
        // Las entidades son cortas: el «;» se busca en los primeros 12 bytes, cortando en un límite de
        // carácter (el texto viene de fuera: «&ñ…» o un emoji no pueden partir un carácter por la mitad)
        let mut lim = rest.len().min(12);
        while !rest.is_char_boundary(lim) { lim -= 1; }
        let end = rest[..lim].find(';');
        let rep = end.and_then(|e| {
            let name = &rest[1..e];
            let c = match name {
                "amp" => Some('&'), "lt" => Some('<'), "gt" => Some('>'), "quot" => Some('"'), "apos" => Some('\''), "nbsp" => Some(' '),
                _ if name.starts_with("#x") || name.starts_with("#X") => u32::from_str_radix(&name[2..], 16).ok().and_then(char::from_u32),
                _ if name.starts_with('#') => name[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            }?;
            Some((c, e + 1))
        });
        match rep {
            Some((c, n)) => { o.push(c); rest = &rest[n..]; }
            None => { o.push('&'); rest = &rest[1..]; }
        }
    }
    o.push_str(rest);
    o
}

/// Quita etiquetas (`<b>…</b>`) y junta los espacios.
fn strip_tags(s: &str) -> String {
    let mut o = String::new();
    let mut inside = false;
    for c in s.chars() {
        match c { '<' => inside = true, '>' if inside => { inside = false; o.push(' '); } _ if !inside => o.push(c), _ => {} }
    }
    o.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Posición de `<name` como etiqueta completa (no `<namefoo`).
fn find_open(s: &str, name: &str) -> Option<(usize, usize)> {
    let pat = format!("<{name}");
    let mut from = 0;
    while let Some(i) = s[from..].find(&pat) {
        let at = from + i;
        let after = at + pat.len();
        match s[after..].chars().next() {
            Some(c) if c == '>' || c == '/' || c.is_whitespace() => return Some((at, after)),
            _ => from = after,
        }
    }
    None
}

/// Atributo de la etiqueta `tag` (texto entre `<` y `>`).
fn attr(tag: &str, name: &str) -> Option<String> {
    for q in ['"', '\''] {
        let pat = format!("{name}={q}");
        let mut from = 0;
        while let Some(i) = tag[from..].find(&pat) {
            let at = from + i;
            // Que sea el atributo entero (no `xhref=`)
            if at == 0 || tag[..at].ends_with(char::is_whitespace) {
                let v = &tag[at + pat.len()..];
                return Some(unescape(&v[..v.find(q)?]));
            }
            from = at + pat.len();
        }
    }
    None
}

/// Etiqueta `name` dentro de `s`: (atributos, contenido o None si se cierra sola).
fn element<'a>(s: &'a str, name: &str) -> Option<(&'a str, Option<&'a str>)> {
    let (at, after) = find_open(s, name)?;
    let gt = after + s[after..].find('>')?;
    let tag = &s[at + 1..gt];
    if tag.ends_with('/') { return Some((tag, None)); }
    let close = format!("</{name}>");
    let end = gt + 1 + s[gt + 1..].find(&close)?;
    Some((tag, Some(&s[gt + 1..end])))
}

/// Texto de un elemento: CDATA, entidades y etiquetas fuera, espacios juntados.
fn text_of(s: &str, name: &str) -> Option<String> {
    let (_, inner) = element(s, name)?;
    let inner = inner?.trim();
    let raw = match inner.strip_prefix("<![CDATA[") { Some(r) => r.split("]]>").next().unwrap_or("").to_string(), None => unescape(inner) };
    let t = strip_tags(&raw);
    (!t.is_empty()).then_some(t)
}

/// Bloques `<item>…</item>` (RSS) o `<entry>…</entry>` (Atom).
fn blocks<'a>(s: &'a str, name: &str) -> Vec<&'a str> {
    let mut v = Vec::new();
    let close = format!("</{name}>");
    let mut rest = s;
    while let Some((at, _)) = find_open(rest, name) {
        let Some(e) = rest[at..].find(&close) else { break };
        v.push(&rest[at..at + e]);
        rest = &rest[at + e + close.len()..];
    }
    v
}

/// Bytes del feed → texto (UTF-8 o, si el feed lo declara, Latin-1 / Windows-1252).
pub(crate) fn decode_text(b: &[u8]) -> String {
    let b = b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b);
    if let Ok(s) = std::str::from_utf8(b) { return s.to_string(); }
    let head = String::from_utf8_lossy(&b[..b.len().min(200)]).to_ascii_lowercase();
    if head.contains("iso-8859-1") || head.contains("windows-1252") || head.contains("latin1") { return b.iter().map(|&c| c as char).collect(); }
    String::from_utf8_lossy(b).into_owned()
}

/// Titulares de un feed RSS 2.0 o Atom, en el orden del feed.
pub(crate) fn parse_feed(xml: &str) -> Vec<Item> {
    let atom = find_open(xml, "feed").is_some() && find_open(xml, "item").is_none();
    let first = if atom { find_open(xml, "entry") } else { find_open(xml, "item") }.map(|p| p.0).unwrap_or(xml.len());
    // Nombre de la fuente: el título del canal (lo que hay antes del primer titular)
    let channel = text_of(&xml[..first], "title").unwrap_or_default();
    let mut out = Vec::new();
    for b in blocks(xml, if atom { "entry" } else { "item" }) {
        let Some(mut title) = text_of(b, "title") else { continue };
        let link = if atom {
            // Atom: <link href="…"/>, mejor el «alternate» (o el que no dice rel)
            let mut best = None;
            let mut rest = b;
            while let Some((at, _)) = find_open(rest, "link") {
                let gt = rest[at..].find('>').map(|g| at + g).unwrap_or(rest.len());
                let tag = &rest[at..gt];
                if let Some(h) = attr(tag, "href") {
                    let rel = attr(tag, "rel");
                    if rel.as_deref().is_none_or(|r| r == "alternate") { best = Some(h); break; }
                    best.get_or_insert(h);
                }
                rest = &rest[gt.min(rest.len())..];
            }
            best.unwrap_or_default()
        } else {
            text_of(b, "link").or_else(|| text_of(b, "guid").filter(|g| g.starts_with("http"))).unwrap_or_default()
        };
        let source = text_of(b, "source").unwrap_or_else(|| channel.clone());
        // Google News pone la fuente al final del título («… - ABC»)
        if !source.is_empty() {
            if let Some(t) = title.strip_suffix(&format!(" - {source}")) { title = t.trim_end().to_string(); }
        }
        let ts = ["pubDate", "dc:date", "published", "updated"].iter().find_map(|k| {
            let v = text_of(b, k)?;
            parse_rfc822(&v).or_else(|| parse_iso8601(&v))
        }).unwrap_or(0);
        out.push(Item { title, source, link: link.trim().to_string(), ts });
    }
    out
}

/// Junta los titulares de varias fuentes: sin repetidos, los más recientes primero.
pub(crate) fn merge(lists: Vec<Vec<Item>>, keep: usize) -> Vec<Item> {
    let mut all: Vec<Item> = Vec::new();
    for l in lists {
        for it in l.into_iter().take(30) {
            let k = it.title.to_lowercase();
            if !all.iter().any(|a| a.title.to_lowercase() == k) { all.push(it); }
        }
    }
    // Los que no traen fecha, al final (en el orden en que llegaron)
    all.sort_by_key(|i| if i.ts > 0 { -i.ts } else { i64::MAX });
    all.truncate(keep);
    all
}

/// Fuentes de widgets.ini (`noticias_fuentes = url1|url2`); sin la clave, Google News.
pub(crate) fn sources_from(v: Option<&str>) -> Vec<String> {
    let l: Vec<String> = v.unwrap_or("").split('|').map(|s| s.trim().to_string()).filter(|s| s.starts_with("https://") || s.starts_with("http://")).take(8).collect();
    if l.is_empty() { vec![DEFAULT_FEED.to_string()] } else { l }
}
fn sources() -> Vec<String> { sources_from(desk::ini_value(KEY).as_deref()) }

/// Abre widgets.ini para cambiar las fuentes (con Google News de ejemplo si aún no hay ninguna).
pub(crate) fn edit_sources() { edit_ini(KEY, DEFAULT_FEED); }

// ================================================================ caché en disco
fn cache_path() -> String { sys::config_dir() + "\\noticias_cache.txt" }

fn clean(s: &str) -> String { s.replace(['\t', '\r', '\n'], " ") }

/// Formato: «v1\t<pedido>\t<fuentes>» y una línea por titular «ts\tfuente\ttítulo\tenlace».
pub(crate) fn cache_to_text(fetched: i64, srcs: &str, items: &[Item]) -> String {
    let mut s = format!("v1\t{fetched}\t{}\n", clean(srcs));
    for i in items { s += &format!("{}\t{}\t{}\t{}\n", i.ts, clean(&i.source), clean(&i.title), clean(&i.link)); }
    s
}

pub(crate) fn cache_from_text(s: &str) -> Option<(i64, String, Vec<Item>)> {
    let mut lines = s.lines();
    let mut h = lines.next()?.split('\t');
    if h.next()? != "v1" { return None; }
    let fetched = h.next()?.parse().ok()?;
    let srcs = h.next().unwrap_or("").to_string();
    let items = lines.filter_map(|l| {
        let p: Vec<&str> = l.split('\t').collect();
        (p.len() == 4 && !p[2].is_empty()).then(|| Item { ts: p[0].parse().unwrap_or(0), source: p[1].into(), title: p[2].into(), link: p[3].into() })
    }).collect();
    Some((fetched, srcs, items))
}

// ================================================================ hilo de datos
struct Shared { items: Vec<Item>, fetched: i64, offline: bool, ver: u64 }
static SHARED: Mutex<Shared> = Mutex::new(Shared { items: Vec::new(), fetched: 0, offline: false, ver: 0 });
/// Última vez que el widget se pintó o latió (el hilo solo trabaja si se ve).
static SEEN: AtomicI64 = AtomicI64::new(0);
static FORCE: AtomicBool = AtomicBool::new(false);
static STARTED: std::sync::Once = std::sync::Once::new();

/// Titulares de muestra para el modo demo (inventados, sin red).
fn demo_items(now: i64) -> Vec<Item> {
    let l: [(&str, &str, i64); 12] = [
        ("Diario de Ejemplo", "La ciudad estrena un carril bici de 12 kilómetros que une el centro con el campus", 4),
        ("Crónica Demo", "Ejemplo: el observatorio publica la imagen más nítida hasta la fecha de una galaxia espiral", 12),
        ("Noticias de Muestra", "Una startup de ejemplo presenta una batería que se carga en diez minutos", 26),
        ("Gaceta Ficticia", "El festival de cine de muestra anuncia su programación con 40 estrenos", 38),
        ("Diario de Ejemplo", "Previsión de ejemplo: fin de semana soleado con temperaturas suaves en casi todo el país", 55),
        ("Crónica Demo", "Ejemplo: la biblioteca municipal amplía su horario y abrirá también los domingos", 74),
        ("Noticias de Muestra", "Un equipo de ejemplo gana la liga juvenil tras una remontada en el último minuto", 96),
        ("Gaceta Ficticia", "Ejemplo: abre un mercado de productores locales en el antiguo edificio de correos", 130),
        ("Diario de Ejemplo", "La orquesta de muestra celebra su aniversario con un concierto gratuito al aire libre", 170),
        ("Crónica Demo", "Ejemplo: nuevas rutas de senderismo señalizadas en la sierra para el otoño", 210),
        ("Noticias de Muestra", "Un museo de ejemplo digitaliza 20.000 piezas de su colección", 260),
        ("Gaceta Ficticia", "Ejemplo: el tren de cercanías suma frecuencias en hora punta desde el lunes", 320),
    ];
    l.iter().map(|(s, t, m)| Item { title: t.to_string(), source: s.to_string(), link: "https://example.com/".into(), ts: now - m * 60 }).collect()
}

fn ensure_started() {
    STARTED.call_once(|| {
        if sys::demo() {
            let mut sh = SHARED.lock().unwrap();
            sh.items = demo_items(now());
            sh.fetched = now() - 3 * 60;
            sh.ver += 1;
            return;
        }
        let cached = std::fs::read_to_string(cache_path()).ok().and_then(|s| cache_from_text(&s));
        let mut last_try = 0;
        let mut last_srcs = String::new();
        if let Some((f, srcs, items)) = cached {
            let mut sh = SHARED.lock().unwrap();
            sh.items = items;
            sh.fetched = f;
            sh.ver += 1;
            // Recientes: no hace falta pedirlos nada más arrancar
            last_try = f;
            last_srcs = srcs;
        }
        std::thread::spawn(move || {
            let mut last_try = last_try;
            let mut last_srcs = last_srcs;
            loop {
                let t = now();
                let srcs = sources();
                let joined = srcs.join("|");
                let force = FORCE.swap(false, Ordering::Relaxed);
                let alive = t - SEEN.load(Ordering::Relaxed) < 90 && deskdata::VISIBLE.load(Ordering::Relaxed);
                let offline = SHARED.lock().unwrap().offline;
                let wait = if offline { RETRY } else { REFRESH };
                if alive && (force || joined != last_srcs || t - last_try >= wait) {
                    last_try = t;
                    let lists: Vec<Vec<Item>> = srcs.iter().filter_map(|u| net::get(u)).map(|b| parse_feed(&decode_text(&b))).filter(|l| !l.is_empty()).collect();
                    let mut sh = SHARED.lock().unwrap();
                    if lists.is_empty() {
                        sh.offline = true;
                    } else {
                        last_srcs = joined.clone();
                        sh.items = merge(lists, KEEP);
                        sh.fetched = t;
                        sh.offline = false;
                        let _ = std::fs::create_dir_all(sys::config_dir());
                        let _ = std::fs::write(cache_path(), cache_to_text(t, &joined, &sh.items));
                    }
                    sh.ver += 1;
                }
                std::thread::sleep(Duration::from_secs(10));
            }
        });
    });
}

// ================================================================ widget
pub struct NewsW {
    ver: u64, items: Vec<Item>, fetched: i64, offline: bool,
    page: usize,
    /// Página que se va (y cuándo empezó el paso)
    prev: Option<(usize, Instant)>,
    turned: Instant,
    /// Titulares ya partidos en dos líneas (se mide una vez)
    wraps: HashMap<String, (String, String)>,
    dot: f32, hovering: bool, moving: bool, minute: i64,
}

impl Default for NewsW {
    fn default() -> Self {
        ensure_started();
        SEEN.store(now(), Ordering::Relaxed);
        NewsW { ver: u64::MAX, items: Vec::new(), fetched: 0, offline: false, page: 0, prev: None, turned: Instant::now(), wraps: HashMap::new(), dot: 0.0, hovering: false, moving: false, minute: 0 }
    }
}

/// Parte un titular en dos líneas que quepan en `maxw` (la segunda, con «…» si sobra).
fn wrap2(ui: &Ui, f: &IDWriteTextFormat, s: &str, maxw: f32) -> (String, String) {
    let words: Vec<&str> = s.split_whitespace().collect();
    let mut l1 = String::new();
    let mut i = 0;
    while i < words.len() {
        let cand = if l1.is_empty() { words[i].to_string() } else { format!("{l1} {}", words[i]) };
        if ui.p.width(f, &cand) > maxw { break; }
        l1 = cand;
        i += 1;
    }
    if l1.is_empty() { return (fit(ui, f, s, maxw), String::new()); }
    let rest = words[i..].join(" ");
    let l2 = if rest.is_empty() { String::new() } else { fit(ui, f, &rest, maxw) };
    (l1, l2)
}

impl NewsW {
    /// Recoge lo que trajo el hilo. true si cambió.
    fn sync(&mut self) -> bool {
        let sh = SHARED.lock().unwrap();
        if sh.ver == self.ver { return false; }
        self.ver = sh.ver;
        if sh.items != self.items { self.wraps.clear(); self.page = 0; self.prev = None; self.dot = 0.0; }
        self.items = sh.items.clone();
        self.fetched = sh.fetched;
        self.offline = sh.offline;
        true
    }
    fn pages(&self) -> usize { self.items.len().div_ceil(ROWS).max(1) }
    fn turn(&mut self, to: usize) {
        let to = to % self.pages();
        if to == self.page { return; }
        self.prev = Some((self.page, Instant::now()));
        self.page = to;
        self.turned = Instant::now();
        self.moving = true;
    }

    /// Un titular: fuente y hace cuánto (en gris; un puntito del acento si es de la última media hora) y
    /// el título en dos líneas.
    fn row(&mut self, ui: &mut Ui, i: usize, y: f32, a: f32, w: f32, live: bool) {
        let Some(it) = self.items.get(i).cloned() else { return };
        let id = 100 + i as u32;
        if live {
            ui.hit(rect(PAD - 10.0, y - 6.0, w - PAD + 10.0, y + ROW - 8.0), id);
            ui.card_k(rect(PAD - 10.0, y - 6.0, w - PAD + 10.0, y + ROW - 8.0), ui.heat(id) * a);
        }
        if a < 0.01 { return; }
        popup::push_alpha(ui.rt, a);
        let ago = if it.ts > 0 { fmt_ago(now() - it.ts) } else { String::new() };
        let aw = if ago.is_empty() { 0.0 } else { ui.p.width(&ui.f.sub, &ago) + 12.0 };
        let fresh = it.ts > 0 && now() - it.ts < 30 * 60;
        let sx = if fresh { PAD + 9.0 } else { PAD };
        if fresh {
            unsafe { ui.rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: PAD + 2.5, Y: y + 7.5 }, radiusX: 2.5, radiusY: 2.5 }, &ui.brush(profiles::accent(1.0))); }
        }
        let src = fit(ui, &ui.f.small, &it.source, w - PAD - sx - aw);
        ui.text(&ui.f.small, &src, sx, y, 15.0, &ui.dim);
        if !ago.is_empty() { ui.text_r(&ui.f.sub, &ago, w - PAD, y, 15.0, &ui.dim); }
        let maxw = w - 2.0 * PAD;
        let (l1, l2) = match self.wraps.get(&it.title) {
            Some(v) => v.clone(),
            None => { let v = wrap2(ui, &ui.f.name, &it.title, maxw); self.wraps.insert(it.title.clone(), v.clone()); v }
        };
        ui.text(&ui.f.name, &l1, PAD, y + 18.0, 19.0, &ui.white);
        if !l2.is_empty() { ui.text(&ui.f.name, &l2, PAD, y + 36.0, 19.0, &ui.white); }
        unsafe { ui.rt.PopLayer(); }
    }
}

impl Widget for NewsW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let (w, h) = size(Kind::News);
        SEEN.store(now(), Ordering::Relaxed);
        self.sync();
        self.hovering = ui.hover.is_some_and(|id| id >= 100);
        let status = if self.offline {
            if self.fetched > 0 { format!("{} · {}", i18n::t("Sin conexión"), fmt_ago(now() - self.fetched)) } else { i18n::t("Sin conexión").to_string() }
        } else { String::new() };
        header(ui, d, i18n::t("Noticias"), &status, w);
        ui.hit(rect(0.0, 0.0, w, LIST_TOP - 8.0), 1);
        if self.items.is_empty() {
            let msg = if self.offline { i18n::t("Sin conexión") } else { i18n::t("Cargando titulares…") };
            ui.glyph(&d.glyph_l, "\u{E900}", w / 2.0 - 20.0, h / 2.0 - 30.0, 40.0, 40.0, &ui.faint);
            ui.text_c(&ui.f.sub, msg, w / 2.0, h / 2.0 + 14.0, 18.0, &ui.dim);
            self.moving = false;
            return;
        }
        let mut moving = false;
        let n = self.items.len();
        let bottom = LIST_TOP + ROWS as f32 * ROW - 6.0;
        unsafe { ui.rt.PushAxisAlignedClip(&rect(0.0, LIST_TOP - 10.0, w, bottom), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE); }
        // Paso de página: los titulares viejos suben y se apagan; los nuevos llegan desde abajo, en cascada
        let e = self.prev.map(|(_, at)| at.elapsed().as_secs_f32() / SLIDE_S);
        if e.is_some_and(|e| e >= 1.0 + 0.06 * ROWS as f32) { self.prev = None; }
        let stag = |e: f32, r: usize| popup::ease_out((e - 0.06 * r as f32).clamp(0.0, 1.0));
        if let (Some((old, _)), Some(e)) = (self.prev, e) {
            for r in 0..ROWS {
                let k = stag(e * 1.1, r);
                let i = old * ROWS + r;
                if i < n { self.row(ui, i, LIST_TOP + r as f32 * ROW - 22.0 * k, 1.0 - k, w, false); }
            }
            moving = true;
        }
        for r in 0..ROWS {
            let i = self.page * ROWS + r;
            if i >= n { break; }
            // Al aparecer el widget, también en cascada
            let k = match e { Some(e) => stag(e * 0.95 - 0.12, r), None => ez(t, 0.08 + 0.07 * r as f32, 0.5) };
            let y = LIST_TOP + r as f32 * ROW + 18.0 * (1.0 - k);
            self.row(ui, i, y, k, w, k > 0.5);
            // Línea fina entre titulares
            if r + 1 < ROWS && i + 1 < n {
                let ly = LIST_TOP + (r + 1) as f32 * ROW - 7.5;
                unsafe { ui.rt.FillRectangle(&rect(PAD, ly, w - PAD, ly + 1.0 / ui.s), &ui.wa(0.08 * k)); }
            }
        }
        unsafe { ui.rt.PopAxisAlignedClip(); }
        moving |= approach(&mut self.dot, self.page as f32, dt, 0.09);
        page_dots(ui, self.pages(), self.dot, w / 2.0, h - 16.0);
        self.moving = moving || t < 1.2;
    }
    fn click(&mut self, id: Option<u32>, right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        let id = id?;
        if right {
            // Clic derecho en la cabecera: cambiar las fuentes (en lo demás, el menú de siempre)
            return (id == 1).then(|| Box::new(edit_sources) as Box<dyn FnOnce()>);
        }
        match id {
            i if i >= 100 => {
                let link = self.items.get((i - 100) as usize)?.link.clone();
                // Solo enlaces web (lo que trae el feed no se ejecuta como programa)
                if link.starts_with("https://") || link.starts_with("http://") { return Some(Box::new(move || sys::open(&link))); }
            }
            i if i >= 10 => self.turn((i - 10) as usize),
            1 => FORCE.store(true, Ordering::Relaxed),
            _ => {}
        }
        None
    }
    fn wheel(&mut self, d: i32) -> bool {
        let n = self.pages();
        if n < 2 { return false; }
        self.turn(if d > 0 { self.page + n - 1 } else { self.page + 1 });
        true
    }
    fn tick(&mut self) -> bool {
        SEEN.store(now(), Ordering::Relaxed);
        let mut r = self.sync();
        // Pasa de página sola (no mientras el ratón está encima de un titular)
        if self.hovering { self.turned = Instant::now(); }
        if self.pages() > 1 && self.turned.elapsed().as_secs_f32() >= TURN_S { self.turn(self.page + 1); r = true; }
        // «hace N min» al día
        let m = now() / 60;
        if m != self.minute { self.minute = m; r = true; }
        r
    }
    fn animating(&self, t: f32) -> bool { t < 1.3 || self.moving }
    fn smooth(&self) -> bool { self.moving }
    fn frame_ms(&self) -> u32 { 16 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?><rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/"><channel><title>Noticias destacadas - Google News</title><link>https://news.google.com/</link><image><title>Google News</title></image>
<item><title>Encuestas en Madrid: resultados de PSOE, PP y Vox - ABC</title><link>https://news.google.com/rss/articles/AAA?oc=5</link><guid isPermaLink="false">AAA</guid><pubDate>Sun, 22 Nov 2026 04:00:06 GMT</pubDate><description>&lt;ol&gt;&lt;li&gt;x&lt;/li&gt;&lt;/ol&gt;</description><source url="https://www.abc.es">ABC</source></item>
<item><title><![CDATA[Tom & Jerry vuelven <b>al cine</b>]]></title><link>https://ejemplo.org/a?x=1&amp;y=2</link><pubDate>Sun, 22 Nov 2026 06:30:00 +0100</pubDate></item>
<item><title>Sin fecha &#8220;citada&#8221; y &#x27;apóstrofo&#x27;</title><guid>https://ejemplo.org/guid</guid></item>
<item><title></title><link>https://ejemplo.org/vacio</link></item>
</channel></rss>"#;

    #[test]
    fn rss_de_google_news() {
        let v = parse_feed(RSS);
        assert_eq!(v.len(), 3, "el de título vacío se descarta");
        assert_eq!(v[0].title, "Encuestas en Madrid: resultados de PSOE, PP y Vox");
        assert_eq!(v[0].source, "ABC");
        assert_eq!(v[0].link, "https://news.google.com/rss/articles/AAA?oc=5");
        assert_eq!(v[0].ts, unix_of(2026, 11, 22, 4, 0, 6).unwrap());
        assert_eq!(v[1].title, "Tom & Jerry vuelven al cine");
        assert_eq!(v[1].source, "Noticias destacadas - Google News", "sin <source>, el título del canal");
        assert_eq!(v[1].link, "https://ejemplo.org/a?x=1&y=2");
        assert_eq!(v[1].ts, unix_of(2026, 11, 22, 5, 30, 0).unwrap());
        assert_eq!(v[2].title, "Sin fecha \u{201C}citada\u{201D} y 'apóstrofo'");
        assert_eq!(v[2].link, "https://ejemplo.org/guid");
        assert_eq!(v[2].ts, 0);
    }

    #[test]
    fn atom() {
        let x = r#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"><title type="text">Blog de ejemplo</title><link href="https://ejemplo.org/"/>
<entry><title>Primera entrada</title><link rel="self" href="https://ejemplo.org/self"/><link rel="alternate" type="text/html" href="https://ejemplo.org/1"/><updated>2026-11-22T10:00:00Z</updated></entry>
<entry><title type="html">Segunda &amp;amp; última</title><link href="https://ejemplo.org/2" /><published>2026-11-22T10:00:00.250+01:00</published></entry>
</feed>"#;
        let v = parse_feed(x);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].title.as_str(), v[0].link.as_str(), v[0].source.as_str()), ("Primera entrada", "https://ejemplo.org/1", "Blog de ejemplo"));
        assert_eq!(v[0].ts, unix_of(2026, 11, 22, 10, 0, 0).unwrap());
        assert_eq!(v[1].title, "Segunda &amp; última");
        assert_eq!(v[1].link, "https://ejemplo.org/2");
        assert_eq!(v[1].ts, unix_of(2026, 11, 22, 9, 0, 0).unwrap());
    }

    #[test]
    fn fechas() {
        assert_eq!(parse_rfc822("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parse_rfc822("Sun, 22 Nov 2026 16:47:27 GMT"), Some(1795366047));
        assert_eq!(parse_rfc822("22 Nov 26 17:47 +0100"), Some(1795366020));
        assert_eq!(parse_rfc822("dom, 22 nov 2026 11:47:27 -05:00"), Some(1795366047));
        assert_eq!(parse_rfc822("Sun, 22 Nov 2026 11:47:27 EST"), Some(1795366047));
        assert_eq!(parse_rfc822("lo que sea"), None);
        assert_eq!(parse_rfc822("Sun, 32 Nov 2026 10:00:00 GMT"), None);
        assert_eq!(parse_iso8601("2026-11-22T16:47:27Z"), Some(1795366047));
        assert_eq!(parse_iso8601("2026-11-22T17:47:27+01:00"), Some(1795366047));
        assert_eq!(parse_iso8601("2026-11-22 16:47:27"), Some(1795366047));
        assert_eq!(parse_iso8601("2026-11-22"), None);
        // Texto ajeno con caracteres de varios bytes: None, nunca un pánico
        assert_eq!(parse_iso8601("2026-ñ1-22T16:47:27Z"), None);
        assert_eq!(parse_iso8601("😀😀😀😀😀😀😀😀"), None);
        assert_eq!(parse_rfc822("Sun, 22 Nov 2026 16:47:27 +ñ1😀"), None);
        assert_eq!(parse_rfc822("😀, ñ ñ ñ"), None);
    }

    #[test]
    fn entidades_y_texto() {
        assert_eq!(unescape("a &amp; b &lt;c&gt; &quot;d&quot; &#241; &#xF1; &nbsp;fin"), "a & b <c> \"d\" ñ ñ  fin");
        assert_eq!(unescape("suelto & sin cerrar &amp"), "suelto & sin cerrar &amp");
        assert_eq!(unescape("&desconocida; ok"), "&desconocida; ok");
        // Caracteres de varios bytes justo detrás de «&» (el corte de 12 bytes no debe partir ninguno)
        assert_eq!(unescape("&ñññññññññ fin"), "&ñññññññññ fin");
        assert_eq!(unescape("a&ñ;b"), "a&ñ;b");
        assert_eq!(unescape("&é&ó&ü&"), "&é&ó&ü&");
        assert_eq!(unescape("&😀😀😀😀 y &amp; 🚀&#128640;"), "&😀😀😀😀 y & 🚀🚀");
        for pre in ["", "x", "xx", "xxx"] {
            let s = format!("{pre}&ab😀😀😀ñ");
            assert_eq!(unescape(&s), s);
        }
        // Un titular entero con todo eso pasa por el parser sin problema
        let v = parse_feed("<rss><channel><title>Canal 📰</title><item><title>Señal &ñ 😀 &amp; más…</title><link>https://a.es/ñ</link></item></channel></rss>");
        assert_eq!(v[0].title, "Señal &ñ 😀 & más…");
        assert_eq!(v[0].source, "Canal 📰");
        assert_eq!(decode_text(b"\xEF\xBB\xBFhola"), "hola");
        assert_eq!(decode_text(b"<?xml encoding=\"ISO-8859-1\"?>a\xF1o"), "<?xml encoding=\"ISO-8859-1\"?>año");
    }

    #[test]
    fn mezcla_y_fuentes() {
        let it = |t: &str, ts: i64| Item { title: t.into(), source: "x".into(), link: String::new(), ts };
        let m = merge(vec![vec![it("A", 10), it("B", 0), it("C", 30)], vec![it("a", 50), it("D", 20)]], 3);
        assert_eq!(m.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(), ["C", "D", "A"]);
        assert_eq!(sources_from(None), vec![DEFAULT_FEED.to_string()]);
        assert_eq!(sources_from(Some(" https://a.es/rss | nada | http://b.es/feed ")), vec!["https://a.es/rss".to_string(), "http://b.es/feed".to_string()]);
        assert_eq!(sources_from(Some("javascript:alert(1)")), vec![DEFAULT_FEED.to_string()]);
    }

    #[test]
    fn cache_de_ida_y_vuelta() {
        let items = vec![Item { title: "Uno\tcon tab".into(), source: "ABC".into(), link: "https://a/1".into(), ts: 5 }, Item { title: "Dos".into(), source: "".into(), link: "".into(), ts: 0 }];
        let s = cache_to_text(99, "https://a|https://b", &items);
        let (f, srcs, back) = cache_from_text(&s).unwrap();
        assert_eq!((f, srcs.as_str()), (99, "https://a|https://b"));
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].title, "Uno con tab");
        assert_eq!(back[1], items[1]);
        assert!(cache_from_text("v0\t1\n").is_none());
    }

    #[test]
    fn hace_cuanto() {
        // (en español: los tests no cambian el idioma; `t` devuelve el texto de la tabla)
        if i18n::en() { return; }
        assert_eq!(fmt_ago(-5), "ahora");
        assert_eq!(fmt_ago(59), "ahora");
        assert_eq!(fmt_ago(12 * 60 + 30), "hace 12 min");
        assert_eq!(fmt_ago(3 * 3600 + 5), "hace 3 h");
        assert_eq!(fmt_ago(30 * 3600), "ayer");
        assert_eq!(fmt_ago(4 * 86400), "hace 4 días");
    }
}
