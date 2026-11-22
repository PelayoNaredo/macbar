//! Campo de texto de una línea dibujado con Direct2D, para los paneles de macbar (hoy, la ventana de
//! Configuración). Caja redondeada sutil, caret del color de acento que parpadea, selección, deshacer y
//! portapapeles, como un campo de Windows pero con el aspecto de macbar.
//!
//! # Uso
//!
//! El panel guarda un `TextField` por campo y le pasa lo que le llega (ver `popup::Content`):
//!
//! ```ignore
//! let mut f = TextField::new("Buscar…", 80);          // texto de ayuda (en español: se traduce) y tope
//! f.paint(ui, rect, ui.f.lbl.clone(), ID_CAMPO);        // en paint: pinta y apunta su zona
//! f.mouse_down(x, y, shift)                             // en mouse(Down): true si cayó dentro (lo enfoca)
//! f.mouse_move(x) / f.mouse_up()                        // para seleccionar arrastrando
//! f.key(vk)   -> Option<Ev>                             // en key(): None = esa tecla no es suya
//! f.chr(c)    -> Ev                                     // en chr(): caracteres escritos (también del IME)
//! f.wake_ms() / f.caret_point()                         // para Content::wake_ms y Content::caret
//! ```
//!
//! `Ev::Changed` dice que cambió el texto; `Ev::Submit` (Intro) y `Ev::Cancel` (Esc) los decide el panel.
//! Teclas: flechas (Ctrl = por palabras, Mayús = seleccionar), Inicio/Fin, Retroceso/Supr (Ctrl = palabra),
//! Ctrl+A/C/X/V, Ctrl+Z/Ctrl+Y (o Ctrl+Mayús+Z). Doble clic selecciona la palabra y triple clic, todo.

use super::*;
use popup::Ui;

/// Lo que pasó tras una tecla o un carácter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ev {
    /// Nada que hacer (pero se repinta).
    None,
    /// Solo se movió el caret o la selección.
    Moved,
    /// Cambió el texto.
    Changed,
    /// Intro.
    Submit,
    /// Esc.
    Cancel,
}

/// Tipo de cambio, para juntar en un solo «deshacer» lo que se escribe seguido.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step { Typing, Deleting, Other }

#[derive(Clone, Debug)]
struct Snap { chars: Vec<char>, caret: usize, anchor: usize }

/// Medio periodo del parpadeo del caret (el de Windows).
const BLINK_MS: u128 = 530;
const PAD: f32 = 10.0;

pub struct TextField {
    chars: Vec<char>,
    /// Posiciones en caracteres: donde está el caret y donde empezó la selección.
    caret: usize,
    anchor: usize,
    focused: bool,
    placeholder: &'static str,
    max: usize,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    last: Step,
    /// El caret se ve fijo un momento tras cada tecla o clic.
    blink0: Instant,
    /// Desplazamiento horizontal del texto (si no cabe, se ve donde está el caret).
    scroll: f32,
    /// Posición x de cada frontera entre caracteres (n + 1), medida con DirectWrite para este texto.
    xs: Vec<f32>,
    xs_for: String,
    /// Dónde se pintó por última vez (caja y origen x del texto).
    rect: D2D_RECT_F,
    text_x: f32,
    selecting: bool,
    clicks: (Option<Instant>, u8),
}

impl TextField {
    pub fn new(placeholder: &'static str, max: usize) -> Self {
        TextField {
            chars: Vec::new(), caret: 0, anchor: 0, focused: false, placeholder, max: max.max(1),
            undo: Vec::new(), redo: Vec::new(), last: Step::Other, blink0: Instant::now(), scroll: 0.0,
            xs: vec![0.0], xs_for: String::new(), rect: rect(0.0, 0.0, 0.0, 0.0), text_x: 0.0, selecting: false, clicks: (None, 0),
        }
    }

