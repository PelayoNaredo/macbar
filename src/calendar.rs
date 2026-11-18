//! Panel del reloj: hora grande, calendario del mes navegable y temporizador rápido.

use super::*;
use popup::{Content, Re, Ui};

const W: f32 = 300.0;
const PAD: f32 = 14.0;
const CELL: f32 = 36.0;

const ID_PREV: u32 = 1;
const ID_NEXT: u32 = 2;
const ID_TODAY: u32 = 3;
const ID_NOTIF: u32 = 4;
const ID_SETTINGS: u32 = 5;
const ID_STOP: u32 = 6;
const ID_TIMER0: u32 = 100; // + índice de TIMERS

const TIMERS: [(u32, &str); 4] = [(60, "1 min"), (5 * 60, "5 min"), (15 * 60, "15 min"), (25 * 60, "25 min")];
pub const MESES: [&str; 12] = ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"];
pub const DIAS: [&str; 7] = ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"];

/// Día de la semana (0 = domingo) por el método de Sakamoto.
pub fn weekday(y: i32, m: u32, d: u32) -> u32 {
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    ((y + y / 4 - y / 100 + y / 400 + t[(m - 1) as usize] + d as i32) % 7) as u32
}

fn days_in(y: i32, m: u32) -> u32 {
    match m { 2 => if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 }, 4 | 6 | 9 | 11 => 30, _ => 31 }
}

/// `slide`: cambio de mes en curso (inicio, sentido: +1 = siguiente).
pub struct Calendar { year: i32, month: u32, slide: Option<(Instant, f32)> }

impl Calendar {
    pub fn new() -> Self { let t = unsafe { GetLocalTime() }; Calendar { year: t.wYear as i32, month: t.wMonth as u32, slide: None } }
    fn rows(&self) -> u32 {
        let first = (weekday(self.year, self.month, 1) + 6) % 7; // lunes = 0
        (first + days_in(self.year, self.month)).div_ceil(7)
    }
}

impl Content for Calendar {
    fn size(&self) -> (f32, f32) {
        (W, PAD + 64.0 + 10.0 + 40.0 + 22.0 + self.rows() as f32 * CELL + 10.0 + 12.0 + 84.0 + 12.0 + 34.0 + PAD)
    }
    fn shown(&mut self) { *self = Calendar::new(); }
    fn tick(&mut self) -> bool { true }
    fn animating(&self) -> bool { self.slide.is_some_and(|(t, _)| t.elapsed().as_secs_f32() < 0.3) }
    fn wheel(&mut self, d: i32) -> bool { self.click(if d > 0 { ID_PREV } else { ID_NEXT }, false); true }

