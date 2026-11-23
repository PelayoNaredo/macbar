//! Widget de noticias: titulares de temas de Google Noticias y de feeds RSS/Atom propios. Se configura en
//! la ventana de Configuración (sección Noticias), que lo guarda en widgets.ini:
//! - `noticias_temas = portada,economia,…` y `noticias_region = ES` (país e idioma de Google Noticias)
//! - `noticias_fuentes = url1|url2` (feeds propios) y `noticias_nombres = url Nombre|…` (su nombre)
//! - `noticias_silenciar = palabra, otra` y `noticias_ocultar = Medio|Otro` (filtros)
//! - `noticias_pasar = 0|8|11|20` (segundos por página; 0 = no pasa sola), `noticias_meta = 1|0` (fuente
//!   y hora) y `noticias_por_pagina = 2|3|4`
//!
//! Un hilo los pide cada 15 min mientras el widget se ve y los guarda en `%APPDATA%\macbar\noticias_cache.txt`,
//! para arrancar con datos aunque no haya red. Los filtros se aplican al enseñarlos (cambiarlos no pide
//! nada a la red). Los titulares pasan por páginas con un deslizamiento suave; un clic abre la noticia.

use super::*;
use desk::{size, DFonts, Kind};
use deskw::{header, Widget};
use popup::Ui;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64};

pub(crate) const DEFAULT_FEED: &str = "https://news.google.com/rss?hl=es&gl=ES&ceid=ES:es";
const KEY: &str = "noticias_fuentes";
const K_TOPICS: &str = "noticias_temas";
const K_REGION: &str = "noticias_region";
const K_NAMES: &str = "noticias_nombres";
const K_MUTE: &str = "noticias_silenciar";
const K_HIDE: &str = "noticias_ocultar";
const K_TURN: &str = "noticias_pasar";
const K_META: &str = "noticias_meta";
const K_PER_PAGE: &str = "noticias_por_pagina";
/// Cada cuánto se piden (s) y, si falló, cada cuánto se reintenta.
const REFRESH: i64 = 15 * 60;
const RETRY: i64 = 2 * 60;
/// Titulares que se guardan (los más recientes de todas las fuentes; los filtros quitan de aquí).
const KEEP: usize = 40;
/// Titulares que se enseñan como mucho (ya filtrados).
const SHOW: usize = 16;
/// Feeds propios como mucho.
pub(crate) const MAX_FEEDS: usize = 8;

const PAD: f32 = 20.0;
const LIST_TOP: f32 = 54.0;
const ROW: f32 = 64.0;
/// Titulares por página que caben en el widget (el máximo; Configuración puede poner menos).
pub(crate) const ROWS_MAX: usize = 4;
/// Segundos por página que se ofrecen (0 = no pasa sola).
pub(crate) const TURN_OPTS: [u32; 4] = [0, 8, 11, 20];
const SLIDE_S: f32 = 0.6;

// ================================================================ configuración
/// Temas de Google Noticias: (clave en widgets.ini, rótulo, tema de la URL; None = portada).
pub(crate) const TOPICS: [(&str, &str, Option<&str>); 9] = [
    ("portada", "Portada", None),
    ("nacional", "Nacional", Some("NATION")),
    ("mundo", "Mundo", Some("WORLD")),
    ("economia", "Economía", Some("BUSINESS")),
    ("tecnologia", "Tecnología", Some("TECHNOLOGY")),
    ("ciencia", "Ciencia", Some("SCIENCE")),
    ("salud", "Salud", Some("HEALTH")),
    ("deportes", "Deportes", Some("SPORTS")),
    ("entretenimiento", "Entretenimiento", Some("ENTERTAINMENT")),
];

/// País e idioma: (código, nombre, hl, gl, ceid) de Google Noticias.
pub(crate) const REGIONS: [(&str, &str, &str, &str, &str); 6] = [
    ("ES", "España", "es", "ES", "ES:es"),
    ("MX", "México", "es-419", "MX", "MX:es-419"),
    ("AR", "Argentina", "es-419", "AR", "AR:es-419"),
    ("CO", "Colombia", "es-419", "CO", "CO:es-419"),
    ("US", "EE. UU.", "en-US", "US", "US:en"),
    ("GB", "Reino Unido", "en-GB", "GB", "GB:en"),
];

