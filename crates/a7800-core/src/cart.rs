//! Картриджи: формат .a78 (заголовок 128 байт) и "сырые" образы без заголовка.
//!
//! Схемы банков:
//! - **Linear** — до 48 КБ, ROM прижата к концу адресного пространства;
//! - **SuperGame** — банки по 16 КБ: на $8000 переключаемый (запись в $8000–$BFFF),
//!   на $C000 всегда последний, на $4000 предпоследний (или RAM); размеры 64, 128, 256 КБ…;
//! - **SuperGame9** — 144 КБ: первые 16 КБ неподвижны на $4000, дальше 8 банков (Alien Brigade, Crossbow);
//! - **Absolute** — 64 КБ (F-18 Hornet);
//! - **Activision** — 128 КБ (Double Dragon, Rampage).
//!
//! Если заголовка нет (или он не говорит о банках), а ROM больше 48 КБ, схему определяет
//! `detect_mapper` по размеру и по коду внутри образа. Это эвристика: если игра ведёт себя
//! странно, добавьте к образу заголовок .a78 с нужным типом.
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
    SuperGame9,
    Absolute,
    Activision,
}

impl Mapper {
    pub fn name(self) -> &'static str {
        match self {
            Mapper::Linear => "без банков",
            Mapper::SuperGame => "SuperGame",
            Mapper::SuperGame9 => "SuperGame 9 банков",
            Mapper::Absolute => "Absolute",
            Mapper::Activision => "Activision",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Cartridge {
    pub title: String,
    pub mapper: Mapper,
    /// Адреса POKEY на картридже: $4000, $0450, $0440 и/или $0800.
    pub pokey_at: Vec<u16>,
    /// `true` — схема банков определена по содержимому образа, а не по заголовку.
    pub auto_detected: bool,
    rom: Vec<u8>,
    ram: Vec<u8>,
    /// Зеркальная RAM: нечётные страницы повторяют чётные.
    ram_mirror: bool,
    bank: usize,
    /// Картридж помечен как PAL (байт 57 заголовка).
    pub pal: bool,
    /// RAM на картридже банкуется, а мы даём плоские 16 КБ — упрощение.
    ram_approx: bool,
    /// RAM на $4000 создана автоматически: игра в неё пишет, а образ о ней ничего не сообщает.
    ram_auto: bool,
    /// Последние записи в ROM-пространство (адрес, значение) — переключения банков.
    bank_log: Vec<(u16, u8)>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum RamKind {
    #[default]
    None,
    Plain,
    Mirror,
}

/// Что картридж сообщает о своём железе (из полей v3 или v4 заголовка).
#[derive(Default)]
struct Hw {
    mapper: Option<Mapper>,
    ram: RamKind,
    ram_approx: bool,
    pokey_at: Vec<u16>,
}

/// Поля типа картриджа версии 3 (байты 53–54).
fn hw_from_v3(ct: u16, len: usize) -> Result<Hw, LoadError> {
    if ct & 0x1000 != 0 {
        return Err(LoadError::Unsupported("банки SOUPER".into()));
    }
    if ct & 0x2000 != 0 {
        return Err(LoadError::Unsupported("banksets".into()));
    }
    let mut hw = Hw::default();
    for (bit, addr) in [(0x0001u16, 0x4000u16), (0x0040, 0x0450), (0x0400, 0x0440), (0x8000, 0x0800)] {
        if ct & bit != 0 {
            hw.pokey_at.push(addr);
        }
    }
    hw.mapper = if ct & 0x0002 != 0 {
        Some(Mapper::SuperGame)
    } else if ct & 0x0300 != 0 {
        // Биты Activision/Absolute в разных версиях спецификации называют по-разному,
        // поэтому различаем по размеру: 64 КБ — Absolute, 128 КБ — Activision.
        match len {
            0x10000 => Some(Mapper::Absolute),
            0x20000 => Some(Mapper::Activision),
            n => return Err(LoadError::Unsupported(format!("особый банкинг для ROM {} КБ", n / 1024))),
        }
    } else {
        None
    };
    hw.ram = if ct & 0x0004 != 0 {
        RamKind::Plain
    } else if ct & 0x0080 != 0 {
        RamKind::Mirror
    } else {
        RamKind::None
    };
    Ok(hw)
}

/// Поля версии 4: схема (байт 64), её параметры (65) и аудио (66–67).
fn hw_from_v4(mapper: u8, options: u8, audio: u16, len: usize) -> Result<Hw, LoadError> {
    let mut hw = Hw::default();
    hw.pokey_at = match audio & 7 {
        1 => vec![0x0440],
        2 => vec![0x0450],
        3 => vec![0x0450, 0x0440],
        4 => vec![0x0800],
        5 => vec![0x4000],
        _ => Vec::new(),
    };
    match mapper {
        0 | 1 => {
            if options & 0x80 != 0 {
                return Err(LoadError::Unsupported("banksets".into()));
            }
            // 0 нет, 1 — RAM 16 КБ, 2 — RAM 8 КБ (зеркальная), 3 и 6 — банкованная RAM,
            // 4 — ROM на $4000 (разбираем по размеру), 5 — банк 6 на $4000 (так и работает)
            let opt = if mapper == 0 { options & 3 } else { options & 7 };
            hw.ram = match opt {
                1 => RamKind::Plain,
                2 => RamKind::Mirror,
                3 | 6 => {
                    hw.ram_approx = true;
                    RamKind::Plain
                }
                _ => RamKind::None,
            };
            hw.mapper = Some(if mapper == 0 { Mapper::Linear } else { Mapper::SuperGame });
        }
        2 | 3 => {
            let (m, want) = if mapper == 2 { (Mapper::Activision, 0x20000) } else { (Mapper::Absolute, 0x10000) };
            if len != want {
                return Err(LoadError::Unsupported(format!(
                    "{} требует ROM {} КБ, а здесь {} КБ",
                    m.name(),
                    want / 1024,
                    len / 1024
                )));
            }
            hw.mapper = Some(m);
        }
        4 => return Err(LoadError::Unsupported("банки SOUPER".into())),
        n => return Err(LoadError::Unsupported(format!("неизвестная схема картриджа {n} в заголовке"))),
    }
    Ok(hw)
}

// ---- определение схемы банков по содержимому ----

fn contains_seq(rom: &[u8], pat: &[u8]) -> bool {
    rom.windows(pat.len()).any(|w| w == pat)
}

/// F-18 Hornet выбирает банк записью 1 или 2 в $8000.
fn has_absolute_switch(rom: &[u8]) -> bool {
    [
        &[0xA9, 0x01, 0x8D, 0x00, 0x80][..], // LDA #1 ; STA $8000
        &[0xA9, 0x02, 0x8D, 0x00, 0x80][..],
        &[0xA2, 0x01, 0x8E, 0x00, 0x80][..], // LDX #1 ; STX $8000
        &[0xA2, 0x02, 0x8E, 0x00, 0x80][..],
    ]
    .iter()
    .any(|p| contains_seq(rom, p))
}

/// Activision переключает банк записью в $FF80–$FF87; SuperGame — записью в $8000.
fn looks_like_activision(rom: &[u8]) -> bool {
    let activision = rom.windows(3).any(|w| {
        matches!(w[0], 0x8D | 0x8E | 0x8C | 0x9D | 0x99) && (0x80..=0x87).contains(&w[1]) && w[2] == 0xFF
    });
    let supergame = rom
        .windows(3)
        .any(|w| matches!(w[0], 0x8D | 0x8E | 0x8C) && w[1] == 0x00 && w[2] == 0x80);
    activision && !supergame
}

/// Выбирает схему банков для образа без указаний в заголовке.
pub fn detect_mapper(rom: &[u8]) -> Result<Mapper, LoadError> {
    let len = rom.len();
    match len {
        0..=0xC000 => Ok(Mapper::Linear),
        0x24000 => Ok(Mapper::SuperGame9),
        0x10000 => Ok(if has_absolute_switch(rom) { Mapper::Absolute } else { Mapper::SuperGame }),
        0x20000 => Ok(if looks_like_activision(rom) { Mapper::Activision } else { Mapper::SuperGame }),
        n if n % 0x4000 == 0 => Ok(Mapper::SuperGame),
        n => Err(LoadError::Unsupported(format!(
            "размер ROM {n} байт не похож на картридж 7800 (ожидается кратный 16 КБ)"
        ))),
    }
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
            let pal = data[57] & 1 != 0;

            // Версия 4: поля 64–67 главнее старых байтов 53–54. Значение 0xFF означает "не задано".
            let v4 = |i: usize| if data[i] == 0xFF { 0 } else { data[i] };
            let v4_set = data[0] >= 4 && (v4(64) | v4(65) | v4(66) | v4(67)) != 0;
            let hw = if v4_set {
                hw_from_v4(v4(64), v4(65), u16::from_be_bytes([v4(66), v4(67)]), rom.len())?
            } else {
                hw_from_v3(u16::from_be_bytes([data[53], data[54]]), rom.len())?
            };

            let (mapper, auto_detected) = match hw.mapper {
                // Заголовок говорит "без банков", но ROM слишком велик — определяем сами
                Some(Mapper::Linear) | None if rom.len() > 0xC000 => (detect_mapper(&rom)?, true),
                Some(m) => (m, false),
                None => (Mapper::Linear, false),
            };
            let mut cart = Self::build(title, rom, mapper, hw.ram, hw.pokey_at, auto_detected)?;
            cart.pal = pal;
            cart.ram_approx = hw.ram_approx;
            return Ok(cart);
        }

        // Образ без заголовка
        let mapper = detect_mapper(data)?;
        let auto_detected = mapper != Mapper::Linear;
        Self::build(String::new(), data.to_vec(), mapper, RamKind::None, Vec::new(), auto_detected)
    }

    fn build(
        title: String,
        rom: Vec<u8>,
        mapper: Mapper,
        ram: RamKind,
        pokey_at: Vec<u16>,
        auto_detected: bool,
    ) -> Result<Self, LoadError> {
        if rom.is_empty() {
            return Err(LoadError::Empty);
        }
        // SuperGame на 144 КБ — это схема "9 банков"
        let mapper = if mapper == Mapper::SuperGame && rom.len() == 0x24000 { Mapper::SuperGame9 } else { mapper };
        match mapper {
            Mapper::Linear if rom.len() > 0xC000 => {
                return Err(LoadError::Unsupported(format!(
                    "ROM {} КБ без банкинга не поместится в адресное пространство",
                    rom.len() / 1024
                )));
            }
            Mapper::SuperGame if rom.len() < 0x4000 || rom.len() % 0x4000 != 0 => {
                return Err(LoadError::Unsupported("размер SuperGame ROM не кратен 16 КБ".into()));
            }
            Mapper::SuperGame9 if rom.len() != 0x24000 => {
                return Err(LoadError::Unsupported("SuperGame 9 банков требует ROM 144 КБ".into()));
            }
            _ => {}
        }
        Ok(Self {
            title,
            mapper,
            pokey_at,
            auto_detected,
            rom,
            ram: if ram != RamKind::None { vec![0; 0x4000] } else { Vec::new() },
            ram_mirror: ram == RamKind::Mirror,
            bank: 0,
            pal: false,
            ram_approx: false,
            ram_auto: false,
            bank_log: Vec::new(),
        })
    }

    pub fn rom_len(&self) -> usize {
        self.rom.len()
    }

    /// Короткое описание для отладочной панели.
    pub fn describe(&self) -> String {
        let mut s = format!("{}, {} КБ", self.mapper.name(), self.rom.len() / 1024);
        if self.auto_detected {
            s.push_str(" (определено автоматически)");
        }
        if !self.ram.is_empty() {
            s.push_str(", RAM на $4000");
            if self.ram_approx {
                s.push_str(" (банкованная — упрощено до 16 КБ)");
            }
            if self.ram_auto {
                s.push_str(" (создана автоматически по записи игры)");
            }
        }
        for a in &self.pokey_at {
            s.push_str(&format!(", POKEY ${a:04X}"));
        }
        if self.pal {
            s.push_str(", PAL");
        }
        s
    }

    /// Текущее состояние банков и последние записи в ROM-пространство — для отладки.
    pub fn state(&self) -> String {
        let writes: Vec<String> = self.bank_log.iter().map(|(a, v)| format!("{a:04X}={v:02X}")).collect();
        format!("банк {}, записи в ROM: {}", self.bank, if writes.is_empty() { "нет".into() } else { writes.join(" ") })
    }

    fn ram_index(&self, a: usize) -> usize {
        let off = a - 0x4000;
        if self.ram_mirror {
            off & 0xFEFF
        } else {
            off
        }
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
                    self.ram[self.ram_index(a)]
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
                            self.ram[self.ram_index(a)]
                        } else if banks >= 2 {
                            // Что здесь на железе, точно не известно; как и MAME, берём предпоследний банк
                            self.rom[(banks - 2) * 0x4000 + (a - 0x4000)]
                        } else {
                            0xFF
                        }
                    }
                    _ => 0xFF,
                }
            }
            Mapper::SuperGame9 => match a {
                0x4000..=0x7FFF => self.rom[a - 0x4000],
                0x8000..=0xBFFF => self.rom[self.bank * 0x4000 + (a - 0x8000)],
                0xC000..=0xFFFF => self.rom[8 * 0x4000 + (a - 0xC000)],
                _ => 0xFF,
            },
            Mapper::Absolute => match a {
                // два банка по 16 КБ на $4000, 32 КБ неподвижны на $8000
                0x4000..=0x7FFF => self.rom[(self.bank & 1) * 0x4000 + (a - 0x4000)],
                0x8000..=0xFFFF => self.rom[a],
                _ => 0xFF,
            },
            Mapper::Activision => match a {
                // 8 банков по 16 КБ. Неподвижно: банк 6 на $4000–$7FFF, первая половина банка 7
                // на $8000–$9FFF, вторая — на $E000–$FFFF. Окно $A000–$DFFF переключается,
                // причём его половины по 8 КБ идут в обратном порядке.
                0x4000..=0x7FFF => self.rom[0x18000 + (a - 0x4000)],
                0x8000..=0x9FFF => self.rom[0x1C000 + (a - 0x8000)],
                0xA000..=0xBFFF => self.rom[(self.bank & 7) * 0x4000 + 0x2000 + (a - 0xA000)],
                0xC000..=0xDFFF => self.rom[(self.bank & 7) * 0x4000 + (a - 0xC000)],
                0xE000..=0xFFFF => self.rom[0x1E000 + (a - 0xE000)],
                _ => 0xFF,
            },
        }
    }

    /// Может ли в $4000–$7FFF быть RAM: там не должно лежать ROM, которую читает игра.
    fn ram_can_appear(&self) -> bool {
        match self.mapper {
            Mapper::SuperGame => true,
            Mapper::Linear => 0x10000 - self.rom.len() >= 0x8000,
            _ => false,
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        let a = addr as usize;
        if (0x4000..0x8000).contains(&a) {
            // Образ без заголовка не говорит, есть ли на картридже RAM. Если игра пишет в
            // $4000–$7FFF — она есть (так, например, у Winter Games), иначе записи терялись бы,
            // а MARIA рисовала бы мусор.
            if self.ram.is_empty() && self.ram_can_appear() {
                self.ram = vec![0; 0x4000];
                self.ram_auto = true;
            }
            if !self.ram.is_empty() {
                let i = self.ram_index(a);
                self.ram[i] = val;
                return;
            }
        }
        if self.bank_log.len() >= 6 {
            self.bank_log.remove(0);
        }
        self.bank_log.push((addr, val));
        match self.mapper {
            Mapper::SuperGame if (0x8000..0xC000).contains(&a) => {
                let banks = self.rom.len() / 0x4000;
                let mask = banks.next_power_of_two() - 1;
                self.bank = (val as usize & mask) % banks;
            }
            Mapper::SuperGame9 if (0x8000..0xC000).contains(&a) => self.bank = (val as usize & 7) + 1,
            // F-18 Hornet пишет 1 (банк 0) или 2 (банк 1) в $8000
            Mapper::Absolute if a == 0x8000 => {
                if val & 1 != 0 {
                    self.bank = 0;
                } else if val & 2 != 0 {
                    self.bank = 1;
                }
            }
            // Double Dragon / Rampage: номер банка — в младших битах адреса записи выше $E000
            Mapper::Activision if a >= 0xE000 => self.bank = a & 7,
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

    /// ROM из `banks` банков по 16 КБ; первый байт каждого банка равен его номеру.
    fn banked(banks: usize) -> Vec<u8> {
        let mut rom = vec![0u8; banks * 0x4000];
        for b in 0..banks {
            rom[b * 0x4000] = b as u8;
        }
        rom
    }

    #[test]
    fn raw_16k_is_mapped_at_c000() {
        let mut rom = vec![0u8; 0x4000];
        rom[0] = 0xAB;
        rom[0x3FFF] = 0xCD;
        let cart = Cartridge::from_bytes(&rom).unwrap();
        assert_eq!(cart.mapper, Mapper::Linear);
        assert!(!cart.auto_detected);
        assert_eq!(cart.read(0xC000), 0xAB);
        assert_eq!(cart.read(0xFFFF), 0xCD);
        assert_eq!(cart.read(0x8000), 0xFF);
    }

    #[test]
    fn raw_128k_without_header_is_loaded_as_supergame() {
        let mut cart = Cartridge::from_bytes(&banked(8)).unwrap();
        assert_eq!(cart.mapper, Mapper::SuperGame);
        assert!(cart.auto_detected);
        assert_eq!(cart.read(0xC000), 7); // последний банк неподвижен
        assert_eq!(cart.read(0x8000), 0); // по умолчанию банк 0
        assert_eq!(cart.read(0x4000), 6); // предпоследний банк
        cart.write(0x8000, 3);
        assert_eq!(cart.read(0x8000), 3);
        cart.write(0xC000, 5); // выше $BFFF банк не переключается
        assert_eq!(cart.read(0x8000), 3);
    }

    #[test]
    fn raw_128k_with_activision_code_is_detected() {
        let mut rom = banked(8);
        rom[0x100..0x103].copy_from_slice(&[0x8D, 0x82, 0xFF]); // STA $FF82
        let cart = Cartridge::from_bytes(&rom).unwrap();
        assert_eq!(cart.mapper, Mapper::Activision);

        // а запись в $8000 возвращает выбор в пользу SuperGame
        rom[0x200..0x203].copy_from_slice(&[0x8D, 0x00, 0x80]);
        assert_eq!(Cartridge::from_bytes(&rom).unwrap().mapper, Mapper::SuperGame);
    }

    #[test]
    fn raw_sizes_are_mapped_sensibly() {
        assert_eq!(Cartridge::from_bytes(&banked(2)).unwrap().mapper, Mapper::Linear);
        assert_eq!(Cartridge::from_bytes(&banked(3)).unwrap().mapper, Mapper::Linear); // 48 КБ
        assert_eq!(Cartridge::from_bytes(&banked(4)).unwrap().mapper, Mapper::SuperGame); // 64 КБ
        assert_eq!(Cartridge::from_bytes(&banked(16)).unwrap().mapper, Mapper::SuperGame); // 256 КБ
        assert_eq!(Cartridge::from_bytes(&banked(9)).unwrap().mapper, Mapper::SuperGame9); // 144 КБ
        assert!(Cartridge::from_bytes(&vec![0u8; 100_000]).is_err());
    }

    #[test]
    fn absolute_is_detected_from_code() {
        let mut rom = banked(4);
        rom[0x40..0x45].copy_from_slice(&[0xA9, 0x02, 0x8D, 0x00, 0x80]);
        assert_eq!(Cartridge::from_bytes(&rom).unwrap().mapper, Mapper::Absolute);
    }

    #[test]
    fn a78_header_without_bank_flags_but_big_rom_is_autodetected() {
        let cart = Cartridge::from_bytes(&a78(banked(8), 0)).unwrap();
        assert_eq!(cart.mapper, Mapper::SuperGame);
        assert!(cart.auto_detected);
        assert_eq!(cart.title, "Test");
    }

    #[test]
    fn a78_header_is_parsed() {
        let mut body = vec![0u8; 0x8000];
        body[0] = 0x11;
        let cart = Cartridge::from_bytes(&a78(body, 0)).unwrap();
        assert_eq!(cart.mapper, Mapper::Linear);
        assert_eq!(cart.read(0x8000), 0x11);
    }

    #[test]
    fn supergame_flag_is_in_low_byte() {
        let cart = Cartridge::from_bytes(&a78(banked(8), 0x0002)).unwrap();
        assert_eq!(cart.mapper, Mapper::SuperGame);
        assert!(!cart.auto_detected);
    }

    #[test]
    fn ram_appears_when_game_writes_to_4000() {
        // образ без заголовка, о RAM ничего не известно: до записи там ROM предпоследнего банка
        let mut cart = Cartridge::from_bytes(&banked(8)).unwrap();
        assert_eq!(cart.read(0x4000), 6);
        cart.write(0x5866, 0x0C);
        assert_eq!(cart.read(0x5866), 0x0C);
        assert!(cart.describe().contains("автоматически по записи"));
        // записи в ROM-пространство ($8000 и выше) RAM не создают
        let mut cart = Cartridge::from_bytes(&banked(8)).unwrap();
        cart.write(0x8000, 3);
        assert!(!cart.describe().contains("RAM"));
        // у 48-КБ образа в $4000–$7FFF лежит ROM — RAM там появиться не может
        let mut cart = Cartridge::from_bytes(&banked(3)).unwrap();
        cart.write(0x4000, 0x55);
        assert_ne!(cart.read(0x4000), 0x55);
    }

    #[test]
    fn supergame_ram_replaces_bank_at_4000() {
        let mut cart = Cartridge::from_bytes(&a78(banked(8), 0x0006)).unwrap();
        cart.write(0x4000, 0x5A);
        assert_eq!(cart.read(0x4000), 0x5A);
    }

    #[test]
    fn supergame9_layout() {
        let mut cart = Cartridge::from_bytes(&banked(9)).unwrap();
        assert_eq!(cart.read(0x4000), 0); // первые 16 КБ неподвижны
        assert_eq!(cart.read(0xC000), 8); // последний банк неподвижен
        cart.write(0x8000, 2); // логический банк 2 = банк 3 в файле
        assert_eq!(cart.read(0x8000), 3);
    }

    #[test]
    fn pokey_locations_from_header() {
        let c = Cartridge::from_bytes(&a78(vec![0; 0x4000], 0x0001)).unwrap();
        assert_eq!(c.pokey_at, vec![0x4000]);
        let c = Cartridge::from_bytes(&a78(vec![0; 0x4000], 0x0040)).unwrap();
        assert_eq!(c.pokey_at, vec![0x0450]);
        let c = Cartridge::from_bytes(&a78(vec![0; 0x4000], 0x8000)).unwrap();
        assert_eq!(c.pokey_at, vec![0x0800]);
        // три бита сразу: $4000, $0450 и второй POKEY на $0440
        let c = Cartridge::from_bytes(&a78(vec![0; 0x4000], 0x0441)).unwrap();
        assert_eq!(c.pokey_at, vec![0x4000, 0x0450, 0x0440]);
    }

    #[test]
    fn mirror_ram_repeats_even_pages() {
        let mut c = Cartridge::from_bytes(&a78(vec![0; 0x4000], 0x0080)).unwrap();
        c.write(0x4000, 0x77);
        assert_eq!(c.read(0x4100), 0x77); // страница 1 — зеркало страницы 0
    }

    #[test]
    fn absolute_and_activision_by_header_and_size() {
        let mut body = banked(4);
        body[0x4000] = 0xA1;
        body[0] = 0xA0;
        let mut c = Cartridge::from_bytes(&a78(body, 0x0100)).unwrap();
        assert_eq!(c.mapper, Mapper::Absolute);
        c.write(0x8000, 1);
        assert_eq!(c.read(0x4000), 0xA0);
        c.write(0x8000, 2);
        assert_eq!(c.read(0x4000), 0xA1);
        c.write(0x8000, 0); // другие значения банк не меняют
        assert_eq!(c.read(0x4000), 0xA1);

        // Activision: пометим начало каждой 8-килобайтной половины
        let mut body = vec![0u8; 0x20000];
        for i in 0..16 {
            body[i * 0x2000] = i as u8;
        }
        let mut c = Cartridge::from_bytes(&a78(body, 0x0200)).unwrap();
        assert_eq!(c.mapper, Mapper::Activision);
        assert_eq!(c.read(0x4000), 12); // банк 6, первая половина
        assert_eq!(c.read(0x6000), 13);
        assert_eq!(c.read(0x8000), 14); // банк 7, первая половина
        assert_eq!(c.read(0xE000), 15); // банк 7, вторая половина
        c.write(0xFF82, 0); // банк 2
        assert_eq!(c.read(0xC000), 4); // начало банка 2 — на $C000
        assert_eq!(c.read(0xA000), 5); // вторая половина — на $A000
    }

    #[test]
    fn unsupported_mappers_are_rejected_clearly() {
        let err = Cartridge::from_bytes(&a78(vec![0; 0x4000], 0x1000)).unwrap_err();
        assert!(matches!(err, LoadError::Unsupported(_)));
    }

    #[test]
    fn describe_mentions_auto_detection() {
        let d = Cartridge::from_bytes(&banked(8)).unwrap().describe();
        assert!(d.contains("SuperGame") && d.contains("автоматически") && d.contains("128"));
    }

    /// Заголовок версии 4: старые байты 53–54 нулевые, всё лежит в 64–67.
    fn a78_v4(body: Vec<u8>, mapper: u8, options: u8, audio: u16, tv: u8) -> Vec<u8> {
        let mut d = a78(body, 0);
        d[0] = 4;
        d[57] = tv;
        d[64] = mapper;
        d[65] = options;
        d[66..68].copy_from_slice(&audio.to_be_bytes());
        d
    }

    #[test]
    fn v4_header_overrides_empty_v3_fields() {
        // SuperGame + 16 КБ RAM на $4000 + POKEY на $0450, байты 53–54 пустые
        let mut c = Cartridge::from_bytes(&a78_v4(banked(8), 1, 1, 2, 0)).unwrap();
        assert_eq!(c.mapper, Mapper::SuperGame);
        assert!(!c.auto_detected);
        assert_eq!(c.pokey_at, vec![0x0450]);
        c.write(0x4000, 0x5A);
        assert_eq!(c.read(0x4000), 0x5A);

        // пример из спецификации: Commando = SuperGame + POKEY на $4000
        let c = Cartridge::from_bytes(&a78_v4(banked(8), 1, 0, 5, 0)).unwrap();
        assert_eq!(c.pokey_at, vec![0x4000]);

        // два POKEY и режим 8 КБ зеркальной RAM у линейной ROM
        let mut c = Cartridge::from_bytes(&a78_v4(vec![0; 0x4000], 0, 2, 3, 0)).unwrap();
        assert_eq!(c.pokey_at, vec![0x0450, 0x0440]);
        c.write(0x4000, 0x77);
        assert_eq!(c.read(0x4100), 0x77);
    }

    #[test]
    fn v4_activision_absolute_and_unsupported() {
        assert_eq!(Cartridge::from_bytes(&a78_v4(banked(8), 2, 0, 0, 0)).unwrap().mapper, Mapper::Activision);
        assert_eq!(Cartridge::from_bytes(&a78_v4(banked(4), 3, 0, 0, 0)).unwrap().mapper, Mapper::Absolute);
        assert!(Cartridge::from_bytes(&a78_v4(banked(4), 2, 0, 0, 0)).is_err()); // Activision не на 64 КБ
        assert!(Cartridge::from_bytes(&a78_v4(banked(8), 4, 0, 0, 0)).is_err()); // SOUPER
        assert!(Cartridge::from_bytes(&a78_v4(banked(8), 1, 0x80, 0, 0)).is_err()); // bankset
    }

    #[test]
    fn v4_with_all_zero_fields_falls_back_to_v3_bytes() {
        let mut d = a78(banked(8), 0x0002);
        d[0] = 4; // версия 4, но поля 64–67 не заполнены
        assert_eq!(Cartridge::from_bytes(&d).unwrap().mapper, Mapper::SuperGame);
    }

    #[test]
    fn tv_type_marks_pal() {
        assert!(Cartridge::from_bytes(&a78_v4(vec![0; 0x4000], 0, 0, 0, 1)).unwrap().pal);
        assert!(!Cartridge::from_bytes(&a78_v4(vec![0; 0x4000], 0, 0, 0, 0)).unwrap().pal);
    }

    #[test]
    fn bank_writes_are_logged() {
        let mut c = Cartridge::from_bytes(&banked(8)).unwrap();
        c.write(0x8000, 3);
        c.write(0x8000, 5);
        assert!(c.state().contains("8000=05"), "{}", c.state());
    }

    #[test]
    fn empty_is_rejected() {
        assert_eq!(Cartridge::from_bytes(&[]).unwrap_err(), LoadError::Empty);
    }
}
