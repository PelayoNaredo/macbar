//! Widget de bolsa: precio, variación y minigráfica de cada valor. Se configura en la ventana de
//! Configuración (sección Bolsa), que lo guarda en widgets.ini:
//! - `bolsa_simbolos = ^IBEX|SAN.MC|AAPL|BTC-USD` (nombres de Yahoo Finance, en orden)
//! - `bolsa_nombres = SAN.MC~Banco Santander~Madrid|…` (nombre y mercado de cada uno, para la lista)
//! - `bolsa_periodo = dia|5dias|mes|ano`, `bolsa_variacion = pct|importe`, `bolsa_grafica = 1|0`,
//!   `bolsa_refresco = 1|5|15` (minutos)
//!
//! Los datos salen de la API pública de gráficos de Yahoo (sin clave): un hilo los pide mientras el widget
//! se ve y los guarda en `%APPDATA%\macbar\bolsa_cache.txt`; sin red se enseña lo último guardado con la
//! marca «Sin conexión». Un clic en una fila abre la página del valor en Yahoo Finance.

use super::*;
use desk::{DFonts, Kind};
use deskw::{header, Widget};
use news::{approach, ez, fit, fmt_ago, now, page_dots};
use popup::Ui;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64};

const KEY: &str = "bolsa_simbolos";
const K_NAMES: &str = "bolsa_nombres";
const K_PERIOD: &str = "bolsa_periodo";
const K_VAR: &str = "bolsa_variacion";
const K_CHART: &str = "bolsa_grafica";
const K_REFRESH: &str = "bolsa_refresco";
pub(crate) const DEFAULT_SYMBOLS: &str = "^IBEX|SAN.MC|AAPL|BTC-USD";
const RETRY: i64 = 60;
pub(crate) const MAX_SYMBOLS: usize = 12;
/// Minutos de refresco que se ofrecen.
pub(crate) const REFRESH_OPTS: [u32; 3] = [1, 5, 15];

// ================================================================ configuración
/// Periodo de la gráfica (y de la variación: contra el cierre de antes del periodo).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Period { #[default] Day, Week, Month, Year }

impl Period {
    pub const ALL: [Period; 4] = [Period::Day, Period::Week, Period::Month, Period::Year];
    pub fn key(self) -> &'static str { match self { Period::Day => "dia", Period::Week => "5dias", Period::Month => "mes", Period::Year => "ano" } }
    pub fn from_key(s: &str) -> Period { Period::ALL.into_iter().find(|p| p.key() == s.trim()).unwrap_or_default() }
    /// Rótulo (en español; se traduce al pintarlo).
    pub fn label(self) -> &'static str { match self { Period::Day => "Día", Period::Week => "5 días", Period::Month => "Mes", Period::Year => "Año" } }
    /// `range` e `interval` de la API de gráficos de Yahoo.
    pub fn range_interval(self) -> (&'static str, &'static str) {
        match self { Period::Day => ("1d", "5m"), Period::Week => ("5d", "30m"), Period::Month => ("1mo", "1d"), Period::Year => ("1y", "1wk") }
    }
}

/// Lo que se elige en Configuración → Bolsa.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Cfg {
    pub syms: Vec<String>,
    /// (símbolo, nombre, mercado) de los que se conocen (de la búsqueda o de la cotización).
    pub names: Vec<(String, String, String)>,
    pub period: Period,
    /// Variación en importe (€/$) en vez de en %.
    pub amount: bool,
    pub chart: bool,
    pub refresh_min: u32,
}

impl Default for Cfg { fn default() -> Self { cfg_from(|_| None) } }

/// Configuración a partir de los valores de widgets.ini (`get` = valor de una clave).
pub(crate) fn cfg_from(get: impl Fn(&str) -> Option<String>) -> Cfg {
    let names = get(K_NAMES).unwrap_or_default().split('|').filter_map(|e| {
        let mut p = e.split('~');
        let s = p.next()?.trim().to_uppercase();
        (!s.is_empty()).then(|| (s, p.next().unwrap_or("").trim().to_string(), p.next().unwrap_or("").trim().to_string()))
    }).collect();
    Cfg {
        syms: symbols_from(get(KEY).as_deref()),
        names,
        period: Period::from_key(&get(K_PERIOD).unwrap_or_default()),
        amount: get(K_VAR).is_some_and(|v| v.trim() == "importe"),
        chart: get(K_CHART).is_none_or(|v| v.trim() != "0"),
        refresh_min: get(K_REFRESH).and_then(|v| v.trim().parse().ok()).filter(|m| REFRESH_OPTS.contains(m)).unwrap_or(5),
    }
}

/// Pares clave/valor que se guardan en widgets.ini.
pub(crate) fn cfg_pairs(c: &Cfg) -> Vec<(&'static str, String)> {
    let clean = |s: &str| s.replace(['|', '~', '\n', '\r', '\t'], " ").trim().to_string();
    // Solo los nombres de los valores que siguen en la lista
    let names: Vec<String> = c.names.iter().filter(|n| c.syms.contains(&n.0)).map(|(s, n, m)| format!("{s}~{}~{}", clean(n), clean(m))).collect();
    vec![
        (KEY, c.syms.join("|")),
        (K_NAMES, names.join("|")),
        (K_PERIOD, c.period.key().to_string()),
        (K_VAR, if c.amount { "importe" } else { "pct" }.to_string()),
        (K_CHART, if c.chart { "1" } else { "0" }.to_string()),
        (K_REFRESH, c.refresh_min.to_string()),
    ]
}

static CFG: Mutex<Option<Cfg>> = Mutex::new(None);
/// Sube cada vez que cambia la configuración (el widget y el hilo lo vigilan).
static CFG_VER: AtomicU64 = AtomicU64::new(1);

/// Configuración vigente (leída de widgets.ini la primera vez).
pub(crate) fn cfg() -> Cfg {
    let mut g = CFG.lock().unwrap();
    g.get_or_insert_with(|| cfg_from(desk::ini_value)).clone()
}
pub(crate) fn cfg_ver() -> u64 { CFG_VER.load(Ordering::Relaxed) }

/// Guarda la configuración y la aplica al momento (el hilo vuelve a pedir los datos si hace falta).
pub(crate) fn set_cfg(c: Cfg) {
    for (k, v) in cfg_pairs(&c) { desk::set_ini_value(k, &v); }
    *CFG.lock().unwrap() = Some(c);
    CFG_VER.fetch_add(1, Ordering::Relaxed);
    FORCE.store(true, Ordering::Relaxed);
    if sys::demo() { demo_fill(); }
}

/// Vuelve a leer widgets.ini (por si se cambió a mano). true si cambió algo.
fn reload_cfg() -> bool {
    let n = cfg_from(desk::ini_value);
    let mut g = CFG.lock().unwrap();
    if g.as_ref() == Some(&n) { return false; }
    *g = Some(n);
    CFG_VER.fetch_add(1, Ordering::Relaxed);
    true
}

/// Mercado de un símbolo por su forma, cuando no se sabe por la búsqueda («SAN.MC» → Madrid).
pub(crate) fn market_of(sym: &str) -> &'static str {
    let s = sym.to_uppercase();
    if s.starts_with('^') { return "Índice"; }
    if s.ends_with("=X") { return "Divisa"; }
    if s.ends_with("=F") { return "Futuro"; }
    if s.ends_with("-USD") || s.ends_with("-EUR") { return "Cripto"; }
    match s.rsplit_once('.').map(|(_, x)| x) {
        Some("MC") => "Madrid", Some("PA") => "París", Some("DE") | Some("F") => "Fráncfort", Some("L") => "Londres",
        Some("MI") => "Milán", Some("AS") => "Ámsterdam", Some("LS") => "Lisboa", Some("SW") => "Suiza",
        Some("T") => "Tokio", Some("HK") => "Hong Kong", Some("TO") => "Toronto", Some("SA") => "São Paulo",
        Some("MX") => "México", Some("BA") => "Buenos Aires",
        _ => "EE. UU.",
    }
}

const PAD: f32 = 20.0;
const LIST_TOP: f32 = 52.0;
const ROW: f32 = 56.0;
const TURN_S: f32 = 12.0;
const SLIDE_S: f32 = 0.6;
/// Minigráfica: dónde empieza y lo ancha que es.
const SPARK_X: f32 = 156.0;
const SPARK_W: f32 = 92.0;