/// URL del RSS de un tema (None = portada) para un país.
pub(crate) fn topic_url(topic: Option<&str>, region: &str) -> String {
    let r = REGIONS.iter().find(|r| r.0 == region).unwrap_or(&REGIONS[0]);
    let q = format!("hl={}&gl={}&ceid={}", r.2, r.3, r.4);
    match topic {
        None => format!("https://news.google.com/rss?{q}"),
        Some(t) => format!("https://news.google.com/rss/headlines/section/topic/{t}?{q}"),
    }
}

/// Lo que se elige en Configuración → Noticias.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Cfg {
    /// Claves de `TOPICS` marcadas, en el orden de `TOPICS`.
    pub topics: Vec<String>,
    pub region: String,
    /// Feeds propios y el nombre de cada uno (si se sabe).
    pub feeds: Vec<String>,
    pub names: Vec<(String, String)>,
    pub mute: Vec<String>,
    pub hide: Vec<String>,
    /// Segundos por página (0 = no pasa sola).
    pub turn: u32,
    pub meta: bool,
    pub per_page: usize,
}

impl Default for Cfg { fn default() -> Self { cfg_from(|_| None) } }

/// ¿Es una dirección web (lo único que se pide)?
/// Solo direcciones http(s) sin espacios, caracteres de control ni «|» (separador en widgets.ini).
pub(crate) fn is_web_url(s: &str) -> bool {
    (s.starts_with("https://") || s.starts_with("http://"))
        && !s.chars().any(|c| c.is_control() || c.is_whitespace() || c == '|')
        && net::split_url(s).is_some()
}

/// Configuración a partir de widgets.ini. Quien venía de antes (solo `noticias_fuentes`, que por defecto
/// era la portada de Google Noticias) conserva lo que veía.
pub(crate) fn cfg_from(get: impl Fn(&str) -> Option<String>) -> Cfg {
    let list = |k: &str, sep: char| -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for s in get(k).unwrap_or_default().split(sep) {
            let s = s.trim().to_string();
            if !s.is_empty() && !v.iter().any(|x| fold(x) == fold(&s)) { v.push(s); }
        }
        v
    };
    let mut feeds: Vec<String> = list(KEY, '|').into_iter().filter(|u| is_web_url(u)).take(MAX_FEEDS).collect();
    let topics: Vec<String> = match get(K_TOPICS) {
        Some(t) => { let t: Vec<&str> = t.split(',').map(str::trim).collect(); TOPICS.iter().filter(|x| t.contains(&x.0)).map(|x| x.0.to_string()).collect() }
        None => {
            // Antes, la portada de España iba como una fuente más (o era lo de por defecto)
            let had = feeds.iter().any(|u| u == DEFAULT_FEED) || get(KEY).is_none();
            feeds.retain(|u| u != DEFAULT_FEED);
            if had { vec!["portada".into()] } else { Vec::new() }
        }
    };
    let region = get(K_REGION).map(|r| r.trim().to_uppercase()).filter(|r| REGIONS.iter().any(|x| x.0 == r)).unwrap_or_else(|| "ES".into());
    let names = get(K_NAMES).unwrap_or_default().split('|').filter_map(|e| {
        let (u, n) = e.trim().split_once(' ')?;
        Some((u.to_string(), n.trim().to_string()))
    }).collect();
    Cfg {
        topics, region, feeds, names,
        mute: list(K_MUTE, ','),
        hide: list(K_HIDE, '|'),
        turn: get(K_TURN).and_then(|v| v.trim().parse().ok()).filter(|v| TURN_OPTS.contains(v)).unwrap_or(11),
        meta: get(K_META).is_none_or(|v| v.trim() != "0"),
        per_page: get(K_PER_PAGE).and_then(|v| v.trim().parse().ok()).filter(|v| (2..=ROWS_MAX).contains(v)).unwrap_or(ROWS_MAX),
    }
}

