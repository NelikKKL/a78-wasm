//! MARIA — графический процессор 7800 (построчная модель).
//!
//! Устройство, как его описывает вики по 7800 и как его реализуют зрелые эмуляторы:
//!
//! - DMA читает списки отображения и графику и пишет в "линейную RAM" — 160 ячеек
//!   по 5 бит: `P2 P1 P0 C1 C0` (3 бита палитры + 2 бита цвета).
//! - **Режим записи** (WM, из 5-байтового заголовка) определяет, как байт графики
//!   превращается в ячейки: WM=0 — 4 ячейки по 2 бита (палитра из заголовка),
//!   WM=1 — 2 ячейки по 4 бита (2 бита палитры + 2 бита цвета вперемешку).
//! - **Режим чтения** (RM, из CTRL) определяет, как ячейка превращается в пиксели:
//!   160-режимы дают два одинаковых пикселя, 320-режимы — два разных.
//! - Ячейка с нулевым цветом прозрачна, если не включён режим Kangaroo.
//!
//! Тайминг: DMA считается в тактах MARIA (7.16 МГц), на один такт процессора их 4.
//!
//! Упрощения: нет задержки на одну строку (буфер не двойной), не рисуется рамка (BC).

use crate::palette::Palette;
use crate::system::Memory;

pub const SCREEN_W: usize = 320;
pub const SCREEN_H: usize = 240;

const CELLS: usize = 160;
const BACKGRND: usize = 0x20;
const DPPH: usize = 0x2C;
const DPPL: usize = 0x30;
const CHARBASE: usize = 0x34;
const CTRL: usize = 0x3C;

/// Максимум DMA на строку в тактах MARIA (строка = 454, минус запуск и остановка).
const MAX_DMA_CLOCKS: u32 = 426;