    fn paint(&mut self, ui: &mut Ui) {
        let now = unsafe { GetLocalTime() };
        let mut y = PAD;
        // ---- Hora grande y fecha larga
        let hora = format!("{}:{:02}", now.wHour, now.wMinute);
        ui.text(&ui.f.big, &hora, PAD + 2.0, y, 40.0, &ui.white);
        let fecha = format!("{}, {} de {} de {}", DIAS[now.wDayOfWeek as usize % 7], now.wDay, MESES[(now.wMonth as usize + 11) % 12], now.wYear);
        let mut f = fecha.chars();
        let fecha: String = f.next().map(|c| c.to_uppercase().collect::<String>() + f.as_str()).unwrap_or_default();
        ui.text(&ui.f.sub, &fecha, PAD + 3.0, y + 42.0, 18.0, &ui.dim);
        y += 64.0 + 10.0;

        // ---- Calendario
        let rows = self.rows();
        let card_h = 40.0 + 22.0 + rows as f32 * CELL + 10.0;
        ui.sep(y - 6.0);
        let titulo = format!("{} {}", MESES[(self.month - 1) as usize], self.year);
        let mut tt = titulo.chars();
        let titulo: String = tt.next().map(|c| c.to_uppercase().collect::<String>() + tt.as_str()).unwrap_or_default();
        // Cambio de mes: título y días entran deslizándose desde el lado del que vienen, con fundido
        let (sdx, sa) = match self.slide {
            Some((t0, dir)) => { let e = popup::ease_out(t0.elapsed().as_secs_f32() / 0.28); (dir * 28.0 * (1.0 - e), e) }
            None => (0.0, 1.0),
        };
        unsafe { ui.rt.PushAxisAlignedClip(&rect(PAD, y, W - PAD - 70.0, y + 40.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE); }
        popup::push_alpha(ui.rt, sa);
        ui.text(&ui.f.name, &titulo, PAD + 14.0 + sdx, y + 6.0, 30.0, &ui.white);
        unsafe { ui.rt.PopLayer(); ui.rt.PopAxisAlignedClip(); }
        let is_now = self.year == now.wYear as i32 && self.month == now.wMonth as u32;
        // Flechas y "Hoy"
        let mut bx = W - PAD - 8.0;
        for (g, id) in [("\u{E76C}", ID_NEXT), ("\u{E76B}", ID_PREV)] {
            let r = rect(bx - 28.0, y + 7.0, bx, y + 35.0);
            let k = ui.heat(id);
            if k > 0.01 { unsafe { ui.rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: bx - 14.0, Y: y + 21.0 }, radiusX: 14.0, radiusY: 14.0 }, &ui.wa(0.13 * k)); } }
            ui.glyph(&ui.f.icon, g, r.left, r.top, 28.0, 28.0, &ui.white);
            ui.hit(r, id);
            bx -= 30.0;
        }
        if !is_now {
            let tw = ui.p.width(&ui.f.small, "Hoy") + 18.0;
            let r = rect(bx - tw, y + 10.0, bx - 2.0, y + 32.0);
            unsafe { ui.rt.FillRoundedRectangle(&rrect(r, 11.0), &ui.wa(0.075 + 0.075 * ui.heat(ID_TODAY))); }
            ui.text_c(&ui.f.small, "Hoy", (r.left + r.right) / 2.0, r.top, 22.0, &ui.white);
            ui.hit(r, ID_TODAY);
        }
        let gx = PAD + (W - PAD * 2.0 - 7.0 * CELL) / 2.0;
        let gy = y + 40.0;
        for (i, d) in ["L", "M", "X", "J", "V", "S", "D"].iter().enumerate() {
            ui.text_c(&ui.f.small, d, gx + i as f32 * CELL + CELL / 2.0, gy, 18.0, &ui.faint);
        }
        let first = (weekday(self.year, self.month, 1) + 6) % 7;
        let n = days_in(self.year, self.month);
        unsafe { ui.rt.PushAxisAlignedClip(&rect(PAD, gy + 20.0, W - PAD, gy + 22.0 + rows as f32 * CELL + 2.0), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE); }
        popup::push_alpha(ui.rt, sa);
        for d in 1..=n {
            let idx = first + d - 1;
            let (c, r) = ((idx % 7) as f32, (idx / 7) as f32);
            let cx = gx + c * CELL + CELL / 2.0 + sdx;
            let cy = gy + 22.0 + r * CELL + CELL / 2.0;
            let today = is_now && d == now.wDay as u32;
            if today {
                unsafe { ui.rt.FillEllipse(&D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: 15.0, radiusY: 15.0 }, &ui.on); }
            }
            let weekend = c >= 5.0;
            let b = if today || !weekend { &ui.white } else { &ui.dim };
            ui.text_c(if today { &ui.f.lbl } else { &ui.f.sub }, &d.to_string(), cx, cy - 15.0, 30.0, b);
        }
        unsafe { ui.rt.PopLayer(); ui.rt.PopAxisAlignedClip(); }
        y += card_h + 12.0;

        // ---- Temporizador
        ui.sep(y - 6.0);
        ui.glyph(&ui.f.icon, "\u{E916}", PAD + 10.0, y + 8.0, 20.0, 22.0, &ui.white);
        ui.text(&ui.f.lbl, "Temporizador", PAD + 34.0, y + 8.0, 22.0, &ui.white);
        match live::timer_left() {
            Some((left, _)) => {
                ui.text_r(&ui.f.val, &live::mmss(left), W - PAD - 14.0, y + 7.0, 24.0, &ui.white);
                let r = rect(PAD + 10.0, y + 42.0, W - PAD - 10.0, y + 72.0);
                ui.button(r, Some("\u{E711}"), "Detener", ID_STOP, false);
            }
            None => {
                let bw = (W - PAD * 2.0 - 20.0 - 3.0 * 8.0) / 4.0;
                for (i, (_, lbl)) in TIMERS.iter().enumerate() {
                    let x0 = PAD + 10.0 + i as f32 * (bw + 8.0);
                    ui.button(rect(x0, y + 42.0, x0 + bw, y + 72.0), None, lbl, ID_TIMER0 + i as u32, false);
                }
            }
        }
        y += 84.0 + 12.0;

        // ---- Accesos
        let bw = (W - PAD * 2.0 - 8.0) / 2.0;
        ui.button(rect(PAD, y, PAD + bw, y + 34.0), Some("\u{EA8F}"), "Notificaciones", ID_NOTIF, false);
        ui.button(rect(PAD + bw + 8.0, y, W - PAD, y + 34.0), Some("\u{E713}"), "Fecha y hora", ID_SETTINGS, false);
    }

    fn click(&mut self, id: u32, _right: bool) -> Re {
        match id {
            ID_PREV => { if self.month == 1 { self.month = 12; self.year -= 1; } else { self.month -= 1; } self.slide = Some((Instant::now(), -1.0)); Re::Stay }
            ID_NEXT => { if self.month == 12 { self.month = 1; self.year += 1; } else { self.month += 1; } self.slide = Some((Instant::now(), 1.0)); Re::Stay }
            ID_TODAY => {
                let now = Calendar::new();
                let dir = if (now.year, now.month) > (self.year, self.month) { 1.0 } else { -1.0 };
                *self = now; self.slide = Some((Instant::now(), dir)); Re::Stay
            }
            ID_NOTIF => Re::CloseRun(Box::new(|| combo(&[VK_LWIN, vk(b'N')]))),
            ID_SETTINGS => Re::CloseRun(Box::new(|| sys::open("ms-settings:dateandtime"))),
            ID_STOP => { live::timer_stop(); Re::Stay }
            i if (ID_TIMER0..ID_TIMER0 + TIMERS.len() as u32).contains(&i) => {
                live::timer_start(TIMERS[(i - ID_TIMER0) as usize].0);
                Re::Close
            }
            _ => Re::Stay,
        }
    }
}
