//! Paneles flotantes con cristal acrílico: creación, animación de entrada y un marco común para los
//! paneles nuevos (calendario, tiempo y Wi-Fi). Cada panel solo describe su contenido.

use super::*;
use std::collections::HashMap;

pub const TIMER_REVEAL: usize = 90;
const TIMER_TICK_P: usize = 91;
const TIMER_ANIM_P: usize = 92;
const REVEAL_MS: f32 = 320.0;
pub const WM_PANEL_DATA: u32 = WM_USER + 40;

pub fn ease_out(t: f32) -> f32 { let u = 1.0 - t.clamp(0.0, 1.0); 1.0 - u * u * u }

#[repr(C)]
struct AccentPolicy { state: u32, flags: u32, color: u32, anim: u32 }
#[repr(C)]
struct CompAttr { attr: u32, data: *mut core::ffi::c_void, size: usize }

/// Vidrio de verdad: desenfoque acrílico detrás de la ventana (el mismo que usan las barras
/// alternativas). A diferencia del "backdrop" del sistema, funciona en ventanas emergentes sin marco.
fn set_glass(hwnd: HWND) -> bool {
    unsafe {
        let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else { return false };
        let Some(f) = windows::Win32::System::LibraryLoader::GetProcAddress(user32, windows::core::s!("SetWindowCompositionAttribute")) else { return false };
        let f: unsafe extern "system" fn(HWND, *mut CompAttr) -> i32 = std::mem::transmute(f);
        // 3 = BLURBEHIND: desenfoque suave (el acrílico, 4, emborrona y oscurece demasiado).
        // Color en AABBGGRR: casi transparente, como el vidrio de macOS
        let mut ap = AccentPolicy { state: 3, flags: 0, color: 0x10282828, anim: 0 };
        let mut d = CompAttr { attr: 19, data: &mut ap as *mut _ as _, size: size_of::<AccentPolicy>() };
        f(hwnd, &mut d) != 0
    }
}

/// Ventana emergente de vidrio oscuro con esquinas redondeadas de Windows 11. Es "layered" para
/// poder fundirla entera al abrir y cerrar.
pub fn create_window(class: PCWSTR, title: PCWSTR, proc: WNDPROC) -> HWND {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW { lpfnWndProc: proc, hInstance: hinst.into(), lpszClassName: class, hCursor: LoadCursorW(None, IDC_ARROW).unwrap(), ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED, class, title, WS_POPUP, 0, 0, 10, 10, None, None, Some(hinst.into()), None).unwrap();
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
        let round: i32 = 2;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as _, 4);
        let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        if !set_glass(hwnd) {
            let backdrop: i32 = 3;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop as *const _ as _, 4);
        }
        hwnd
    }
}

/// Tinte que cada panel pinta encima del vidrio (suave: el desenfoque ya oscurece).
pub const TINT: D2D1_COLOR_F = D2D1_COLOR_F { r: 0.16, g: 0.17, b: 0.19, a: 0.22 };

/// Brillo de vidrio estilo Apple: un velo blanco muy sutil arriba y un borde de 1 px con luz arriba.
pub fn glass_edge(rt: &ID2D1HwndRenderTarget, w: f32, h: f32, s: f32) {
    unsafe {
        let stops = |a: f32, b: f32| [D2D1_GRADIENT_STOP { position: 0.0, color: rgba(1.0, 1.0, 1.0, a) }, D2D1_GRADIENT_STOP { position: 1.0, color: rgba(1.0, 1.0, 1.0, b) }];
        let grad = |a: f32, b: f32, y1: f32| -> Option<ID2D1LinearGradientBrush> {
            let col = rt.CreateGradientStopCollection(&stops(a, b), D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok()?;
            rt.CreateLinearGradientBrush(&D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: 0.0, Y: 0.0 }, endPoint: Vector2 { X: 0.0, Y: y1 } }, None, &col).ok()
        };
        if let Some(sheen) = grad(0.07, 0.0, h * 0.45) { rt.FillRectangle(&rect(0.0, 0.0, w, h), &sheen); }
        let px = 1.0 / s;
        if let Some(edge) = grad(0.30, 0.08, h) {
            rt.DrawRoundedRectangle(&rrect(rect(px / 2.0, px / 2.0, w - px / 2.0, h - px / 2.0), 8.0), &edge, px, None);
        }
    }
}