/// Что произошло на строке.
#[derive(Clone, Copy, Debug, Default)]
pub struct LineInfo {
    /// Сколько тактов процессора MARIA отобрала на DMA.
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
    offset: u8,
    h16: bool,
    h8: bool,
    first_zone: bool,
    /// Режим записи запоминается между заголовками, строками и кадрами.
    write_mode: bool,
    cells: [u8; CELLS],
    /// Сколько раз использован недопустимый режим чтения (RM = 1).
    pub unsupported_bytes: u32,
    /// (RM, WM) последнего недопустимого режима.
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
            offset: 0,
            h16: false,
            h8: false,
            first_zone: true,
            write_mode: false,
            cells: [0; CELLS],
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
        self.first_zone = true;
    }

    /// Строит одну строку и записывает её в `row` (RGBA, `SCREEN_W * 4` байт).
    pub fn render_line(&mut self, mem: &Memory, row: &mut [u8], palette: &Palette) -> LineInfo {
        let mut info = LineInfo::default();
        self.cells.fill(0);

        let ctrl = self.regs[CTRL];
        if self.enabled && (ctrl & 0x60) == 0x40 {
            let mut clocks = 16u32; // запуск DMA
            if self.zone_left == 0 {
                self.load_zone(mem, &mut info);
            }
            clocks += self.walk_display_list(mem, ctrl);

            // Последняя строка зоны: остановка DMA и чтение следующей записи DLL.
            // Флаг DLI следующей зоны вызывает прерывание сразу после этого DMA.
            if self.zone_left == 1 {
                clocks += 6;
                if mem.peek(self.dll) & 0x80 != 0 {
                    clocks += 17;
                    info.nmi = true;
                }
            }
            self.zone_left -= 1;
            self.offset = self.offset.wrapping_sub(1) & 0x0F;

            info.dma_cycles = if clocks >= MAX_DMA_CLOCKS {
                crate::CYCLES_PER_LINE as u32 // MARIA заняла всю строку — процессор стоит
            } else {
                (clocks + 3) / 4
            };
        }

        if ctrl & 3 == 1 {
            self.unsupported_bytes = self.unsupported_bytes.wrapping_add(1);
            self.last_unsupported = Some((1, self.write_mode));
        }
        self.blit(row, palette, ctrl);
        info
    }

    /// Читает запись DLL: [флаги+высота, адрес DL (старший, младший)].
    fn load_zone(&mut self, mem: &Memory, info: &mut LineInfo) {
        let b0 = mem.peek(self.dll);
        let hi = mem.peek(self.dll.wrapping_add(1)) as u16;
        let lo = mem.peek(self.dll.wrapping_add(2)) as u16;
        self.dll = self.dll.wrapping_add(3);
        self.dl = (hi << 8) | lo;
        self.offset = b0 & 0x0F;
        self.zone_left = self.offset + 1;
        self.h16 = b0 & 0x40 != 0;
        self.h8 = b0 & 0x20 != 0;
        if self.first_zone {
            self.first_zone = false;
            // DLI первой зоны срабатывает после DMA её первой строки
            info.nmi = b0 & 0x80 != 0;
        }
    }

    /// "Дырявый" DMA: данные из ПЗУ (≥ $8000) пропускаются по биту адреса.
    fn holey(&self, a: u16) -> bool {
        a >= 0x8000 && ((self.h16 && a & 0x1000 != 0) || (self.h8 && a & 0x0800 != 0))
    }

    /// Проходит по списку отображения для текущей строки зоны.
    /// Возвращает затраченные такты MARIA.
    fn walk_display_list(&mut self, mem: &Memory, ctrl: u8) -> u32 {
        let two_byte_chars = ctrl & 0x10 != 0;
        let kangaroo = ctrl & 0x04 != 0;
        let charbase = (self.regs[CHARBASE] as u16) << 8;
        let row_off = (self.offset as u16) << 8;

        let mut p = self.dl;
        let mut clocks = 0u32;

        while clocks < MAX_DMA_CLOCKS {
            let b1 = mem.peek(p.wrapping_add(1));
            if b1 & 0x5F == 0 {
                break; // конец списка
            }
            let addr = ((mem.peek(p.wrapping_add(2)) as u16) << 8) | mem.peek(p) as u16;

            let (pal, width, hpos, indirect) = if b1 & 0x1F == 0 {
                // 5-байтовый заголовок: адрес.lo, режим, адрес.hi, палитра+ширина, x
                let b3 = mem.peek(p.wrapping_add(3));
                self.write_mode = b1 & 0x80 != 0;
                let r = (b3 >> 5, 32 - (b3 & 0x1F) as usize, mem.peek(p.wrapping_add(4)), b1 & 0x20 != 0);
                p = p.wrapping_add(5);
                clocks += 10;
                r
            } else {
                // 4-байтовый заголовок: адрес.lo, палитра+ширина, адрес.hi, x
                let r = (b1 >> 5, 32 - (b1 & 0x1F) as usize, mem.peek(p.wrapping_add(3)), false);
                p = p.wrapping_add(4);
                clocks += 8;
                r
            };

            let mut x = hpos;
            for i in 0..width {
                if clocks >= MAX_DMA_CLOCKS {
                    break;
                }
                if indirect {
                    // Символьный режим: байт карты → адрес в таблице символов
                    let cell = mem.peek(addr.wrapping_add(i as u16)) as u16;
                    let data_addr = (charbase | cell).wrapping_add(row_off);
                    if self.holey(data_addr) {
                        clocks += 3;
                        continue;
                    }
                    clocks += 3;
                    x = self.write_cells(mem.peek(data_addr), pal, x, kangaroo);
                    clocks += 3;
                    if two_byte_chars {
                        x = self.write_cells(mem.peek(data_addr.wrapping_add(1)), pal, x, kangaroo);
                        clocks += 3;
                    }
                } else {
                    let data_addr = addr.wrapping_add(i as u16).wrapping_add(row_off);
                    if self.holey(data_addr) {
                        clocks += 3;
                        continue;
                    }
                    x = self.write_cells(mem.peek(data_addr), pal, x, kangaroo);
                    clocks += 3;
                }
            }
        }
        clocks
    }

    /// Превращает байт графики в ячейки линейной RAM. Возвращает следующую позицию.
    fn write_cells(&mut self, data: u8, pal: u8, mut x: u8, kangaroo: bool) -> u8 {
        let p = pal << 2; // палитра в старших трёх битах ячейки
        if self.write_mode {
            // WM=1: байт abcdefgh → ячейки (P2 e f a b) и (P2 g h c d)
            self.put_cell(x, (p & 0x10) | (data & 0x0C) | (data >> 6), kangaroo);
            x = x.wrapping_add(1);
            self.put_cell(x, (p & 0x10) | ((data & 0x03) << 2) | ((data & 0x30) >> 4), kangaroo);
            x.wrapping_add(1)
        } else {
            // WM=0: четыре двухбитных пикселя
            for i in 0..4 {
                self.put_cell(x, p | ((data >> (6 - 2 * i)) & 0x03), kangaroo);
                x = x.wrapping_add(1);
            }
            x
        }
    }

    fn put_cell(&mut self, x: u8, cell: u8, kangaroo: bool) {
        if (cell & 3 != 0 || kangaroo) && (x as usize) < CELLS {
            self.cells[x as usize] = cell;
        }
    }

    /// Выводит линейную RAM на экран по режиму чтения.
    fn blit(&self, row: &mut [u8], palette: &Palette, ctrl: u8) {
        let bg = self.regs[BACKGRND];
        let rm = ctrl & 3;
        let color_kill = ctrl & 0x80 != 0;

        let color = |d: u8| -> [u8; 3] {
            let mut c = if d & 3 == 0 { bg } else { self.regs[BACKGRND + d as usize] };
            if color_kill {
                c &= 0x0F; // нет цветовой вспышки — остаётся только яркость
            }
            palette[c as usize]
        };

        for (i, &c) in self.cells.iter().enumerate() {
            let (d0, d1) = match rm {
                // 160A/160B: оба пикселя одинаковые
                0 | 1 => (c, c),
                // 320B/320D: биты палитры P1 P0 становятся младшими битами цвета
                2 => (
                    (c & 0x10) | (c & 0x02) | ((c >> 3) & 1),
                    (c & 0x10) | ((c << 1) & 0x02) | ((c >> 2) & 1),
                ),
                // 320A/320C: каждый пиксель — один бит цвета, цвет 0 или 2
                _ => ((c & 0x1C) | (c & 0x02), (c & 0x1C) | ((c << 1) & 0x02)),
            };
            for (k, d) in [d0, d1].into_iter().enumerate() {
                let rgb = color(d);
                let o = (i * 2 + k) * 4;
                row[o] = rgb[0];
                row[o + 1] = rgb[1];
                row[o + 2] = rgb[2];
                row[o + 3] = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette;

    #[test]
    fn mode_160a_writes_four_cells_and_skips_transparent() {
        let mut m = Maria::new();
        let end = m.write_cells(0b01_10_11_00, 2, 0, false);
        assert_eq!(end, 4);
        // палитра 2 = 0b010 → биты ячейки 0b010_00 | цвет; цвет 0 не пишется
        assert_eq!(&m.cells[0..4], &[0b01001, 0b01010, 0b01011, 0]);
    }

    #[test]
    fn kangaroo_mode_writes_transparent_cells() {
        let mut m = Maria::new();
        m.cells[3] = 0b00111;
        m.write_cells(0b00_00_00_00, 0, 0, true);
        assert_eq!(m.cells[3], 0); // перезаписано "прозрачной" ячейкой
    }

    #[test]
    fn mode_160b_interleaves_palette_and_color_bits() {
        let mut m = Maria::new();
        m.write_mode = true;
        // abcdefgh = 11 01 10 00 → ячейка1: P2 e f a b = 0 1 0 1 1, ячейка2: P2 g h c d = 0 0 0 0 1
        let end = m.write_cells(0b1101_1000, 0, 0, false);
        assert_eq!(end, 2);
        assert_eq!(m.cells[0], 0b01011);
        assert_eq!(m.cells[1], 0b00001);
    }

    fn rgb(row: &[u8], x: usize) -> [u8; 3] {
        [row[x * 4], row[x * 4 + 1], row[x * 4 + 2]]
    }

    #[test]
    fn read_modes_split_cells_into_pixels() {
        let pal = palette::ntsc();
        let mut m = Maria::new();
        m.write(0x20, 0x92); // фон
        m.write(0x26, 0x46); // палитра 1, цвет 2
        m.write(0x22, 0x1E); // палитра 0, цвет 2
        m.write(0x23, 0xCA); // палитра 0, цвет 3
        let mut row = vec![0u8; SCREEN_W * 4];

        // 320A (RM=3): ячейка P=001, C=11 → оба пикселя "палитра 1, цвет 2"
        m.cells[0] = 0b00111;
        m.blit(&mut row, &pal, 0x03);
        assert_eq!(rgb(&row, 0), pal[0x46]);
        assert_eq!(rgb(&row, 1), pal[0x46]);

        // 320D/320B (RM=2): P1 P0 → младшие биты цвета; ячейка P=010, C=10:
        // пиксель 0: цвет (C1, P1) = (1, 1) = 3, пиксель 1: (C0, P0) = (0, 0) = 0 → фон
        m.cells[0] = 0b01010;
        m.blit(&mut row, &pal, 0x02);
        assert_eq!(rgb(&row, 0), pal[0xCA]);
        assert_eq!(rgb(&row, 1), pal[0x92]);

        // 160 (RM=0): обе точки одинаковые
        m.cells[0] = 0b00010; // палитра 0, цвет 2
        m.blit(&mut row, &pal, 0x00);
        assert_eq!(rgb(&row, 0), pal[0x1E]);
        assert_eq!(rgb(&row, 1), pal[0x1E]);
    }

    #[test]
    fn color_kill_keeps_only_luminance() {
        let pal = palette::ntsc();
        let mut m = Maria::new();
        m.write(0x20, 0x92);
        let mut row = vec![0u8; SCREEN_W * 4];
        m.blit(&mut row, &pal, 0x80);
        assert_eq!(rgb(&row, 0), pal[0x02]);
    }
}