    // ------------------------------------------------------------ estado
    pub fn text(&self) -> String { self.chars.iter().collect() }
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool { self.chars.is_empty() }
    #[allow(dead_code)]
    pub fn focused(&self) -> bool { self.focused }
    pub fn focus(&mut self) { if !self.focused { self.focused = true; self.blink0 = Instant::now(); } }
    pub fn blur(&mut self) { self.focused = false; self.selecting = false; self.anchor = self.caret; }
    /// Cambia el texto entero (el caret, al final; el deshacer empieza de cero).
    pub fn set_text(&mut self, s: &str) {
        self.chars = s.chars().filter(|c| !c.is_control()).take(self.max).collect();
        self.caret = self.chars.len();
        self.anchor = self.caret;
        self.undo.clear();
        self.redo.clear();
        self.last = Step::Other;
    }
    /// Selección (desde, hasta) en caracteres, ordenada.
    pub fn selection(&self) -> (usize, usize) { (self.caret.min(self.anchor), self.caret.max(self.anchor)) }
    pub fn selected_text(&self) -> String { let (a, b) = self.selection(); self.chars[a..b].iter().collect() }
    #[allow(dead_code)]
    pub fn caret_pos(&self) -> usize { self.caret }

    // ------------------------------------------------------------ edición (sin Windows: se prueba sola)
    fn snap(&self) -> Snap { Snap { chars: self.chars.clone(), caret: self.caret, anchor: self.anchor } }
    /// Guarda el estado para deshacer, salvo si sigue la misma racha (escribir una palabra = un paso).
    fn checkpoint(&mut self, step: Step) {
        let joined = step != Step::Other && step == self.last
            && !(step == Step::Typing && self.caret > 0 && self.chars.get(self.caret - 1).is_some_and(|c| c.is_whitespace()));
        if !joined {
            self.undo.push(self.snap());
            if self.undo.len() > 100 { self.undo.remove(0); }
        }
        self.redo.clear();
        self.last = step;
    }
    fn cut_selection(&mut self) -> bool {
        let (a, b) = self.selection();
        if a == b { return false; }
        self.chars.drain(a..b);
        self.caret = a;
        self.anchor = a;
        true
    }
    /// Escribe `s` en el caret (sustituye la selección). Los saltos de línea pasan a espacios.
    pub fn insert(&mut self, s: &str) -> Ev {
        let clean: Vec<char> = s.chars().map(|c| if c == '\n' || c == '\t' { ' ' } else { c }).filter(|c| !c.is_control()).collect();
        let (a, b) = self.selection();
        let room = self.max.saturating_sub(self.chars.len() - (b - a));
        let clean: Vec<char> = clean.into_iter().take(room).collect();
        if clean.is_empty() && a == b { return Ev::None; }
        let typing = clean.len() == 1 && a == b;
        self.checkpoint(if typing { Step::Typing } else { Step::Other });
        self.cut_selection();
        let n = clean.len();
        self.chars.splice(self.caret..self.caret, clean);
        self.caret += n;
        self.anchor = self.caret;
        self.blink0 = Instant::now();
        Ev::Changed
    }
    /// Retroceso (con `word`, la palabra anterior entera).
    pub fn backspace(&mut self, word: bool) -> Ev {
        let from = if self.caret != self.anchor { self.anchor } else if self.caret == 0 { return Ev::None } else if word { self.word_left(self.caret) } else { self.caret - 1 };
        // El deshacer guarda el estado de antes (sin la selección que se va a borrar)
        self.checkpoint(Step::Deleting);
        self.anchor = from;
        self.cut_selection();
        self.blink0 = Instant::now();
        Ev::Changed
    }
    /// Supr (con `word`, hasta el final de la palabra).
    pub fn delete(&mut self, word: bool) -> Ev {
        let to = if self.caret != self.anchor { self.anchor } else if self.caret >= self.chars.len() { return Ev::None } else if word { self.word_right(self.caret) } else { self.caret + 1 };
        self.checkpoint(Step::Deleting);
        self.anchor = to;
        self.cut_selection();
        self.blink0 = Instant::now();
        Ev::Changed
    }
    fn is_word(c: char) -> bool { c.is_alphanumeric() || c == '_' }
    fn word_left(&self, mut i: usize) -> usize {
        while i > 0 && !Self::is_word(self.chars[i - 1]) { i -= 1; }
        while i > 0 && Self::is_word(self.chars[i - 1]) { i -= 1; }
        i
    }
    fn word_right(&self, mut i: usize) -> usize {
        let n = self.chars.len();
        while i < n && !Self::is_word(self.chars[i]) { i += 1; }
        while i < n && Self::is_word(self.chars[i]) { i += 1; }
        i
    }
    /// Lleva el caret a `pos` (con `extend`, alarga la selección).
    pub fn move_to(&mut self, pos: usize, extend: bool) -> Ev {
        self.caret = pos.min(self.chars.len());
        if !extend { self.anchor = self.caret; }
        self.last = Step::Other;
        self.blink0 = Instant::now();
        Ev::Moved
    }
    pub fn left(&mut self, word: bool, extend: bool) -> Ev {
        // Sin Mayús y con selección, la flecha lleva al borde de la selección
        if !extend && self.caret != self.anchor && !word { let a = self.selection().0; return self.move_to(a, false); }
        let to = if word { self.word_left(self.caret) } else { self.caret.saturating_sub(1) };
        self.move_to(to, extend)
    }
    pub fn right(&mut self, word: bool, extend: bool) -> Ev {
        if !extend && self.caret != self.anchor && !word { let b = self.selection().1; return self.move_to(b, false); }
        let to = if word { self.word_right(self.caret) } else { self.caret + 1 };
        self.move_to(to, extend)
    }
    pub fn select_all(&mut self) -> Ev { self.anchor = 0; self.caret = self.chars.len(); self.last = Step::Other; Ev::Moved }
    /// Selecciona la palabra que hay en `i` (doble clic).
    pub fn select_word(&mut self, i: usize) -> Ev {
        let i = i.min(self.chars.len());
        let on_word = self.chars.get(i).is_some_and(|c| Self::is_word(*c)) || (i > 0 && self.chars.get(i - 1).is_some_and(|c| Self::is_word(*c)));
        if !on_word { return self.move_to(i, false); }
        let mut a = i;
        while a > 0 && Self::is_word(self.chars[a - 1]) { a -= 1; }
        let mut b = i;
        while b < self.chars.len() && Self::is_word(self.chars[b]) { b += 1; }
        self.anchor = a;
        self.caret = b;
        self.last = Step::Other;
        Ev::Moved
    }
    pub fn undo(&mut self) -> Ev {
        let Some(s) = self.undo.pop() else { return Ev::None };
        self.redo.push(self.snap());
        self.restore(s)
    }
    pub fn redo(&mut self) -> Ev {
        let Some(s) = self.redo.pop() else { return Ev::None };
        self.undo.push(self.snap());
        self.restore(s)
    }
    fn restore(&mut self, s: Snap) -> Ev {
        self.chars = s.chars;
        self.caret = s.caret.min(self.chars.len());
        self.anchor = s.anchor.min(self.chars.len());
        self.last = Step::Other;
        self.blink0 = Instant::now();
        Ev::Changed
    }
    /// Quita la selección y la devuelve (para Ctrl+X).
    pub fn cut(&mut self) -> Option<String> {
        let s = self.selected_text();
        if s.is_empty() { return None; }
        self.checkpoint(Step::Other);
        self.cut_selection();
        Some(s)
    }