#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) struct Quote {
    pub sym: String,
    pub name: String,
    pub currency: String,
    /// Índice (sin moneda)
    pub index: bool,
    pub price: f64,
    /// Cierre anterior (la variación del día se mide contra él)
    pub prev: f64,
    pub time: i64,
    /// Cierres de cada 5 min de hoy
    pub closes: Vec<f32>,
    /// Para el detalle de Extragrande (0 = Yahoo no lo da): apertura (la del periodo), máximo y mínimo
    /// del día, volumen y máximo y mínimo de 52 semanas
    pub open: f64, pub high: f64, pub low: f64, pub volume: f64, pub wk_high: f64, pub wk_low: f64,
}

impl Quote {
    pub fn change_pct(&self) -> f64 { if self.prev > 0.0 { (self.price / self.prev - 1.0) * 100.0 } else { 0.0 } }
}

// ================================================================ JSON mínimo (lo justo para el gráfico de Yahoo)
fn after<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let k = format!("\"{key}\":");
    Some(s[s.find(&k)? + k.len()..].trim_start())
}

fn num_after(s: &str, key: &str) -> Option<f64> {
    let r = after(s, key)?;
    let end = r.find(|c: char| !(c.is_ascii_digit() || "+-.eE".contains(c))).unwrap_or(r.len());
    r[..end].parse().ok().filter(|v: &f64| v.is_finite())
}

/// Cadena JSON tras la clave (con sus escapes resueltos).
pub(crate) fn str_after(s: &str, key: &str) -> Option<String> {
    let r = after(s, key)?.strip_prefix('"')?;
    let mut o = String::new();
    let mut it = r.chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return Some(o),
            '\\' => match it.next()? {
                'n' => o.push(' '), 't' => o.push(' '), 'r' => {}
                'u' => {
                    let h: String = it.by_ref().take(4).collect();
                    o.push(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32).unwrap_or('?'));
                }
                c => o.push(c),
            },
            c => o.push(c),
        }
    }
    None
}

/// Lista de números tras la clave (`[1.5,null,2]`; los `null` se saltan).
fn nums_after(s: &str, key: &str) -> Option<Vec<f32>> {
    let r = after(s, key)?.strip_prefix('[')?;
    let body = &r[..r.find(']')?];
    Some(body.split(',').filter_map(|p| p.trim().parse::<f64>().ok()).filter(|v| v.is_finite()).map(|v| v as f32).collect())
}

/// Respuesta de `v8/finance/chart` → cotización (None si Yahoo no conoce el símbolo).
pub(crate) fn parse_chart(json: &str, sym: &str) -> Option<Quote> {
    let res = after(json, "result")?;
    if res.starts_with("null") { return None; }
    let meta = after(res, "meta")?;
    let price = num_after(meta, "regularMarketPrice")?;
    let prev = num_after(meta, "chartPreviousClose").or_else(|| num_after(meta, "previousClose")).unwrap_or(0.0);
    let name = str_after(meta, "shortName").or_else(|| str_after(meta, "longName")).unwrap_or_else(|| sym.to_string());
    let closes = after(res, "indicators").and_then(|i| nums_after(i, "close")).unwrap_or_default();
    let open = after(res, "indicators").and_then(|i| nums_after(i, "open")).and_then(|o| o.first().copied()).unwrap_or(0.0) as f64;
    let pos = |k: &str| num_after(meta, k).filter(|v| *v > 0.0).unwrap_or(0.0);
    Some(Quote {
        sym: sym.to_string(),
        name: pretty_name(&name),
        currency: str_after(meta, "currency").unwrap_or_default(),
        index: str_after(meta, "instrumentType").is_some_and(|t| t == "INDEX"),
        price,
        prev,
        time: num_after(meta, "regularMarketTime").unwrap_or(0.0) as i64,
        closes,
        open, high: pos("regularMarketDayHigh"), low: pos("regularMarketDayLow"), volume: pos("regularMarketVolume"),
        wk_high: pos("fiftyTwoWeekHigh"), wk_low: pos("fiftyTwoWeekLow"),
    })
}

#[cfg(test)]
pub(crate) fn chart_url(sym: &str) -> String { chart_url_for(sym, Period::Day) }
pub(crate) fn chart_url_for(sym: &str, p: Period) -> String {
    let (r, i) = p.range_interval();
    format!("https://query1.finance.yahoo.com/v8/finance/chart/{}?range={r}&interval={i}", net::encode(sym))
}
fn quote_page(sym: &str) -> String { format!("https://finance.yahoo.com/quote/{}", net::encode(sym)) }

// ================================================================ formato
/// «BANCO SANTANDER, S.A.» → «Banco Santander» (las palabras largas en mayúsculas pasan a nombre propio
/// y se quita la forma jurídica del final).
pub(crate) fn pretty_name(s: &str) -> String {
    let mut s = s.trim().to_string();
    for suf in [", S.A.", " S.A.", ", Inc.", " Inc.", ", Inc", " Inc", " plc", " PLC", " N.V.", " SE", " AG"] {
        if s.len() > suf.len() + 2 { if let Some(r) = s.strip_suffix(suf) { s = r.trim_end_matches([',', ' ']).to_string(); break; } }
    }
    s.split(' ').map(|w| {
        let letters = w.chars().filter(|c| c.is_alphabetic()).count();
        if letters >= 5 && !w.chars().any(|c| c.is_lowercase()) {
            let mut cs = w.chars();
            cs.next().map(|f| f.to_string() + &cs.as_str().to_lowercase()).unwrap_or_default()
        } else { w.to_string() }
    }).collect::<Vec<_>>().join(" ")
}

/// Símbolo para enseñar: «^IBEX» → «IBEX», «EURUSD=X» → «EURUSD».
pub(crate) fn display_symbol(s: &str) -> String { s.trim_start_matches('^').trim_end_matches("=X").to_string() }

/// Miles y decimales al estilo del idioma: «18.873,10» / «18,873.10» (y 4 decimales por debajo de 1).
pub(crate) fn fmt_num(v: f64, dec: usize, en: bool) -> String {
    let s = format!("{:.*}", dec, v.abs());
    let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
    let (th, dp) = if en { (',', '.') } else { ('.', ',') };
    let mut o = String::new();
    // En español, los números de 4 cifras van sin separador (1234,50), como dice la RAE
    let group = en || int.len() > 4;
    for (i, c) in int.chars().enumerate() {
        if group && i > 0 && (int.len() - i) % 3 == 0 { o.push(th); }
        o.push(c);
    }
    if !frac.is_empty() { o.push(dp); o += frac; }
    if v < 0.0 && o.chars().any(|c| c != '0' && c.is_ascii_digit()) { o.insert(0, '\u{2212}'); }
    o
}

fn cur_symbol(c: &str) -> &'static str {
    match c { "EUR" => "€", "USD" => "$", "GBP" => "£", "JPY" | "CNY" => "¥", "CHF" => "CHF", _ => "" }
}

/// Precio con su moneda: «6,92 €» / «$228.41» (los índices, sin moneda).
pub(crate) fn fmt_price(q: &Quote, en: bool) -> String {
    let dec = if q.price.abs() < 1.0 { 4 } else { 2 };
    let n = fmt_num(q.price, dec, en);
    let c = if q.index { "" } else { cur_symbol(&q.currency) };
    match (c, en) {
        ("", _) => n,
        ("$" | "£" | "€" | "¥", true) => format!("{c}{n}"),
        _ => format!("{n} {c}"),
    }
}

/// Variación: «+0,84 %» / «−1,12 %» (en inglés, «+0.84%»).
pub(crate) fn fmt_pct(p: f64, en: bool) -> String {
    let n = fmt_num(p.abs(), 2, en);
    let sign = if n.chars().all(|c| !c.is_ascii_digit() || c == '0') { "" } else if p < 0.0 { "\u{2212}" } else { "+" };
    if en { format!("{sign}{n}%") } else { format!("{sign}{n} %") }
}

/// Variación en importe: «+0,98 €» / «−1.12 $» (los índices, en puntos y sin moneda).
pub(crate) fn fmt_change(q: &Quote, en: bool) -> String {
    let d = q.price - q.prev;
    let dec = if q.price.abs() < 1.0 { 4 } else { 2 };
    let n = fmt_num(d.abs(), dec, en);
    let sign = if n.chars().all(|c| !c.is_ascii_digit() || c == '0') { "" } else if d < 0.0 { "\u{2212}" } else { "+" };
    let c = if q.index { "" } else { cur_symbol(&q.currency) };
    match (c, en) {
        ("", _) => format!("{sign}{n}"),
        ("$" | "£" | "€" | "¥", true) => format!("{sign}{c}{n}"),
        _ => format!("{sign}{n} {c}"),
    }
}

