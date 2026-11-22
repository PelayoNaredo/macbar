//! Secciones «Noticias» y «Bolsa» de la ventana de Configuración: todo lo que antes había que escribir a
//! mano en widgets.ini, con campos de texto propios (`textfield.rs`).
//!
//! - Bolsa: lista de valores (se reordena arrastrando o con ↑ ↓, × para quitar), buscador con
//!   sugerencias de Yahoo Finance (en un hilo, 300 ms después de dejar de escribir; la petición solo lleva
//!   lo escrito) y opciones de la gráfica, la variación y el refresco.
//! - Noticias: temas de Google Noticias, país e idioma, fuentes propias (se comprueban al añadirlas),
//!   palabras a silenciar, medios a ocultar y opciones de las páginas.
//!
//! Cada cambio se guarda al momento en widgets.ini (`stocks::set_cfg`, `news::set_cfg`) y el widget lo
//! enseña en el siguiente tic. Es un submódulo de `settings` para usar sus controles (interruptor,
//! selector) tal cual.

use super::*;
use crate::textfield::{Ev, TextField};
use crate::{news, stocks};

/// Ids de las zonas de estas secciones (por encima de los de `settings`).
pub(super) const FID: u32 = 50_000;
pub(super) const END: u32 = FID + 2000;
/// Filas con un control de `settings` (interruptor o selector): FID + hueco × 16 (+ 1.. sus opciones).
const L_PERIOD: u32 = 1;
const L_CHART: u32 = 2;
const L_VAR: u32 = 3;
const L_REFRESH: u32 = 4;
const L_TURN: u32 = 10;
const L_META: u32 = 11;
const L_PER_PAGE: u32 = 12;
fn lid(slot: u32) -> u32 { FID + slot * 16 }
const FIELD0: u32 = FID + 1000;
const SUGG0: u32 = FID + 1100;
/// «Añadir tal cual» (sin sugerencia).
const RAW: u32 = SUGG0 + 99;
const STOCK0: u32 = FID + 1200;
const TOPIC0: u32 = FID + 1500;
const REGION0: u32 = FID + 1530;
const SRC0: u32 = FID + 1600;
const MUTE0: u32 = FID + 1700;
const HIDE0: u32 = FID + 1800;
const NOW0: u32 = FID + 1900;
/// Claves de animación propias (no son zonas pulsables).
const ANIM0: u32 = 900_000;

const STOCK_H: f32 = 46.0;
const SUGGS: usize = 8;
const DEBOUNCE_MS: u64 = 300;
const MAX_WORDS: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Page { Stocks, News }

impl Page {
    pub(super) fn of(s: Sec) -> Option<Page> { match s { Sec::Stocks => Some(Page::Stocks), Sec::News => Some(Page::News), _ => None } }
}

/// Campos de texto (índice en `Feeds::fields`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fid { Search = 0, Src = 1, Mute = 2, Hide = 3 }
const FIELDS: [Fid; 4] = [Fid::Search, Fid::Src, Fid::Mute, Fid::Hide];

impl Fid {
    fn page(self) -> Page { if self == Fid::Search { Page::Stocks } else { Page::News } }
    fn id(self) -> u32 { FIELD0 + self as u32 * 4 }
}

// ================================================================ búsqueda de valores (Yahoo Finance)
/// Una sugerencia del buscador.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Sugg { pub sym: String, pub name: String, pub exch: String, pub kind: String }

pub(crate) fn search_url(q: &str) -> String {
    format!("https://query1.finance.yahoo.com/v1/finance/search?q={}&quotesCount={SUGGS}&newsCount=0", net::encode(q.trim()))
}

/// Objetos `{…}` de primer nivel dentro de un array JSON (`s` empieza tras el `[`).
fn json_objects(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start, mut in_str, mut esc) = (0i32, 0usize, false, false);
    for (i, c) in s.char_indices() {
        if in_str {
            match c { _ if esc => esc = false, '\\' => esc = true, '"' => in_str = false, _ => {} }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => { if depth == 0 { start = i; } depth += 1; }
            '}' => { depth -= 1; if depth == 0 { out.push(&s[start..=i]); } if depth < 0 { break; } }
            ']' if depth == 0 => break,
            _ => {}
        }
    }
    out
}

/// Tipos de valor de Yahoo y su rótulo (en español; se traduce al pintarlo).
const KINDS: [(&str, &str); 8] = [
    ("EQUITY", "Acción"), ("ETF", "ETF"), ("INDEX", "Índice"), ("CRYPTOCURRENCY", "Cripto"), ("CURRENCY", "Divisa"),
    ("MUTUALFUND", "Fondo"), ("FUTURE", "Futuro"), ("OPTION", "Opción"),
];
fn kind_label(qt: &str) -> &'static str { KINDS.iter().find(|k| k.0 == qt).map(|k| k.1).unwrap_or("") }
/// Rótulo del tipo en el idioma de la interfaz (los que no son nuestros, tal cual).
fn kind_t(k: &str) -> String { KINDS.iter().find(|x| x.1 == k).map(|x| i18n::t(x.1).to_string()).unwrap_or_else(|| k.to_string()) }

/// Respuesta de `v1/finance/search` → sugerencias (las de símbolo raro, fuera).
pub(crate) fn parse_search(json: &str) -> Vec<Sugg> {
    let Some(i) = json.find("\"quotes\":") else { return Vec::new() };
    let Some(r) = json[i + 9..].trim_start().strip_prefix('[') else { return Vec::new() };
    json_objects(r).into_iter().filter_map(|o| {
        let sym = stocks::str_after(o, "symbol")?.trim().to_uppercase();
        if !stocks::valid_symbol(&sym) { return None; }
        let name = stocks::str_after(o, "longname").or_else(|| stocks::str_after(o, "shortname")).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| sym.clone());
        let exch = stocks::str_after(o, "exchDisp").or_else(|| stocks::str_after(o, "exchange")).unwrap_or_default();
        let qt = stocks::str_after(o, "quoteType").unwrap_or_default();
        let kind = match kind_label(&qt) { "" => stocks::str_after(o, "typeDisp").unwrap_or_default(), k => k.to_string() };
        Some(Sugg { sym, name: stocks::pretty_name(&name), exch, kind })
    }).take(SUGGS).collect()
}

/// Sugerencias de muestra para el modo demo (sin red).
fn demo_search(q: &str) -> Vec<Sugg> {
    const L: [(&str, &str, &str, &str); 14] = [
        ("SAN.MC", "Banco Santander", "Madrid", "Acción"), ("SAN", "Banco Santander", "NYSE", "Acción"),
        ("BBVA.MC", "BBVA", "Madrid", "Acción"), ("ITX.MC", "Inditex", "Madrid", "Acción"), ("IBE.MC", "Iberdrola", "Madrid", "Acción"),
        ("TEF.MC", "Telefónica", "Madrid", "Acción"), ("AAPL", "Apple", "NASDAQ", "Acción"), ("MSFT", "Microsoft", "NASDAQ", "Acción"),
        ("NVDA", "NVIDIA", "NASDAQ", "Acción"), ("^IBEX", "IBEX 35", "Madrid", "Índice"), ("^GSPC", "S&P 500", "SNP", "Índice"),
        ("BTC-USD", "Bitcoin USD", "CCC", "Cripto"), ("ETH-USD", "Ethereum USD", "CCC", "Cripto"), ("EURUSD=X", "EUR/USD", "CCY", "Divisa"),
    ];
    let f = news::fold(q.trim());
    L.iter().filter(|s| news::fold(s.0).contains(&f) || news::fold(s.1).contains(&f))
        .map(|s| Sugg { sym: s.0.into(), name: s.1.into(), exch: s.2.into(), kind: s.3.into() }).collect()
}