/// Render target de un panel (se recrea al cambiar de tamaño).
pub fn make_target(hwnd: HWND) -> Option<ID2D1HwndRenderTarget> {
    unsafe {
        let s = scale_of(hwnd);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let fac = D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let props = D2D1_RENDER_TARGET_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, ..Default::default() };
        let hp = D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd, pixelSize: D2D_SIZE_U { width: (rc.right - rc.left) as u32, height: (rc.bottom - rc.top) as u32 }, presentOptions: D2D1_PRESENT_OPTIONS_NONE };
        let t = fac.CreateHwndRenderTarget(&props, &hp).ok()?;
        t.SetDpi(96.0 * s, 96.0 * s);
        t.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        Some(t)
    }
}

// ---------------------------------------------------------------- animación de entrada
// Animación en curso de cada panel: (inicio, x, y, entrando, opacidad de partida)
thread_local! { static REVEALS: RefCell<HashMap<isize, (Instant, i32, i32, bool, f32)>> = RefCell::new(HashMap::new()); }
const HIDE_MS: f32 = 160.0;

fn cur_alpha(hwnd: HWND) -> f32 {
    let mut a = 0u8;
    unsafe { let _ = GetLayeredWindowAttributes(hwnd, None, Some(&mut a), None); }
    a as f32 / 255.0
}

/// Muestra el panel: baja 10 px hasta su sitio mientras la ventana entera aparece con un fundido.
pub fn begin(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    let s = scale_of(hwnd);
    // Si se estaba cerrando, vuelve a abrirse desde la opacidad que tenga ahora
    let from = if unsafe { IsWindowVisible(hwnd).as_bool() } { cur_alpha(hwnd) } else { 0.0 };
    REVEALS.with(|r| r.borrow_mut().insert(hwnd.0 as isize, (Instant::now(), x, y, true, from)));
    unsafe {
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), (from * 255.0) as u8, LWA_ALPHA);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_SHOWWINDOW);
        let _ = s;
        SetTimer(Some(hwnd), TIMER_REVEAL, 10, None);
        let _ = InvalidateRect(Some(bar_hwnd()), None, false);
    }
}

/// Cierra el panel con un fundido rápido (sube un poco mientras desaparece) y luego lo oculta.
pub fn dismiss(hwnd: HWND) {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || closing(hwnd) { return; }
        let mut r = RECT::default();
        let _ = GetWindowRect(hwnd, &mut r);
        let from = cur_alpha(hwnd);
        REVEALS.with(|m| m.borrow_mut().insert(hwnd.0 as isize, (Instant::now(), r.left, r.top, false, from)));
        SetTimer(Some(hwnd), TIMER_REVEAL, 10, None);
        let _ = InvalidateRect(Some(bar_hwnd()), None, false);
    }
}

/// ¿Se está cerrando (fundido de salida)?
pub fn closing(hwnd: HWND) -> bool { REVEALS.with(|r| r.borrow().get(&(hwnd.0 as isize)).is_some_and(|e| !e.3)) }

/// Visible y no cerrándose: lo que cuenta como "abierto".
pub fn shown(hwnd: HWND) -> bool { let v = unsafe { IsWindowVisible(hwnd).as_bool() }; v && !closing(hwnd) }