/// ¿Vale como símbolo de Yahoo? (letras, cifras y `^.-=_`, hasta 20).
pub(crate) fn valid_symbol(s: &str) -> bool { !s.is_empty() && s.len() <= 20 && s.chars().all(|c| c.is_ascii_alphanumeric() || "^.-=_".contains(c)) }

/// Símbolos de widgets.ini (separados por `|` o comas); sin la clave, los de ejemplo. Con la clave
/// vacía (se quitaron todos), ninguno.
pub(crate) fn symbols_from(v: Option<&str>) -> Vec<String> {
    let mut l: Vec<String> = Vec::new();
    for s in v.unwrap_or(DEFAULT_SYMBOLS).split(['|', ',']) {
        let s = s.trim().to_uppercase();
        if !valid_symbol(&s) { continue; }
        if !l.contains(&s) { l.push(s); }
    }
    l.truncate(MAX_SYMBOLS);
    if l.is_empty() && v.is_some_and(|v| !v.trim().is_empty()) { return symbols_from(None); }
    l
}

/// Abre la Configuración en la sección de la bolsa (clic derecho en la cabecera del widget).
pub(crate) fn edit_symbols() { settings::open_at_stocks(); }

// ================================================================ caché en disco
fn cache_path() -> String { sys::config_dir() + "\\bolsa_cache.txt" }

/// «v1\t<pedido>\t<periodo>» y una línea por valor: símbolo, nombre, moneda, índice, precio, cierre
/// anterior, hora y cierres, y (desde los tamaños) apertura, máximo, mínimo, volumen y 52 semanas. (Las
/// cachés de antes no traen periodo, que era el día, ni el detalle.)
pub(crate) fn cache_to_text(fetched: i64, period: Period, qs: &[Quote]) -> String {
    let mut s = format!("v1\t{fetched}\t{}\n", period.key());
    for q in qs {
        let cl: Vec<String> = q.closes.iter().map(|c| c.to_string()).collect();
        s += &format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n", q.sym, q.name.replace(['\t', '\n', '\r'], " "), q.currency, q.index as u8, q.price, q.prev, q.time, cl.join(","), q.open, q.high, q.low, q.volume, q.wk_high, q.wk_low);
    }
    s
}

pub(crate) fn cache_from_text(s: &str) -> Option<(i64, Period, Vec<Quote>)> {
    let mut lines = s.lines();
    let mut h = lines.next()?.split('\t');
    if h.next()? != "v1" { return None; }
    let fetched = h.next()?.parse().ok()?;
    let period = Period::from_key(h.next().unwrap_or(""));
    let qs = lines.filter_map(|l| {
        let p: Vec<&str> = l.split('\t').collect();
        if p.len() != 8 && p.len() != 14 { return None; }
        let x = |i: usize| p.get(i).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0);
        Some(Quote {
            sym: p[0].into(), name: p[1].into(), currency: p[2].into(), index: p[3] == "1",
            price: p[4].parse().ok()?, prev: p[5].parse().ok()?, time: p[6].parse().unwrap_or(0),
            closes: p[7].split(',').filter_map(|c| c.parse().ok()).collect(),
            open: x(8), high: x(9), low: x(10), volume: x(11), wk_high: x(12), wk_low: x(13),
        })
    }).collect();
    Some((fetched, period, qs))
}

// ================================================================ hilo de datos
struct Shared { quotes: Vec<Quote>, fetched: i64, offline: bool, ver: u64 }
static SHARED: Mutex<Shared> = Mutex::new(Shared { quotes: Vec::new(), fetched: 0, offline: false, ver: 0 });
static SEEN: AtomicI64 = AtomicI64::new(0);
static FORCE: AtomicBool = AtomicBool::new(false);
static STARTED: std::sync::Once = std::sync::Once::new();

/// Nombre (y si es índice) que ya trae la última cotización de un símbolo.
pub(crate) fn known_name(sym: &str) -> Option<String> {
    SHARED.lock().unwrap().quotes.iter().find(|q| q.sym == sym).map(|q| q.name.clone()).filter(|n| !n.is_empty())
}

/// Un valor de muestra (precios de ejemplo y un día inventado, sin red).
fn demo_quote(sym: &str, name: &str, cur: &str, index: bool, prev: f64, price: f64, seed: f32, now: i64) -> Quote {
    // El volumen, inventado (los índices no lo traen)
    let volume = if index { 0.0 } else { (seed as f64 * 7.3e6 + 2.1e7) * if price > 10_000.0 { 1000.0 } else { 1.0 } };
    let n = 78;
    let closes = (0..n).map(|i| {
        let x = i as f32 / (n - 1) as f32;
        let trend = prev as f32 + (price - prev) as f32 * x;
        let amp = (price - prev).abs().max(prev * 0.004) as f32;
        let wob = (x * 9.0 + seed).sin() * 0.45 + (x * 23.0 + seed * 2.0).sin() * 0.25 + (x * 51.0 + seed * 3.0).sin() * 0.12;
        if i == n - 1 { price as f32 } else { trend + wob * amp * (0.3 + 0.7 * (x * 3.1).sin().abs()) }
    }).collect();
    let closes: Vec<f32> = closes;
    let (lo, hi) = closes.iter().fold((f32::MAX, f32::MIN), |(a, b), &c| (a.min(c), b.max(c)));
    Quote { sym: sym.into(), name: name.into(), currency: cur.into(), index, price, prev, time: now - 120, open: closes[0] as f64, high: hi as f64, low: lo as f64, volume,
        wk_high: hi as f64 * 1.21, wk_low: lo as f64 * 0.78, closes }
}

/// Valores de muestra para el modo demo.
fn demo_quotes(now: i64) -> Vec<Quote> {
    vec![
        demo_quote("^IBEX", "IBEX 35", "EUR", true, 11744.6, 11842.3, 0.7, now),
        demo_quote("SAN.MC", "Banco Santander", "EUR", false, 6.998, 6.920, 2.1, now),
        demo_quote("AAPL", "Apple", "USD", false, 227.23, 228.41, 4.0, now),
        demo_quote("BTC-USD", "Bitcoin USD", "USD", false, 59820.0, 61204.5, 5.3, now),
    ]
}

/// Modo demo: los valores de la configuración, con los de muestra o inventados a partir del símbolo.
fn demo_fill() {
    let base = demo_quotes(now());
    let c = cfg();
    let qs = c.syms.iter().map(|s| base.iter().find(|q| &q.sym == s).cloned().unwrap_or_else(|| {
        let h = s.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
        let prev = 20.0 + (h % 400) as f64;
        let price = prev * (1.0 + ((h / 400) % 61) as f64 / 1000.0 - 0.03);
        let name = c.names.iter().find(|n| &n.0 == s).map(|n| n.1.clone()).unwrap_or_else(|| s.clone());
        demo_quote(s, &name, "EUR", s.starts_with('^'), prev, price, (h % 97) as f32 / 10.0, now())
    })).collect();
    let mut sh = SHARED.lock().unwrap();
    sh.quotes = qs;
    sh.fetched = now() - 120;
    sh.ver += 1;
}

/// Arranca los datos (caché o demo) sin esperar a que se pinte el widget: Configuración enseña los
/// nombres y medios que ya se conocen.
pub(crate) fn warm() { ensure_started(); }