    // ------------------------------------------------------------ teclado y ratón
    /// Tecla pulsada (WM_KEYDOWN). Lee Ctrl y Mayús del teclado. None = no es suya (↑ ↓, Tab…).
    pub fn key(&mut self, vk: u16) -> Option<Ev> {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
        let (ctrl, shift) = unsafe { (GetKeyState(VK_CONTROL.0 as i32) < 0, GetKeyState(VK_SHIFT.0 as i32) < 0) };
        let ev = self.key_with(vk, ctrl, shift)?;
        // El portapapeles, aquí (lo de arriba no toca Windows y se prueba sin él)
        match (ctrl, vk) {
            (true, 0x43) => { let s = self.selected_text(); if !s.is_empty() { deskw::copy_text(&s); } }
            (true, 0x58) => { if let Some(s) = self.cut() { deskw::copy_text(&s); return Some(Ev::Changed); } }
            (true, 0x56) => { if let Some(p) = search::read_clipboard() { return Some(self.insert(&p.split_whitespace().collect::<Vec<_>>().join(" "))); } }
            _ => {}
        }
        Some(ev)
    }

    /// Como `key`, con Ctrl y Mayús dados (Ctrl+C/X/V devuelven `Ev::None`: el portapapeles lo pone `key`).
    pub fn key_with(&mut self, vk: u16, ctrl: bool, shift: bool) -> Option<Ev> {
        if !self.focused { return None; }
        self.blink0 = Instant::now();
        let ev = match vk {
            0x25 /* ← */ => self.left(ctrl, shift),
            0x27 /* → */ => self.right(ctrl, shift),
            0x24 /* Inicio */ => self.move_to(0, shift),
            0x23 /* Fin */ => self.move_to(self.chars.len(), shift),
            0x08 /* Retroceso */ => self.backspace(ctrl),
            0x2E /* Supr */ => self.delete(ctrl),
            0x0D /* Intro */ => Ev::Submit,
            0x1B /* Esc */ => Ev::Cancel,
            0x41 if ctrl => self.select_all(),
            0x5A if ctrl && shift => self.redo(),
            0x5A if ctrl => self.undo(),
            0x59 if ctrl => self.redo(),
            0x43 | 0x58 | 0x56 if ctrl => Ev::None,
            // Las letras sin Ctrl llegan como carácter (WM_CHAR); con Ctrl no se escriben
            _ if ctrl => Ev::None,
            0x10..=0x12 | 0x14 => Ev::None,
            _ => return None,
        };
        Some(ev)
    }