/// Pares clave/valor que se guardan en widgets.ini.
pub(crate) fn cfg_pairs(c: &Cfg) -> Vec<(&'static str, String)> {
    let clean = |s: &str, bad: &[char]| s.replace(bad, " ").trim().to_string();
    let names: Vec<String> = c.names.iter().filter(|n| c.feeds.contains(&n.0)).map(|(u, n)| format!("{u} {}", clean(n, &['|', '\n', '\r']))).collect();
    vec![
        (K_TOPICS, c.topics.join(",")),
        (K_REGION, c.region.clone()),
        (KEY, c.feeds.join("|")),
        (K_NAMES, names.join("|")),
        (K_MUTE, c.mute.iter().map(|m| clean(m, &[',', '\n', '\r'])).collect::<Vec<_>>().join(", ")),
        (K_HIDE, c.hide.iter().map(|m| clean(m, &['|', '\n', '\r'])).collect::<Vec<_>>().join("|")),
        (K_TURN, c.turn.to_string()),
        (K_META, if c.meta { "1" } else { "0" }.to_string()),
        (K_PER_PAGE, c.per_page.to_string()),
    ]
}

impl Cfg {
    /// Feeds que se piden: los temas marcados (en su país) y los propios. Sin ninguno, la portada.
    pub fn urls(&self) -> Vec<String> {
        let mut v: Vec<String> = TOPICS.iter().filter(|t| self.topics.iter().any(|k| k == t.0)).map(|t| topic_url(t.2, &self.region)).collect();
        v.extend(self.feeds.iter().cloned());
        if v.is_empty() { v.push(topic_url(None, &self.region)); }
        v
    }
    /// ¿Se enseña este titular? (ni palabras silenciadas en el título ni medios ocultos)
    pub fn allows(&self, it: &Item) -> bool {
        !self.mute.iter().any(|m| matches_term(&it.title, m)) && !self.hide.iter().any(|m| matches_term(&it.source, m))
    }
}

/// Texto para comparar: minúsculas y sin tildes ni diéresis («Económía» → «economia»).
pub(crate) fn fold(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).map(|c| match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a', 'é' | 'è' | 'ê' | 'ë' => 'e', 'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o', 'ú' | 'ù' | 'û' | 'ü' => 'u', 'ñ' => 'n', 'ç' => 'c', c => c,
    }).collect()
}

