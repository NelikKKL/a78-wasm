//! MARIA — графический процессор 7800 (упрощённая построчная модель).
//!
//! Что есть:
//! - список зон (DLL) и списки отображения (DL) с 4- и 5-байтовыми заголовками;
//! - режимы 160A, 160B и 320A, символьный (косвенный) режим, "дырявый" DMA (H8/H16);
//! - прерывание DLI (NMI) на первой строке зоны;
//! - приблизительная кража тактов у процессора.
//!
//! TODO: 320B/C/D, режим Kangaroo, рамка (BC), colorkill, точный тайминг DMA
//! и однострочная задержка буфера строки.

use crate::palette::Palette;
use crate::system::Memory;

pub const SCREEN_W: usize = 320;
pub const SCREEN_H: usize = 224;

const BACKGRND: usize = 0x20;
const DPPH: usize = 0x2C;
const DPPL: usize = 0x30;
const CHARBASE: usize = 0x34;
const CTRL: usize = 0x3C;

/// Что произошло на строке.
#[derive(Clone, Copy, Debug, Default)]
pub struct LineInfo {
    /// Сколько тактов MARIA отобрала у процессора.
    pub dma_cycles: u32,
    /// Нужно ли выдать NMI (DLI).
    pub nmi: bool,
}

pub struct Maria {
    regs: [u8; 0x40],
    /// Включена ли MARIA (бит 1 INPTCTRL).
    pub enabled: bool,
    /// Флаг вертикального гашения (бит 7 MSTAT).
    pub vblank: bool,
    dll: u16,
    dl: u16,
    zone_left: u8,
    zone_offset: u8,
    h16: bool,
    h8: bool,
    /// Буфер строки: 0 — прозрачно, иначе `палитра * 4 + цвет` (1..=31).
    line: [u8; SCREEN_W],
    /// Сколько байтов графики пропущено из-за ещё не реализованного режима.
    pub unsupported_bytes: u32,
    /// (RM, WM) последнего неподдержанного режима.
    pub last_unsupported: Option<(u8, bool)>,
}

impl Default for Maria {
    fn default() -> Self {
        Self::new()
    }
}

impl Maria {
    pub fn new() -> Self {
        Self {
            regs: [0; 0x40],
            enabled: true,
            vblank: true,
            dll: 0,
            dl: 0,
            zone_left: 0,
            zone_offset: 0,
            h16: false,
            h8: false,
            line: [0; SCREEN_W],
            unsupported_bytes: 0,
            last_unsupported: None,
        }
    }

    /// Значение регистра для отладки (последнее записанное).
    pub fn reg(&self, reg: u8) -> u8 {
        self.regs[(reg & 0x3F) as usize]
    }

    /// Чтение регистра MARIA ($20–$3F).
    pub fn read(&self, reg: u8) -> u8 {
        match reg & 0x3F {
            0x28 => (self.vblank as u8) << 7,
            _ => 0,
        }
    }

    pub fn write(&mut self, reg: u8, val: u8) {
        self.regs[(reg & 0x3F) as usize] = val;
    }

    /// Вызывается на первой видимой строке кадра: запоминает указатель на DLL.
    pub fn start_frame(&mut self) {
        self.dll = ((self.regs[DPPH] as u16) << 8) | self.regs[DPPL] as u16;
        self.zone_left = 0;
    }

    /// Строит одну строку и записывает её в `row` (RGBA, `SCREEN_W * 4` байт).
    pub fn render_line(&mut self, mem: &Memory, row: &mut [u8], palette: &Palette) -> LineInfo {
        let mut info = LineInfo::default();
        self.line.fill(0);

        let ctrl = self.regs[CTRL];
        let dma_on = self.enabled && (ctrl & 0x60) == 0x40;
        if dma_on {
            if self.zone_left == 0 {
                // Новая запись DLL: флаги+высота, адрес DL (старший, младший)
                let b0 = mem.peek(self.dll);
                let hi = mem.peek(self.dll.wrapping_add(1)) as u16;
                let lo = mem.peek(self.dll.wrapping_add(2)) as u16;
                self.dll = self.dll.wrapping_add(3);
                self.dl = (hi << 8) | lo;
                self.h16 = b0 & 0x40 != 0;
                self.h8 = b0 & 0x20 != 0;
                self.zone_offset = b0 & 0x0F;
                self.zone_left = self.zone_offset + 1;
                info.nmi = b0 & 0x80 != 0;
                info.dma_cycles += 8;
            }
            info.dma_cycles += 5 + self.walk_display_list(mem);
            self.zone_left -= 1;
            self.zone_offset = self.zone_offset.wrapping_sub(1) & 0x0F;
        }

        self.blit(row, palette);
        info
    }

    fn blit(&self, row: &mut [u8], palette: &Palette) {
        let bg = self.regs[BACKGRND];
        for x in 0..SCREEN_W {
            let v = self.line[x] as usize;
            let color = if v == 0 { bg } else { self.regs[BACKGRND + v] };
            let rgb = palette[color as usize];
            row[x * 4] = rgb[0];
            row[x * 4 + 1] = rgb[1];
            row[x * 4 + 2] = rgb[2];
            row[x * 4 + 3] = 255;
        }
    }

    /// "Дырявый" DMA: данные из ПЗУ (≥ $8000) пропускаются по биту адреса.
    fn holey(&self, a: u16) -> bool {
        a >= 0x8000 && ((self.h16 && a & 0x1000 != 0) || (self.h8 && a & 0x0800 != 0))
    }