fn ensure_started() {
    STARTED.call_once(|| {
        // (render de estados vacíos: sin red)
        if sys::demo() { if desk::shot_empty() { let mut sh = SHARED.lock().unwrap(); sh.offline = true; sh.ver += 1; } else { demo_fill(); } return; }
        let mut last_try = 0;
        let mut last_period = Period::Day;
        if let Some((f, p, qs)) = std::fs::read_to_string(cache_path()).ok().and_then(|s| cache_from_text(&s)) {
            let mut sh = SHARED.lock().unwrap();
            sh.quotes = qs;
            sh.fetched = f;
            sh.ver += 1;
            last_try = f;
            last_period = p;
        }
        std::thread::spawn(move || {
            let mut last_try = last_try;
            let mut last_key = (SHARED.lock().unwrap().quotes.iter().map(|q| q.sym.clone()).collect::<Vec<_>>(), last_period);
            let mut n = 0u32;
            loop {
                // Cada 10 s se mira si widgets.ini cambió a mano (lo de Configuración llega al momento)
                if n % 10 == 0 { reload_cfg(); }
                n = n.wrapping_add(1);
                let t = now();
                let c = cfg();
                let key = (c.syms.clone(), c.period);
                let force = FORCE.swap(false, Ordering::Relaxed);
                let alive = t - SEEN.load(Ordering::Relaxed) < 90 && deskdata::VISIBLE.load(Ordering::Relaxed);
                let offline = SHARED.lock().unwrap().offline;
                let wait = if offline { RETRY } else { c.refresh_min as i64 * 60 };
                if alive && (force || key != last_key || t - last_try >= wait) {
                    last_try = t;
                    // Con otro periodo, lo guardado no vale (su gráfica es de otro tramo)
                    let old = if key.1 == last_key.1 { SHARED.lock().unwrap().quotes.clone() } else { Vec::new() };
                    let mut got = 0;
                    let qs: Vec<Quote> = c.syms.iter().filter_map(|s| {
                        let fresh = net::get(&chart_url_for(s, c.period)).and_then(|b| parse_chart(&String::from_utf8_lossy(&b), s));
                        if fresh.is_some() { got += 1; }
                        // Si este falla, se queda el último que hubo
                        fresh.or_else(|| old.iter().find(|q| &q.sym == s).cloned())
                    }).collect();
                    let mut sh = SHARED.lock().unwrap();
                    if got == 0 && !c.syms.is_empty() {
                        sh.offline = true;
                        // Con símbolos nuevos y sin red, al menos se quitan los que ya no están
                        if key != last_key { sh.quotes = qs; }
                    } else {
                        last_key = key;
                        sh.quotes = qs;
                        sh.fetched = t;
                        sh.offline = false;
                        let _ = std::fs::create_dir_all(sys::config_dir());
                        let _ = std::fs::write(cache_path(), cache_to_text(t, c.period, &sh.quotes));
                    }
                    sh.ver += 1;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    });
}

// ================================================================ dibujo
/// Minigráfica del día, en la tinta del widget (blanca o casi negra): línea, degradado suave debajo,
/// línea de puntos en el cierre anterior y punto final.
/// `k` (0..1) la descubre de izquierda a derecha.
fn sparkline(ui: &Ui, q: &Quote, x0: f32, y0: f32, w: f32, h: f32, k: f32, day: bool) {
    let v = &q.closes;
    if v.len() < 2 || k <= 0.01 { return; }
    let up = q.price >= q.prev;
    let prev = q.prev as f32;
    let (mut lo, mut hi) = v.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    if prev > 0.0 { lo = lo.min(prev); hi = hi.max(prev); }
    if hi - lo < hi.abs() * 1e-5 { hi += 1.0; lo -= 1.0; }
    // La jornada entera ocupa el ancho (a media sesión, la línea llega hasta donde va el día); los
    // demás periodos, de punta a punta
    let full = if day { v.len().max(78) } else { v.len() } as f32 - 1.0;
    let pts: Vec<(f32, f32)> = v.iter().enumerate().map(|(i, &c)| (x0 + w * i as f32 / full, y0 + h - (c - lo) / (hi - lo) * h)).collect();
    unsafe {
        let Ok(f) = ui.rt.GetFactory() else { return };
        ui.rt.PushAxisAlignedClip(&rect(x0 - 4.0, y0 - 4.0, x0 + (w + 8.0) * k, y0 + h + 4.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        let base = y0 + h + 2.0;
        // Cierre anterior: puntitos finos
        if prev > 0.0 {
            let py = y0 + h - (prev - lo) / (hi - lo) * h;
            let mut x = x0;
            let b = ui.wa(0.22);
            while x < x0 + w { ui.rt.FillRectangle(&rect(x, py - 0.5, x + 1.6, py + 0.5), &b); x += 4.0; }
        }
        let path = |close: bool| -> Option<ID2D1PathGeometry> {
            let g = f.CreatePathGeometry().ok()?;
            let s = g.Open().ok()?;
            s.SetSegmentFlags(D2D1_PATH_SEGMENT_FORCE_ROUND_LINE_JOIN);
            let first = if close { Vector2 { X: pts[0].0, Y: base } } else { Vector2 { X: pts[0].0, Y: pts[0].1 } };
            s.BeginFigure(first, if close { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
            for (i, p) in pts.iter().enumerate() { if close || i > 0 { s.AddLine(Vector2 { X: p.0, Y: p.1 }); } }
            if close { s.AddLine(Vector2 { X: pts[pts.len() - 1].0, Y: base }); }
            s.EndFigure(if close { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            s.Close().ok()?;
            Some(g)
        };
        // Lo que baja, algo más apagado que lo que sube (sin color: solo la intensidad)
        let line = if up { 0.92 } else { 0.62 };
        let fill = ui.p.vgrad(y0, base, ui.ic(0.18), ui.ic(0.0));
        if let Some(g) = path(true) { ui.rt.FillGeometry(&g, &fill, None); }
        let stroke = f.CreateStrokeStyle(&D2D1_STROKE_STYLE_PROPERTIES { startCap: D2D1_CAP_STYLE_ROUND, endCap: D2D1_CAP_STYLE_ROUND, lineJoin: D2D1_LINE_JOIN_ROUND, ..Default::default() }, None).ok();
        if let Some(g) = path(false) { ui.rt.DrawGeometry(&g, &ui.wa(line), 1.5, stroke.as_ref()); }
        let (lx, ly) = pts[pts.len() - 1];
        ui.rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: lx, Y: ly }, radiusX: 2.2, radiusY: 2.2 }, &ui.white);
        ui.rt.PopAxisAlignedClip();
    }
}

// ================================================================ widget
/// Cómo se enseña cada valor según el tamaño.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Look {
    /// Ficha: símbolo y variación, precio grande y gráfica debajo (Pequeño, Mediano y Banda, en línea)
    Tile,
    /// Fila de siempre: símbolo y nombre, minigráfica, precio y variación (listas de Columna y Extragrande)
    Row,
    /// Fila estrecha de dos líneas (Alto)
    Narrow,
    /// Fila de una línea bajo la gráfica grande (Grande)
    Mini,
}

/// Valores por página y cómo se ven en cada tamaño.
fn stocks_grid(s: desk::Sz) -> (usize, Look) {
    use desk::Sz::*;
    match s { Pequeno => (1, Look::Tile), Mediano => (2, Look::Tile), Banda => (4, Look::Tile), Alto => (4, Look::Narrow), Grande => (5, Look::Mini), Columna => (6, Look::Row), Extragrande => (5, Look::Row) }
}

/// Volumen corto: «28,4 M», «1,2 mil M» («28.4M», «1.2B» en inglés).
pub(crate) fn fmt_volume(v: f64, en: bool) -> String {
    let (n, u) = match v {
        v if v >= 1e9 => (v / 1e9, if en { "B" } else { " mil M" }),
        v if v >= 1e6 => (v / 1e6, if en { "M" } else { " M" }),
        v if v >= 1e3 => (v / 1e3, if en { "K" } else { " mil" }),
        v => (v, ""),
    };
    let dec = if n >= 100.0 || u.is_empty() { 0 } else { 1 };
    format!("{}{u}", fmt_num(n, dec, en))
}

pub struct StocksW {
    ver: u64, quotes: Vec<Quote>, fetched: i64, offline: bool,
    page: usize, prev: Option<(usize, Instant)>, turned: Instant,
    dot: f32, hovering: bool, moving: bool, minute: i64,
    cfg: Cfg, cfg_ver: u64,
    /// Valor de la gráfica grande (Grande y Extragrande) y cuándo se eligió (la gráfica se redibuja)
    sel: usize, sel_at: Option<Instant>,
}

impl Default for StocksW {
    fn default() -> Self {
        ensure_started();
        SEEN.store(now(), Ordering::Relaxed);
        StocksW { ver: u64::MAX, quotes: Vec::new(), fetched: 0, offline: false, page: 0, prev: None, turned: Instant::now(), dot: 0.0, hovering: false, moving: false, minute: 0, cfg: cfg(), cfg_ver: cfg_ver(), sel: 0, sel_at: None }
    }
}

impl StocksW {
    fn sync(&mut self) -> bool {
        let cv = cfg_ver();
        let cfg_changed = cv != self.cfg_ver;
        if cfg_changed { self.cfg_ver = cv; self.cfg = cfg(); }
        let sh = SHARED.lock().unwrap();
        if sh.ver == self.ver { return cfg_changed; }
        self.ver = sh.ver;
        self.quotes = sh.quotes.clone();
        self.fetched = sh.fetched;
        self.offline = sh.offline;
        if self.page >= self.pages() { self.page = 0; self.prev = None; }
        if self.sel >= self.quotes.len() { self.sel = self.page * self.per(); }
        true
    }
    fn per(&self) -> usize { stocks_grid(desk::sz(Kind::Stocks)).0 }
    fn pages(&self) -> usize { self.quotes.len().div_ceil(self.per()).max(1) }
    fn turn(&mut self, to: usize) {
        let to = to % self.pages();
        if to == self.page { return; }
        self.prev = Some((self.page, Instant::now()));
        self.page = to;
        self.turned = Instant::now();
        self.moving = true;
        // La gráfica grande pasa al primero de la página nueva
        self.sel = to * self.per();
        self.sel_at = Some(Instant::now());
    }
    /// Variación como se eligió (% o importe).
    fn change(&self, q: &Quote, en: bool) -> String { if self.cfg.amount { fmt_change(q, en) } else { fmt_pct(q.change_pct(), en) } }
    /// Variación alineada a la derecha en `right` con su ▲▼ delante (en gris: sin color).
    fn change_r(&self, ui: &Ui, q: &Quote, right: f32, y: f32, bh: f32) {
        let en = i18n::en();
        let up = q.price >= q.prev;
        let pct = self.change(q, en);
        ui.text_r(&ui.f.sub, &pct, right, y, bh, &ui.dim);
        let px = right - ui.p.width(&ui.f.sub, &pct) - 5.0;
        ui.text_r(&ui.f.small, if up { "▲" } else { "▼" }, px, y, bh, &ui.wa(if up { 0.7 } else { 0.45 }));
    }

    /// Una fila: símbolo y nombre, minigráfica, precio y variación (▲▼ y el signo, en gris).
    fn row(&self, ui: &mut Ui, i: usize, x0: f32, x1: f32, y: f32, a: f32, spark: f32, live: bool) {
        let Some(q) = self.quotes.get(i) else { return };
        let id = 100 + i as u32;
        let r = rect(x0 - 10.0, y - 4.0, x1 + 10.0, y + ROW - 6.0);
        // Con gráfica grande encima o al lado, la fila del valor que enseña va resaltada
        let on = i == self.sel && matches!(desk::sz(Kind::Stocks), desk::Sz::Columna | desk::Sz::Extragrande);
        if on { unsafe { ui.rt.FillRoundedRectangle(&rrect(r, 10.0), &ui.wa(0.07 * a)); } }
        if live { ui.hit(r, id); if !on { ui.card_k(r, ui.heat(id) * a); } }
        if a < 0.01 { return; }
        let en = i18n::en();
        popup::push_alpha(ui.rt, a);
        // Sin gráfica, el nombre se alarga hasta el precio
        let price = fmt_price(q, en);
        let spark_x = x0 + SPARK_X - PAD;
        let name_w = if self.cfg.chart { SPARK_X - PAD - 12.0 } else { x1 - x0 - ui.p.width(&ui.f.name, &price) - 16.0 };
        ui.text(&ui.f.name, &fit(ui, &ui.f.name, &display_symbol(&q.sym), name_w), x0, y + 3.0, 20.0, &ui.white);
        ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &q.name, name_w), x0, y + 24.0, 18.0, &ui.dim);
        if self.cfg.chart { sparkline(ui, q, spark_x, y + 6.0, SPARK_W.min(x1 - spark_x - 112.0), 32.0, spark, self.cfg.period == Period::Day); }
        ui.text_r(&ui.f.name, &price, x1, y + 3.0, 20.0, &ui.white);
        self.change_r(ui, q, x1, y + 24.0, 18.0);
        unsafe { ui.rt.PopLayer(); }
    }

    /// Ficha (Pequeño, Mediano, Banda): símbolo y variación, nombre si cabe, precio grande y la gráfica.
    fn tile(&self, ui: &mut Ui, d: &DFonts, i: usize, r: D2D_RECT_F, a: f32, spark: f32, live: bool) {
        let Some(q) = self.quotes.get(i) else { return };
        let id = 100 + i as u32;
        let hr = rect(r.left - 10.0, r.top - 6.0, r.right + 10.0, r.bottom + 4.0);
        if live { ui.hit(hr, id); ui.card_k(hr, ui.heat(id) * a); }
        if a < 0.01 { return; }
        let en = i18n::en();
        popup::push_alpha(ui.rt, a);
        let pw = ui.p.width(&ui.f.sub, &self.change(q, en)) + 18.0;
        ui.text(&ui.f.name, &fit(ui, &ui.f.name, &display_symbol(&q.sym), r.right - r.left - pw), r.left, r.top, 20.0, &ui.white);
        self.change_r(ui, q, r.right, r.top + 1.0, 20.0);
        let mut y = r.top + 22.0;
        let with_name = r.bottom - r.top >= 112.0 || !self.cfg.chart;
        if with_name { ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &q.name, r.right - r.left), r.left, y, 18.0, &ui.dim); y += 18.0; }
        let price = fmt_price(q, en);
        let pf = if ui.p.width(&d.big, &price) <= r.right - r.left { &d.big } else { &ui.f.val };
        ui.text(pf, &price, r.left, y, 30.0, &ui.white);
        y += 34.0;
        if self.cfg.chart && r.bottom - y >= 18.0 { sparkline(ui, q, r.left, y, r.right - r.left, r.bottom - y, spark, self.cfg.period == Period::Day); }
        unsafe { ui.rt.PopLayer(); }
    }

    /// Fila estrecha (Alto): símbolo y precio; debajo, una minigráfica corta y la variación.
    fn narrow(&self, ui: &mut Ui, i: usize, x0: f32, x1: f32, y: f32, sh: f32, a: f32, spark: f32, live: bool) {
        let Some(q) = self.quotes.get(i) else { return };
        let id = 100 + i as u32;
        let r = rect(x0 - 10.0, y - 4.0, x1 + 10.0, y + sh - 6.0);
        if live { ui.hit(r, id); ui.card_k(r, ui.heat(id) * a); }
        if a < 0.01 { return; }
        let en = i18n::en();
        popup::push_alpha(ui.rt, a);
        let price = fmt_price(q, en);
        let pw = ui.p.width(&ui.f.name, &price) + 8.0;
        ui.text(&ui.f.name, &fit(ui, &ui.f.name, &display_symbol(&q.sym), x1 - x0 - pw), x0, y + 3.0, 20.0, &ui.white);
        ui.text_r(&ui.f.name, &price, x1, y + 3.0, 20.0, &ui.white);
        let cw = ui.p.width(&ui.f.sub, &self.change(q, en)) + 22.0;
        if self.cfg.chart { sparkline(ui, q, x0, y + 28.0, (x1 - x0 - cw).min(70.0), 14.0, spark, self.cfg.period == Period::Day); }
        else { ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &q.name, x1 - x0 - cw), x0, y + 24.0, 18.0, &ui.dim); }
        self.change_r(ui, q, x1, y + 24.0, 18.0);
        unsafe { ui.rt.PopLayer(); }
    }

    /// Fila de una línea (Grande): símbolo, minigráfica, precio y variación; la elegida, resaltada.
    fn mini(&self, ui: &mut Ui, i: usize, x0: f32, x1: f32, y: f32, sh: f32, a: f32, spark: f32, live: bool) {
        let Some(q) = self.quotes.get(i) else { return };
        let id = 200 + i as u32;
        let r = rect(x0 - 10.0, y, x1 + 10.0, y + sh - 2.0);
        let on = i == self.sel;
        if live { ui.hit(r, id); }
        if on { unsafe { ui.rt.FillRoundedRectangle(&rrect(r, 8.0), &ui.wa(0.07 * a)); } }
        else if live { ui.card_k(r, ui.heat(id) * a); }
        if a < 0.01 { return; }
        let en = i18n::en();
        popup::push_alpha(ui.rt, a);
        let my = y + (sh - 2.0) / 2.0 - 10.0;
        let cx = x1 - 66.0;
        ui.text(&ui.f.name, &fit(ui, &ui.f.name, &display_symbol(&q.sym), 78.0), x0, my, 20.0, &ui.white);
        if self.cfg.chart { sparkline(ui, q, x0 + 86.0, my + 3.0, (cx - 96.0 - (x0 + 86.0)).max(20.0) - 18.0, 14.0, spark, self.cfg.period == Period::Day); }
        ui.text_r(&ui.f.name, &fmt_price(q, en), cx - 8.0, my, 20.0, &ui.white);
        self.change_r(ui, q, x1, my + 1.0, 20.0);
        unsafe { ui.rt.PopLayer(); }
    }

    /// Valor elegido en grande: símbolo y nombre, precio, variación y gráfica de `ch` de alto. Con
    /// `detail`, debajo, apertura, máximo y mínimo, cierre anterior y volumen (si Yahoo los da).
    fn feature(&self, ui: &mut Ui, d: &DFonts, x0: f32, x1: f32, y: f32, ch: f32, k: f32, detail: bool) -> f32 {
        let Some(q) = self.quotes.get(self.sel).or_else(|| self.quotes.first()) else { return y };
        let i = self.quotes.iter().position(|x| x.sym == q.sym).unwrap_or(0);
        let en = i18n::en();
        let id = 100 + i as u32;
        let hot = ui.heat(id);
        let pw = ui.p.width(&ui.f.sub, &self.change(q, en)) + 18.0;
        let sym = display_symbol(&q.sym);
        ui.text(&ui.f.name, &sym, x0, y, 20.0, &ui.white);
        let sx = x0 + ui.p.width(&ui.f.name, &sym) + 8.0;
        if q.name != sym { ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &q.name, x1 - sx - pw), sx, y + 1.0, 20.0, &ui.dim); }
        self.change_r(ui, q, x1, y + 1.0, 20.0);
        let price = fmt_price(q, en);
        let pf = if detail { &d.huge } else { &d.big };
        ui.text(pf, &price, x0, y + 24.0, if detail { 44.0 } else { 30.0 }, &ui.white);
        let y0 = y;
        let mut y = y + if detail { 74.0 } else { 58.0 };
        if self.cfg.chart || detail {
            sparkline(ui, q, x0, y, x1 - x0, ch, k, self.cfg.period == Period::Day);
            y += ch + 12.0;
        }
        // Un clic en el valor grande abre su página
        ui.hit(rect(x0 - 10.0, y0 - 6.0, x1 + 10.0, y - 6.0), id);
        if hot > 0.01 { ui.card_k(rect(x0 - 10.0, y0 - 6.0, x1 + 10.0, y - 6.0), hot * 0.6); }
        if detail {
            // Dos columnas de datos (rótulo en gris y valor): lo que no da Yahoo no sale
            let num = |v: f64| if v > 0.0 { Some(fmt_num(v, if v.abs() < 1.0 { 4 } else { 2 }, en)) } else { None };
            let open_lbl = if self.cfg.period == Period::Day { i18n::t("Apertura") } else { i18n::t("Inicio del periodo") };
            let hl = match (num(q.high), num(q.low)) { (Some(h), Some(l)) => Some(format!("{h} / {l}")), _ => None };
            let wk = match (num(q.wk_high), num(q.wk_low)) { (Some(h), Some(l)) => Some(format!("{h} / {l}")), _ => None };
            let cells: Vec<(&str, String)> = [
                (open_lbl, num(q.open)),
                (i18n::t("Máx. / mín. del día"), hl),
                (i18n::t("Cierre anterior"), num(q.prev)),
                (i18n::t("Volumen"), (q.volume > 0.0).then(|| fmt_volume(q.volume, en))),
                (i18n::t("52 semanas"), wk),
            ].into_iter().filter_map(|(l, v)| v.map(|v| (l, v))).take(4).collect();
            let cw = (x1 - x0 - 20.0) / 2.0;
            for (j, (lbl, val)) in cells.iter().enumerate() {
                let (cx, cy) = (x0 + (j % 2) as f32 * (cw + 20.0), y + (j / 2) as f32 * 40.0);
                ui.text(&ui.f.small, &fit(ui, &ui.f.small, lbl, cw), cx, cy, 15.0, &ui.dim);
                ui.text(&ui.f.name, &fit(ui, &ui.f.name, val, cw), cx, cy + 16.0, 20.0, &ui.white);
            }
            y += cells.len().div_ceil(2) as f32 * 40.0;
        }
        y
    }
}

