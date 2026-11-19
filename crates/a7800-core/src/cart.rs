//! Картриджи: формат .a78 (заголовок 128 байт) и "сырые" образы.
//!
//! Поддерживаются:
//! - линейная ROM (16/32/48 КБ), прижатая к концу адресного пространства;
//! - SuperGame (банки по 16 КБ на $8000, запись 0–7 в $8000–$BFFF);
//! - Absolute (F-18 Hornet, 64 КБ) и Activision (Double Dragon, Rampage, 128 КБ);
//! - необязательная RAM 16 КБ на $4000 и "банк 6 на $4000";
//! - положение POKEY ($4000, $0450 или $0800) — сам чип лежит в `pokey.rs`.
//!
//! Не поддерживаются (картридж отвергается с понятной ошибкой): SOUPER, banksets.

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
    Absolute,
    Activision,
}

#[derive(Clone, Debug)]
pub struct Cartridge {
    pub title: String,
    pub mapper: Mapper,
    /// Где в адресном пространстве стоит POKEY: $4000, $0450 или $0800.
    pub pokey_at: Option<u16>,
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

            // Тип картриджа — 16 бит, big-endian: байт 53 старший, байт 54 младший.
            let ct = u16::from_be_bytes([data[53], data[54]]);
            if ct & 0x1000 != 0 {
                return Err(LoadError::Unsupported("банки SOUPER".into()));
            }
            if ct & 0x2000 != 0 {
                return Err(LoadError::Unsupported("banksets".into()));
            }
            let pokey_at = if ct & 0x0001 != 0 {
                Some(0x4000)
            } else if ct & 0x0040 != 0 {
                Some(0x0450)
            } else if ct & 0x8000 != 0 {
                Some(0x0800)
            } else {
                None
            };
            let mapper = if ct & 0x0002 != 0 {
                Mapper::SuperGame
            } else if ct & 0x0300 != 0 {
                // Биты Activision/Absolute в разных версиях спецификации называют по-разному,
                // поэтому различаем по размеру: 64 КБ — Absolute, 128 КБ — Activision.
                match rom.len() {
                    0x10000 => Mapper::Absolute,
                    0x20000 => Mapper::Activision,
                    n => {
                        return Err(LoadError::Unsupported(format!(
                            "особый банкинг для ROM {} КБ",
                            n / 1024
                        )))
                    }
                }
            } else {
                Mapper::Linear
            };
            let ram_4000 = ct & 0x0004 != 0 || ct & 0x0080 != 0;
            let bank6 = ct & 0x0010 != 0;
            return Self::build(title, rom, mapper, ram_4000, bank6, pokey_at);
        }

        // Образ без заголовка
        Self::build(String::new(), data.to_vec(), Mapper::Linear, false, false, None)
    }

    fn build(
        title: String,
        rom: Vec<u8>,
        mapper: Mapper,
        ram_4000: bool,
        bank6: bool,
        pokey_at: Option<u16>,
    ) -> Result<Self, LoadError> {
        if rom.is_empty() {
            return Err(LoadError::Empty);
        }
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
            pokey_at,
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
            Mapper::Absolute => match a {
                // два банка по 16 КБ на $4000, 32 КБ неподвижны на $8000
                0x4000..=0x7FFF => self.rom[(self.bank & 1) * 0x4000 + (a - 0x4000)],
                0x8000..=0xFFFF => self.rom[a],
                _ => 0xFF,
            },
            Mapper::Activision => match a {
                // 8 банков по 16 КБ; окно $A000–$DFFF переключается, остальное — банки 6 и 7
                0x4000..=0x5FFF => self.rom[6 * 0x4000 + 0x2000 + (a - 0x4000)],
                0x6000..=0x7FFF => self.rom[6 * 0x4000 + (a - 0x6000)],
                0x8000..=0x9FFF => self.rom[7 * 0x4000 + 0x2000 + (a - 0x8000)],
                0xA000..=0xDFFF => self.rom[(self.bank & 7) * 0x4000 + (a - 0xA000)],
                0xE000..=0xFFFF => self.rom[7 * 0x4000 + (a - 0xE000)],
                _ => 0xFF,
            },
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        let a = addr as usize;
        if (0x4000..0x8000).contains(&a) && !self.ram.is_empty() {
            self.ram[a - 0x4000] = val;
            return;
        }
        match self.mapper {
            Mapper::SuperGame if (0x8000..0xC000).contains(&a) => self.bank = (val & 7) as usize,
            // F-18 Hornet пишет 1 или 2 в $8000
            Mapper::Absolute if a == 0x8000 => self.bank = (val.wrapping_sub(1) & 1) as usize,
            // Double Dragon / Rampage: номер банка — в младших битах адреса $FF80–$FF87
            Mapper::Activision if a >= 0xFF80 => self.bank = a & 7,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a78(body: Vec<u8>, cart_type: u16) -> Vec<u8> {
        let mut data = vec![0u8; 128];
        data[1..10].copy_from_slice(b"ATARI7800");
        data[17..21].copy_from_slice(b"Test");
        data[49..53].copy_from_slice(&(body.len() as u32).to_be_bytes());
        data[53..55].copy_from_slice(&cart_type.to_be_bytes());
        data.extend(body);
        data
    }

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
        let mut body = vec![0u8; 0x8000];
        body[0] = 0x11;
        let cart = Cartridge::from_bytes(&a78(body, 0)).unwrap();
        assert_eq!(cart.title, "Test");
        assert_eq!(cart.mapper, Mapper::Linear);
        assert_eq!(cart.read(0x8000), 0x11);
    }

    #[test]
    fn supergame_flag_is_in_low_byte_and_switches_banks() {
        // 128 КБ, в каждом банке по 16 КБ первый байт = номер банка
        let mut body = vec![0u8; 0x20000];
        for b in 0..8 {
            body[b * 0x4000] = b as u8;
        }
        let mut cart = Cartridge::from_bytes(&a78(body, 0x0002)).unwrap();
        assert_eq!(cart.mapper, Mapper::SuperGame);
        assert_eq!(cart.read(0xC000), 7); // последний банк неподвижен
        cart.write(0x8000, 3);
        assert_eq!(cart.read(0x8000), 3);
        cart.write(0xC000, 5); // выше $BFFF банк не переключается
        assert_eq!(cart.read(0x8000), 3);
    }

    #[test]
    fn pokey_location_from_header() {
        let c = Cartridge::from_bytes(&a78(vec![0; 0x4000], 0x0001)).unwrap();
        assert_eq!(c.pokey_at, Some(0x4000));
        let c = Cartridge::from_bytes(&a78(vec![0; 0x4000], 0x0040)).unwrap();
        assert_eq!(c.pokey_at, Some(0x0450));
        let c = Cartridge::from_bytes(&a78(vec![0; 0x4000], 0x8000)).unwrap();
        assert_eq!(c.pokey_at, Some(0x0800));
    }

    #[test]
    fn absolute_and_activision_are_chosen_by_size() {
        let mut body = vec![0u8; 0x10000];
        body[0] = 0xA0; // начало банка 0
        body[0x4000] = 0xA1; // начало банка 1
        let mut c = Cartridge::from_bytes(&a78(body, 0x0100)).unwrap();
        assert_eq!(c.mapper, Mapper::Absolute);
        c.write(0x8000, 1);
        assert_eq!(c.read(0x4000), 0xA0);
        c.write(0x8000, 2);
        assert_eq!(c.read(0x4000), 0xA1);

        let mut body = vec![0u8; 0x20000];
        for b in 0..8 {
            body[b * 0x4000] = b as u8;
        }
        let mut c = Cartridge::from_bytes(&a78(body, 0x0200)).unwrap();
        assert_eq!(c.mapper, Mapper::Activision);
        c.write(0xFF82, 0);
        assert_eq!(c.read(0xA000), 2);
        c.write(0xFF85, 0);
        assert_eq!(c.read(0xA000), 5);
        assert_eq!(c.read(0xE000), 7); // начало банка 7
    }

    #[test]
    fn unsupported_mappers_are_rejected_clearly() {
        let err = Cartridge::from_bytes(&a78(vec![0; 0x4000], 0x1000)).unwrap_err();
        assert!(matches!(err, LoadError::Unsupported(_)));
    }

    #[test]
    fn empty_is_rejected() {
        assert_eq!(Cartridge::from_bytes(&[]).unwrap_err(), LoadError::Empty);
    }
}
