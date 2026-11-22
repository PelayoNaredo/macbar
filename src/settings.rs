//! Ventana de Configuración (como «Ajustes del Sistema» de macOS): reúne en un panel las opciones del menú
//! de clic derecho de la barra y las de config.ini que antes solo se podían tocar a mano.
//!
//! Cada control llama a la MISMA función que el menú (casi siempre `bar_action` con su id) y lee el estado
//! de la misma fuente, así que el menú y la ventana nunca se contradicen. Lo que solo vivía en config.ini
//! (`alerta_cpu`, `ciudad`) se guarda con `sys::set_config_value`.

use super::*;
use popup::{Content, Re, Ui};

const W: f32 = 640.0;
const H: f32 = 460.0;
/// Ancho de la columna de secciones.
const SIDE: f32 = 176.0;
/// Bordes del panel derecho.
const X0: f32 = SIDE + 20.0;
const X1: f32 = W - 20.0;
/// Donde empieza el contenido que se desplaza (encima va el título de la sección).
const TOP: f32 = 58.0;
const ITEM_H: f32 = 30.0;
const SIDE_Y0: f32 = 52.0;
/// Separación interior de las filas dentro de su grupo.
const IN: f32 = 14.0;
const CTL_H: f32 = 26.0;
const REPO: &str = "https://github.com/PelayoNaredo/macbar";
/// Umbrales del aviso de procesador (0 = apagado), en el orden del selector.
const CPU_OPTS: [u32; 4] = [80, 90, 95, 0];
/// Selector del color del texto (auto, claro, oscuro) → modo de `desk::set_ink_mode` (0 auto, 1 oscuro, 2 claro).
const INK_ORDER: [u32; 3] = [0, 2, 1];
const ID_SEC0: u32 = 1;
const ID_ROW0: u32 = 10_000;
/// Hueco reservado en el id de cada fila para la animación de su selector (no es pulsable).
const SUB_ANIM: u32 = 15;

/// Doble clic en el escritorio para cambiar de cara (widgets o iconos). En esta rama aún no existe
/// `faces.rs`: al integrar, estas dos funciones pasan a ser `crate::faces::dblclick_on()` y
/// `crate::faces::toggle_dblclick()` (que además avisa a su vigilante sin reiniciar).
mod caras {
    pub fn on() -> bool { crate::faces::dblclick_on() }
    pub fn toggle() { crate::faces::toggle_dblclick(); }
}

// ---------------------------------------------------------------- secciones y filas
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sec { General, Island, Desk, Tools, Profiles, Weather, About }
const SECS: [Sec; 7] = [Sec::General, Sec::Island, Sec::Desk, Sec::Tools, Sec::Profiles, Sec::Weather, Sec::About];

impl Sec {
    fn label(self) -> &'static str {
        i18n::t(match self {
            Sec::General => "General", Sec::Island => "Isla y tarjeta", Sec::Desk => "Escritorio y widgets", Sec::Tools => "Herramientas",
            Sec::Profiles => "Perfiles y temas", Sec::Weather => "Tiempo", Sec::About => "Acerca de",
        })
    }
    fn glyph(self) -> &'static str {
        match self {
            Sec::General => "\u{E713}", Sec::Island => "\u{E8BD}", Sec::Desk => "\u{E8A9}", Sec::Tools => "\u{E90F}",
            Sec::Profiles => "\u{E771}", Sec::Weather => "\u{E706}", Sec::About => "\u{E946}",
        }
    }
    fn index(self) -> usize { SECS.iter().position(|s| *s == self).unwrap_or(0) }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Autostart, Lang, Position, Solid, Accent, Present,
    CardClick, CardHover, CardGlance, Cpu,
    Widgets, Widget(usize), Style, Ink, FaceDbl,
    Profile, ProfileSave, ProfileDir, Theme,
    City, Refresh,
    Version, Repo, Folder, Quit,
    Face, Caffeine, Pomodoro, Picker, Note,
}

/// Grupos de filas de cada sección (con su rótulo, en español: se traduce al pintarlo).
/// `n_widgets`: cuántos widgets hay (una fila por cada uno).
fn groups(sec: Sec, n_widgets: usize) -> Vec<(Option<&'static str>, Vec<Row>)> {
    use Row::*;
    match sec {
        Sec::General => vec![
            (None, vec![Autostart, Lang]),
            (Some("Barra de menús"), vec![Position, Solid]),
            (Some("Aspecto"), vec![Accent, Present]),
        ],
        Sec::Island => vec![
            (Some("Tarjeta de la isla"), vec![CardClick, CardHover, CardGlance]),
            (Some("Avisos"), vec![Cpu]),
        ],
        Sec::Desk => vec![
            (None, vec![Widgets]),
            (Some("Widgets"), (0..n_widgets).map(Widget).collect()),
            (Some("Aspecto"), vec![Style, Ink]),
            (Some("Doble escritorio"), vec![Face, FaceDbl]),
        ],
        Sec::Tools => vec![
            (None, vec![Caffeine, Pomodoro]),
            (None, vec![Picker, Note]),
        ],
        Sec::Profiles => vec![
            (Some("Perfiles"), vec![Profile, ProfileSave, ProfileDir]),
            (Some("Tema"), vec![Theme]),
        ],
        Sec::Weather => vec![(None, vec![City, Refresh])],
        Sec::About => vec![(None, vec![Version, Repo, Folder]), (None, vec![Quit])],
    }
}

/// Filas de una sección en orden (el índice de cada una va en el id de sus zonas pulsables).
fn rows_of(sec: Sec, n_widgets: usize) -> Vec<Row> { groups(sec, n_widgets).into_iter().flat_map(|g| g.1).collect() }

/// Id de una zona pulsable: sección, fila y parte (0 = la fila entera, 1.. = cada opción o botón).
fn id_of(sec: usize, row: usize, sub: u32) -> u32 { ID_ROW0 + sec as u32 * 1000 + row as u32 * 16 + sub }
fn decode(id: u32) -> Option<(usize, usize, u32)> {
    let r = id.checked_sub(ID_ROW0)?;
    Some(((r / 1000) as usize, ((r % 1000) / 16) as usize, (r % 1000) % 16))
}

/// Umbral de `alerta_cpu` → opción del selector. Sin valor (o ilegible) es 90 %, el de `live`.
fn cpu_index(v: Option<&str>) -> usize {
    match v.and_then(|s| s.trim().parse::<u32>().ok()) {
        Some(0) => 3,
        Some(n) if n <= 85 => 0,
        Some(n) if n <= 92 => 1,
        Some(_) => 2,
        None => 1,
    }
}
fn cpu_value(i: usize) -> u32 { CPU_OPTS[i.min(CPU_OPTS.len() - 1)] }