/// Avanza la animación (desde WM_TIMER). Devuelve true mientras sigue.
pub fn step(hwnd: HWND) -> bool {
    let Some((t0, _x, _y, entering, from)) = REVEALS.with(|r| r.borrow().get(&(hwnd.0 as isize)).copied()) else { unsafe { let _ = KillTimer(Some(hwnd), TIMER_REVEAL); } return false };
    let ms = t0.elapsed().as_secs_f32() * 1000.0;
    let dur = if entering { REVEAL_MS } else { HIDE_MS };
    let t = (ms / dur).min(1.0);
    unsafe {
        if entering {
            // Como en macOS: la opacidad llega enseguida (150 ms) y la escala termina con un pelín de rebote
            let a = from + (1.0 - from) * ease_out(ms / 150.0);
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), (a * 255.0) as u8, LWA_ALPHA);
            if t >= 1.0 { let _ = KillTimer(Some(hwnd), TIMER_REVEAL); REVEALS.with(|r| r.borrow_mut().remove(&(hwnd.0 as isize))); }
        } else {
            let a = from * (1.0 - ease_out(t));
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), (a * 255.0) as u8, LWA_ALPHA);
            if t >= 1.0 {
                let _ = KillTimer(Some(hwnd), TIMER_REVEAL);
                REVEALS.with(|r| r.borrow_mut().remove(&(hwnd.0 as isize)));
                let _ = ShowWindow(hwnd, SW_HIDE);
                let _ = InvalidateRect(Some(bar_hwnd()), None, false);
            }
        }
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    t < 1.0
}

/// Escala del contenido durante la animación: al abrir crece de 0,94 a 1 con rebote suave; al cerrar
/// se encoge un poco mientras se desvanece. Pivota en el centro de arriba (por donde "sale" del botón).
pub fn content_matrix(hwnd: HWND, w: f32) -> Matrix3x2 {
    let sc = REVEALS.with(|r| r.borrow().get(&(hwnd.0 as isize)).map(|(t0, _, _, entering, _)| {
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        if *entering { 0.94 + 0.06 * crate::ease_back(ms / REVEAL_MS) } else { 1.0 - 0.035 * ease_out(ms / HIDE_MS) }
    })).unwrap_or(1.0);
    let px = w / 2.0;
    Matrix3x2 { M11: sc, M12: 0.0, M21: 0.0, M22: sc, M31: px * (1.0 - sc), M32: 0.0 }
}

/// Segundos desde que empezó a abrirse (solo mientras dura la apertura).
pub fn reveal_elapsed(hwnd: HWND) -> Option<f32> {
    REVEALS.with(|r| r.borrow().get(&(hwnd.0 as isize)).filter(|e| e.3).map(|e| e.0.elapsed().as_secs_f32()))
}

pub const IDENTITY: Matrix3x2 = Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 };
/// La opacidad la lleva ya la ventana entera: el contenido se pinta siempre opaco.
pub fn alpha(_hwnd: HWND) -> f32 { 1.0 }

/// Capa con opacidad para el contenido (D2D permite capa sin objeto desde Windows 8).
pub fn push_alpha(rt: &ID2D1HwndRenderTarget, a: f32) {
    use std::mem::ManuallyDrop;
    let lp = D2D1_LAYER_PARAMETERS {
        contentBounds: rect(-1e6, -1e6, 1e6, 1e6),
        geometricMask: ManuallyDrop::new(None),
        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
        maskTransform: Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 },
        opacity: a,
        opacityBrush: ManuallyDrop::new(None),
        layerOptions: D2D1_LAYER_OPTIONS_NONE,
    };
    unsafe { rt.PushLayer(&lp, None); }
}

/// Capa del contenido que, al abrir, se "despliega" de arriba abajo: un borde suave baja por el panel
/// (en 260 ms) y destapa las filas en cascada, como si salieran de la barra. Fuera de la apertura es
/// una capa normal con opacidad `a`.
pub fn push_reveal(rt: &ID2D1HwndRenderTarget, hwnd: HWND, h: f32, a: f32) {
    use std::mem::ManuallyDrop;
    let t = REVEALS.with(|r| r.borrow().get(&(hwnd.0 as isize)).filter(|e| e.3).map(|e| e.0.elapsed().as_secs_f32() / 0.26));
    let Some(t) = t.filter(|t| *t < 1.0) else { return push_alpha(rt, a) };
    const SOFT: f32 = 70.0;
    let edge = -SOFT + (h + 2.0 * SOFT) * ease_out(t);
    let brush: Option<ID2D1Brush> = unsafe {
        let stops = [
            D2D1_GRADIENT_STOP { position: 0.0, color: rgba(1.0, 1.0, 1.0, 1.0) },
            D2D1_GRADIENT_STOP { position: 1.0, color: rgba(1.0, 1.0, 1.0, 0.0) },
        ];
        rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok().and_then(|col| {
            rt.CreateLinearGradientBrush(&D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: Vector2 { X: 0.0, Y: edge - SOFT }, endPoint: Vector2 { X: 0.0, Y: edge } }, None, &col).ok()
        }).and_then(|b| windows::core::Interface::cast(&b).ok())
    };
    let Some(brush) = brush else { return push_alpha(rt, a) };
    let mut lp = D2D1_LAYER_PARAMETERS {
        contentBounds: rect(-1e6, -1e6, 1e6, 1e6),
        geometricMask: ManuallyDrop::new(None),
        maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
        maskTransform: IDENTITY,
        opacity: a,
        opacityBrush: ManuallyDrop::new(Some(brush)),
        layerOptions: D2D1_LAYER_OPTIONS_NONE,
    };
    unsafe { rt.PushLayer(&lp, None); ManuallyDrop::drop(&mut lp.opacityBrush); }
}