    /// Carácter escrito (WM_CHAR, también lo que compone el IME). Los de control los lleva `key`.
    pub fn chr(&mut self, c: char) -> Ev {
        // (Las mitades sueltas de un par sustituto no llegan: char::from_u32 ya las descarta)
        if !self.focused || c.is_control() { return Ev::None; }
        self.insert(&c.to_string())
    }

    /// Carácter más cercano a la x de la pantalla.
    fn index_at(&self, x: f32) -> usize {
        let rel = x - self.text_x + self.scroll;
        let mut best = 0;
        let mut bd = f32::MAX;
        for (i, &px) in self.xs.iter().enumerate() {
            let d = (px - rel).abs();
            if d < bd { bd = d; best = i; }
        }
        best.min(self.chars.len())
    }
    pub fn contains(&self, x: f32, y: f32) -> bool { x >= self.rect.left && x <= self.rect.right && y >= self.rect.top && y <= self.rect.bottom }

    /// Botón pulsado en (x, y). true si cayó dentro: el campo se enfoca y pone ahí el caret (doble clic,
    /// la palabra; triple, todo).
    pub fn mouse_down(&mut self, x: f32, y: f32, shift: bool) -> bool {
        if !self.contains(x, y) { return false; }
        let now = Instant::now();
        let n = match self.clicks { (Some(t), n) if now.duration_since(t) < Duration::from_millis(450) => n % 3 + 1, _ => 1 };
        self.clicks = (Some(now), n);
        self.focus();
        let i = self.index_at(x);
        match n {
            2 => { self.select_word(i); }
            3 => { self.select_all(); }
            _ => { self.move_to(i, shift); self.selecting = true; }
        }
        self.blink0 = now;
        true
    }
    /// Ratón movido con el botón pulsado (selección arrastrando). true = repintar.
    pub fn mouse_move(&mut self, x: f32) -> bool {
        if !self.selecting { return false; }
        let i = self.index_at(x);
        if i == self.caret { return false; }
        self.caret = i;
        true
    }
    pub fn mouse_up(&mut self) { self.selecting = false; }

    /// Cuánto falta para el próximo cambio del caret (solo con el campo enfocado).
    pub fn wake_ms(&self) -> Option<u32> {
        if !self.focused { return None; }
        let e = self.blink0.elapsed().as_millis();
        Some((BLINK_MS - e % BLINK_MS) as u32 + 5)
    }
    /// Caret en pantalla: (x, arriba, alto), para la ventana del IME.
    pub fn caret_point(&self) -> Option<(f32, f32, f32)> {
        if !self.focused { return None; }
        let x = self.text_x + self.xs.get(self.caret).copied().unwrap_or(0.0) - self.scroll;
        let h = self.rect.bottom - self.rect.top;
        Some((x, self.rect.top + h * 0.2, h * 0.6))
    }

    // ------------------------------------------------------------ dibujo
    /// Mide las fronteras entre caracteres con DirectWrite (una vez por texto).
    fn measure(&mut self, ui: &Ui, f: &IDWriteTextFormat) {
        let s = self.text();
        if s == self.xs_for && self.xs.len() == self.chars.len() + 1 { return; }
        let mut xs = Vec::with_capacity(self.chars.len() + 1);
        let lay = ui.p.layout(f, &s);
        let mut u16pos = 0u32;
        for c in &self.chars {
            let (mut px, mut py) = (0.0f32, 0.0f32);
            let mut m = DWRITE_HIT_TEST_METRICS::default();
            unsafe { let _ = lay.HitTestTextPosition(u16pos, false, &mut px, &mut py, &mut m); }
            xs.push(px);
            u16pos += c.len_utf16() as u32;
        }
        xs.push(if s.is_empty() { 0.0 } else { ui.p.width(f, &s) });
        // Por si la maquetación devuelve algo fuera de orden (texto de derecha a izquierda): monótono
        for i in 1..xs.len() { if xs[i] < xs[i - 1] { xs[i] = xs[i - 1]; } }
        self.xs = xs;
        self.xs_for = s;
    }