// ---------------------------------------------------------------- estado (leído de las mismas fuentes que el menú)
#[derive(Clone, PartialEq, Default, Debug)]
struct State {
    autostart: bool,
    lang: i8,
    bottom: bool,
    solid: bool,
    accent: Option<usize>,
    present: bool,
    card: [bool; 3],
    cpu: usize,
    widgets: bool,
    each: Vec<(&'static str, bool)>,
    floating: bool,
    ink: i8,
    face_dbl: bool,
    profile: Option<usize>,
    saved: [bool; 3],
    city: String,
    latlon: bool,
    away: bool,
    caffeine: bool,
    pomo: bool,
    note: bool,
}

fn read() -> State {
    State {
        autostart: sys::autostart_enabled(),
        lang: i18n::mode(),
        bottom: crate::bar_bottom(),
        solid: crate::solid_bar(),
        accent: profiles::accent_index(),
        present: profiles::presenting(),
        card: [card::opt(card::OPT_CLICK), card::opt(card::OPT_HOVER), card::opt(card::OPT_GLANCE)],
        cpu: cpu_index(sys::config_value("alerta_cpu").as_deref()),
        widgets: desk::enabled(),
        each: desk::menu_items(),
        floating: desk::floating(),
        ink: desk::ink_mode(),
        face_dbl: caras::on(),
        profile: profiles::active(),
        saved: [0, 1, 2].map(profiles::exists),
        city: sys::config_value("ciudad").unwrap_or_default(),
        latlon: sys::config_value("lat").is_some() && sys::config_value("lon").is_some(),
        away: crate::faces::AWAY.load(std::sync::atomic::Ordering::Relaxed),
        caffeine: crate::live::caffeine_on(),
        pomo: crate::live::POMO.load(std::sync::atomic::Ordering::Relaxed) != 0,
        note: crate::notes::is_open(),
    }
}

// ---------------------------------------------------------------- qué hace cada control
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Act {
    /// La acción del menú con ese id; el panel sigue abierto.
    Bar(u32),
    /// Igual, pero cierra el panel (reinicia macbar, abre un diálogo o una carpeta).
    BarClose(u32),
    Autostart(bool),
    Style,
    FaceDbl,
    Cpu(usize),
    SaveProfile,
    Url,
    Folder,
    EditConfig,
    EditCity,
    SaveCity,
    Nothing,
}

fn action(row: Row, sub: u32, st: &State) -> Act {
    use Row::*;
    let pick = |n: u32| (1..=n).contains(&sub).then(|| sub - 1);
    match row {
        Autostart if sub == 0 => Act::Autostart(!st.autostart),
        Lang => match pick(3) { Some(i) if i as i8 != st.lang => Act::Bar(crate::BAR_LANG0 + i), _ => Act::Nothing },
        Position => match pick(2) {
            Some(0) if st.bottom => Act::BarClose(crate::BAR_POS_TOP),
            Some(1) if !st.bottom => Act::BarClose(crate::BAR_POS_BOTTOM),
            _ => Act::Nothing,
        },
        Solid if sub == 0 => Act::Bar(crate::BAR_SOLID),
        Accent => match pick(profiles::ACCENTS.len() as u32) { Some(i) if st.accent != Some(i as usize) => Act::Bar(crate::BAR_ACCENT0 + i), _ => Act::Nothing },
        Present if sub == 0 => Act::Bar(crate::BAR_PRESENT),
        CardClick if sub == 0 => Act::Bar(crate::BAR_CARD_CLICK),
        CardHover if sub == 0 => Act::Bar(crate::BAR_CARD_HOVER),
        CardGlance if sub == 0 => Act::Bar(crate::BAR_CARD_GLANCE),
        Cpu => match pick(4) { Some(i) if i as usize != st.cpu => Act::Cpu(i as usize), _ => Act::Nothing },
        Widgets if sub == 0 => Act::Bar(crate::BAR_WIDGETS),
        Widget(i) if sub == 0 && st.widgets && i < st.each.len() => Act::Bar(crate::BAR_WIDGET0 + i as u32),
        Style => match pick(2) { Some(i) if (i == 0) != st.floating => Act::Style, _ => Act::Nothing },
        Ink => match pick(3) { Some(i) if INK_ORDER[i as usize] as i8 != st.ink => Act::Bar(crate::BAR_INK0 + INK_ORDER[i as usize]), _ => Act::Nothing },
        FaceDbl if sub == 0 => Act::FaceDbl,
        // Un perfil ya guardado se carga y macbar se reinicia; uno vacío guarda en él lo de ahora
        Profile => match pick(3) {
            Some(i) if st.profile != Some(i as usize) => if st.saved[i as usize] { Act::BarClose(crate::BAR_PROFILE0 + i) } else { Act::Bar(crate::BAR_PROFILE0 + i) },
            _ => Act::Nothing,
        },
        ProfileSave if sub == 1 && st.profile.is_some() => Act::SaveProfile,
        ProfileDir if sub == 1 => Act::BarClose(crate::BAR_PROFILES_DIR),
        Theme if sub == 1 => Act::BarClose(crate::BAR_THEME_EXPORT),
        Theme if sub == 2 => Act::BarClose(crate::BAR_THEME_IMPORT),
        City if sub == 1 => Act::EditCity,
        City if sub == 2 => Act::SaveCity,
        Refresh if sub == 1 => Act::Bar(crate::BAR_WEATHER_NOW),
        Repo if sub == 1 => Act::Url,
        Folder if sub == 1 => Act::Folder,
        Folder if sub == 2 => Act::EditConfig,
        Face => match pick(2) { Some(i) if (i == 1) != st.away => Act::Bar(crate::BAR_FACE), _ => Act::Nothing },
        Caffeine if sub == 0 => Act::Bar(crate::BAR_CAFFEINE),
        Pomodoro if sub == 0 => Act::Bar(crate::BAR_POMODORO),
        Picker if sub == 1 => Act::BarClose(crate::BAR_PICKER),
        Note if sub == 0 => Act::BarClose(crate::BAR_NOTE),
        Quit if sub == 1 => Act::BarClose(crate::BAR_QUIT),
        _ => Act::Nothing,
    }
}

// ---------------------------------------------------------------- aspecto de cada fila
enum Ctl {
    Switch(bool),
    /// Selector segmentado: opciones y la elegida.
    Seg(Vec<String>, Option<usize>),
    /// Muestras de color de acento.
    Swatch(Option<usize>),
    /// Botones (glifo, texto).
    Buttons(Vec<(&'static str, String)>),
    /// Campo de texto de la ciudad con su botón de guardar.
    Field,
    Value(String),
}

/// (etiqueta, descripción, control, activa)
fn spec(row: Row, st: &State) -> (String, String, Ctl, bool) {
    use Row::*;
    let t = |s: &'static str| i18n::t(s).to_string();
    let seg = |v: &[&'static str], sel: Option<usize>| Ctl::Seg(v.iter().map(|s| i18n::t(s).to_string()).collect(), sel);
    match row {
        Autostart => (t("Iniciar con Windows"), t("Abre macbar al iniciar sesión"), Ctl::Switch(st.autostart), true),
        Lang => (t("Idioma"), String::new(), seg(&["Automático", "Español", "English"], Some(st.lang.clamp(0, 2) as usize)), true),
        Position => (t("Posición de la barra"), t("macbar se reinicia para colocarla"), seg(&["Arriba", "Abajo"], Some(st.bottom as usize)), true),
        Solid => (t("Barra sólida con ventanas maximizadas"), t("Fondo opaco cuando una ventana ocupa la pantalla"), Ctl::Switch(st.solid), true),
        Accent => {
            let name = match st.accent { Some(i) => t(profiles::ACCENTS[i].0), None => t("Personalizado (config.ini)") };
            (t("Color de acento"), name, Ctl::Swatch(st.accent), true)
        }
        Present => (t("Modo presentación"), t("Sin avisos en la isla ni widgets"), Ctl::Switch(st.present), true),
        CardClick => (t("Abrir con un clic en la isla"), t("Si no, el clic pausa o reanuda la música"), Ctl::Switch(st.card[0]), true),
        CardHover => (t("Abrir al dejar el ratón encima"), t("La tarjeta sale sola tras un momento"), Ctl::Switch(st.card[1]), true),
        CardGlance => (t("Vistazo con la isla vacía"), t("La tarjeta enseña un resumen aunque no suene nada"), Ctl::Switch(st.card[2]), true),
        Cpu => (t("Aviso de procesador alto"), t("Si pasa del umbral 2 minutos seguidos"), seg(&["80 %", "90 %", "95 %", "Apagado"], Some(st.cpu)), true),
        Widgets => (t("Widgets del escritorio"), t("Calendario, tiempo, música, sistema y más"), Ctl::Switch(st.widgets), true),
        Widget(i) => {
            let (label, on) = st.each.get(i).copied().unwrap_or(("", false));
            (label.to_string(), String::new(), Ctl::Switch(on && st.widgets), st.widgets)
        }
        Style => (t("Estilo"), t("Flotantes sin fondo o tarjetas de cristal"), seg(&["Flotante", "Tarjeta"], Some(!st.floating as usize)), true),
        Ink => {
            let sel = INK_ORDER.iter().position(|m| *m as i8 == st.ink);
            (t("Color del texto"), t("Automático: según el fondo de cada widget"), seg(&["Auto", "Claro", "Oscuro"], sel), true)
        }
        FaceDbl => (t("Doble clic en el escritorio"), t("Cambia entre los widgets y los iconos"), Ctl::Switch(st.face_dbl), true),
        Profile => {
            let sel = st.profile;
            (t("Perfil"), t("Al cambiar, se guarda el actual y macbar se reinicia"), seg(&profiles::NAMES, sel), true)
        }
        ProfileSave => {
            let desc = match st.profile { Some(i) => i18n::tf("Guarda los ajustes de ahora en «{}»", &[&i18n::t(profiles::NAMES[i])]), None => t("Elige antes un perfil") };
            (t("Guardar el perfil actual"), desc, Ctl::Buttons(vec![("\u{E74E}", t("Guardar"))]), st.profile.is_some())
        }
        ProfileDir => (t("Carpeta de perfiles"), t("Una copia de config.ini y widgets.ini por perfil"), Ctl::Buttons(vec![("\u{E8B7}", t("Abrir"))]), true),
        Theme => (t("Tema"), t("Acento y aspecto de los widgets en un archivo"), Ctl::Buttons(vec![("\u{E898}", t("Exportar…")), ("\u{E896}", t("Importar…"))]), true),
        City => {
            let desc = if st.latlon { t("lat y lon de config.ini tienen prioridad") } else { t("Para la barra, el panel y el widget del tiempo") };
            (t("Ciudad"), desc, Ctl::Field, true)
        }
        Refresh => (t("Actualizar el tiempo"), t("Vuelve a pedirlo ahora"), Ctl::Buttons(vec![("\u{E72C}", t("Actualizar"))]), true),
        Version => (t("Versión"), String::new(), Ctl::Value(env!("CARGO_PKG_VERSION").to_string()), true),
        Repo => (t("Código fuente"), "github.com/PelayoNaredo/macbar".to_string(), Ctl::Buttons(vec![("\u{E8A7}", t("Abrir"))]), true),
        Face => (t("Ahora se ve"), t("Win+Alt+W o doble clic en un hueco del escritorio"), seg(&["Widgets", "Iconos"], Some(st.away as usize)), true),
        Caffeine => (t("Cafeína"), t("No deja que el equipo se duerma"), Ctl::Switch(st.caffeine), true),
        Pomodoro => (t("Pomodoro"), t("25 min de concentración y descansos de 5"), Ctl::Switch(st.pomo), true),
        Picker => (t("Cuentagotas de color"), t("Copia el color de cualquier punto de la pantalla"), Ctl::Buttons(vec![("\u{E790}", t("Elegir color"))]), true),
        Note => (t("Nota rápida"), t("No sale en capturas ni al compartir pantalla"), Ctl::Switch(st.note), true),
        Quit => (t("Salir de macbar"), t("Cierra la barra, la isla y los widgets"), Ctl::Buttons(vec![("\u{E7E8}", t("Salir"))]), true),
        Folder => (t("Carpeta de configuración"), "%APPDATA%\\macbar".to_string(), Ctl::Buttons(vec![("\u{E8B7}", t("Abrir")), ("\u{E70F}", t("Editar config.ini"))]), true),
    }
}

// ---------------------------------------------------------------- panel
pub struct Settings {
    sec: Sec,
    /// Sección que se va (fundido cruzado): cuál, cuándo empezó, hacia dónde y su desplazamiento.
    prev: Option<(Sec, Instant, f32, f32)>,
    st: State,
    /// Algo cambió: se vuelve a leer el estado en el próximo pintado (después de ejecutar la acción).
    stale: bool,
    /// Valores animados de interruptores y selectores (por id).
    anim: HashMap<u32, f32>,
    moving: bool,
    scroll: f32,
    scroll_to: f32,
    max_scroll: f32,
    side_y: f32,
    opened: Instant,
    last: Instant,
    /// Texto de la ciudad mientras se escribe.
    edit: Option<String>,
}

/// Abre la ventana de Configuración centrada bajo la barra (si ya está abierta, se queda).
pub fn open() {
    if popup::hwnd_of(popup::Kind::Settings).is_some_and(popup::shown) { return; }
    let s = scale_of(bar_hwnd());
    let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let wpx = (W * s) as i32;
    popup::toggle(popup::Kind::Settings, sw / 2 + wpx / 2);
}

fn mix(a: D2D1_COLOR_F, b: D2D1_COLOR_F, k: f32) -> D2D1_COLOR_F {
    rgba(a.r + (b.r - a.r) * k, a.g + (b.g - a.g) * k, a.b + (b.b - a.b) * k, a.a + (b.a - a.a) * k)
}
fn hex(c: u32, a: f32) -> D2D1_COLOR_F { rgba(((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0, a) }

/// Recorta el texto con «…» para que quepa en `maxw`.
fn fit(ui: &Ui, f: &IDWriteTextFormat, s: &str, maxw: f32) -> String {
    if maxw <= 0.0 { return String::new(); }
    if ui.p.width(f, s) <= maxw { return s.to_string(); }
    let mut v: Vec<char> = s.chars().collect();
    while !v.is_empty() {
        v.pop();
        let t = v.iter().collect::<String>().trim_end().to_string() + "…";
        if ui.p.width(f, &t) <= maxw { return t; }
    }
    String::new()
}

/// Botón redondeado (como `Ui::button`, sin apuntar zona: la pone quien lo llama).
fn btn(ui: &Ui, r: D2D_RECT_F, glyph: &str, label: &str, k: f32) {
    unsafe { ui.rt.FillRoundedRectangle(&rrect(r, 7.0), &ui.wa(0.085 + 0.075 * k)); }
    let lw = ui.p.width(&ui.f.lbl, label);
    let x0 = (r.left + r.right) / 2.0 - (lw + 20.0) / 2.0;
    ui.glyph(&ui.f.icon, glyph, x0, r.top, 16.0, r.bottom - r.top, &ui.white);
    ui.text(&ui.f.lbl, label, x0 + 20.0, r.top, r.bottom - r.top, &ui.white);
}
fn btn_w(ui: &Ui, label: &str) -> f32 { ui.p.width(&ui.f.lbl, label) + 20.0 + 26.0 }

impl Settings {
    pub fn new() -> Self {
        Settings {
            sec: Sec::General, prev: None, st: State::default(), stale: true, anim: HashMap::new(), moving: false,
            scroll: 0.0, scroll_to: 0.0, max_scroll: 0.0, side_y: -1.0, opened: Instant::now(), last: Instant::now(), edit: None,
        }
    }

    /// Valor animado hacia `target` (la primera vez aparece ya en su sitio).
    fn ease(&mut self, id: u32, target: f32, dt: f32) -> f32 {
        let v = self.anim.entry(id).or_insert(target);
        if (target - *v).abs() < 0.002 { *v = target; } else { *v += (target - *v) * (1.0 - (-dt / 0.07).exp()); self.moving = true; }
        *v
    }

    fn go(&mut self, i: usize) {
        let i = i.min(SECS.len() - 1);
        if SECS[i] == self.sec { return; }
        let dir = if i > self.sec.index() { 1.0 } else { -1.0 };
        self.prev = Some((self.sec, Instant::now(), dir, self.scroll));
        self.sec = SECS[i];
        self.scroll = 0.0;
        self.scroll_to = 0.0;
        self.edit = None;
    }

    /// Ejecuta la acción. Las que tocan Windows van en `Re::Run`: el marco las corre fuera del préstamo
    /// del panel (crear o cerrar ventanas manda mensajes al momento).
    fn perform(&mut self, a: Act) -> Re {
        let bar = |id: u32| move || crate::bar_action(bar_hwnd(), id);
        if !matches!(a, Act::Nothing | Act::EditCity) { self.stale = true; }
        match a {
            Act::Nothing => Re::Stay,
            Act::EditCity => { if self.edit.is_none() { self.edit = Some(self.st.city.clone()); } Re::Stay }
            Act::SaveCity => {
                let v = self.edit.take().unwrap_or_else(|| self.st.city.clone()).trim().to_string();
                Re::Run(Box::new(move || {
                    if v.is_empty() { sys::remove_config_value("ciudad"); } else { sys::set_config_value("ciudad", &v); }
                    crate::bar_action(bar_hwnd(), crate::BAR_WEATHER_NOW);
                }))
            }
            Act::Bar(id) => Re::Run(Box::new(bar(id))),
            Act::BarClose(id) => Re::CloseRun(Box::new(bar(id))),
            // Lo mismo que la opción del menú (menus::A::Autostart)
            Act::Autostart(on) => Re::Run(Box::new(move || sys::set_autostart(on))),
            Act::Style => Re::Run(Box::new(desk::toggle_style)),
            Act::FaceDbl => Re::Run(Box::new(caras::toggle)),
            Act::Cpu(i) => Re::Run(Box::new(move || sys::set_config_value("alerta_cpu", &cpu_value(i).to_string()))),
            Act::SaveProfile => Re::Run(Box::new(profiles::save_active)),
            Act::Url => Re::CloseRun(Box::new(|| sys::open(REPO))),
            Act::Folder => Re::CloseRun(Box::new(|| { let _ = std::fs::create_dir_all(sys::config_dir()); sys::open(&sys::config_dir()); })),
            Act::EditConfig => Re::CloseRun(Box::new(sys::edit_config)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Pinta una sección: título fijo y filas que se desplazan. `live` = la que manda (apunta zonas).
    fn section(&mut self, ui: &mut Ui, sec: Sec, dy: f32, alpha: f32, scroll: f32, live: bool, dt: f32) {
        let rt = ui.rt;
        let mut base = Matrix3x2::default();
        unsafe { rt.GetTransform(&mut base); }
        // Recorte del contenido que se desplaza (con la transformación sin el desplazamiento del fundido)
        unsafe { rt.PushAxisAlignedClip(&rect(SIDE + 1.0, TOP - 6.0, W, H), D2D1_ANTIALIAS_MODE_ALIASED); }
        let mut m = base;
        m.M32 += dy * base.M22;
        unsafe { rt.SetTransform(&m); }
        popup::push_alpha(rt, alpha);

        let si = sec.index();
        let mut y = TOP - scroll;
        let px = 1.0 / ui.s;
        // Acerca de: cabecera con el nombre
        if sec == Sec::About {
            let cx = (X0 + X1) / 2.0;
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: y + 30.0 }, radiusX: 26.0, radiusY: 26.0 };
            unsafe { rt.FillEllipse(&e, &ui.on); }
            ui.glyph(&ui.f.icon_xl, "\u{E713}", cx - 26.0, y + 4.0, 52.0, 52.0, &ui.brush(rgba(1.0, 1.0, 1.0, 0.97)));
            ui.text_c(&ui.f.val, "macbar", cx, y + 64.0, 22.0, &ui.white);
            ui.text_c(&ui.f.sub, i18n::t("Barra de menús estilo macOS para Windows 11"), cx, y + 86.0, 16.0, &ui.dim);
            y += 116.0;
        }
        let mut ri = 0usize;
        for (cap, list) in groups(sec, self.st.each.len()) {
            if let Some(c) = cap {
                ui.text(&ui.f.small, i18n::t(c), X0 + 4.0, y, 20.0, &ui.faint);
                y += 22.0;
            }
            // Medidas de cada fila (las que no caben en una línea ponen el control debajo)
            let specs: Vec<_> = list.iter().map(|r| spec(*r, &self.st)).collect();
            let lay: Vec<(f32, bool, f32)> = specs.iter().map(|(label, desc, ctl, _)| {
                let cw = self.ctl_w(ui, ctl);
                let avail = X1 - X0 - 2.0 * IN;
                let lw = ui.p.width(&ui.f.lbl, label);
                let stacked = matches!(ctl, Ctl::Field) || lw + 16.0 + cw > avail || (!desc.is_empty() && cw + 16.0 + 150.0 > avail);
                let base_h = if desc.is_empty() { 40.0 } else { 52.0 };
                (if stacked { base_h + 34.0 } else { base_h }, stacked, cw)
            }).collect();
            let gh: f32 = lay.iter().map(|l| l.0).sum();
            unsafe { rt.FillRoundedRectangle(&rrect(rect(X0, y, X1, y + gh), 10.0), &ui.wa(0.05)); }
            let mut ry = y;
            for (j, ((label, desc, ctl, en), (rh, stacked, cw))) in specs.into_iter().zip(lay).enumerate() {
                let id0 = id_of(si, ri, 0);
                if j > 0 { unsafe { rt.FillRectangle(&rect(X0 + IN, ry, X1 - IN, ry + px), &ui.card_edge); } }
                // Resaltado suave de la fila bajo el ratón (cualquiera de sus partes)
                let heat = if live && en { (0..16).map(|s| ui.heat(id0 + s)).fold(0.0f32, f32::max) } else { 0.0 };
                if heat > 0.01 { unsafe { rt.FillRoundedRectangle(&rrect(rect(X0 + 4.0, ry + 3.0, X1 - 4.0, ry + rh - 3.0), 8.0), &ui.wa(0.045 * heat)); } }
                if !en { popup::push_alpha(rt, 0.4); }
                let base_h = if desc.is_empty() { 40.0 } else { 52.0 };
                let (l, r) = (X0 + IN, X1 - IN);
                let tw = if stacked { r - l } else { r - l - cw - 16.0 };
                if desc.is_empty() {
                    ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &label, tw), l, ry, base_h, &ui.white);
                } else {
                    ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, &label, tw), l, ry + 8.0, 18.0, &ui.white);
                    ui.text(&ui.f.sub, &fit(ui, &ui.f.sub, &desc, tw), l, ry + 27.0, 15.0, &ui.dim);
                }
                let (cx, cy) = if stacked { (l, ry + base_h + 13.0) } else { (r - cw, ry + base_h / 2.0) };
                let visible = ry + rh > TOP - 6.0 && ry < H;
                let hits = live && en && visible;
                if hits { ui.hit(rect(X0, ry.max(TOP - 6.0), X1, ry + rh), id0); }
                self.ctl(ui, &ctl, cx, cy, cw, id0, hits, dt);
                if !en { unsafe { rt.PopLayer(); } }
                ry += rh;
                ri += 1;
            }
            y += gh + 16.0;
        }
        if live {
            let content = y + scroll - TOP;
            self.max_scroll = (content - (H - TOP)).max(0.0);
            self.scroll_to = self.scroll_to.clamp(0.0, self.max_scroll);
        }
        unsafe {
            rt.PopLayer();
            rt.SetTransform(&base);
        }
        // Barra de desplazamiento fina (solo si no cabe)
        if live && self.max_scroll > 0.5 {
            let vis = H - TOP;
            let bh = (vis * vis / (vis + self.max_scroll)).max(30.0);
            let by = TOP + (vis - bh) * (scroll / self.max_scroll).clamp(0.0, 1.0);
            unsafe { rt.FillRoundedRectangle(&rrect(rect(W - 7.0, by + 2.0, W - 4.0, by + bh - 4.0), 1.5), &ui.wa(0.22 * alpha)); }
        }
        unsafe { rt.PopAxisAlignedClip(); }
        // Título de la sección (fijo, fuera del recorte)
        unsafe { rt.SetTransform(&m); }
        popup::push_alpha(rt, alpha);
        ui.text(&ui.f.val, sec.label(), X0 + 2.0, 16.0, 26.0, &ui.white);
        unsafe { rt.PopLayer(); rt.SetTransform(&base); }
    }

    /// Ancho de un control.
    fn ctl_w(&self, ui: &Ui, c: &Ctl) -> f32 {
        match c {
            Ctl::Switch(_) => 38.0,
            Ctl::Seg(v, _) => v.iter().map(|s| ui.p.width(&ui.f.lbl, s)).fold(0.0f32, f32::max).max(30.0) * v.len() as f32 + 24.0 * v.len() as f32,
            Ctl::Swatch(_) => profiles::ACCENTS.len() as f32 * 26.0 - 8.0,
            Ctl::Buttons(b) => b.iter().map(|(_, l)| btn_w(ui, l)).sum::<f32>() + 8.0 * (b.len().max(1) - 1) as f32,
            Ctl::Field => X1 - X0 - 2.0 * IN,
            Ctl::Value(s) => ui.p.width(&ui.f.lbl, s),
        }
    }

    /// Pinta un control con su borde izquierdo en `x` y centrado en `cy`.
    #[allow(clippy::too_many_arguments)]
    fn ctl(&mut self, ui: &mut Ui, c: &Ctl, x: f32, cy: f32, cw: f32, id0: u32, hits: bool, dt: f32) {
        let rt = ui.rt;
        match c {
            Ctl::Switch(on) => {
                // Interruptor tipo iOS: la bolita se desliza y el fondo pasa al color de acento
                let v = self.ease(id0 + SUB_ANIM, if *on { 1.0 } else { 0.0 }, dt);
                let r = rect(x, cy - 11.0, x + 38.0, cy + 11.0);
                let off = ui.ic(0.16 + 0.06 * ui.heat(id0));
                unsafe {
                    rt.FillRoundedRectangle(&rrect(r, 11.0), &ui.brush(mix(off, crate::profiles::accent(1.0), v)));
                    let kx = x + 11.0 + v * 16.0;
                    let sh = D2D1_ELLIPSE { point: Vector2 { X: kx, Y: cy + 0.8 }, radiusX: 9.2, radiusY: 9.2 };
                    rt.FillEllipse(&sh, &ui.brush(rgba(0.0, 0.0, 0.0, 0.16)));
                    let e = D2D1_ELLIPSE { point: Vector2 { X: kx, Y: cy }, radiusX: 9.0, radiusY: 9.0 };
                    rt.FillEllipse(&e, &ui.brush(rgba(1.0, 1.0, 1.0, 1.0)));
                }
            }
            Ctl::Seg(opts, sel) => {
                let n = opts.len().max(1) as f32;
                let sw = cw / n;
                let r = rect(x, cy - CTL_H / 2.0, x + cw, cy + CTL_H / 2.0);
                unsafe { rt.FillRoundedRectangle(&rrect(r, 7.0), &ui.wa(0.07)); }
                for i in 0..opts.len() {
                    let k = ui.heat(id0 + 1 + i as u32);
                    if k > 0.01 && Some(i) != *sel {
                        let sr = rect(x + i as f32 * sw + 2.0, r.top + 2.0, x + (i + 1) as f32 * sw - 2.0, r.bottom - 2.0);
                        unsafe { rt.FillRoundedRectangle(&rrect(sr, 6.0), &ui.wa(0.05 * k)); }
                    }
                }
                // Píldora que se desliza a la opción elegida
                if let Some(s) = sel {
                    let v = self.ease(id0 + SUB_ANIM, *s as f32, dt);
                    let pr = rect(x + v * sw + 2.0, r.top + 2.0, x + (v + 1.0) * sw - 2.0, r.bottom - 2.0);
                    let fill = if ui.ink > 0.5 { rgba(1.0, 1.0, 1.0, 1.0) } else { rgba(1.0, 1.0, 1.0, 0.17) };
                    unsafe {
                        if ui.ink > 0.5 { rt.FillRoundedRectangle(&rrect(rect(pr.left, pr.top + 0.6, pr.right, pr.bottom + 0.6), 6.0), &ui.brush(rgba(0.0, 0.0, 0.0, 0.10))); }
                        rt.FillRoundedRectangle(&rrect(pr, 6.0), &ui.brush(fill));
                    }
                }
                for (i, o) in opts.iter().enumerate() {
                    let b = if Some(i) == *sel { &ui.white } else { &ui.dim };
                    ui.text_c(&ui.f.lbl, o, x + (i as f32 + 0.5) * sw, r.top, CTL_H, b);
                    if hits { ui.hit(rect(x + i as f32 * sw, r.top, x + (i + 1) as f32 * sw, r.bottom), id0 + 1 + i as u32); }
                }
            }
            Ctl::Swatch(sel) => {
                // El aro de la elegida se desliza de una muestra a otra
                if let Some(s) = sel {
                    let v = self.ease(id0 + SUB_ANIM, *s as f32, dt);
                    let e = D2D1_ELLIPSE { point: Vector2 { X: x + 9.0 + v * 26.0, Y: cy }, radiusX: 12.5, radiusY: 12.5 };
                    unsafe { rt.DrawEllipse(&e, &ui.on, 1.8, None); }
                }
                for (i, (_, col)) in profiles::ACCENTS.iter().enumerate() {
                    let k = ui.heat(id0 + 1 + i as u32);
                    let (ex, rad) = (x + 9.0 + i as f32 * 26.0, 8.5 + 1.0 * k);
                    let e = D2D1_ELLIPSE { point: Vector2 { X: ex, Y: cy }, radiusX: rad, radiusY: rad };
                    // «Color de Windows»: un degradado (el color real depende del sistema)
                    let b = if *col == 0 { ui.p.dgrad(ex - rad, cy - rad, ex + rad, cy + rad, hex(0x3B82F5, 1.0), hex(0xEC4899, 1.0)) } else { ui.brush(hex(*col, 1.0)) };
                    unsafe { rt.FillEllipse(&e, &b); }
                    if hits { ui.hit(rect(ex - 13.0, cy - 13.0, ex + 13.0, cy + 13.0), id0 + 1 + i as u32); }
                }
            }
            Ctl::Buttons(list) => {
                let mut bx = x;
                for (i, (g, l)) in list.iter().enumerate() {
                    let w = btn_w(ui, l);
                    let r = rect(bx, cy - CTL_H / 2.0, bx + w, cy + CTL_H / 2.0);
                    btn(ui, r, g, l, ui.heat(id0 + 1 + i as u32));
                    if hits { ui.hit(r, id0 + 1 + i as u32); }
                    bx += w + 8.0;
                }
            }
            Ctl::Field => {
                let save = i18n::t("Guardar");
                let bw = btn_w(ui, save);
                let fr = rect(x, cy - 14.0, x + cw - bw - 8.0, cy + 14.0);
                let editing = self.edit.is_some();
                unsafe {
                    rt.FillRoundedRectangle(&rrect(fr, 7.0), &ui.wa(0.075 + 0.04 * ui.heat(id0 + 1)));
                    if editing { rt.DrawRoundedRectangle(&rrect(fr, 7.0), &ui.on, 1.5, None); }
                }
                let txt = self.edit.clone().unwrap_or_else(|| self.st.city.clone());
                let tx = fr.left + 10.0;
                let room = fr.right - tx - 10.0;
                if txt.is_empty() {
                    ui.text(&ui.f.lbl, i18n::t("Tu ubicación"), tx, fr.top, 28.0, &ui.faint);
                } else {
                    // Si no cabe, se ve el final (donde se escribe)
                    let mut shown = txt.clone();
                    while ui.p.width(&ui.f.lbl, &shown) > room && !shown.is_empty() { shown.remove(0); }
                    ui.text(&ui.f.lbl, &shown, tx, fr.top, 28.0, &ui.white);
                }
                if editing {
                    let w = if txt.is_empty() { 0.0 } else { ui.p.width(&ui.f.lbl, &txt).min(room) + 1.0 };
                    unsafe { rt.FillRectangle(&rect(tx + w, cy - 8.0, tx + w + 1.6, cy + 8.0), &ui.on); }
                }
                let br = rect(fr.right + 8.0, cy - CTL_H / 2.0, fr.right + 8.0 + bw, cy + CTL_H / 2.0);
                btn(ui, br, "\u{E74E}", save, ui.heat(id0 + 2));
                if hits { ui.hit(fr, id0 + 1); ui.hit(br, id0 + 2); }
            }
            Ctl::Value(s) => ui.text(&ui.f.lbl, s, x, cy - 10.0, 20.0, &ui.dim),
        }
    }
}

impl Content for Settings {
    fn size(&self) -> (f32, f32) { (W, H) }

    fn shown(&mut self) {
        self.st = read();
        self.stale = false;
        self.anim.clear();
        self.prev = None;
        self.scroll = 0.0;
        self.scroll_to = 0.0;
        self.side_y = -1.0;
        self.edit = None;
        self.opened = Instant::now();
        self.last = Instant::now();
    }

    /// Cada segundo: si algo cambió por otro camino (menú, atajo, otro panel), se ve aquí al momento.
    fn tick(&mut self) -> bool {
        if self.edit.is_some() { return false; }
        let n = read();
        if n != self.st { self.st = n; return true; }
        false
    }

    fn animating(&self) -> bool {
        self.moving || self.prev.is_some() || self.opened.elapsed().as_secs_f32() < 0.5 || (self.scroll - self.scroll_to).abs() > 0.3
    }

    fn paint(&mut self, ui: &mut Ui) {
        let rt = ui.rt;
        let dt = self.last.elapsed().as_secs_f32().min(1.0 / 30.0);
        self.last = Instant::now();
        if self.stale { self.st = read(); self.stale = false; }
        self.moving = false;
        let since = self.opened.elapsed().as_secs_f32();
        let ka = 1.0 - (-dt / 0.06).exp();
        self.scroll += (self.scroll_to - self.scroll) * ka;
        if (self.scroll - self.scroll_to).abs() < 0.3 { self.scroll = self.scroll_to; }

        // ---- Columna de secciones
        let b0 = popup::rise(rt, popup::stagger(since, 0.0, 0.3));
        unsafe {
            rt.FillRectangle(&rect(0.0, 0.0, SIDE, H), &ui.wa(0.035));
            rt.FillRectangle(&rect(SIDE, 0.0, SIDE + 1.0 / ui.s, H), &ui.card_edge);
        }
        ui.text(&ui.f.val, i18n::t("Configuración"), 18.0, 16.0, 26.0, &ui.white);
        let target = SIDE_Y0 + self.sec.index() as f32 * (ITEM_H + 2.0);
        if self.side_y < 0.0 { self.side_y = target; }
        self.side_y += (target - self.side_y) * (1.0 - (-dt / 0.06).exp());
        if (target - self.side_y).abs() < 0.3 { self.side_y = target; } else { self.moving = true; }
        unsafe { rt.FillRoundedRectangle(&rrect(rect(10.0, self.side_y, SIDE - 10.0, self.side_y + ITEM_H), 7.0), &ui.on); }
        let on_accent = ui.brush(rgba(1.0, 1.0, 1.0, 0.97));
        for (i, s) in SECS.iter().enumerate() {
            let y = SIDE_Y0 + i as f32 * (ITEM_H + 2.0);
            let id = ID_SEC0 + i as u32;
            let sel = *s == self.sec;
            if !sel { ui.card_k(rect(10.0, y, SIDE - 10.0, y + ITEM_H), ui.heat(id)); }
            // Sobre la píldora de acento, en blanco (se cruza mientras se desliza)
            let k = (1.0 - (self.side_y - y).abs() / ITEM_H).clamp(0.0, 1.0);
            let b = if k > 0.5 { &on_accent } else { &ui.white };
            ui.glyph(&ui.f.icon, s.glyph(), 18.0, y, 20.0, ITEM_H, b);
            ui.text(&ui.f.lbl, &fit(ui, &ui.f.lbl, s.label(), SIDE - 58.0), 46.0, y, ITEM_H, b);
            ui.hit(rect(10.0, y, SIDE - 10.0, y + ITEM_H), id);
        }
        popup::rise_end(rt, b0);

        // ---- Contenido (fundido cruzado con un pequeño desplazamiento al cambiar de sección)
        let b1 = popup::rise(rt, popup::stagger(since, 0.05, 0.3));
        let cur = self.sec;
        let scroll = self.scroll;
        match self.prev {
            Some((ps, t0, dir, pscroll)) => {
                let k = ease_out(t0.elapsed().as_secs_f32() / 0.24);
                if k >= 1.0 {
                    self.prev = None;
                    self.section(ui, cur, 0.0, 1.0, scroll, true, dt);
                } else {
                    self.section(ui, ps, -dir * 12.0 * k, 1.0 - k, pscroll, false, dt);
                    self.section(ui, cur, dir * 12.0 * (1.0 - k), k, scroll, true, dt);
                }
            }
            None => self.section(ui, cur, 0.0, 1.0, scroll, true, dt),
        }
        popup::rise_end(rt, b1);
    }

    fn click(&mut self, id: u32, _right: bool) -> Re {
        if (ID_SEC0..ID_SEC0 + SECS.len() as u32).contains(&id) { self.go((id - ID_SEC0) as usize); return Re::Stay; }
        let Some((si, ri, sub)) = decode(id) else { return Re::Stay };
        if si != self.sec.index() { return Re::Stay; }
        let Some(row) = rows_of(self.sec, self.st.each.len()).get(ri).copied() else { return Re::Stay };
        let a = action(row, sub, &self.st);
        // Un clic fuera del campo deja de editar la ciudad (sin guardar)
        if !matches!(a, Act::EditCity | Act::SaveCity) { self.edit = None; }
        self.perform(a)
    }

    fn wheel(&mut self, delta: i32) -> bool {
        if self.max_scroll <= 0.0 { return false; }
        self.scroll_to = (self.scroll_to - delta as f32 / 120.0 * 60.0).clamp(0.0, self.max_scroll);
        true
    }

    fn key(&mut self, vk: u16) -> Option<Re> {
        let i = self.sec.index();
        match vk {
            v if v == VK_RETURN.0 && self.edit.is_some() => Some(self.perform(Act::SaveCity)),
            v if v == VK_ESCAPE.0 && self.edit.is_some() => { self.edit = None; Some(Re::Stay) }
            v if v == VK_UP.0 => { self.go(if i == 0 { SECS.len() - 1 } else { i - 1 }); Some(Re::Stay) }
            v if v == VK_DOWN.0 => { self.go((i + 1) % SECS.len()); Some(Re::Stay) }
            _ => None,
        }
    }

    fn chr(&mut self, c: char) -> bool {
        let Some(e) = self.edit.as_mut() else { return false };
        match c {
            '\u{8}' => { e.pop(); }
            // Ctrl+Retroceso: borra la última palabra
            '\u{7f}' => { let t = e.trim_end().to_string(); e.truncate(t.rfind(' ').map(|i| i + 1).unwrap_or(0)); }
            '\u{16}' => {
                let Some(p) = search::read_clipboard() else { return false };
                e.push_str(&p.split_whitespace().collect::<Vec<_>>().join(" "));
            }
            c if c.is_control() => return false,
            c => e.push(c),
        }
        if e.chars().count() > 60 { *e = e.chars().take(60).collect(); }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secciones_y_filas() {
        let mut all = Vec::new();
        for s in SECS {
            let rows = rows_of(s, 7);
            assert!(!rows.is_empty(), "{s:?} sin filas");
            // Cabe en los ids (16 partes por fila, 1000 por sección)
            assert!(rows.len() * 16 < 1000);
            all.extend(rows);
        }
        // Ninguna fila sale dos veces
        for (i, r) in all.iter().enumerate() { assert!(!all[i + 1..].contains(r), "{r:?} repetida"); }
        assert_eq!(rows_of(Sec::Desk, 7).iter().filter(|r| matches!(r, Row::Widget(_))).count(), 7);
        assert_eq!(rows_of(Sec::Desk, 0).iter().filter(|r| matches!(r, Row::Widget(_))).count(), 0);
        assert!(SECS.iter().enumerate().all(|(i, s)| s.index() == i));
    }

    #[test]
    fn ids() {
        for (s, r, sub) in [(0, 0, 0), (5, 3, 2), (2, 9, 15), (3, 61, 7)] {
            assert_eq!(decode(id_of(s, r, sub)), Some((s, r, sub)));
        }
        assert_eq!(decode(ID_SEC0), None);
        // Los de las secciones no se confunden con los de las filas
        assert!(id_of(0, 0, 0) > ID_SEC0 + SECS.len() as u32);
    }

    #[test]
    fn umbral_cpu() {
        assert_eq!(cpu_index(None), 1);
        assert_eq!(cpu_index(Some("basura")), 1);
        assert_eq!(cpu_index(Some("0")), 3);
        assert_eq!(cpu_index(Some("80")), 0);
        assert_eq!(cpu_index(Some(" 90 ")), 1);
        assert_eq!(cpu_index(Some("95")), 2);
        assert_eq!(cpu_index(Some("99")), 2);
        for i in 0..CPU_OPTS.len() { assert_eq!(cpu_index(Some(&cpu_value(i).to_string())), i); }
        assert_eq!(cpu_value(9), 0);
    }

    #[test]
    fn acciones() {
        let mut st = State { widgets: true, each: vec![("a", true), ("b", false)], floating: true, ..State::default() };
        // Color del texto: auto, claro, oscuro → modos 0, 2, 1
        assert_eq!(action(Row::Ink, 1, &st), Act::Nothing);
        assert_eq!(action(Row::Ink, 2, &st), Act::Bar(crate::BAR_INK0 + 2));
        assert_eq!(action(Row::Ink, 3, &st), Act::Bar(crate::BAR_INK0 + 1));
        // Elegir lo que ya está no hace nada
        assert_eq!(action(Row::Position, 1, &st), Act::Nothing);
        assert_eq!(action(Row::Position, 2, &st), Act::BarClose(crate::BAR_POS_BOTTOM));
        assert_eq!(action(Row::Lang, 1, &st), Act::Nothing);
        assert_eq!(action(Row::Lang, 3, &st), Act::Bar(crate::BAR_LANG0 + 2));
        assert_eq!(action(Row::Style, 1, &st), Act::Nothing);
        assert_eq!(action(Row::Style, 2, &st), Act::Style);
        assert_eq!(action(Row::Cpu, 4, &st), Act::Cpu(3));
        assert_eq!(action(Row::Cpu, 5, &st), Act::Nothing);
        assert_eq!(action(Row::Autostart, 0, &st), Act::Autostart(true));
        // Perfiles: uno vacío se guarda sin reiniciar; uno guardado se carga (cierra el panel)
        st.saved = [false, true, false];
        assert_eq!(action(Row::Profile, 1, &st), Act::Bar(crate::BAR_PROFILE0));
        assert_eq!(action(Row::Profile, 2, &st), Act::BarClose(crate::BAR_PROFILE0 + 1));
        assert_eq!(action(Row::ProfileSave, 1, &st), Act::Nothing);
        st.profile = Some(1);
        assert_eq!(action(Row::Profile, 2, &st), Act::Nothing);
        assert_eq!(action(Row::ProfileSave, 1, &st), Act::SaveProfile);
        // Cada widget, solo con los widgets encendidos
        assert_eq!(action(Row::Widget(1), 0, &st), Act::Bar(crate::BAR_WIDGET0 + 1));
        assert_eq!(action(Row::Widget(5), 0, &st), Act::Nothing);
        st.widgets = false;
        assert_eq!(action(Row::Widget(1), 0, &st), Act::Nothing);
        // Acentos: la muestra i es BAR_ACCENT0 + i
        assert_eq!(action(Row::Accent, 2, &st), Act::Bar(crate::BAR_ACCENT0 + 1));
        assert_eq!(action(Row::Accent, profiles::ACCENTS.len() as u32 + 1, &st), Act::Nothing);
    }
}