// ================================================================ estado
enum Search { Idle, Waiting, Busy, Done(Vec<Sugg>), Failed }
enum Check { Idle, Busy, Ok(String), Err(String) }

/// Lo que traen los hilos (búsqueda y comprobación de feeds), con su número de petición.
#[derive(Default)]
struct Inbox {
    search: Option<(u64, Option<Vec<Sugg>>)>,
    check: Option<(u64, Result<(usize, String, String), String>)>,
}
static INBOX: Mutex<Inbox> = Mutex::new(Inbox { search: None, check: None });

/// Arrastre de una fila de la bolsa: cuál, dónde se cogió y dónde va el ratón.
struct Drag { from: usize, y0: f32, y: f32, live: bool }

/// Hueco en el que cae la fila arrastrada (todas miden lo mismo: basta con lo que se movió).
fn drag_to(d: &Drag, n: usize) -> usize {
    let pos = (d.from as f32 * STOCK_H + d.y - d.y0) / STOCK_H;
    (pos.round().max(0.0) as usize).min(n.saturating_sub(1))
}

pub(super) struct Feeds {
    fields: [TextField; 4],
    focus: Option<Fid>,
    st: stocks::Cfg,
    nw: news::Cfg,
    search: Search,
    req: u64,
    due: Option<Instant>,
    /// Sugerencia resaltada con ↑ ↓.
    sel: usize,
    check: Check,
    check_gen: u64,
    drag: Option<Drag>,
    /// Arriba de la primera fila de la lista de la bolsa en el último fotograma (todas miden STOCK_H).
    list_top: Option<f32>,
    swallow: bool,
    /// Medios de los titulares de ahora (para ofrecer ocultarlos).
    now_src: Vec<String>,
}

impl Feeds {
    pub(super) fn new() -> Self {
        Feeds {
            fields: [
                TextField::new("Buscar: Santander, Apple, Bitcoin…", 60),
                TextField::new("Pega la URL de un RSS o Atom", 300),
                TextField::new("Palabra o frase", 60),
                TextField::new("Nombre del medio", 60),
            ],
            focus: None, st: stocks::Cfg::default(), nw: news::Cfg::default(), search: Search::Idle, req: 0, due: None, sel: 0,
            check: Check::Idle, check_gen: 0, drag: None, list_top: None, swallow: false, now_src: Vec::new(),
        }
    }
    pub(super) fn reload(&mut self) {
        stocks::warm();
        news::warm();
        self.st = stocks::cfg();
        self.nw = news::cfg();
        self.now_src = news::current_sources();
        self.drag = None;
        self.blur();
    }
    fn field(&mut self, f: Fid) -> &mut TextField { &mut self.fields[f as usize] }
    pub(super) fn focused(&self) -> bool { self.focus.is_some() }
    pub(super) fn blur(&mut self) {
        for f in self.fields.iter_mut() { f.blur(); }
        self.focus = None;
    }
    fn focus_on(&mut self, f: Fid) {
        self.blur();
        self.fields[f as usize].focus();
        self.focus = Some(f);
    }
    /// El clic que llega al soltar un arrastre no cuenta (podría caer en el × de otra fila).
    pub(super) fn swallow_click(&mut self) -> bool { std::mem::take(&mut self.swallow) }
    pub(super) fn wake_ms(&self) -> Option<u32> {
        let blink = self.focus.and_then(|f| self.fields[f as usize].wake_ms());
        let due = self.due.map(|d| d.saturating_duration_since(Instant::now()).as_millis() as u32 + 5);
        match (blink, due) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) }
    }
    pub(super) fn caret(&self) -> Option<(f32, f32, f32)> { self.focus.and_then(|f| self.fields[f as usize].caret_point()) }

    fn suggestions(&self) -> &[Sugg] { if let Search::Done(v) = &self.search { v.as_slice() } else { &[] } }
    /// Lo escrito, si vale como símbolo para añadirlo tal cual.
    fn raw_symbol(&self) -> Option<String> {
        let s = self.fields[0].text().trim().to_uppercase();
        stocks::valid_symbol(&s).then_some(s)
    }

    // ------------------------------------------------------------ bolsa
    fn save_stocks(&mut self) { stocks::set_cfg(self.st.clone()); }
    fn add_stock(&mut self, sym: &str, name: &str, market: &str) {
        let sym = sym.trim().to_uppercase();
        if !stocks::valid_symbol(&sym) || self.st.syms.contains(&sym) || self.st.syms.len() >= stocks::MAX_SYMBOLS { return; }
        self.st.syms.push(sym.clone());
        self.st.names.retain(|n| n.0 != sym);
        if !name.is_empty() || !market.is_empty() { self.st.names.push((sym, name.to_string(), market.to_string())); }
        self.save_stocks();
        // Se vacía el buscador para el siguiente
        self.fields[0].set_text("");
        self.search = Search::Idle;
        self.due = None;
        self.sel = 0;
    }
    fn add_suggestion(&mut self, i: usize) {
        let Some(s) = self.suggestions().get(i).cloned() else { return };
        self.add_stock(&s.sym, &s.name, &s.exch);
    }
    fn move_stock(&mut self, from: usize, to: usize) {
        let n = self.st.syms.len();
        if from >= n || to >= n || from == to { return; }
        let s = self.st.syms.remove(from);
        self.st.syms.insert(to, s);
        self.save_stocks();
    }
    fn remove_stock(&mut self, i: usize) {
        if i >= self.st.syms.len() { return; }
        self.st.syms.remove(i);
        self.save_stocks();
    }
    /// Nombre y mercado de un valor de la lista.
    fn stock_info(&self, sym: &str) -> (String, String) {
        let known = self.st.names.iter().find(|n| n.0 == sym);
        let name = known.map(|n| n.1.clone()).filter(|n| !n.is_empty()).or_else(|| stocks::known_name(sym)).unwrap_or_default();
        let market = known.map(|n| n.2.clone()).filter(|m| !m.is_empty()).unwrap_or_else(|| i18n::t(stocks::market_of(sym)).to_string());
        (name, market)
    }

    // ------------------------------------------------------------ noticias
    fn save_news(&mut self) { news::set_cfg(self.nw.clone()); }
    fn add_word(&mut self, f: Fid) {
        let w = self.fields[f as usize].text().trim().to_string();
        if w.is_empty() { return; }
        let list = if f == Fid::Mute { &mut self.nw.mute } else { &mut self.nw.hide };
        if !list.iter().any(|x| news::fold(x) == news::fold(&w)) && list.len() < MAX_WORDS { list.push(w); }
        self.field(f).set_text("");
        self.save_news();
    }
    /// Comprueba la URL escrita en un hilo; si funciona, se añade (ver `data`).
    fn start_check(&mut self) {
        let mut url = self.fields[1].text().trim().to_string();
        if url.is_empty() { return; }
        // «elpais.com/rss» también vale
        if !url.contains("://") { url = format!("https://{url}"); }
        if self.nw.feeds.iter().any(|f| f.eq_ignore_ascii_case(&url)) { self.check = Check::Err(i18n::t("Ya está en la lista").into()); return; }
        if self.nw.feeds.len() >= news::MAX_FEEDS { self.check = Check::Err(i18n::tf("Como mucho {} fuentes propias", &[&news::MAX_FEEDS])); return; }
        self.check_gen += 1;
        let g = self.check_gen;
        self.check = Check::Busy;
        std::thread::spawn(move || {
            let r = news::check_feed(&url);
            INBOX.lock().unwrap().check = Some((g, r));
            popup::notify(popup::Kind::Settings);
        });
    }
    fn remove_feed(&mut self, i: usize) {
        if i >= self.nw.feeds.len() { return; }
        self.nw.feeds.remove(i);
        self.save_news();
    }
    fn feed_name(&self, url: &str) -> String {
        self.nw.names.iter().find(|n| n.0 == url).map(|n| n.1.clone()).filter(|n| !n.is_empty())
            .unwrap_or_else(|| net::split_url(url).map(|p| p.1.trim_start_matches("www.").to_string()).unwrap_or_else(|| url.to_string()))
    }
}