impl Widget for StocksW {
    fn paint(&mut self, ui: &mut Ui, d: &DFonts, t: f32, dt: f32) {
        let l = desk::lay(Kind::Stocks);
        let (w, h) = (l.w, l.h);
        SEEN.store(now(), Ordering::Relaxed);
        self.sync();
        self.hovering = ui.hover.is_some_and(|id| id >= 100);
        let status = match (self.offline, self.fetched > 0) {
            (true, true) => format!("{} · {}", i18n::t("Sin conexión"), fmt_ago(now() - self.fetched)),
            (true, false) => i18n::t("Sin conexión").to_string(),
            (false, true) => fmt_ago(now() - self.fetched),
            _ => String::new(),
        };
        // Con otro periodo que el día, se dice cuál (la variación es la de ese tramo)
        let status = match (self.cfg.period, status.is_empty()) {
            (Period::Day, _) => status,
            (p, true) => i18n::t(p.label()).to_string(),
            (p, false) => format!("{} · {status}", i18n::t(p.label())),
        };
        // En Pequeño y Alto, solo la hora de los datos (o «Sin conexión»): no cabe más
        let status = if (l.sz == desk::Sz::Pequeno || l.sz == desk::Sz::Alto) && ui.p.width(&ui.f.sub, &status) > w - 2.0 * PAD - 60.0 {
            if self.offline { i18n::t("Sin conexión").to_string() } else { String::new() }
        } else { status };
        header(ui, d, i18n::t("Bolsa"), &status, w);
        ui.hit(rect(0.0, 0.0, w, LIST_TOP - 8.0), 1);
        if self.quotes.is_empty() {
            let msg = if self.offline { i18n::t("Sin conexión") } else { i18n::t("Cargando cotizaciones…") };
            ui.glyph(&d.glyph_l, "\u{E9D2}", w / 2.0 - 20.0, h / 2.0 - 30.0, 40.0, 40.0, &ui.faint);
            ui.text_c(&ui.f.sub, msg, w / 2.0, h / 2.0 + 14.0, 18.0, &ui.dim);
            self.moving = false;
            return;
        }
        let n = self.quotes.len();
        let mut moving = false;
        let (per, look) = stocks_grid(l.sz);
        let pages = self.pages();
        let dots_h = if pages > 1 { 22.0 } else { 6.0 };
        // Zona de la lista (y, en Grande y Extragrande, la gráfica grande del valor elegido)
        let sel_k = self.sel_at.map(|a| popup::ease_out((a.elapsed().as_secs_f32() / 0.7).min(1.0))).unwrap_or_else(|| ez(t, 0.3, 0.9));
        moving |= sel_k < 1.0;
        let (lx0, lx1, ltop, lbottom) = match l.sz {
            desk::Sz::Grande | desk::Sz::Columna => {
                // En Columna, con menos valores que filas, la gráfica se queda con el sitio que sobra
                // (con 4 valores quedaba un hueco de dos filas al pie)
                let ch = if l.sz == desk::Sz::Columna { 62.0 + per.saturating_sub(n) as f32 * ROW } else { 62.0 };
                let y = self.feature(ui, d, PAD, w - PAD, LIST_TOP - 2.0, ch, sel_k, false);
                if l.sz == desk::Sz::Columna { unsafe { ui.rt.FillRectangle(&rect(PAD, y - 6.0, w - PAD, y - 6.0 + 1.0 / ui.s), &ui.wa(0.08)); } }
                (PAD, w - PAD, y + if l.sz == desk::Sz::Columna { 6.0 } else { -4.0 }, h - dots_h)
            }
            desk::Sz::Extragrande => {
                let split = 360.0;
                self.feature(ui, d, split + 2.0 * PAD, w - PAD, LIST_TOP - 2.0, 118.0, sel_k, true);
                unsafe { ui.rt.FillRectangle(&rect(split + PAD, LIST_TOP, split + PAD + 1.0 / ui.s, h - PAD), &ui.wa(0.08)); }
                (PAD, split, LIST_TOP, h - dots_h)
            }
            _ => (PAD, w - PAD, LIST_TOP, h - dots_h),
        };
        // Hueco de cada valor
        let tile_gap = 2.0 * PAD;
        let slot_h = match look { Look::Tile => lbottom - ltop - 8.0, Look::Row => ROW, Look::Narrow | Look::Mini => (lbottom - ltop) / per.min(n).max(1) as f32 };
        let tile_w = (lx1 - lx0 - tile_gap * (per - 1) as f32) / per as f32;
        let place = |s: usize| -> (f32, f32) {
            match look { Look::Tile => (lx0 + s as f32 * (tile_w + tile_gap), ltop), Look::Narrow => (lx0, ltop + s as f32 * slot_h + ((slot_h - 50.0) / 2.0).max(0.0)), _ => (lx0, ltop + s as f32 * slot_h) }
        };
        let draw = |me: &Self, ui: &mut Ui, i: usize, s: usize, dy: f32, a: f32, spark: f32, live: bool| {
            let (x, y) = place(s);
            match look {
                Look::Tile => me.tile(ui, d, i, rect(x, y + dy, x + tile_w, y + dy + slot_h), a, spark, live),
                Look::Row => me.row(ui, i, lx0, lx1, y + dy, a, spark, live),
                Look::Narrow => me.narrow(ui, i, lx0, lx1, y + dy, slot_h, a, spark, live),
                Look::Mini => me.mini(ui, i, lx0, lx1, y + dy, slot_h, a, spark, live),
            }
        };
        unsafe { ui.rt.PushAxisAlignedClip(&rect(0.0, ltop - 8.0, if l.sz == desk::Sz::Extragrande { lx1 + PAD / 2.0 } else { w }, lbottom), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE); }
        let e = self.prev.map(|(_, at)| at.elapsed().as_secs_f32() / SLIDE_S);
        if e.is_some_and(|e| e >= 1.0 + 0.06 * per as f32) { self.prev = None; }
        let stag = |e: f32, r: usize| popup::ease_out((e - 0.06 * r as f32).clamp(0.0, 1.0));
        if let (Some((old, _)), Some(e)) = (self.prev, e) {
            for s in 0..per {
                let k = stag(e * 1.1, s);
                let i = old * per + s;
                if i < n { draw(self, ui, i, s, -20.0 * k, 1.0 - k, 1.0, false); }
            }
            moving = true;
        }
        for s in 0..per {
            let i = self.page * per + s;
            if i >= n { break; }
            let k = match e { Some(e) => stag(e * 0.95 - 0.12, s), None => ez(t, 0.08 + 0.07 * s as f32, 0.5) };
            // La minigráfica se dibuja de izquierda a derecha al entrar
            let spark = match e { Some(_) => k, None => ez(t, 0.25 + 0.08 * s as f32, 0.9) };
            draw(self, ui, i, s, 16.0 * (1.0 - k), k, spark, k > 0.5);
            // Línea fina entre filas (las fichas van sin ella)
            if look == Look::Row && s + 1 < per && i + 1 < n {
                let ly = ltop + (s + 1) as f32 * slot_h - 5.5;
                unsafe { ui.rt.FillRectangle(&rect(lx0, ly, lx1, ly + 1.0 / ui.s), &ui.wa(0.08 * k)); }
            }
            if look == Look::Narrow && s + 1 < per && i + 1 < n {
                let ly = ltop + (s + 1) as f32 * slot_h - 5.0;
                unsafe { ui.rt.FillRectangle(&rect(lx0, ly, lx1, ly + 1.0 / ui.s), &ui.wa(0.08 * k)); }
            }
        }
        unsafe { ui.rt.PopAxisAlignedClip(); }
        moving |= approach(&mut self.dot, self.page as f32, dt, 0.09);
        page_dots(ui, pages, self.dot, (lx0 + lx1) / 2.0, h - 14.0);
        self.moving = moving || t < 1.4;
    }
    fn layouts(&self) -> bool { true }
    fn click(&mut self, id: Option<u32>, right: bool, _pt: POINT) -> Option<Box<dyn FnOnce()>> {
        let id = id?;
        if right { return (id == 1).then(|| Box::new(edit_symbols) as Box<dyn FnOnce()>); }
        match id {
            // Filas de Grande: pasan ese valor a la gráfica grande
            i if i >= 200 => {
                let s = (i - 200) as usize;
                if s < self.quotes.len() && s != self.sel { self.sel = s; self.sel_at = Some(Instant::now()); self.turned = Instant::now(); self.moving = true; }
            }
            i if i >= 100 => {
                let s = (i - 100) as usize;
                // En Columna y Extragrande, la primera vez la fila pasa a la gráfica grande; ya elegida, abre Yahoo
                if matches!(desk::sz(Kind::Stocks), desk::Sz::Columna | desk::Sz::Extragrande) && s != self.sel && s < self.quotes.len() {
                    self.sel = s; self.sel_at = Some(Instant::now()); self.turned = Instant::now(); self.moving = true;
                    return None;
                }
                let url = quote_page(&self.quotes.get(s)?.sym);
                return Some(Box::new(move || sys::open(&url)));
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
        if self.hovering { self.turned = Instant::now(); }
        if self.pages() > 1 && self.turned.elapsed().as_secs_f32() >= TURN_S { self.turn(self.page + 1); r = true; }
        let m = now() / 60;
        if m != self.minute { self.minute = m; r = true; }
        r
    }
    fn animating(&self, t: f32) -> bool { t < 1.5 || self.moving }
    fn smooth(&self) -> bool { self.moving }
    fn frame_ms(&self) -> u32 { 16 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHART: &str = r#"{"chart":{"result":[{"meta":{"currency":"EUR","symbol":"^IBEX","exchangeName":"MCE","instrumentType":"INDEX","regularMarketTime":1795192531,"regularMarketPrice":18873.1,"regularMarketChangePercent":-1.632,"longName":"IBEX 35","shortName":"IBEX 35","chartPreviousClose":19186.2,"previousClose":19186.2,"currentTradingPeriod":{"pre":{"timezone":"CET"}}},"timestamp":[1795161600,1795161900,1795162200],"indicators":{"quote":[{"open":[19000.5,19010.0,null],"close":[19005.25,null,18990.0],"volume":[0,0,0]}]}}],"error":null}}"#;

    #[test]
    fn grafico_de_yahoo() {
        let q = parse_chart(CHART, "^IBEX").unwrap();
        assert_eq!(q.sym, "^IBEX");
        assert_eq!(q.name, "IBEX 35");
        assert_eq!(q.currency, "EUR");
        assert!(q.index);
        assert_eq!(q.price, 18873.1);
        assert_eq!(q.prev, 19186.2);
        assert_eq!(q.time, 1795192531);
        assert_eq!(q.closes, vec![19005.25, 18990.0], "los null se saltan y no se lee «open»");
        assert!((q.change_pct() + 1.6319).abs() < 0.001);
        // Detalle: la apertura es el primer «open»; lo que no trae la respuesta queda a 0 (no se enseña)
        assert_eq!((q.open, q.high, q.low, q.volume), (19000.5, 0.0, 0.0, 0.0));
        let j = r#"{"chart":{"result":[{"meta":{"regularMarketPrice":10,"regularMarketDayHigh":11.5,"regularMarketDayLow":9.25,"regularMarketVolume":28400000,"fiftyTwoWeekHigh":14,"fiftyTwoWeekLow":7}}]}}"#;
        let q = parse_chart(j, "X").unwrap();
        assert_eq!((q.high, q.low, q.volume, q.wk_high, q.wk_low, q.open), (11.5, 9.25, 28_400_000.0, 14.0, 7.0, 0.0));
        // Acción con nombre escapado y sin cierre anterior en la gráfica
        let j = r#"{"chart":{"result":[{"meta":{"currency":"USD","instrumentType":"EQUITY","regularMarketPrice":228.41,"previousClose":227.23,"shortName":"Café \"Co\" INC."},"indicators":{"quote":[{"close":[1.5e2,2.0E2]}],"adjclose":[{"adjclose":[9,9]}]}}],"error":null}}"#;
        let q = parse_chart(j, "CAFE").unwrap();
        assert_eq!(q.name, "Café \"Co\" INC.");
        assert!(!q.index);
        assert_eq!(q.prev, 227.23);
        assert_eq!(q.closes, vec![150.0, 200.0]);
        // Símbolo que no existe
        assert!(parse_chart(r#"{"chart":{"result":null,"error":{"code":"Not Found"}}}"#, "NOPE").is_none());
        assert!(parse_chart("<html>", "X").is_none());
    }

    #[test]
    fn numeros_y_precios() {
        assert_eq!(fmt_num(18873.1, 2, false), "18.873,10");
        assert_eq!(fmt_num(18873.1, 2, true), "18,873.10");
        assert_eq!(fmt_num(1234.5, 2, false), "1234,50");
        assert_eq!(fmt_num(1234.5, 2, true), "1,234.50");
        assert_eq!(fmt_num(1234567.0, 0, false), "1.234.567");
        assert_eq!(fmt_num(0.123456, 4, false), "0,1235");
        assert_eq!(fmt_num(-5.5, 2, false), "\u{2212}5,50");
        assert_eq!(fmt_num(-0.001, 2, false), "0,00");
        let q = |p: f64, cur: &str, index: bool| Quote { sym: "X".into(), currency: cur.into(), index, price: p, prev: p, ..Default::default() };
        assert_eq!(fmt_price(&q(6.92, "EUR", false), false), "6,92 €");
        assert_eq!(fmt_price(&q(228.41, "USD", false), true), "$228.41");
        assert_eq!(fmt_price(&q(228.41, "USD", false), false), "228,41 $");
        assert_eq!(fmt_price(&q(18873.1, "EUR", true), false), "18.873,10");
        assert_eq!(fmt_price(&q(0.5, "XYZ", false), true), "0.5000");
    }

    #[test]
    fn porcentajes() {
        assert_eq!(fmt_pct(0.844, false), "+0,84 %");
        assert_eq!(fmt_pct(-1.116, false), "\u{2212}1,12 %");
        assert_eq!(fmt_pct(2.3, true), "+2.30%");
        assert_eq!(fmt_pct(-0.001, false), "0,00 %");
    }

    #[test]
    fn nombres_y_simbolos() {
        assert_eq!(pretty_name("BANCO SANTANDER, S.A."), "Banco Santander");
        assert_eq!(pretty_name("IBEX 35"), "IBEX 35");
        assert_eq!(pretty_name("Apple Inc."), "Apple");
        assert_eq!(pretty_name("Bitcoin USD"), "Bitcoin USD");
        assert_eq!(pretty_name("IBERDROLA"), "Iberdrola");
        assert_eq!(pretty_name("ÑANDÚ ÁRBOLES 😀"), "Ñandú Árboles 😀");
        assert_eq!(pretty_name("😀 Inc."), "😀");
        assert!(parse_chart(r#"{"chart":{"result":[{"meta":{"regularMarketPrice":1,"shortName":"Ñ😀\u00"}}]}}"#, "X").is_some());
        assert_eq!(display_symbol("^IBEX"), "IBEX");
        assert_eq!(display_symbol("EURUSD=X"), "EURUSD");
        assert_eq!(display_symbol("SAN.MC"), "SAN.MC");
        assert_eq!(symbols_from(None), vec!["^IBEX", "SAN.MC", "AAPL", "BTC-USD"]);
        assert_eq!(symbols_from(Some(" aapl | msft,aapl, ;rm -rf, EURUSD=X ")), vec!["AAPL", "MSFT", "EURUSD=X"]);
        assert_eq!(symbols_from(Some("$$$")), symbols_from(None));
        assert_eq!(chart_url("^IBEX"), "https://query1.finance.yahoo.com/v8/finance/chart/%5EIBEX?range=1d&interval=5m");
        assert_eq!(quote_page("SAN.MC"), "https://finance.yahoo.com/quote/SAN.MC");
    }

    #[test]
    fn configuracion() {
        // Sin nada en widgets.ini: los valores de ejemplo, el día, en %, con gráfica y cada 5 min
        let c = Cfg::default();
        assert_eq!(c.syms, vec!["^IBEX", "SAN.MC", "AAPL", "BTC-USD"]);
        assert_eq!((c.period, c.amount, c.chart, c.refresh_min), (Period::Day, false, true, 5));
        let ini = |pairs: Vec<(&'static str, String)>| move |k: &str| pairs.iter().find(|p| p.0 == k).map(|p| p.1.clone());
        let c2 = Cfg {
            syms: vec!["SAN.MC".into(), "AAPL".into()],
            names: vec![("SAN.MC".into(), "Banco | Santander".into(), "Madrid~CATS".into()), ("MSFT".into(), "Microsoft".into(), "NASDAQ".into())],
            period: Period::Year, amount: true, chart: false, refresh_min: 15,
        };
        let back = cfg_from(ini(cfg_pairs(&c2)));
        assert_eq!(back.syms, c2.syms);
        assert_eq!((back.period, back.amount, back.chart, back.refresh_min), (Period::Year, true, false, 15));
        // Los separadores no rompen el guardado y los nombres de valores quitados no se guardan
        assert_eq!(back.names, vec![("SAN.MC".to_string(), "Banco   Santander".to_string(), "Madrid CATS".to_string())]);
        // Valores raros: lo de por defecto
        let c3 = cfg_from(ini(vec![("bolsa_periodo", "siglo".into()), ("bolsa_refresco", "7".into()), ("bolsa_simbolos", "".into())]));
        assert_eq!((c3.period, c3.refresh_min), (Period::Day, 5));
        assert!(c3.syms.is_empty(), "si se quitaron todos, ninguno");
        // Periodos → range/interval de Yahoo
        assert_eq!(chart_url_for("SAN.MC", Period::Week), "https://query1.finance.yahoo.com/v8/finance/chart/SAN.MC?range=5d&interval=30m");
        assert_eq!(chart_url_for("AAPL", Period::Month), "https://query1.finance.yahoo.com/v8/finance/chart/AAPL?range=1mo&interval=1d");
        assert_eq!(chart_url_for("AAPL", Period::Year), "https://query1.finance.yahoo.com/v8/finance/chart/AAPL?range=1y&interval=1wk");
        for p in Period::ALL { assert_eq!(Period::from_key(p.key()), p); }
        assert_eq!(market_of("SAN.MC"), "Madrid");
        assert_eq!(market_of("^IBEX"), "Índice");
        assert_eq!(market_of("BTC-USD"), "Cripto");
        assert_eq!(market_of("AAPL"), "EE. UU.");
    }

    #[test]
    fn volumen_y_tamanos() {
        assert_eq!(fmt_volume(28_400_000.0, false), "28,4 M");
        assert_eq!(fmt_volume(28_400_000.0, true), "28.4M");
        assert_eq!(fmt_volume(1_234_000_000.0, false), "1,2 mil M");
        assert_eq!(fmt_volume(5_600.0, true), "5.6K");
        assert_eq!(fmt_volume(950.0, false), "950");
        assert_eq!(fmt_volume(250_000_000.0, false), "250 M");
        // Cada tamaño enseña al menos un valor y nunca más de los que caben en la lista
        for s in desk::Sz::ALL { let (n, _) = stocks_grid(s); assert!((1..=MAX_SYMBOLS).contains(&n), "{s:?}"); }
        assert_eq!(stocks_grid(desk::Sz::Pequeno), (1, Look::Tile));
        assert_eq!(stocks_grid(desk::Sz::Grande), (5, Look::Mini));
    }

    #[test]
    fn variacion_en_importe() {
        let q = |p: f64, prev: f64, cur: &str, index: bool| Quote { sym: "X".into(), currency: cur.into(), index, price: p, prev, ..Default::default() };
        assert_eq!(fmt_change(&q(6.92, 6.998, "EUR", false), false), "\u{2212}0,08 €");
        assert_eq!(fmt_change(&q(228.41, 227.23, "USD", false), true), "+$1.18");
        assert_eq!(fmt_change(&q(11842.3, 11744.6, "EUR", true), false), "+97,70");
        assert_eq!(fmt_change(&q(5.0, 5.0, "EUR", false), false), "0,00 €");
    }

    #[test]
    fn cache_de_ida_y_vuelta() {
        let qs = demo_quotes(1_000_000);
        let (f, p, back) = cache_from_text(&cache_to_text(42, Period::Month, &qs)).unwrap();
        assert_eq!((f, p), (42, Period::Month));
        // Las de antes, sin periodo, eran del día
        assert_eq!(cache_from_text("v1\t7\n").map(|c| (c.0, c.1)), Some((7, Period::Day)));
        assert_eq!(back.len(), qs.len());
        for (a, b) in back.iter().zip(&qs) {
            assert_eq!((&a.sym, &a.name, &a.currency, a.index, a.price, a.prev, a.time), (&b.sym, &b.name, &b.currency, b.index, b.price, b.prev, b.time));
            assert_eq!(a.closes, b.closes);
        }
        for (a, b) in back.iter().zip(&qs) { assert_eq!((a.open, a.high, a.low, a.volume, a.wk_high, a.wk_low), (b.open, b.high, b.low, b.volume, b.wk_high, b.wk_low)); }
        // Las de antes (8 campos, sin detalle) siguen valiendo
        let old = cache_from_text("v1\t7\tdia\nAAPL\tApple\tUSD\t0\t228.41\t227.23\t5\t1,2\n").unwrap();
        assert_eq!((old.2[0].price, old.2[0].open, old.2[0].volume), (228.41, 0.0, 0.0));
        // El último punto del día de muestra es el precio
        assert!(qs.iter().all(|q| (*q.closes.last().unwrap() as f64 - q.price).abs() < 0.01 * q.price));
    }
}