/// ¿Sale `term` en `text` empezando palabra? («rey» silencia «Rey» y «reyes», no «Monterrey»), sin
/// distinguir mayúsculas ni tildes. Vale también para frases («copa del rey»).
pub(crate) fn matches_term(text: &str, term: &str) -> bool {
    let (t, w) = (fold(text), fold(term.trim()));
    if w.is_empty() { return false; }
    let mut from = 0;
    while let Some(i) = t[from..].find(&w) {
        let at = from + i;
        if t[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric()) { return true; }
        from = at + t[at..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    false
}

static CFG: Mutex<Option<Cfg>> = Mutex::new(None);
static CFG_VER: AtomicU64 = AtomicU64::new(1);

/// Configuración vigente (leída de widgets.ini la primera vez).
pub(crate) fn cfg() -> Cfg { CFG.lock().unwrap().get_or_insert_with(|| cfg_from(desk::ini_value)).clone() }
pub(crate) fn cfg_ver() -> u64 { CFG_VER.load(Ordering::Relaxed) }

/// Guarda la configuración y la aplica al momento.
pub(crate) fn set_cfg(c: Cfg) {
    for (k, v) in cfg_pairs(&c) { desk::set_ini_value(k, &v); }
    *CFG.lock().unwrap() = Some(c);
    CFG_VER.fetch_add(1, Ordering::Relaxed);
    FORCE.store(true, Ordering::Relaxed);
}

fn reload_cfg() {
    let n = cfg_from(desk::ini_value);
    let mut g = CFG.lock().unwrap();
    if g.as_ref() != Some(&n) { *g = Some(n); CFG_VER.fetch_add(1, Ordering::Relaxed); }
}

/// Medios de los titulares que hay ahora (para ofrecer ocultarlos), sin repetir.
pub(crate) fn current_sources() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for it in SHARED.lock().unwrap().items.iter() {
        if !it.source.is_empty() && !v.iter().any(|s| fold(s) == fold(&it.source)) { v.push(it.source.clone()); }
    }
    v
}

/// Comprueba un feed: (titulares, nombre, URL del feed). Si es una página web que anuncia su feed
/// (`<link rel="alternate" type="application/rss+xml">`), se usa ese. El error, en una frase.
pub(crate) fn check_feed(url: &str) -> Result<(usize, String, String), String> {
    if sys::demo() { return Ok((12, "Feed de ejemplo".into(), url.to_string())); }
    let url = url.trim();
    if !is_web_url(url) { return Err(i18n::t("La dirección tiene que empezar por https://").into()); }
    let body = net::fetch(url).map_err(|e| e.message())?;
    let xml = decode_text(&body);
    let items = parse_feed(&xml);
    if !items.is_empty() { return Ok((items.len(), feed_title(&xml), url.to_string())); }
    if let Some(alt) = discover_feed(&xml, url) {
        let b = net::fetch(&alt).map_err(|e| e.message())?;
        let x = decode_text(&b);
        let items = parse_feed(&x);
        if !items.is_empty() { return Ok((items.len(), feed_title(&x), alt)); }
    }
    let lower = xml.to_ascii_lowercase();
    Err(if lower.contains("<rss") || lower.contains("<feed") { i18n::t("El feed no tiene titulares") } else { i18n::t("No es un feed RSS ni Atom") }.into())
}

/// Feed que anuncia una página HTML (absoluto o relativo a su servidor).
pub(crate) fn discover_feed(html: &str, base: &str) -> Option<String> {
    let mut rest = html;
    while let Some((at, _)) = find_open(rest, "link") {
        let gt = rest[at..].find('>').map(|g| at + g).unwrap_or(rest.len());
        let tag = &rest[at..gt];
        let ty = attr(tag, "type").unwrap_or_default().to_ascii_lowercase();
        if attr(tag, "rel").is_some_and(|r| r.to_ascii_lowercase().contains("alternate")) && (ty.contains("rss") || ty.contains("atom")) {
            let h = attr(tag, "href")?;
            if is_web_url(&h) { return Some(h); }
            let (sec, host, port, _) = net::split_url(base)?;
            let origin = format!("{}://{host}{}", if sec { "https" } else { "http" }, if port == 443 || port == 80 { String::new() } else { format!(":{port}") });
            if let Some(p) = h.strip_prefix("//") { return Some(format!("{}://{p}", if sec { "https" } else { "http" })); }
            if h.starts_with('/') { return Some(origin + &h); }
            let dir = base.rsplit_once('/').map(|(d, _)| d).filter(|d| d.len() > origin.len()).unwrap_or(&origin);
            return Some(format!("{dir}/{h}"));
        }
        rest = &rest[gt.min(rest.len())..];
        if rest.is_empty() { break; }
    }
    None
}

/// Nombre del feed (el título del canal).
pub(crate) fn feed_title(xml: &str) -> String {
    let atom = find_open(xml, "feed").is_some() && find_open(xml, "item").is_none();
    let first = if atom { find_open(xml, "entry") } else { find_open(xml, "item") }.map(|p| p.0).unwrap_or(xml.len());
    text_of(&xml[..first], "title").unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item { pub title: String, pub source: String, pub link: String, pub ts: i64 }

// ================================================================ utilidades comunes (también de la bolsa)
pub(crate) fn now() -> i64 { deskdata::now_unix() as i64 }


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

/// Abre la Configuración en la sección de las noticias (clic derecho en la cabecera del widget).
pub(crate) fn edit_sources() { settings::open_at_news(); }

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

/// Arranca los datos (caché o demo) sin esperar a que se pinte el widget: Configuración enseña los
/// nombres y medios que ya se conocen.
pub(crate) fn warm() { ensure_started(); }

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
            let mut n = 0u32;
            loop {
                // Cada 10 s se mira si widgets.ini cambió a mano (lo de Configuración llega al momento)
                if n % 10 == 0 { reload_cfg(); }
                n = n.wrapping_add(1);
                let t = now();
                let srcs = cfg().urls();
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
                std::thread::sleep(Duration::from_secs(1));
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
    cfg: Cfg, cfg_ver: u64,
}

impl Default for NewsW {
    fn default() -> Self {
        ensure_started();
        SEEN.store(now(), Ordering::Relaxed);
        NewsW { ver: u64::MAX, items: Vec::new(), fetched: 0, offline: false, page: 0, prev: None, turned: Instant::now(), wraps: HashMap::new(), dot: 0.0, hovering: false, moving: false, minute: 0, cfg: cfg(), cfg_ver: cfg_ver() }
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
        let cv = cfg_ver();
        let cfg_changed = cv != self.cfg_ver;
        if cfg_changed { self.cfg_ver = cv; self.cfg = cfg(); }
        let sh = SHARED.lock().unwrap();
        if sh.ver == self.ver && !cfg_changed { return false; }
        self.ver = sh.ver;
        // Los filtros se aplican aquí: cambiarlos no pide nada a la red
        let items: Vec<Item> = sh.items.iter().filter(|i| self.cfg.allows(i)).take(SHOW).cloned().collect();
        if items != self.items || cfg_changed { self.wraps.clear(); self.page = 0; self.prev = None; self.dot = 0.0; }
        self.items = items;
        self.fetched = sh.fetched;
        self.offline = sh.offline;
        true
    }
    fn rows(&self) -> usize { self.cfg.per_page.clamp(1, ROWS_MAX) }
    fn pages(&self) -> usize { self.items.len().div_ceil(self.rows()).max(1) }
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
        let fresh = self.cfg.meta && it.ts > 0 && now() - it.ts < 30 * 60;
        let sx = if fresh { PAD + 9.0 } else { PAD };
        if fresh {
            unsafe { ui.rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: PAD + 2.5, Y: y + 7.5 }, radiusX: 2.5, radiusY: 2.5 }, &ui.brush(profiles::accent(1.0))); }
        }
        // Sin fuente ni hora, el titular sube a su sitio
        let ty = if self.cfg.meta {
            let src = fit(ui, &ui.f.small, &it.source, w - PAD - sx - aw);
            ui.text(&ui.f.small, &src, sx, y, 15.0, &ui.dim);
            if !ago.is_empty() { ui.text_r(&ui.f.sub, &ago, w - PAD, y, 15.0, &ui.dim); }
            y + 18.0
        } else { y + 8.0 };
        let maxw = w - 2.0 * PAD;
        let (l1, l2) = match self.wraps.get(&it.title) {
            Some(v) => v.clone(),
            None => { let v = wrap2(ui, &ui.f.name, &it.title, maxw); self.wraps.insert(it.title.clone(), v.clone()); v }
        };
        ui.text(&ui.f.name, &l1, PAD, ty, 19.0, &ui.white);
        if !l2.is_empty() { ui.text(&ui.f.name, &l2, PAD, ty + 18.0, 19.0, &ui.white); }
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
        // Con menos titulares por página, cada uno tiene más aire (la lista ocupa lo mismo)
        let rows = self.rows();
        let slot = ROW * ROWS_MAX as f32 / rows as f32;
        let top = LIST_TOP + (slot - ROW) / 2.0;
        let bottom = LIST_TOP + ROWS_MAX as f32 * ROW - 6.0;
        unsafe { ui.rt.PushAxisAlignedClip(&rect(0.0, LIST_TOP - 10.0, w, bottom), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE); }
        // Paso de página: los titulares viejos suben y se apagan; los nuevos llegan desde abajo, en cascada
        let e = self.prev.map(|(_, at)| at.elapsed().as_secs_f32() / SLIDE_S);
        if e.is_some_and(|e| e >= 1.0 + 0.06 * rows as f32) { self.prev = None; }
        let stag = |e: f32, r: usize| popup::ease_out((e - 0.06 * r as f32).clamp(0.0, 1.0));
        if let (Some((old, _)), Some(e)) = (self.prev, e) {
            for r in 0..rows {
                let k = stag(e * 1.1, r);
                let i = old * rows + r;
                if i < n { self.row(ui, i, top + r as f32 * slot - 22.0 * k, 1.0 - k, w, false); }
            }
            moving = true;
        }
        for r in 0..rows {
            let i = self.page * rows + r;
            if i >= n { break; }
            // Al aparecer el widget, también en cascada
            let k = match e { Some(e) => stag(e * 0.95 - 0.12, r), None => ez(t, 0.08 + 0.07 * r as f32, 0.5) };
            let y = top + r as f32 * slot + 18.0 * (1.0 - k);
            self.row(ui, i, y, k, w, k > 0.5);
            // Línea fina entre titulares
            if r + 1 < rows && i + 1 < n {
                let ly = LIST_TOP + (r + 1) as f32 * slot - 7.5;
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
        if self.cfg.turn > 0 && self.pages() > 1 && self.turned.elapsed().as_secs_f32() >= self.cfg.turn as f32 { self.turn(self.page + 1); r = true; }
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
    }

    fn ini(pairs: Vec<(&'static str, String)>) -> impl Fn(&str) -> Option<String> {
        move |k: &str| pairs.iter().find(|p| p.0 == k).map(|p| p.1.clone())
    }

    #[test]
    fn configuracion_y_migracion() {
        // Sin nada: la portada de España, como antes
        let c = Cfg::default();
        assert_eq!(c.topics, vec!["portada"]);
        assert_eq!(c.urls(), vec![DEFAULT_FEED.to_string()]);
        assert_eq!((c.turn, c.meta, c.per_page), (11, true, 4));
        // De v0.5: la portada iba como fuente; se pasa a tema y las otras siguen
        let c = cfg_from(ini(vec![("noticias_fuentes", format!(" https://a.es/rss | nada | {DEFAULT_FEED}|javascript:alert(1)| http://b.es/feed "))]));
        assert_eq!(c.topics, vec!["portada"]);
        assert_eq!(c.feeds, vec!["https://a.es/rss", "http://b.es/feed"]);
        // Fuentes propias sin la portada: solo esas
        let c = cfg_from(ini(vec![("noticias_fuentes", "https://a.es/rss".into())]));
        assert!(c.topics.is_empty());
        assert_eq!(c.urls(), vec!["https://a.es/rss".to_string()]);
        // Ida y vuelta por widgets.ini
        let c = Cfg {
            topics: vec!["economia".into(), "portada".into(), "inventado".into()], region: "MX".into(),
            feeds: vec!["https://a.es/rss".into()], names: vec![("https://a.es/rss".into(), "Diario | A".into()), ("https://ya-no.es".into(), "x".into())],
            mute: vec!["fútbol".into(), "Copa del Rey".into()], hide: vec!["ABC".into(), "El País".into()],
            turn: 0, meta: false, per_page: 3,
        };
        let back = cfg_from(ini(cfg_pairs(&c)));
        assert_eq!(back.topics, vec!["portada", "economia"], "en el orden de los temas y sin los que no existen");
        assert_eq!(back.region, "MX");
        assert_eq!(back.feeds, c.feeds);
        assert_eq!(back.names, vec![("https://a.es/rss".to_string(), "Diario   A".to_string())]);
        assert_eq!(back.mute, c.mute);
        assert_eq!(back.hide, c.hide);
        assert_eq!((back.turn, back.meta, back.per_page), (0, false, 3));
        // Valores raros: lo de por defecto
        let c = cfg_from(ini(vec![("noticias_temas", "".into()), ("noticias_region", "zz".into()), ("noticias_pasar", "9".into()), ("noticias_por_pagina", "40".into())]));
        assert!(c.topics.is_empty());
        assert_eq!((c.region.as_str(), c.turn, c.per_page), ("ES", 11, 4));
        // Sin temas ni fuentes, la portada del país elegido
        let c = cfg_from(ini(vec![("noticias_temas", "".into()), ("noticias_region", "us".into())]));
        assert_eq!(c.urls(), vec!["https://news.google.com/rss?hl=en-US&gl=US&ceid=US:en".to_string()]);
    }

    #[test]
    fn urls_de_los_temas() {
        assert_eq!(topic_url(None, "ES"), DEFAULT_FEED);
        assert_eq!(topic_url(Some("BUSINESS"), "ES"), "https://news.google.com/rss/headlines/section/topic/BUSINESS?hl=es&gl=ES&ceid=ES:es");
        assert_eq!(topic_url(Some("NATION"), "MX"), "https://news.google.com/rss/headlines/section/topic/NATION?hl=es-419&gl=MX&ceid=MX:es-419");
        assert_eq!(topic_url(Some("TECHNOLOGY"), "GB"), "https://news.google.com/rss/headlines/section/topic/TECHNOLOGY?hl=en-GB&gl=GB&ceid=GB:en");
        // Un país que no existe: España
        assert_eq!(topic_url(Some("WORLD"), "??"), "https://news.google.com/rss/headlines/section/topic/WORLD?hl=es&gl=ES&ceid=ES:es");
        let c = Cfg { topics: vec!["portada".into(), "salud".into()], feeds: vec!["https://a.es/rss".into()], ..Cfg::default() };
        assert_eq!(c.urls(), vec![DEFAULT_FEED.to_string(), topic_url(Some("HEALTH"), "ES"), "https://a.es/rss".to_string()]);
        for t in TOPICS { assert!(t.2.is_none_or(|x| x.chars().all(|c| c.is_ascii_uppercase()))); }
    }

    #[test]
    fn filtros() {
        assert_eq!(fold("Económía ÑANDÚ Über"), "economia nandu uber");
        // Sin distinguir mayúsculas ni tildes, y empezando palabra
        assert!(matches_term("El FÚTBOL vuelve", "futbol"));
        assert!(matches_term("La final de la Copa del Rey", "copa del rey"));
        assert!(matches_term("Los Reyes Magos llegan", "rey"));
        assert!(!matches_term("Monterrey crece", "rey"));
        assert!(matches_term("«Trump» dice", "trump"));
        assert!(matches_term("Trump's tweet", "TRUMP"));
        assert!(!matches_term("Nada que ver", "  "));
        assert!(matches_term("Avión 😀 aterriza", "avion"));
        let it = |t: &str, s: &str| Item { title: t.into(), source: s.into(), link: String::new(), ts: 0 };
        let c = Cfg { mute: vec!["fútbol".into()], hide: vec!["abc".into()], ..Cfg::default() };
        assert!(!c.allows(&it("Gol en el futbol", "El País")));
        assert!(!c.allows(&it("Otra cosa", "ABC")));
        assert!(!c.allows(&it("Otra cosa", "ABC Sevilla")));
        assert!(c.allows(&it("Otra cosa", "Cabcd")));
        assert!(c.allows(&it("Economía", "El País")));
    }

    #[test]
    fn descubrir_el_feed() {
        let html = r#"<html><head><link rel="stylesheet" href="/a.css"><link rel="alternate" type="application/rss+xml" title="RSS" href="/feed/"></head></html>"#;
        assert_eq!(discover_feed(html, "https://blog.es/post/1").as_deref(), Some("https://blog.es/feed/"));
        let html = r#"<link rel='alternate' type='application/atom+xml' href='https://otro.es/atom.xml'/>"#;
        assert_eq!(discover_feed(html, "https://blog.es/").as_deref(), Some("https://otro.es/atom.xml"));
        let html = r#"<link rel="alternate" type="application/rss+xml" href="rss.xml">"#;
        assert_eq!(discover_feed(html, "https://blog.es/seccion/pagina").as_deref(), Some("https://blog.es/seccion/rss.xml"));
        assert_eq!(discover_feed("<html>nada</html>", "https://blog.es/"), None);
        assert_eq!(feed_title(RSS), "Noticias destacadas - Google News");
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

#[cfg(test)]
mod tests_url_segura {
    #[test]
    fn url_con_saltos_o_barra_no_vale() {
        assert!(super::is_web_url("https://example.com/feed.xml"));
        assert!(!super::is_web_url("https://example.com/a\nbolsa_simbolos = X"));
        assert!(!super::is_web_url("https://example.com/a|b"));
        assert!(!super::is_web_url("https://example.com/a b"));
    }
}