    /// Проходит по списку отображения для текущей строки зоны. Возвращает такты DMA.
    fn walk_display_list(&mut self, mem: &Memory) -> u32 {
        let ctrl = self.regs[CTRL];
        let rm = ctrl & 0x03;
        let two_byte_chars = ctrl & 0x10 != 0;
        let charbase = self.regs[CHARBASE] as u16;
        let offset = self.zone_offset as u16;

        let mut p = self.dl;
        let mut wm = false;
        let mut cycles = 0u32;

        for _ in 0..48 {
            let b1 = mem.peek(p.wrapping_add(1));
            if b1 & 0x5F == 0 {
                break; // конец списка
            }
            let addr = ((mem.peek(p.wrapping_add(2)) as u16) << 8) | mem.peek(p) as u16;

            let (pal, width, hpos, indirect) = if b1 & 0x1F != 0 {
                // 4-байтовый заголовок: адрес.lo, палитра+ширина, адрес.hi, x
                let r = (
                    b1 >> 5,
                    32 - (b1 & 0x1F) as usize,
                    mem.peek(p.wrapping_add(3)) as usize,
                    false,
                );
                p = p.wrapping_add(4);
                cycles += 8;
                r
            } else {
                // 5-байтовый: адрес.lo, режим, адрес.hi, палитра+ширина, x
                let b3 = mem.peek(p.wrapping_add(3));
                wm = b1 & 0x80 != 0;
                let r = (
                    b3 >> 5,
                    32 - (b3 & 0x1F) as usize,
                    mem.peek(p.wrapping_add(4)) as usize,
                    b1 & 0x20 != 0,
                );
                p = p.wrapping_add(5);
                cycles += 10;
                r
            };

            let ppb = pixels_per_byte(rm, wm);
            let mut hx = hpos * 2;

            if !indirect {
                let base = addr.wrapping_add(offset << 8);
                for i in 0..width {
                    let a = base.wrapping_add(i as u16);
                    if self.holey(a) {
                        hx += ppb;
                        continue;
                    }
                    hx = self.plot_byte(mem.peek(a), pal, wm, rm, hx);
                    cycles += 3;
                }
            } else {
                let gbase = ((charbase + offset) & 0xFF) << 8;
                let n: u16 = if two_byte_chars { 2 } else { 1 };
                for i in 0..width {
                    let cell = mem.peek(addr.wrapping_add(i as u16)) as u16;
                    let ga = gbase.wrapping_add(cell);
                    for k in 0..n {
                        let a = ga.wrapping_add(k);
                        if self.holey(a) {
                            hx += ppb;
                            continue;
                        }
                        hx = self.plot_byte(mem.peek(a), pal, wm, rm, hx);
                    }
                    cycles += if two_byte_chars { 9 } else { 6 };
                }
            }
        }
        cycles
    }

    /// Рисует один байт графики с позиции `hx` (в точках разрешения 320). Возвращает новую позицию.
    fn plot_byte(&mut self, b: u8, pal: u8, wm: bool, rm: u8, mut hx: usize) -> usize {
        match (rm, wm) {
            // 160A: 4 точки по 2 бита
            (0, false) => {
                for k in 0..4 {
                    let ci = (b >> (6 - 2 * k)) & 3;
                    self.put(hx, 2, ci, pal);
                    hx += 2;
                }
            }
            // 160B: 2 точки; цвет и 2 бита палитры вперемешку
            (0, true) => {
                let p0 = (pal & 4) | ((b >> 2) & 3);
                self.put(hx, 2, (b >> 6) & 3, p0);
                hx += 2;
                let p1 = (pal & 4) | (b & 3);
                self.put(hx, 2, (b >> 4) & 3, p1);
                hx += 2;
            }
            // 320A: 8 точек по 1 биту, цвет 2 палитры
            (3, false) => {
                for k in 0..8 {
                    if b & (0x80 >> k) != 0 {
                        self.put(hx, 1, 2, pal);
                    }
                    hx += 1;
                }
            }
            // TODO: 320B / 320C / 320D
            _ => {
                self.unsupported_bytes = self.unsupported_bytes.wrapping_add(1);
                self.last_unsupported = Some((rm, wm));
                hx += 8;
            }
        }
        hx
    }

    fn put(&mut self, x: usize, w: usize, ci: u8, pal: u8) {
        if ci == 0 {
            return; // прозрачно
        }
        let v = pal * 4 + ci;
        for i in 0..w {
            if x + i < SCREEN_W {
                self.line[x + i] = v;
            }
        }
    }
}

fn pixels_per_byte(rm: u8, wm: bool) -> usize {
    match (rm, wm) {
        (0, true) => 4,
        _ => 8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_ignores_transparent_and_clips() {
        let mut m = Maria::new();
        m.put(0, 2, 0, 1);
        assert_eq!(m.line[0], 0);
        m.put(SCREEN_W - 1, 2, 3, 2);
        assert_eq!(m.line[SCREEN_W - 1], 2 * 4 + 3);
    }

    #[test]
    fn mode_160a_decodes_two_bit_pixels() {
        let mut m = Maria::new();
        let end = m.plot_byte(0b01_10_11_00, 0, false, 0, 0);
        assert_eq!(end, 8);
        assert_eq!(&m.line[0..8], &[1, 1, 2, 2, 3, 3, 0, 0]);
    }
}