// ---------------------------------------------------------------- marco común
pub struct Fonts {
    pub big: IDWriteTextFormat, pub name: IDWriteTextFormat, pub lbl: IDWriteTextFormat, pub sub: IDWriteTextFormat,
    pub small: IDWriteTextFormat, pub val: IDWriteTextFormat, pub icon: IDWriteTextFormat, pub icon_l: IDWriteTextFormat, pub icon_xl: IDWriteTextFormat,
}

/// Lo que cada panel puede pedir al pulsar algo.
pub enum Re { Stay, Close, CloseRun(Box<dyn FnOnce()>) }

pub trait Content {
    fn size(&self) -> (f32, f32);
    fn paint(&mut self, ui: &mut Ui);
    fn click(&mut self, id: u32, right: bool) -> Re;
    fn shown(&mut self) {}
    /// Cada segundo mientras está abierto. true = repintar.
    fn tick(&mut self) -> bool { false }
    /// Llegaron datos del hilo de trabajo (WM_PANEL_DATA).
    fn data(&mut self) {}
    fn wheel(&mut self, _delta: i32) -> bool { false }
    /// true mientras el contenido tenga una animación propia en marcha (pide fotogramas).
    fn animating(&self) -> bool { false }
}

pub struct Ui<'a> {
    pub p: Painter<'a>,
    pub rt: &'a ID2D1HwndRenderTarget,
    pub f: &'a Fonts,
    #[allow(dead_code)]
    pub w: f32,
    pub s: f32,
    pub hover: Option<u32>,
    /// Brillo de "ratón encima" de cada zona (0..1): se enciende y apaga con fundido.
    pub glow: HashMap<u32, f32>,
    pub hits: Vec<(D2D_RECT_F, u32)>,
    pub white: ID2D1Brush, pub dim: ID2D1Brush, pub faint: ID2D1Brush, pub card: ID2D1Brush, pub card_edge: ID2D1Brush,
    pub off: ID2D1Brush, pub on: ID2D1Brush, pub hov: ID2D1Brush, pub track: ID2D1Brush,
}

