//! Системная шина 7800: карта памяти и все чипы, к которым обращается процессор.
//!
//! Карта памяти (упрощённо):
//! ```text
//! $0000-$001F  TIA            $0100-$011F  зеркало TIA
//! $0020-$003F  MARIA          $0120-$013F  зеркало MARIA
//! $0040-$00FF  RAM (блок 0)   $0140-$01FF  RAM (блок 1, стек)
//! $0280-$02FF  RIOT (порты и таймер)
//! $0480-$04FF  RAM RIOT 128 байт (и зеркало $0580)
//! $1800-$27FF  RAM 4 КБ       $2800-$3FFF  зеркала
//! $4000-$FFFF  картридж (поверх — BIOS, пока он включён)
//! ```

use crate::cart::{Cartridge, LoadError};
use crate::cpu;
use crate::input::{Button, InputState};
use crate::maria::Maria;
use crate::riot::Riot;
use crate::tia::Tia;

/// Память, доступная без побочных эффектов: RAM, BIOS, картридж.
/// MARIA читает её напрямую (DMA).
pub struct Memory {
    ram: Box<[u8; 0x1000]>,
    pub cart: Option<Cartridge>,
    bios: Option<Vec<u8>>,
    pub bios_enabled: bool,
}

impl Memory {
    pub fn new() -> Self {
        Self { ram: Box::new([0; 0x1000]), cart: None, bios: None, bios_enabled: false }
    }

    pub fn set_bios(&mut self, data: &[u8]) -> Result<(), LoadError> {
        if data.len() != 0x1000 && data.len() != 0x4000 {
            return Err(LoadError::BadBios(data.len()));
        }
        self.bios = Some(data.to_vec());
        Ok(())
    }

    pub fn has_bios(&self) -> bool {
        self.bios.is_some()
    }

    fn ram_index(addr: u16) -> Option<usize> {
        match addr {
            0x0040..=0x00FF | 0x0140..=0x01FF => Some(0x800 + addr as usize),
            0x1800..=0x27FF => Some(addr as usize - 0x1800),
            0x2800..=0x3FFF => Some((addr as usize - 0x2800) % 0x1000),
            _ => None,
        }
    }

    pub fn peek(&self, addr: u16) -> u8 {
        if let Some(i) = Self::ram_index(addr) {
            return self.ram[i];
        }
        if addr >= 0x4000 {
            if self.bios_enabled {
                if let Some(b) = &self.bios {
                    let start = 0x10000 - b.len();
                    if addr as usize >= start {
                        return b[addr as usize - start];
                    }
                }
            }
            return self.cart.as_ref().map_or(0xFF, |c| c.read(addr));
        }
        0
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        if let Some(i) = Self::ram_index(addr) {
            self.ram[i] = val;
        } else if addr >= 0x4000 {
            if let Some(c) = self.cart.as_mut() {
                c.write(addr, val);
            }
        }
    }
}

impl Default for Memory {
    fn default() -> Self {
        Self::new()
    }
}

pub struct System {
    pub mem: Memory,
    pub maria: Maria,
    pub riot: Riot,
    pub tia: Tia,
    pub input: InputState,
    inptctrl: u8,
    locked: bool,
    /// Выставляется записью в WSYNC: процессор простаивает до конца строки.
    pub wsync: bool,
}

impl System {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            mem: Memory::new(),
            maria: Maria::new(),
            riot: Riot::new(),
            tia: Tia::new(sample_rate),
            input: InputState::default(),
            inptctrl: 0,
            locked: false,
            wsync: false,
        }
    }

    /// Последнее значение, записанное в INPTCTRL ($01).
    pub fn inptctrl(&self) -> u8 {
        self.inptctrl
    }

    /// Сброс консоли. Картридж, BIOS и состояние переключателей сохраняются.
    pub fn reset(&mut self) {
        self.mem.ram.fill(0);
        self.maria = Maria::new();
        self.riot = Riot::new();
        self.tia.reset();
        self.inptctrl = 0;
        self.locked = false;
        self.wsync = false;
        // С BIOS стартуем как настоящая консоль: BIOS сам включит MARIA и отключится.
        // Без BIOS имитируем состояние после его работы.
        let bios = self.mem.has_bios();
        self.mem.bios_enabled = bios;
        self.maria.enabled = !bios;
    }

    /// Переносит состояние кнопок во входные линии RIOT. Вызывается раз в кадр.
    pub fn sync_ports(&mut self) {
        let i = &self.input;

        // SWCHA: P0 — старший полубайт (R L D U), P1 — младший. Ноль = нажато.
        let mut a = 0xFFu8;
        for (player, shift) in [(0usize, 4u8), (1, 0)] {
            if i.button(player, Button::Right) {
                a &= !(0x08 << shift);
            }
            if i.button(player, Button::Left) {
                a &= !(0x04 << shift);
            }
            if i.button(player, Button::Down) {
                a &= !(0x02 << shift);
            }
            if i.button(player, Button::Up) {
                a &= !(0x01 << shift);
            }
        }

        // SWCHB: бит 0 Reset, 1 Select, 3 Pause (ноль = нажато), 6/7 сложность (1 = A).
        let mut b = 0xFFu8;
        if i.reset {
            b &= !0x01;
        }
        if i.select {
            b &= !0x02;
        }
        if i.pause {
            b &= !0x08;
        }
        if !i.difficulty[0] {
            b &= !0x40;
        }
        if !i.difficulty[1] {
            b &= !0x80;
        }

        self.riot.swcha_in = a;
        self.riot.swchb_in = b;
    }

    fn tia_read(&self, reg: u8) -> u8 {
        let i = &self.input;
        // INPT0–3: правая кнопка (1 = нажата), INPT4/5: левая кнопка (0 = нажата).
        match reg & 0x0F {
            0x09 => (i.button(0, Button::Fire2) as u8) << 7,
            0x0B => (i.button(1, Button::Fire2) as u8) << 7,
            0x0C => if i.button(0, Button::Fire1) { 0x00 } else { 0x80 },
            0x0D => if i.button(1, Button::Fire1) { 0x00 } else { 0x80 },
            _ => 0,
        }
    }

    fn write_inptctrl(&mut self, val: u8) {
        if self.locked {
            return;
        }
        self.inptctrl = val;
        self.locked = val & 0x01 != 0;
        self.maria.enabled = val & 0x02 != 0;
        self.mem.bios_enabled = val & 0x04 == 0 && self.mem.has_bios();
    }
}

impl cpu::Bus for System {
    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x003F | 0x0100..=0x013F => {
                let reg = (addr & 0x3F) as u8;
                if reg < 0x20 {
                    self.tia_read(reg)
                } else {
                    self.maria.read(reg)
                }
            }
            0x0280..=0x02FF => self.riot.read(addr),
            0x0480..=0x04FF | 0x0580..=0x05FF => self.riot.ram_read(addr),
            _ => self.mem.peek(addr),
        }
    }

    fn write(&mut self, addr: u16, val: u8) {
        match addr {
            0x0000..=0x003F | 0x0100..=0x013F => {
                let reg = (addr & 0x3F) as u8;
                match reg {
                    0x01 => self.write_inptctrl(val),
                    0x15..=0x1A => self.tia.write(reg, val),
                    0x24 => self.wsync = true,
                    _ => {}
                }
                if reg >= 0x20 {
                    self.maria.write(reg, val);
                }
            }
            0x0280..=0x02FF => self.riot.write(addr, val),
            0x0480..=0x04FF | 0x0580..=0x05FF => self.riot.ram_write(addr, val),
            _ => self.mem.write(addr, val),
        }
    }
}