    /// Pinta el campo en `r` y apunta su zona `id`. `glyph`: icono a la izquierda (lupa), opcional.
    pub fn paint(&mut self, ui: &mut Ui, r: D2D_RECT_F, f: &IDWriteTextFormat, glyph: Option<&str>, id: u32) {
        let rt = ui.rt;
        self.rect = r;
        let h = r.bottom - r.top;
        let k = ui.heat(id);
        unsafe {
            rt.FillRoundedRectangle(&rrect(r, 7.0), &ui.wa(0.075 + 0.035 * k));
            if self.focused { rt.DrawRoundedRectangle(&rrect(r, 7.0), &ui.on, 1.5, None); }
        }
        let mut tx = r.left + PAD;
        if let Some(g) = glyph {
            ui.glyph(&ui.f.icon, g, r.left + 6.0, r.top, 20.0, h, &ui.dim);
            tx = r.left + 30.0;
        }
        self.text_x = tx;
        let room = (r.right - PAD - tx).max(10.0);
        self.measure(ui, f);
        let caret_x = self.xs.get(self.caret).copied().unwrap_or(0.0);
        let total = *self.xs.last().unwrap_or(&0.0);
        // El caret siempre a la vista; si el texto cabe, sin desplazar
        if caret_x - self.scroll > room - 2.0 { self.scroll = caret_x - room + 2.0; }
        if caret_x - self.scroll < 0.0 { self.scroll = caret_x; }
        self.scroll = self.scroll.clamp(0.0, (total - room + 2.0).max(0.0));
        unsafe { rt.PushAxisAlignedClip(&rect(tx - 2.0, r.top + 1.0, r.right - PAD + 2.0, r.bottom - 1.0), D2D1_ANTIALIAS_MODE_ALIASED); }
        if self.chars.is_empty() {
            if !self.placeholder.is_empty() { ui.text(f, i18n::t(self.placeholder), tx, r.top, h, &ui.faint); }
        } else {
            let (a, b) = self.selection();
            if a != b && self.focused {
                let (x0, x1) = (tx + self.xs[a] - self.scroll, tx + self.xs[b] - self.scroll);
                unsafe { rt.FillRoundedRectangle(&rrect(rect(x0, r.top + 5.0, x1, r.bottom - 5.0), 2.0), &ui.brush(profiles::accent(0.32))); }
            }
            ui.text(f, &self.text(), tx - self.scroll, r.top, h, &ui.white);
        }
        let on = self.blink0.elapsed().as_millis() / BLINK_MS % 2 == 0;
        if self.focused && on {
            let x = (tx + caret_x - self.scroll).min(r.right - PAD);
            let cy = (r.top + r.bottom) / 2.0;
            unsafe { rt.FillRectangle(&rect(x, cy - 8.0, x + 1.5, cy + 8.0), &ui.on); }
        }
        unsafe { rt.PopAxisAlignedClip(); }
        ui.hit(r, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tf(s: &str) -> TextField { let mut t = TextField::new("", 50); t.set_text(s); t.focus(); t }

    #[test]
    fn escribir_y_borrar() {
        let mut t = tf("");
        for c in "hola mundo".chars() { assert_eq!(t.chr(c), Ev::Changed); }
        assert_eq!(t.text(), "hola mundo");
        assert_eq!(t.caret_pos(), 10);
        t.backspace(false);
        assert_eq!(t.text(), "hola mund");
        t.backspace(true);
        assert_eq!(t.text(), "hola ");
        t.backspace(true);
        assert_eq!(t.text(), "");
        assert_eq!(t.backspace(false), Ev::None);
        // Los de control no se escriben; sin foco, nada
        assert_eq!(t.chr('\u{8}'), Ev::None);
        t.blur();
        assert_eq!(t.chr('x'), Ev::None);
        assert_eq!(t.key_with(0x25, false, false), None);
    }

    #[test]
    fn caret_y_palabras() {
        let mut t = tf("uno dos, tres");
        t.move_to(0, false);
        t.right(true, false);
        assert_eq!(t.caret_pos(), 3);
        t.right(true, false);
        assert_eq!(t.caret_pos(), 7);
        t.left(true, false);
        assert_eq!(t.caret_pos(), 4);
        t.delete(true);
        assert_eq!(t.text(), "uno , tres");
        t.key_with(0x23, false, false);
        assert_eq!(t.caret_pos(), 10);
        t.key_with(0x24, false, false);
        assert_eq!(t.caret_pos(), 0);
        t.delete(false);
        assert_eq!(t.text(), "no , tres");
        // Tope de caracteres
        let mut t = TextField::new("", 5);
        t.focus();
        t.insert("123456789");
        assert_eq!(t.text(), "12345");
        assert_eq!(t.chr('6'), Ev::None);
    }

    #[test]
    fn seleccion() {
        let mut t = tf("Banco Santander");
        t.move_to(0, false);
        for _ in 0..5 { t.right(false, true); }
        assert_eq!(t.selected_text(), "Banco");
        // Escribir sustituye la selección
        t.chr('X');
        assert_eq!(t.text(), "X Santander");
        t.select_all();
        assert_eq!(t.selection(), (0, 11));
        assert_eq!(t.cut().as_deref(), Some("X Santander"));
        assert_eq!(t.text(), "");
        let mut t = tf("abc def");
        t.select_word(5);
        assert_eq!(t.selected_text(), "def");
        // ← con selección va a su borde izquierdo
        t.left(false, false);
        assert_eq!((t.caret_pos(), t.selection()), (4, (4, 4)));
        // Ctrl+Mayús+→ selecciona por palabras
        t.move_to(0, false);
        t.key_with(0x27, true, true);
        assert_eq!(t.selected_text(), "abc");
        t.key_with(0x41, true, false);
        assert_eq!(t.selected_text(), "abc def");
        t.backspace(false);
        assert!(t.is_empty());
        // Pegar con saltos de línea: una sola línea
        t.insert("a\nb\tc");
        assert_eq!(t.text(), "a b c");
    }

    #[test]
    fn deshacer() {
        let mut t = tf("");
        for c in "hola mundo".chars() { t.chr(c); }
        // Escribir seguido es un paso por palabra
        t.undo();
        assert_eq!(t.text(), "hola ");
        t.undo();
        assert_eq!(t.text(), "");
        assert_eq!(t.undo(), Ev::None);
        t.redo();
        assert_eq!(t.text(), "hola ");
        t.key_with(0x59, true, false);
        assert_eq!(t.text(), "hola mundo");
        t.backspace(false);
        t.backspace(false);
        assert_eq!(t.text(), "hola mun");
        t.key_with(0x5A, true, false);
        assert_eq!(t.text(), "hola mundo", "borrar seguido también es un paso");
        // Ctrl+Mayús+Z rehace
        t.key_with(0x5A, true, true);
        assert_eq!(t.text(), "hola mun");
        // Un cambio nuevo borra lo que se podía rehacer
        t.undo();
        t.chr('!');
        assert_eq!(t.redo(), Ev::None);
        assert_eq!(t.text(), "hola mundo!");
    }

    #[test]
    fn teclas_del_panel() {
        let mut t = tf("x");
        assert_eq!(t.key_with(0x0D, false, false), Some(Ev::Submit));
        assert_eq!(t.key_with(0x1B, false, false), Some(Ev::Cancel));
        // ↑ ↓ y Tab no son suyas (las usa el panel: sugerencias, secciones)
        assert_eq!(t.key_with(0x26, false, false), None);
        assert_eq!(t.key_with(0x28, false, false), None);
        assert_eq!(t.key_with(0x09, false, false), None);
        // Las letras con Ctrl no escriben
        assert_eq!(t.key_with(0x4B, true, false), Some(Ev::None));
        assert_eq!(t.text(), "x");
        // Caracteres de varios bytes y emojis cuentan como uno
        let mut t = tf("año😀");
        assert_eq!(t.caret_pos(), 4);
        t.backspace(false);
        assert_eq!(t.text(), "año");
        t.left(false, false);
        t.chr('ñ');
        assert_eq!(t.text(), "añño");
    }
}