impl Ui<'_> {
    pub fn brush(&self, c: D2D1_COLOR_F) -> ID2D1Brush { self.p.brush(c) }
    /// Texto centrado verticalmente (por su altura de mayúscula) en la caja [top, top+bh].
    pub fn text(&self, f: &IDWriteTextFormat, t: &str, x: f32, top: f32, bh: f32, b: &ID2D1Brush) {
        let cap = self.p.text_top(f) - 50.0;
        self.p.draw_at(f, t, x, top + bh / 2.0 + cap, b, None);
    }
    pub fn text_c(&self, f: &IDWriteTextFormat, t: &str, cx: f32, top: f32, bh: f32, b: &ID2D1Brush) {
        let w = self.p.width(f, t);
        self.text(f, t, cx - w / 2.0, top, bh, b);
    }
    pub fn text_r(&self, f: &IDWriteTextFormat, t: &str, right: f32, top: f32, bh: f32, b: &ID2D1Brush) {
        let w = self.p.width(f, t);
        self.text(f, t, right - w, top, bh, b);
    }
    pub fn glyph(&self, f: &IDWriteTextFormat, g: &str, x: f32, top: f32, bw: f32, bh: f32, b: &ID2D1Brush) {
        let [l, t, iw, ih] = self.p.ink(f, g);
        self.p.draw_at(f, g, x + (bw - iw) / 2.0 - l, self.p.snap(top + bh / 2.0 - ih / 2.0 - t), b, None);
    }
    /// Zona de contenido: SIN caja (vidrio limpio, como en macOS). Solo un brillo suave al pasar el ratón.
    pub fn card(&self, r: D2D_RECT_F, hot: bool) {
        if hot { unsafe { self.rt.FillRoundedRectangle(&rrect(r, 10.0), &self.hov); } }
    }
    /// Línea fina que separa secciones.
    pub fn sep(&self, y: f32) {
        let px = 1.0 / self.s;
        unsafe { self.rt.FillRectangle(&rect(16.0, y, self.w - 16.0, y + px), &self.card_edge); }
    }
    pub fn hit(&mut self, r: D2D_RECT_F, id: u32) { self.hits.push((r, id)); }
    pub fn is_hot(&self, id: u32) -> bool { self.hover == Some(id) }
    /// Cuánto brilla ahora una zona por el ratón (0..1, animado).
    pub fn heat(&self, id: u32) -> f32 { self.glow.get(&id).copied().unwrap_or(0.0) }
    /// Brocha blanca con la opacidad dada.
    pub fn wa(&self, a: f32) -> ID2D1Brush { self.p.brush(rgba(1.0, 1.0, 1.0, a)) }
    /// Zona sin caja que se ilumina con fundido según `heat`.
    pub fn card_k(&self, r: D2D_RECT_F, k: f32) {
        if k > 0.01 { unsafe { self.rt.FillRoundedRectangle(&rrect(r, 10.0), &self.wa(0.07 * k)); } }
    }
    /// Botón redondeado con texto (y glifo opcional) que reacciona al ratón.
    pub fn button(&mut self, r: D2D_RECT_F, glyph: Option<&str>, label: &str, id: u32, active: bool) {
        let k = self.heat(id);
        unsafe {
            let rad = ((r.bottom - r.top) / 2.0).min(10.0);
            if active { self.rt.FillRoundedRectangle(&rrect(r, rad), &self.on); }
            else { self.rt.FillRoundedRectangle(&rrect(r, rad), &self.wa(0.075 + 0.075 * k)); }
        }
        let lw = self.p.width(&self.f.lbl, label);
        let gw = match (glyph.is_some(), label.is_empty()) { (true, true) => 16.0, (true, false) => 20.0, _ => 0.0 };
        let x0 = (r.left + r.right) / 2.0 - (lw + gw) / 2.0;
        if let Some(g) = glyph { self.glyph(&self.f.icon, g, x0, r.top, 16.0, r.bottom - r.top, &self.white); }
        self.text(&self.f.lbl, label, x0 + gw, r.top, r.bottom - r.top, &self.white);
        self.hit(r, id);
    }
}

// ---------------------------------------------------------------- ventanas de los paneles
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind { Calendar, Weather, Wifi }

struct Pw {
    hwnd: HWND,
    kind: Kind,
    target: Option<ID2D1HwndRenderTarget>,
    dw: IDWriteFactory,
    cache: RefCell<Caches>,
    f: Fonts,
    hits: Vec<(D2D_RECT_F, u32)>,
    hover: Option<u32>,
    content: Option<Box<dyn Content>>,
    anchor_right: i32,
    size: (f32, f32),
    hidden_at: Option<Instant>,
    return_to: HWND,
    glow: HashMap<u32, f32>,
    last_paint: Instant,
}

thread_local! { static PANELS: RefCell<Vec<Pw>> = const { RefCell::new(Vec::new()) }; }
static HWNDS: Mutex<Vec<(Kind, isize)>> = Mutex::new(Vec::new());

/// Ventana de un panel (para que su hilo de trabajo le avise con WM_PANEL_DATA).
pub fn hwnd_of(k: Kind) -> Option<HWND> { HWNDS.lock().unwrap().iter().find(|(x, _)| *x == k).map(|(_, h)| HWND(*h as _)) }

pub fn notify(k: Kind) { if let Some(h) = hwnd_of(k) { unsafe { let _ = PostMessageW(Some(h), WM_PANEL_DATA, WPARAM(0), LPARAM(0)); } } }