// ================================================================ filas
/// Clase de pastilla: casilla (varias), opción (una), quitar (×) o añadir (+).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ChipK { Check, Radio, Remove, Add }
struct Chip { label: String, on: bool, id: u32, k: ChipK }

enum R {
    /// Etiqueta y descripción con un control de `settings` (id0 = la fila; sus opciones, id0 + 1..).
    Line(String, String, Ctl, u32),
    Chips(Vec<Chip>),
    /// Campo de texto con su botón (glifo, texto) opcional.
    Field(Fid, Option<(&'static str, String)>),
    /// Texto suelto (con un glifo; `accent` = el glifo en el color de acento).
    Note(String, Option<&'static str>, bool),
    Sugg(usize),
    Raw(String),
    /// Hueco `k` de la lista de la bolsa con el valor `i` (None = el que se arrastra).
    Stock(usize, Option<usize>),
    Source(usize),
}

const CHIP_H: f32 = 26.0;
const CHIP_GAP: f32 = 6.0;

fn chip_w(ui: &Ui, c: &Chip) -> f32 {
    let extra = match c.k { ChipK::Check if c.on => 18.0, ChipK::Remove | ChipK::Add => 16.0, _ => 0.0 };
    ui.p.width(&ui.f.lbl, &c.label) + 24.0 + extra
}

/// Posición de cada pastilla en filas que se parten al no caber: (x, y) desde arriba a la izquierda.
fn chip_layout(ui: &Ui, chips: &[Chip], avail: f32) -> (Vec<(f32, f32, f32)>, f32) {
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut v = Vec::new();
    for c in chips {
        let w = chip_w(ui, c).min(avail);
        if x > 0.0 && x + w > avail { x = 0.0; y += CHIP_H + CHIP_GAP; }
        v.push((x, y, w));
        x += w + CHIP_GAP;
    }
    (v, if chips.is_empty() { 0.0 } else { y + CHIP_H })
}

impl Settings {
    /// Pinta una de estas secciones desde `y`; devuelve dónde acaba.
    pub(super) fn feeds_page(&mut self, ui: &mut Ui, page: Page, y: f32, live: bool, dt: f32) -> f32 {
        let groups = match page { Page::Stocks => self.stocks_groups(), Page::News => self.news_groups() };
        if live { self.feeds.list_top = None; }
        let y = self.render(ui, groups, y, live, dt);
        // La fila que se arrastra, por encima de todo y donde va el ratón
        if live {
            if let (Some(d), Some(t0)) = (self.feeds.drag.as_ref().filter(|d| d.live), self.feeds.list_top) {
                let (from, dy) = (d.from, d.y - d.y0);
                let top = t0 + from as f32 * STOCK_H + dy;
                let r = rect(X0 + 2.0, top + 1.0, X1 - 2.0, top + STOCK_H - 1.0);
                let v = if ui.ink > 0.5 { 0.97 } else { 0.22 };
                unsafe {
                    ui.rt.FillRoundedRectangle(&rrect(rect(r.left, r.top + 2.0, r.right, r.bottom + 3.0), 9.0), &ui.brush(rgba(0.0, 0.0, 0.0, 0.18)));
                    ui.rt.FillRoundedRectangle(&rrect(r, 9.0), &ui.brush(rgba(v, v, v + 0.01, 0.97)));
                    ui.rt.FillRoundedRectangle(&rrect(r, 9.0), &ui.wa(0.06));
                }
                // Al soltarla, se desliza desde aquí a su hueco
                self.anim.insert(ANIM0 + from as u32, top);
                self.stock_row(ui, from, top, STOCK_H, false, true);
            }
        }
        y
    }

    fn stocks_groups(&self) -> Vec<(Option<String>, Vec<R>)> {
        let f = &self.feeds;
        let t = |s: &'static str| i18n::t(s).to_string();
        let mut add = vec![R::Field(Fid::Search, None)];
        let has_text = !f.fields[0].text().trim().is_empty();
        if has_text {
            match &f.search {
                Search::Waiting | Search::Busy => add.push(R::Note(t("Buscando…"), None, false)),
                Search::Done(v) if v.is_empty() => {
                    add.push(R::Note(t("Sin resultados en Yahoo Finance"), None, false));
                    if let Some(s) = f.raw_symbol() { add.push(R::Raw(s)); }
                }
                Search::Done(v) => for i in 0..v.len() { add.push(R::Sugg(i)); },
                Search::Failed => {
                    add.push(R::Note(t("Sin conexión con Yahoo Finance"), Some("\u{E7BA}"), false));
                    if let Some(s) = f.raw_symbol() { add.push(R::Raw(s)); }
                }
                Search::Idle => {}
            }
        }
        let n = f.st.syms.len();
        if n >= stocks::MAX_SYMBOLS { add.push(R::Note(i18n::tf("Como mucho {} valores: quita alguno para añadir otro", &[&stocks::MAX_SYMBOLS]), None, false)); }
        let mut list: Vec<R> = Vec::new();
        if n == 0 { list.push(R::Note(t("Aún no hay valores: búscalos arriba"), None, false)); }
        // Mientras se arrastra, las demás filas dejan el hueco donde caería
        let order: Vec<Option<usize>> = match f.drag.as_ref().filter(|d| d.live) {
            Some(d) => {
                let to = self.drag_target();
                let mut o: Vec<Option<usize>> = (0..n).filter(|&i| i != d.from).map(Some).collect();
                o.insert(to.min(o.len()), None);
                o
            }
            None => (0..n).map(Some).collect(),
        };
        for (k, i) in order.into_iter().enumerate() { list.push(R::Stock(k, i)); }
        let seg = |v: &[&'static str], sel: usize| Ctl::Seg(v.iter().map(|s| i18n::t(s).to_string()).collect(), Some(sel));
        let pi = stocks::Period::ALL.iter().position(|p| *p == f.st.period).unwrap_or(0);
        let ri = stocks::REFRESH_OPTS.iter().position(|m| *m == f.st.refresh_min).unwrap_or(1);
        vec![
            (Some(t("Añadir un valor")), add),
            (Some(i18n::tf("Tus valores ({})", &[&n])), list),
            (Some(t("Gráfica")), vec![
                R::Line(t("Periodo"), t("También el de la variación"), seg(&stocks::Period::ALL.map(|p| p.label()), pi), lid(L_PERIOD)),
                R::Line(t("Mostrar la gráfica"), String::new(), Ctl::Switch(f.st.chart), lid(L_CHART)),
            ]),
            (Some(t("Opciones")), vec![
                R::Line(t("Variación"), String::new(), seg(&["En %", "En importe"], f.st.amount as usize), lid(L_VAR)),
                R::Line(t("Actualizar cada"), String::new(), seg(&["1 min", "5 min", "15 min"], ri), lid(L_REFRESH)),
            ]),
        ]
    }

    fn news_groups(&self) -> Vec<(Option<String>, Vec<R>)> {
        let f = &self.feeds;
        let t = |s: &'static str| i18n::t(s).to_string();
        let region = news::REGIONS.iter().find(|r| r.0 == f.nw.region).unwrap_or(&news::REGIONS[0]);
        let topics: Vec<Chip> = news::TOPICS.iter().enumerate().map(|(i, tp)| Chip {
            // «Nacional» es el país elegido
            label: if tp.0 == "nacional" { t(region.1) } else { t(tp.1) },
            on: f.nw.topics.iter().any(|k| k == tp.0), id: TOPIC0 + i as u32, k: ChipK::Check,
        }).collect();
        let regions: Vec<Chip> = news::REGIONS.iter().enumerate().map(|(i, r)| Chip { label: t(r.1), on: r.0 == f.nw.region, id: REGION0 + i as u32, k: ChipK::Radio }).collect();
        let mut topic_rows = vec![R::Chips(topics)];
        if f.nw.topics.is_empty() && f.nw.feeds.is_empty() { topic_rows.push(R::Note(t("Sin temas ni fuentes propias se ve la portada"), None, false)); }

        let mut src = vec![R::Field(Fid::Src, Some(("\u{E710}", t("Añadir"))))];
        match &f.check {
            Check::Busy => src.push(R::Note(t("Comprobando…"), None, false)),
            Check::Ok(m) => src.push(R::Note(m.clone(), Some("\u{E73E}"), true)),
            Check::Err(e) => src.push(R::Note(e.clone(), Some("\u{E7BA}"), false)),
            Check::Idle => {}
        }
        for i in 0..f.nw.feeds.len() { src.push(R::Source(i)); }

        let words = |l: &[String], id0: u32| R::Chips(l.iter().enumerate().map(|(i, w)| Chip { label: w.clone(), on: true, id: id0 + i as u32, k: ChipK::Remove }).collect());
        let mut mute = vec![R::Note(t("Los titulares con estas palabras no salen (da igual mayúsculas o tildes)"), None, false), R::Field(Fid::Mute, Some(("\u{E710}", t("Añadir"))))];
        if !f.nw.mute.is_empty() { mute.push(words(&f.nw.mute, MUTE0)); }
        let mut hide = vec![R::Field(Fid::Hide, Some(("\u{E710}", t("Añadir"))))];
        if !f.nw.hide.is_empty() { hide.push(words(&f.nw.hide, HIDE0)); }
        // Los medios que salen ahora, para ocultarlos de un toque
        let now: Vec<Chip> = f.now_src.iter().filter(|s| !f.nw.hide.iter().any(|h| news::matches_term(s, h))).take(10).enumerate()
            .map(|(i, s)| Chip { label: s.clone(), on: false, id: NOW0 + i as u32, k: ChipK::Add }).collect();
        if !now.is_empty() { hide.push(R::Note(t("Salen ahora (toca para ocultar):"), None, false)); hide.push(R::Chips(now)); }

        let seg = |v: &[&'static str], sel: usize| Ctl::Seg(v.iter().map(|s| i18n::t(s).to_string()).collect(), Some(sel));
        let ti = news::TURN_OPTS.iter().position(|v| *v == f.nw.turn).unwrap_or(2);
        let pp = f.nw.per_page.clamp(2, news::ROWS_MAX) - 2;
        vec![
            (Some(t("Temas de Google Noticias")), topic_rows),
            (Some(t("País e idioma")), vec![R::Chips(regions)]),
            (Some(t("Fuentes propias")), src),
            (Some(t("Palabras a silenciar")), mute),
            (Some(t("Medios a ocultar")), hide),
            (Some(t("Opciones")), vec![
                R::Line(t("Pasar de página"), String::new(), seg(&["No", "8 s", "11 s", "20 s"], ti), lid(L_TURN)),
                R::Line(t("Fuente y hora"), t("Encima de cada titular"), Ctl::Switch(f.nw.meta), lid(L_META)),
                R::Line(t("Titulares por página"), t("Los que caben en el widget, como mucho"), seg(&["2", "3", "4"], pp), lid(L_PER_PAGE)),
            ]),
        ]
    }

    /// Hueco al que iría la fila que se arrastra.
    fn drag_target(&self) -> usize {
        self.feeds.drag.as_ref().map(|d| drag_to(d, self.feeds.st.syms.len())).unwrap_or(0)
    }

    fn row_h(&self, ui: &Ui, r: &R) -> f32 {
        let avail = X1 - X0 - 2.0 * IN;
        match r {
            R::Line(label, desc, ctl, _) => {
                let cw = self.ctl_w(ui, ctl);
                let lw = ui.p.width(&ui.f.lbl, label);
                let stacked = lw + 16.0 + cw > avail || (!desc.is_empty() && cw + 16.0 + 150.0 > avail);
                let base = if desc.is_empty() { 40.0 } else { 52.0 };
                if stacked { base + 34.0 } else { base }
            }
            R::Chips(c) => chip_layout(ui, c, avail).1 + 20.0,
            R::Field(..) => 46.0,
            R::Note(..) => 30.0,
            R::Sugg(_) | R::Raw(_) => 42.0,
            R::Stock(..) => STOCK_H,
            R::Source(_) => 46.0,
        }
    }

    fn render(&mut self, ui: &mut Ui, groups: Vec<(Option<String>, Vec<R>)>, mut y: f32, live: bool, dt: f32) -> f32 {
        let px = 1.0 / ui.s;
        for (cap, rows) in groups {
            if rows.is_empty() { continue; }
            if let Some(c) = cap {
                ui.text(&ui.f.small, &c, X0 + 4.0, y, 20.0, &ui.faint);
                y += 22.0;
            }
            let hs: Vec<f32> = rows.iter().map(|r| self.row_h(ui, r)).collect();
            let gh: f32 = hs.iter().sum();
            unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(X0, y, X1, y + gh), 10.0), &ui.wa(0.05)); }
            let mut ry = y;
            let mut prev_field = false;
            let mut prev_note = false;
            for (j, (r, h)) in rows.into_iter().zip(hs).enumerate() {
                // Sin raya delante de las notas ni de las pastillas que van con su campo
                let joined = matches!(r, R::Note(..)) || ((prev_field || prev_note) && matches!(r, R::Chips(_)));
                if j > 0 && !joined { unsafe { ui.rt.FillRectangle(&rect(X0 + IN, ry, X1 - IN, ry + px), &ui.card_edge); } }
                prev_field = matches!(r, R::Field(..));
                prev_note = matches!(r, R::Note(..));
                let visible = ry + h > TOP - 6.0 && ry < H;
                self.row_paint(ui, r, ry, h, live && visible, dt);
                ry += h;
            }
            y += gh + 16.0;
        }
        y
    }

    fn row_paint(&mut self, ui: &mut Ui, r: R, y: f32, h: f32, hits: bool, dt: f32) {
        let (l, rr) = (X0 + IN, X1 - IN);
        match r {
            R::Line(label, desc, ctl, id0) => {
                let heat = if hits { (0..16).map(|s| ui.heat(id0 + s)).fold(0.0f32, f32::max) } else { 0.0 };
                if heat > 0.01 { unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(X0 + 4.0, y + 3.0, X1 - 4.0, y + h - 3.0), 8.0), &ui.wa(0.045 * heat)); } }
                let cw = self.ctl_w(ui, &ctl);
                let base = if desc.is_empty() { 40.0 } else { 52.0 };
                let stacked = h > base + 1.0;
                let tw = if stacked { rr - l } else { rr - l - cw - 16.0 };
                if desc.is_empty() {
                    ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &label, tw), l, y, base, &ui.white);
                } else {
                    ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &label, tw), l, y + 8.0, 18.0, &ui.white);
                    ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &desc, tw), l, y + 27.0, 15.0, &ui.dim);
                }
                let (cx, cy) = if stacked { (l, y + base + 13.0) } else { (rr - cw, y + base / 2.0) };
                if hits { ui.hit(rect(X0, y.max(TOP - 6.0), X1, y + h), id0); }
                self.ctl(ui, &ctl, cx, cy, cw, id0, hits, dt);
            }
            R::Chips(chips) => {
                let (pos, _) = chip_layout(ui, &chips, rr - l);
                for (c, (cx, cy, w)) in chips.iter().zip(pos) {
                    let r = rect(l + cx, y + 10.0 + cy, l + cx + w, y + 10.0 + cy + CHIP_H);
                    self.chip(ui, c, r, hits);
                }
            }
            R::Field(fid, button) => {
                let bw = button.as_ref().map(|b| btn_w(ui, &b.1) + 8.0).unwrap_or(0.0);
                let cy = y + h / 2.0;
                let fr = rect(l, cy - 14.0, rr - bw, cy + 14.0);
                let id = fid.id();
                let glyph = (fid == Fid::Search).then_some("\u{E721}");
                let font = ui.f.lbl.clone();
                self.feeds.field(fid).paint(ui, fr, &font, glyph, id);
                if !hits { ui.hits.retain(|h| h.1 != id); }
                if let Some((g, label)) = button {
                    let br = rect(rr - bw + 8.0, cy - CTL_H / 2.0, rr, cy + CTL_H / 2.0);
                    btn(ui, br, g, &label, ui.heat(id + 1));
                    if hits { ui.hit(br, id + 1); }
                }
            }
            R::Note(text, glyph, accent) => {
                let mut x = l;
                if let Some(g) = glyph {
                    let b = if accent { ui.on.clone() } else { ui.dim.clone() };
                    ui.glyph(&ui.f.icon, g, l - 2.0, y, 18.0, h, &b);
                    x += 20.0;
                }
                ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &text, rr - x), x, y, h, &ui.dim);
            }
            R::Sugg(i) => {
                let Some(s) = self.feeds.suggestions().get(i).cloned() else { return };
                let id = SUGG0 + i as u32;
                let added = self.feeds.st.syms.contains(&s.sym);
                let k = ui.heat(id).max(if self.feeds.sel == i && self.feeds.focus == Some(Fid::Search) { 1.0 } else { 0.0 });
                if k > 0.01 && !added { unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(X0 + 4.0, y + 3.0, X1 - 4.0, y + h - 3.0), 8.0), &ui.wa(0.06 * k)); } }
                let right = if added { i18n::t("Añadido") } else { "" };
                let rw = if added { ui.p.width(&ui.f.sub, right) + 8.0 } else { 26.0 };
                let meta: Vec<String> = [stocks::display_symbol(&s.sym), s.exch.clone(), kind_t(&s.kind)]
                    .into_iter().filter(|x| !x.is_empty()).collect();
                let tw = rr - l - rw - 8.0;
                if added { popup::push_alpha(ui.rt, 0.5); }
                ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &s.name, tw), l, y + 5.0, 18.0, &ui.white);
                ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &meta.join(" · "), tw), l, y + 22.0, 15.0, &ui.dim);
                if added {
                    unsafe { ui.rt.PopLayer(); }
                    ui.text_r(&ui.f.sub, right, rr, y, h, &ui.faint);
                } else {
                    ui.glyph(&ui.f.icon, "\u{E710}", rr - 20.0, y, 20.0, h, &ui.wa(0.55 + 0.4 * k));
                    if hits { ui.hit(rect(X0, y, X1, y + h), id); }
                }
            }
            R::Raw(sym) => {
                let k = ui.heat(RAW);
                if k > 0.01 { unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(X0 + 4.0, y + 3.0, X1 - 4.0, y + h - 3.0), 8.0), &ui.wa(0.06 * k)); } }
                ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &i18n::tf("Añadir «{}» tal cual", &[&sym]), rr - l - 30.0), l, y, h, &ui.white);
                ui.glyph(&ui.f.icon, "\u{E710}", rr - 20.0, y, 20.0, h, &ui.wa(0.55 + 0.4 * k));
                if hits { ui.hit(rect(X0, y, X1, y + h), RAW); }
            }
            R::Stock(k, i) => {
                if hits && k == 0 { self.feeds.list_top = Some(y); }
                // El hueco de la que se arrastra se queda vacío (ella se pinta encima, al final)
                let Some(i) = i else { return };
                // Con el arrastre en marcha, cada fila se desliza a su hueco nuevo
                let ty = y;
                let ay = if self.feeds.drag.as_ref().is_some_and(|d| d.live) || self.anim.contains_key(&(ANIM0 + i as u32)) {
                    let v = self.ease(ANIM0 + i as u32, ty, dt);
                    if (v - ty).abs() < 0.3 { self.anim.remove(&(ANIM0 + i as u32)); }
                    v
                } else { ty };
                self.stock_row(ui, i, ay, h, hits && self.feeds.drag.is_none(), false);
            }
            R::Source(i) => {
                let Some(url): Option<String> = self.feeds.nw.feeds.get(i).cloned() else { return };
                let id = SRC0 + i as u32 * 2;
                let k = ui.heat(id).max(ui.heat(id + 1));
                if k > 0.01 { unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(X0 + 4.0, y + 3.0, X1 - 4.0, y + h - 3.0), 8.0), &ui.wa(0.045 * k)); } }
                let name = self.feeds.feed_name(&url);
                ui.glyph(&ui.f.icon, "\u{E8A1}", l - 2.0, y, 18.0, h, &ui.dim);
                let tw = rr - l - 22.0 - 30.0;
                ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &name, tw), l + 22.0, y + 7.0, 18.0, &ui.white);
                ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &url, tw), l + 22.0, y + 24.0, 15.0, &ui.dim);
                if hits { ui.hit(rect(X0, y, X1, y + h), id); }
                self.remove_btn(ui, rr - 12.0, y + h / 2.0, id + 1, k, hits);
            }
        }
    }

    /// × redondo que aparece al pasar el ratón por la fila (centrado en cx, cy).
    fn remove_btn(&self, ui: &mut Ui, cx: f32, cy: f32, id: u32, row_k: f32, hits: bool) {
        let a = row_k.max(0.0);
        if a > 0.01 {
            let kk = ui.heat(id);
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: 11.0, radiusY: 11.0 };
            unsafe { ui.rt.FillEllipse(&e, &ui.wa((0.06 + 0.10 * kk) * a)); }
            ui.glyph(&ui.f.small, "\u{2715}", cx - 11.0, cy - 11.0, 22.0, 22.0, &ui.wa((0.55 + 0.4 * kk) * a));
        }
        if hits { ui.hit(rect(cx - 12.0, cy - 12.0, cx + 12.0, cy + 12.0), id); }
    }

    /// Una fila de la lista de la bolsa: asa, símbolo y nombre, mercado; al pasar el ratón, ↑ ↓ ×.
    fn stock_row(&self, ui: &mut Ui, i: usize, y: f32, h: f32, hits: bool, lifted: bool) {
        let Some(sym): Option<String> = self.feeds.st.syms.get(i).cloned() else { return };
        let (l, rr) = (X0 + IN, X1 - IN);
        let id = STOCK0 + i as u32 * 4;
        let k = if lifted { 1.0 } else { (0..4).map(|s| ui.heat(id + s)).fold(0.0f32, f32::max) };
        if k > 0.01 && !lifted { unsafe { ui.rt.FillRoundedRectangle(&rrect(rect(X0 + 4.0, y + 3.0, X1 - 4.0, y + h - 3.0), 8.0), &ui.wa(0.045 * k)); } }
        // Asa (de donde se coge para arrastrar; vale toda la fila)
        ui.glyph(&ui.f.icon, "\u{E700}", l - 4.0, y, 20.0, h, &ui.wa(0.28 + 0.3 * k));
        let (name, market) = self.feeds.stock_info(&sym);
        let x = l + 24.0;
        let buttons = 3.0 * 26.0;
        let mw = ui.p.width(&ui.f.sub, &market).min(130.0);
        let tw = rr - x - mw.max(buttons) - 12.0;
        ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &stocks::display_symbol(&sym), tw), x, y + 7.0, 18.0, &ui.white);
        if !name.is_empty() { ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &name, tw), x, y + 24.0, 15.0, &ui.dim); }
        let show_btns = k > 0.5 && !lifted;
        if hits { ui.hit(rect(X0, y, X1, y + h), id); }
        if !show_btns {
            ui.text_r(&ui.f.sub, &fit(ui, &ui.f.sub, &market, 130.0), rr, y, h, &ui.dim);
        }
        if show_btns || hits {
            let n = self.feeds.st.syms.len();
            let cy = y + h / 2.0;
            for (j, (g, en)) in [("\u{E70E}", i > 0), ("\u{E70D}", i + 1 < n)].into_iter().enumerate() {
                let cx = rr - 12.0 - (2 - j) as f32 * 26.0;
                let bid = id + 1 + j as u32;
                if show_btns {
                    let kk = ui.heat(bid);
                    let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: 11.0, radiusY: 11.0 };
                    if en { unsafe { ui.rt.FillEllipse(&e, &ui.wa(0.06 + 0.10 * kk)); } }
                    ui.glyph(&ui.f.icon, g, cx - 11.0, cy - 11.0, 22.0, 22.0, &ui.wa(if en { 0.55 + 0.4 * kk } else { 0.18 }));
                }
                if hits && en { ui.hit(rect(cx - 12.0, cy - 12.0, cx + 12.0, cy + 12.0), bid); }
            }
            self.remove_btn(ui, rr - 12.0, cy, id + 3, if show_btns { 1.0 } else { 0.0 }, hits);
        }
    }

    /// Una pastilla (tema, país, palabra…).
    fn chip(&self, ui: &mut Ui, c: &Chip, r: D2D_RECT_F, hits: bool) {
        let k = ui.heat(c.id);
        let rad = CHIP_H / 2.0;
        let filled = c.on && matches!(c.k, ChipK::Check | ChipK::Radio);
        unsafe {
            if filled {
                // Como la opción elegida de un selector
                let fill = if ui.ink > 0.5 { rgba(1.0, 1.0, 1.0, 1.0) } else { rgba(1.0, 1.0, 1.0, 0.17 + 0.04 * k) };
                if ui.ink > 0.5 { ui.rt.FillRoundedRectangle(&rrect(rect(r.left, r.top + 0.6, r.right, r.bottom + 0.6), rad), &ui.brush(rgba(0.0, 0.0, 0.0, 0.10))); }
                ui.rt.FillRoundedRectangle(&rrect(r, rad), &ui.brush(fill));
            } else {
                ui.rt.FillRoundedRectangle(&rrect(r, rad), &ui.wa(0.07 + 0.06 * k));
            }
        }
        let mut x = r.left + 12.0;
        let text_b = if filled || c.k == ChipK::Remove { ui.white.clone() } else { ui.dim.clone() };
        match c.k {
            ChipK::Check if c.on => { ui.glyph(&ui.f.icon, "\u{E73E}", x - 2.0, r.top, 14.0, CHIP_H, &ui.on); x += 16.0; }
            ChipK::Add => { ui.glyph(&ui.f.lbl, "+", x - 2.0, r.top, 12.0, CHIP_H, &ui.dim); x += 14.0; }
            _ => {}
        }
        let room = r.right - x - if c.k == ChipK::Remove { 26.0 } else { 12.0 };
        ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &c.label, room), x, r.top, CHIP_H, &text_b);
        if c.k == ChipK::Remove {
            ui.glyph(&ui.f.small, "\u{2715}", r.right - 24.0, r.top, 16.0, CHIP_H, &ui.wa(0.45 + 0.45 * k));
        }
        if hits { ui.hit(r, c.id); }
    }

    // ------------------------------------------------------------ eventos
    /// Lanza la búsqueda cuando pasó la espera (se llama al pintar).
    pub(super) fn feeds_pump(&mut self) {
        let f = &mut self.feeds;
        let Some(d) = f.due else { return };
        if Instant::now() < d { return; }
        f.due = None;
        let q = f.fields[0].text().trim().to_string();
        if q.is_empty() { f.search = Search::Idle; return; }
        f.req += 1;
        let g = f.req;
        f.search = Search::Busy;
        std::thread::spawn(move || {
            let r = if sys::demo() { Some(demo_search(&q)) } else { net::get(&search_url(&q)).map(|b| parse_search(&String::from_utf8_lossy(&b))) };
            INBOX.lock().unwrap().search = Some((g, r));
            popup::notify(popup::Kind::Settings);
        });
    }

    /// Llegó algo de un hilo (WM_PANEL_DATA).
    pub(super) fn feeds_data(&mut self) {
        let (s, c) = { let mut ib = INBOX.lock().unwrap(); (ib.search.take(), ib.check.take()) };
        let f = &mut self.feeds;
        if let Some((g, r)) = s {
            // Solo la respuesta a lo último que se escribió
            if g == f.req && f.due.is_none() {
                f.search = match r { Some(v) => Search::Done(v), None => Search::Failed };
                f.sel = 0;
            }
        }
        if let Some((g, r)) = c {
            if g == f.check_gen {
                match r {
                    Ok((n, name, url)) => {
                        let name = if name.is_empty() { f.feed_name(&url) } else { name };
                        f.check = Check::Ok(format!("{} · {name}", i18n::tf("{} titulares", &[&n])));
                        if !f.nw.feeds.contains(&url) && f.nw.feeds.len() < news::MAX_FEEDS {
                            f.nw.feeds.push(url.clone());
                            f.nw.names.retain(|x| x.0 != url);
                            f.nw.names.push((url, name));
                            f.save_news();
                        }
                        f.fields[1].set_text("");
                    }
                    Err(e) => f.check = Check::Err(e),
                }
            }
        }
    }

    pub(super) fn feeds_click(&mut self, id: u32) -> Re {
        let f = &mut self.feeds;
        let pick = |base: u32, n: usize| (base + 1..=base + n as u32).contains(&id).then(|| (id - base - 1) as usize);
        match id {
            RAW => { if let Some(s) = f.raw_symbol() { f.add_stock(&s, "", ""); } }
            i if (SUGG0..SUGG0 + SUGGS as u32).contains(&i) => f.add_suggestion((i - SUGG0) as usize),
            i if (STOCK0..STOCK0 + 4 * stocks::MAX_SYMBOLS as u32).contains(&i) => {
                let (n, sub) = (((i - STOCK0) / 4) as usize, (i - STOCK0) % 4);
                match sub { 1 if n > 0 => f.move_stock(n, n - 1), 2 => f.move_stock(n, n + 1), 3 => f.remove_stock(n), _ => {} }
            }
            i if i == lid(L_CHART) => { f.st.chart = !f.st.chart; f.save_stocks(); }
            i if i == lid(L_META) => { f.nw.meta = !f.nw.meta; f.save_news(); }
            i if (TOPIC0..TOPIC0 + news::TOPICS.len() as u32).contains(&i) => {
                let key = news::TOPICS[(i - TOPIC0) as usize].0;
                let on = f.nw.topics.iter().any(|k| k == key);
                if on { f.nw.topics.retain(|k| k != key); } else { f.nw.topics.push(key.to_string()); }
                // En el orden de la lista (el de las URLs)
                let order: Vec<String> = news::TOPICS.iter().filter(|t| f.nw.topics.iter().any(|k| k == t.0)).map(|t| t.0.to_string()).collect();
                f.nw.topics = order;
                f.save_news();
            }
            i if (REGION0..REGION0 + news::REGIONS.len() as u32).contains(&i) => {
                let code = news::REGIONS[(i - REGION0) as usize].0.to_string();
                if code != f.nw.region { f.nw.region = code; f.save_news(); }
            }
            i if i == Fid::Src.id() + 1 => f.start_check(),
            i if i == Fid::Mute.id() + 1 => f.add_word(Fid::Mute),
            i if i == Fid::Hide.id() + 1 => f.add_word(Fid::Hide),
            i if (SRC0..SRC0 + 2 * news::MAX_FEEDS as u32).contains(&i) => { if (i - SRC0) % 2 == 1 { f.remove_feed(((i - SRC0) / 2) as usize); } }
            i if (MUTE0..MUTE0 + MAX_WORDS as u32).contains(&i) => { let n = (i - MUTE0) as usize; if n < f.nw.mute.len() { f.nw.mute.remove(n); f.save_news(); } }
            i if (HIDE0..HIDE0 + MAX_WORDS as u32).contains(&i) => { let n = (i - HIDE0) as usize; if n < f.nw.hide.len() { f.nw.hide.remove(n); f.save_news(); } }
            i if (NOW0..NOW0 + 10).contains(&i) => {
                let avail: Vec<String> = f.now_src.iter().filter(|s| !f.nw.hide.iter().any(|h| news::matches_term(s, h))).take(10).cloned().collect();
                if let Some(s) = avail.get((i - NOW0) as usize) { if f.nw.hide.len() < MAX_WORDS { f.nw.hide.push(s.clone()); f.save_news(); } }
            }
            _ => {
                if let Some(p) = pick(lid(L_PERIOD), 4) { f.st.period = stocks::Period::ALL[p]; f.save_stocks(); }
                else if let Some(v) = pick(lid(L_VAR), 2) { f.st.amount = v == 1; f.save_stocks(); }
                else if let Some(v) = pick(lid(L_REFRESH), 3) { f.st.refresh_min = stocks::REFRESH_OPTS[v]; f.save_stocks(); }
                else if let Some(v) = pick(lid(L_TURN), 4) { f.nw.turn = news::TURN_OPTS[v]; f.save_news(); }
                else if let Some(v) = pick(lid(L_PER_PAGE), 3) { f.nw.per_page = v + 2; f.save_news(); }
            }
        }
        Re::Stay
    }

    /// Teclas con un campo enfocado: todas son suyas (también ↑ ↓ y Esc).
    pub(super) fn feeds_key(&mut self, vk: u16) -> Option<Re> {
        let fid = self.feeds.focus?;
        if Page::of(self.sec) != Some(fid.page()) { self.feeds.blur(); return None; }
        let f = &mut self.feeds;
        if fid == Fid::Search {
            let n = f.suggestions().len();
            match vk {
                0x26 /* ↑ */ => { if n > 0 { f.sel = (f.sel + n - 1) % n; } return Some(Re::Stay); }
                0x28 /* ↓ */ => { if n > 0 { f.sel = (f.sel + 1) % n; } return Some(Re::Stay); }
                _ => {}
            }
        }
        let ev = f.field(fid).key(vk);
        match ev {
            Some(Ev::Submit) => match fid {
                Fid::Search => {
                    if !f.suggestions().is_empty() { let s = f.sel; f.add_suggestion(s); }
                    else if let Some(s) = f.raw_symbol().filter(|_| matches!(f.search, Search::Failed | Search::Done(_))) { f.add_stock(&s, "", ""); }
                }
                Fid::Src => f.start_check(),
                Fid::Mute | Fid::Hide => f.add_word(fid),
            },
            Some(Ev::Cancel) => {
                // Esc: lo escrito se descarta y el campo deja de editarse
                f.field(fid).set_text("");
                if fid == Fid::Search { f.search = Search::Idle; f.due = None; }
                if fid == Fid::Src { f.check = Check::Idle; }
                f.blur();
            }
            Some(Ev::Changed) => self.feeds_changed(fid),
            _ => {}
        }
        Some(Re::Stay)
    }

    pub(super) fn feeds_chr(&mut self, c: char) -> bool {
        let Some(fid) = self.feeds.focus else { return false };
        if self.feeds.field(fid).chr(c) == Ev::Changed { self.feeds_changed(fid); }
        true
    }

    /// Cambió el texto de un campo: el buscador espera un poco antes de pedir sugerencias.
    fn feeds_changed(&mut self, fid: Fid) {
        let f = &mut self.feeds;
        match fid {
            Fid::Search => {
                if f.fields[0].text().trim().is_empty() { f.search = Search::Idle; f.due = None; }
                else { f.search = Search::Waiting; f.due = Some(Instant::now() + Duration::from_millis(DEBOUNCE_MS)); f.sel = 0; }
            }
            Fid::Src => { if !matches!(f.check, Check::Busy) { f.check = Check::Idle; } }
            _ => {}
        }
    }

    pub(super) fn feeds_mouse(&mut self, ev: popup::Mouse, x: f32, y: f32) -> bool {
        let Some(page) = Page::of(self.sec) else { return false };
        let f = &mut self.feeds;
        match ev {
            popup::Mouse::Down => {
                let shift = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x10) < 0 };
                let hit = if y >= TOP - 6.0 { FIELDS.into_iter().filter(|fd| fd.page() == page).find(|fd| f.fields[*fd as usize].contains(x, y)) } else { None };
                match hit {
                    Some(fd) => { if f.focus != Some(fd) { f.focus_on(fd); } f.field(fd).mouse_down(x, y, shift); }
                    None => {
                        let was = f.focus.is_some();
                        f.blur();
                        // ¿Se coge una fila de la bolsa? (no sus botones de la derecha)
                        if let (Page::Stocks, Some(t0)) = (page, f.list_top) {
                            let k = ((y - t0) / STOCK_H).floor();
                            if x < X1 - IN - 3.0 * 26.0 && y >= TOP - 6.0 && k >= 0.0 && (k as usize) < f.st.syms.len() {
                                f.drag = Some(Drag { from: k as usize, y0: y, y, live: false });
                            }
                        }
                        return was;
                    }
                }
                true
            }
            popup::Mouse::Move => {
                if let Some(fd) = f.focus { if f.field(fd).mouse_move(x) { return true; } }
                let Some(d) = f.drag.as_mut() else { return false };
                d.y = y;
                if !d.live && (d.y - d.y0).abs() > 4.0 { d.live = true; }
                d.live
            }
            popup::Mouse::Up => {
                if let Some(fd) = f.focus { f.field(fd).mouse_up(); }
                let Some(d) = f.drag.take() else { return false };
                if !d.live { return false; }
                let to = drag_to(&d, f.st.syms.len());
                f.move_stock(d.from, to);
                f.swallow = true;
                true
            }
        }
    }

    /// Prueba visual (MACBAR_SETTINGSTEST): `bolsa`, `bolsa_buscar` (escribe «santander» en el buscador),
    /// `bolsa_abajo`, `bolsa_arrastre` (una fila a medio arrastrar),
    /// `noticias`, `noticias_medio` (añade una fuente de ejemplo y una palabra, y escribe otra) o
    /// `noticias_abajo` (baja hasta el final).
    pub(super) fn feeds_test_hook(&mut self) {
        static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let Ok(v) = std::env::var("MACBAR_SETTINGSTEST") else { return };
        if DONE.swap(true, Ordering::Relaxed) { return; }
        match v.as_str() {
            "bolsa" => self.sec = Sec::Stocks,
            "bolsa_abajo" => { self.sec = Sec::Stocks; self.scroll_to = 10_000.0; }
            // Una fila cogida y arrastrada fila y media hacia abajo (sin soltar)
            "bolsa_arrastre" => { self.sec = Sec::Stocks; self.feeds.drag = Some(Drag { from: 0, y0: 0.0, y: STOCK_H * 1.6, live: true }); }
            "bolsa_buscar" => {
                self.sec = Sec::Stocks;
                self.feeds.focus_on(Fid::Search);
                for c in "santander".chars() { self.feeds.field(Fid::Search).chr(c); }
                self.feeds_changed(Fid::Search);
                self.feeds.due = Some(Instant::now());
            }
            "noticias" => self.sec = Sec::News,
            "noticias_medio" => {
                self.sec = Sec::News;
                self.scroll_to = 250.0;
                self.feeds.field(Fid::Mute).set_text("fútbol");
                self.feeds.add_word(Fid::Mute);
                self.feeds.focus_on(Fid::Mute);
                for c in "Copa del Rey".chars() { self.feeds.field(Fid::Mute).chr(c); }
                self.feeds.field(Fid::Src).set_text("https://ejemplo.org/feed.xml");
                self.feeds.start_check();
            }
            "noticias_abajo" => {
                self.sec = Sec::News;
                self.scroll_to = 10_000.0;
                self.feeds.field(Fid::Src).set_text("https://ejemplo.org/feed.xml");
                self.feeds.start_check();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busqueda_de_yahoo() {
        let j = r#"{"explains":[],"count":3,"quotes":[{"exchange":"MCE","shortname":"BANCO SANTANDER S.A.","quoteType":"EQUITY","symbol":"SAN.MC","index":"quotes","score":20219.0,"typeDisp":"Equity","longname":"Banco Santander, S.A.","exchDisp":"Madrid Stock Exchange CATS","sector":"Financial {Services}","industry":"Banks—Diversified","isYahooFinance":true},{"exchange":"NYQ","shortname":"Banco \"Santander\" Chile","quoteType":"EQUITY","symbol":"BSAC","typeDisp":"Equity","exchDisp":"NYSE"},{"symbol":"$$$ raro","quoteType":"EQUITY"},{"exchange":"CCC","quoteType":"CRYPTOCURRENCY","symbol":"btc-usd","shortname":"Bitcoin USD","typeDisp":"Cryptocurrency","exchDisp":"CCC"},{"exchange":"X","quoteType":"MONEYMARKET","symbol":"MM1","typeDisp":"Money Market"}],"news":[{"uuid":"x","title":"no es un valor","symbol":"NO"}],"nav":[]}"#;
        let v = parse_search(j);
        assert_eq!(v.len(), 4, "el de símbolo raro, fuera; las noticias no cuentan: {v:?}");
        assert_eq!(v[0], Sugg { sym: "SAN.MC".into(), name: "Banco Santander".into(), exch: "Madrid Stock Exchange CATS".into(), kind: "Acción".into() });
        assert_eq!(v[1].name, "Banco \"Santander\" Chile");
        assert_eq!(v[1].exch, "NYSE");
        assert_eq!((v[2].sym.as_str(), v[2].kind.as_str()), ("BTC-USD", "Cripto"));
        assert_eq!(v[3].kind, "Money Market", "tipo desconocido: el que da Yahoo");
        assert!(parse_search(r#"{"quotes":[],"news":[]}"#).is_empty());
        assert!(parse_search("<html>error</html>").is_empty());
        assert!(parse_search(r#"{"quotes":[{"symbol":"A""#).is_empty(), "cortado a medias: nada");
        assert_eq!(search_url("Banco Santander"), "https://query1.finance.yahoo.com/v1/finance/search?q=Banco%20Santander&quotesCount=8&newsCount=0");
        assert_eq!(search_url(" ñ&x=1 "), "https://query1.finance.yahoo.com/v1/finance/search?q=%C3%B1%26x%3D1&quotesCount=8&newsCount=0");
    }

    /// Red de verdad (manual): `cargo test --release --offline -- --ignored red_buscador_y_temas --nocapture`
    #[test]
    #[ignore]
    fn red_buscador_y_temas() {
        let body = net::get(&search_url("santander")).expect("Yahoo busca");
        let v = parse_search(&String::from_utf8_lossy(&body));
        println!("Yahoo «santander»: {:?}", v.iter().map(|s| format!("{} {} [{} · {}]", s.sym, s.name, s.exch, s.kind)).collect::<Vec<_>>());
        assert!(v.iter().any(|s| s.sym == "SAN.MC"));
        for region in ["ES", "MX", "US"] {
            for t in news::TOPICS {
                let url = news::topic_url(t.2, region);
                let b = net::fetch(&url).unwrap_or_else(|e| panic!("{url}: {e:?}"));
                let items = news::parse_feed(&news::decode_text(&b));
                println!("{region} {:16} {:3} titulares · {}", t.0, items.len(), items.first().map(|i| i.title.as_str()).unwrap_or(""));
                // (Algún tema de algún país trae pocos: lo que importa es que responda y se entienda)
                assert!(!items.is_empty(), "{url}");
            }
        }
        // Comprobar una fuente propia: un feed, una web que anuncia el suyo y algo que no es un feed
        let ok = news::check_feed("https://hnrss.org/frontpage").expect("feed");
        println!("hnrss: {} titulares · {}", ok.0, ok.1);
        let bad = news::check_feed("https://example.com/");
        println!("example.com: {bad:?}");
        assert!(bad.is_err());
        println!("404: {:?}", news::check_feed("https://news.google.com/esto-no-existe-rss"));
    }

    #[test]
    fn objetos_json() {
        let o = json_objects(r#"{"a":"}"},{"b":{"c":1}} , {"d":"\"{"}] , {"fuera":1}"#);
        assert_eq!(o, vec![r#"{"a":"}"}"#, r#"{"b":{"c":1}}"#, r#"{"d":"\"{"}"#]);
    }

    #[test]
    fn demo_sin_red() {
        let v = demo_search("santander");
        assert!(v.len() >= 2 && v.iter().all(|s| s.name.contains("Santander")));
        assert_eq!(demo_search("TELEFONICA")[0].sym, "TEF.MC", "sin distinguir tildes");
    }

    #[test]
    fn arrastre_e_ids() {
        let d = |from: usize, dy: f32| Drag { from, y0: 100.0, y: 100.0 + dy, live: true };
        assert_eq!(drag_to(&d(0, 10.0), 4), 0);
        assert_eq!(drag_to(&d(0, STOCK_H * 0.6), 4), 1);
        assert_eq!(drag_to(&d(1, STOCK_H * 9.0), 4), 3, "no pasa del último");
        assert_eq!(drag_to(&d(2, -STOCK_H * 9.0), 4), 0, "ni del primero");
        let ids = [FID, END - 1, lid(L_PER_PAGE) + 3, FIELD0 + 3 * 4 + 1, NOW0 + 9, STOCK0 + 4 * 12];
        assert!(ids.iter().all(|i| (FID..END).contains(i)));
        assert!(lid(L_PER_PAGE) + 15 < FIELD0, "las filas no pisan los campos");
        assert!(SUGG0 + SUGGS as u32 <= RAW && RAW < STOCK0);
    }
}
