//! Картриджи: формат .a78 (заголовок 128 байт) и "сырые" образы.
//!
//! Поддерживаются:
//! - линейная ROM (16/32/48 КБ), прижатая к концу адресного пространства;
//! - SuperGame (банки по 16 КБ, переключаются записью в $8000–$FFFF);
//! - необязательная RAM 16 КБ на $4000.
//!
//! TODO: Activision, Absolute, SOUPER, POKEY/YM2151 на картридже.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    Empty,
    Unsupported(String),
    BadBios(usize),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Empty => write!(f, "файл пустой"),
            LoadError::Unsupported(s) => write!(f, "не поддерживается: {s}"),
            LoadError::BadBios(n) => {
                write!(f, "неверный размер BIOS: {n} байт (ожидается 4096 или 16384)")
            }
        }
    }
}

impl std::error::Error for LoadError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mapper {
    Linear,
    SuperGame,
}

#[derive(Clone, Debug)]
pub struct Cartridge {
    pub title: String,
    pub mapper: Mapper,
    pub has_pokey: bool,
    rom: Vec<u8>,
    ram: Vec<u8>,
    bank6_at_4000: bool,
    bank: usize,
}

impl Cartridge {
    pub fn from_bytes(data: &[u8]) -> Result<Self, LoadError> {
        if data.is_empty() {
            return Err(LoadError::Empty);
        }

        // Заголовок .a78: байты 1..10 = "ATARI7800"
        if data.len() > 128 && &data[1..10] == b"ATARI7800" {
            let title = String::from_utf8_lossy(&data[17..49])
                .trim_end_matches('\0')
                .trim()
                .to_string();
            let declared = u32::from_be_bytes([data[49], data[50], data[51], data[52]]) as usize;
            let body = &data[128..];
            let len = if declared > 0 && declared <= body.len() { declared } else { body.len() };
            let rom = body[..len].to_vec();

            let hi = data[53];
            let lo = data[54];
            if lo & 0x03 != 0 {
                return Err(LoadError::Unsupported("банки Activision/Absolute".into()));
            }
            let supergame = hi & 0x02 != 0;
            let ram_4000 = hi & 0x04 != 0;
            let bank6 = hi & 0x10 != 0;
            let has_pokey = hi & 0x01 != 0 || hi & 0x40 != 0 || lo & 0x04 != 0;
            return Self::build(title, rom, supergame, ram_4000, bank6, has_pokey);
        }

        // Образ без заголовка
        Self::build(String::new(), data.to_vec(), false, false, false, false)
    }

    fn build(
        title: String,
        rom: Vec<u8>,
        supergame: bool,
        ram_4000: bool,
        bank6: bool,
        has_pokey: bool,
    ) -> Result<Self, LoadError> {
        if rom.is_empty() {
            return Err(LoadError::Empty);
        }
        let mapper = if supergame { Mapper::SuperGame } else { Mapper::Linear };
        match mapper {
            Mapper::Linear if rom.len() > 0xC000 => {
                return Err(LoadError::Unsupported(format!(
                    "ROM {} КБ без банкинга (нужен заголовок .a78)",
                    rom.len() / 1024
                )));
            }
            Mapper::SuperGame if rom.len() < 0x4000 || rom.len() % 0x4000 != 0 => {
                return Err(LoadError::Unsupported("размер SuperGame ROM не кратен 16 КБ".into()));
            }
            _ => {}
        }
        Ok(Self {
            title,
            mapper,
            has_pokey,
            rom,
            ram: if ram_4000 { vec![0; 0x4000] } else { Vec::new() },
            bank6_at_4000: bank6,
            bank: 0,
        })
    }

    pub fn rom_len(&self) -> usize {
        self.rom.len()
    }

    /// Чтение по адресу ≥ $4000.
    pub fn read(&self, addr: u16) -> u8 {
        let a = addr as usize;
        match self.mapper {
            Mapper::Linear => {
                let start = 0x10000 - self.rom.len();
                if a >= start {
                    self.rom[a - start]
                } else if !self.ram.is_empty() && (0x4000..0x8000).contains(&a) {
                    self.ram[a - 0x4000]
                } else {
                    0xFF
                }
            }
            Mapper::SuperGame => {
                let banks = self.rom.len() / 0x4000;
                match a {
                    0xC000..=0xFFFF => self.rom[(banks - 1) * 0x4000 + (a - 0xC000)],
                    0x8000..=0xBFFF => self.rom[(self.bank % banks) * 0x4000 + (a - 0x8000)],
                    0x4000..=0x7FFF => {
                        if !self.ram.is_empty() {
                            self.ram[a - 0x4000]
                        } else if self.bank6_at_4000 && banks > 6 {
                            self.rom[6 * 0x4000 + (a - 0x4000)]
                        } else {
                            0xFF
                        }
                    }
                    _ => 0xFF,
                }
            }
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        let a = addr as usize;
        if (0x4000..0x8000).contains(&a) && !self.ram.is_empty() {
            self.ram[a - 0x4000] = val;
            return;
        }
        if self.mapper == Mapper::SuperGame && a >= 0x8000 {
            self.bank = (val & 7) as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_16k_is_mapped_at_c000() {
        let mut rom = vec![0u8; 0x4000];
        rom[0] = 0xAB;
        rom[0x3FFF] = 0xCD;
        let cart = Cartridge::from_bytes(&rom).unwrap();
        assert_eq!(cart.read(0xC000), 0xAB);
        assert_eq!(cart.read(0xFFFF), 0xCD);
        assert_eq!(cart.read(0x8000), 0xFF);
    }

    #[test]
    fn a78_header_is_parsed() {
        let mut data = vec![0u8; 128 + 0x8000];
        data[1..10].copy_from_slice(b"ATARI7800");
        data[17..21].copy_from_slice(b"Test");
        data[49..53].copy_from_slice(&0x8000u32.to_be_bytes());
        data[128] = 0x11;
        let cart = Cartridge::from_bytes(&data).unwrap();
        assert_eq!(cart.title, "Test");
        assert_eq!(cart.read(0x8000), 0x11);
    }

    #[test]
    fn empty_is_rejected() {
        assert_eq!(Cartridge::from_bytes(&[]).unwrap_err(), LoadError::Empty);
    }
}