/// Panel abierto ahora mismo (para resaltar su botón en la barra).
pub fn open_kind() -> Option<Kind> {
    PANELS.with(|p| p.borrow().iter().find(|w| shown(w.hwnd)).map(|w| w.kind))
}

fn new_content(k: Kind) -> Box<dyn Content> {
    match k {
        Kind::Calendar => Box::new(calendar::Calendar::new()),
        Kind::Weather => Box::new(weather::WeatherPanel::new()),
        Kind::Wifi => Box::new(wifi::WifiPanel::new()),
    }
}

/// Crea de antemano las ventanas (ocultas): al pulsar, el panel aparece sin esperar.
pub fn prewarm() { for k in [Kind::Calendar, Kind::Weather, Kind::Wifi] { ensure(k); } }

fn ensure(k: Kind) {
    if PANELS.with(|p| p.borrow().iter().any(|w| w.kind == k)) { return; }
    let hwnd = create_window(w!("MacBarPanel"), w!("macbar"), Some(wndproc));
    let dw: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).unwrap() };
    let mk = |fam, size, wt| make_format(&dw, fam, size, wt);
    let f = Fonts {
        big: mk(FONT, 30.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
        name: mk(FONT, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
        lbl: mk(FONT, 12.5, DWRITE_FONT_WEIGHT_SEMI_BOLD),
        sub: mk(FONT, 11.5, DWRITE_FONT_WEIGHT_NORMAL),
        small: mk(FONT, 11.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
        val: mk(FONT, 15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
        icon: mk(ICONS, 14.0, DWRITE_FONT_WEIGHT_NORMAL),
        icon_l: mk(ICONS, 16.0, DWRITE_FONT_WEIGHT_NORMAL),
        icon_xl: mk(ICONS, 30.0, DWRITE_FONT_WEIGHT_NORMAL),
    };
    HWNDS.lock().unwrap().push((k, hwnd.0 as isize));
    let pw = Pw { hwnd, kind: k, target: None, dw, cache: RefCell::new(Caches::default()), f, hits: Vec::new(), hover: None,
        content: Some(new_content(k)), anchor_right: 0, size: (300.0, 300.0), hidden_at: None, return_to: HWND::default(), glow: HashMap::new(), last_paint: Instant::now() };
    PANELS.with(|p| p.borrow_mut().push(pw));
}

fn with_pw<R>(hwnd: HWND, f: impl FnOnce(&mut Pw) -> R) -> Option<R> {
    PANELS.with(|p| p.borrow_mut().iter_mut().find(|w| w.hwnd == hwnd).map(f))
}

/// Abre o cierra un panel. `anchor_right`: borde derecho del botón, en píxeles de pantalla.
pub fn toggle(k: Kind, anchor_right: i32) {
    ensure(k);
    let Some(hwnd) = hwnd_of(k) else { return };
    let vis = shown(hwnd);
    if vis { hide(hwnd, true); return; }
    let just = with_pw(hwnd, |w| w.hidden_at.is_some_and(|t| t.elapsed() < Duration::from_millis(350))).unwrap_or(false);
    if just { return; }
    // Solo un panel a la vez
    let others: Vec<HWND> = PANELS.with(|p| p.borrow().iter().filter(|w| w.kind != k).map(|w| w.hwnd).collect());
    for o in others { hide(o, false); }
    let size = with_pw(hwnd, |w| {
        w.anchor_right = anchor_right;
        w.return_to = last_fg();
        w.hover = None;
        w.glow.clear();
        if let Some(c) = w.content.as_mut() { c.shown(); w.size = c.size(); }
        w.target = None;
        w.size
    });
    let Some((pw, ph)) = size else { return };
    let s = scale_of(bar_hwnd());
    let (wpx, hpx) = ((pw * s) as i32, (ph * s) as i32);
    let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let x = (anchor_right - wpx).clamp((8.0 * s) as i32, sw - wpx - (8.0 * s) as i32);
    begin(hwnd, x, ((BAR_H + 4.0) * s) as i32, wpx, hpx);
    unsafe {
        let _ = SetForegroundWindow(hwnd);
        SetTimer(Some(hwnd), TIMER_TICK_P, 1000, None);
    }
}

/// Cierra todos los paneles de este marco.
pub fn hide_all() {
    let all: Vec<HWND> = PANELS.with(|p| p.borrow().iter().map(|w| w.hwnd).collect());
    for h in all { hide(h, false); }
}

/// Oculta un panel. Las llamadas a Windows van fuera del préstamo (ShowWindow manda WM_ACTIVATE al momento).
fn hide(hwnd: HWND, restore: bool) {
    let ret = with_pw(hwnd, |w| { w.hidden_at = Some(Instant::now()); w.return_to });
    unsafe {
        if !shown(hwnd) { return; }
        let _ = KillTimer(Some(hwnd), TIMER_TICK_P);
        dismiss(hwnd);
        let _ = InvalidateRect(Some(bar_hwnd()), None, false);
        if restore {
            if let Some(r) = ret {
                let fg = GetForegroundWindow();
                if (fg.0.is_null() || sys::window_pid(fg) == std::process::id()) && !r.0.is_null() { let _ = SetForegroundWindow(r); }
            }
        }
    }
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        BeginPaint(hwnd, &mut ps);
        let a = alpha(hwnd);
        with_pw(hwnd, |w| {
            if w.target.is_none() { w.target = make_target(hwnd); }
            let Some(rt) = w.target.clone() else { return };
            let s = scale_of(hwnd);
            let (pw, ph) = w.size;
            // Fundido del brillo de "ratón encima": rápido al entrar, algo más lento al salir
            let dt = w.last_paint.elapsed().as_secs_f32().min(1.0 / 30.0);
            w.last_paint = Instant::now();
            if let Some(h) = w.hover { w.glow.entry(h).or_insert(0.0); }
            let mut moving = false;
            for (id, v) in w.glow.iter_mut() {
                let t = if w.hover == Some(*id) { 1.0 } else { 0.0 };
                let tau = if t > *v { 0.05 } else { 0.14 };
                if (t - *v).abs() < 0.01 { *v = t; } else { *v += (t - *v) * (1.0 - (-dt / tau).exp()); moving = true; }
            }
            w.glow.retain(|_, v| *v > 0.0);
            let p = Painter { rt: &rt, dw: &w.dw, cache: &w.cache, h: 100.0, s };
            let mut ui = Ui {
                glow: w.glow.clone(),
                white: p.brush(rgba(1.0, 1.0, 1.0, 0.95)), dim: p.brush(rgba(1.0, 1.0, 1.0, 0.55)), faint: p.brush(rgba(1.0, 1.0, 1.0, 0.30)),
                card: p.brush(rgba(1.0, 1.0, 1.0, 0.075)), card_edge: p.brush(rgba(1.0, 1.0, 1.0, 0.09)),
                off: p.brush(rgba(1.0, 1.0, 1.0, 0.13)), on: p.brush(rgba(0.23, 0.51, 0.96, 1.0)),
                hov: p.brush(rgba(1.0, 1.0, 1.0, 0.07)), track: p.brush(rgba(1.0, 1.0, 1.0, 0.14)),
                p, rt: &rt, f: &w.f, w: pw, s, hover: w.hover, hits: Vec::new(),
            };
            rt.BeginDraw();
            rt.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
            let tint = ui.brush(TINT);
            rt.FillRectangle(&rect(0.0, 0.0, pw, ph), &tint);
            glass_edge(&rt, pw, ph, s);
            rt.SetTransform(&content_matrix(hwnd, pw));
            push_reveal(&rt, hwnd, ph, a);
            if let Some(c) = w.content.as_mut() { c.paint(&mut ui); }
            rt.PopLayer();
            if moving || w.content.as_ref().is_some_and(|c| c.animating()) || REVEALS.with(|r| r.borrow().contains_key(&(hwnd.0 as isize))) {
                SetTimer(Some(hwnd), TIMER_ANIM_P, 15, None);
            }
            rt.SetTransform(&IDENTITY);
            w.hits = std::mem::take(&mut ui.hits);
            drop(ui);
            if rt.EndDraw(None, None).is_err() { w.target = None; }
        });
        let _ = EndPaint(hwnd, &ps);
    }
}

fn hit_at(hwnd: HWND, lp: LPARAM) -> Option<u32> {
    let s = scale_of(hwnd);
    let x = (lp.0 & 0xFFFF) as i16 as f32 / s;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / s;
    with_pw(hwnd, |w| w.hits.iter().rev().find(|(r, _)| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom).map(|(_, i)| *i)).flatten()
}

/// Si el contenido cambió de tamaño, la ventana se ajusta (anclada a su borde derecho).
fn resize_if_needed(hwnd: HWND) {
    let r = with_pw(hwnd, |w| {
        let n = w.content.as_ref().map(|c| c.size()).unwrap_or(w.size);
        let ch = n != w.size;
        if ch { w.size = n; w.target = None; }
        (ch, n, w.anchor_right)
    });
    let Some((true, (pw, ph), ar)) = r else { return };
    unsafe {
        let s = scale_of(hwnd);
        let mut wr = RECT::default();
        let _ = GetWindowRect(hwnd, &mut wr);
        let (wpx, hpx) = ((pw * s) as i32, (ph * s) as i32);
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let x = (ar - wpx).clamp((8.0 * s) as i32, sw - wpx - (8.0 * s) as i32);
        let _ = SetWindowPos(hwnd, None, x, wr.top, wpx, hpx, SWP_NOZORDER | SWP_NOACTIVATE);
        REVEALS.with(|r| if let Some(e) = r.borrow_mut().get_mut(&(hwnd.0 as isize)) { e.1 = x; });
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => { paint(hwnd); LRESULT(0) }
            WM_ERASEBKGND => LRESULT(1),
            WM_ACTIVATE => { if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE { hide(hwnd, false); } LRESULT(0) }
            WM_KEYDOWN => { if wp.0 as u16 == VK_ESCAPE.0 { hide(hwnd, true); } LRESULT(0) }
            WM_TIMER => {
                if wp.0 == TIMER_REVEAL { step(hwnd); }
                else if wp.0 == TIMER_ANIM_P { let _ = KillTimer(Some(hwnd), TIMER_ANIM_P); let _ = InvalidateRect(Some(hwnd), None, false); }
                else if wp.0 == TIMER_TICK_P {
                    let r = with_pw(hwnd, |w| w.content.as_mut().map(|c| c.tick()).unwrap_or(false)).unwrap_or(false);
                    if r { resize_if_needed(hwnd); let _ = InvalidateRect(Some(hwnd), None, false); }
                }
                LRESULT(0)
            }
            WM_PANEL_DATA => {
                with_pw(hwnd, |w| if let Some(c) = w.content.as_mut() { c.data(); });
                resize_if_needed(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let h = hit_at(hwnd, lp);
                let ch = with_pw(hwnd, |w| { let c = w.hover != h; w.hover = h; c }).unwrap_or(false);
                if ch { let _ = InvalidateRect(Some(hwnd), None, false); }
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
                LRESULT(0)
            }
            0x02A3 => { with_pw(hwnd, |w| w.hover = None); let _ = InvalidateRect(Some(hwnd), None, false); LRESULT(0) }
            WM_MOUSEWHEEL => {
                let d = ((wp.0 >> 16) & 0xFFFF) as i16 as i32;
                let r = with_pw(hwnd, |w| w.content.as_mut().map(|c| c.wheel(d)).unwrap_or(false)).unwrap_or(false);
                if r { resize_if_needed(hwnd); let _ = InvalidateRect(Some(hwnd), None, false); }
                LRESULT(0)
            }
            WM_LBUTTONUP | WM_RBUTTONUP => {
                if let Some(id) = hit_at(hwnd, lp) {
                    // El contenido decide; las llamadas a Windows van después, fuera del préstamo
                    let re = with_pw(hwnd, |w| w.content.as_mut().map(|c| c.click(id, msg == WM_RBUTTONUP))).flatten();
                    match re {
                        Some(Re::Close) => hide(hwnd, true),
                        Some(Re::CloseRun(f)) => { hide(hwnd, false); f(); }
                        _ => { resize_if_needed(hwnd); let _ = InvalidateRect(Some(hwnd), None, false); }
                    }
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
