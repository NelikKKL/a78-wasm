//! POKEY — звуковой чип, который ставили в картриджи (Ballblazer, Commando и др.).
//!
//! Реализовано: 4 канала, базовые частоты 64 кГц / 15 кГц и 1.79 МГц, 16-битные пары
//! каналов, все режимы искажения (полиномы 4/5/9/17), фильтры высоких частот,
//! регистр RANDOM. Не реализовано: клавиатура, потенциометры, последовательный порт, IRQ.

const REG_AUDCTL: u8 = 0x08;
const REG_STIMER: u8 = 0x09;
const REG_SKCTL: u8 = 0x0F;

pub struct Pokey {
    audf: [u8; 4],
    audc: [u8; 4],
    audctl: u8,
    skctl: u8,
    counter: [u32; 4],
    out: [bool; 4],
    hp: [bool; 2],
    base_cnt: u32,
    p4: u8,
    p5: u8,
    p9: u16,
    p17: u32,
    level: f32,
}

impl Default for Pokey {
    fn default() -> Self {
        Self::new()
    }
}

impl Pokey {
    pub fn new() -> Self {
        Self {
            audf: [0; 4],
            audc: [0; 4],
            audctl: 0,
            skctl: 0,
            counter: [0; 4],
            out: [false; 4],
            hp: [false; 2],
            base_cnt: 0,
            p4: 0x0F,
            p5: 0x1F,
            p9: 0x1FF,
            p17: 0x1FFFF,
            level: 0.0,
        }
    }

    /// Текущий уровень звука 0.0 … 1.0 (сумма четырёх каналов).
    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn read(&self, reg: u8) -> u8 {
        match reg & 0x0F {
            0x0A => {
                // RANDOM: старшие биты регистра сдвига, инвертированные
                let v = if self.audctl & 0x80 != 0 {
                    (self.p9 & 0xFF) as u8
                } else {
                    ((self.p17 >> 9) & 0xFF) as u8
                };
                !v
            }
            0x0E | 0x0F | 0x09 => 0xFF, // IRQST, SKSTAT, KBCODE: ничего не нажато, ошибок нет
            _ => 0,                     // POT0-7, ALLPOT
        }
    }

    pub fn write(&mut self, reg: u8, val: u8) {
        match reg & 0x0F {
            r @ (0x00 | 0x02 | 0x04 | 0x06) => self.audf[(r / 2) as usize] = val,
            r @ (0x01 | 0x03 | 0x05 | 0x07) => {
                self.audc[(r / 2) as usize] = val;
                self.update_level();
            }
            REG_AUDCTL => {
                self.audctl = val;
                self.update_level();
            }
            REG_STIMER => self.counter = [0; 4],
            REG_SKCTL => self.skctl = val,
            _ => {}
        }
    }

    /// Делитель канала: (тактируется ли каждый такт, период). `None` — канал занят старшим байтом пары.
    fn timing(&self, ch: usize) -> Option<(bool, u32)> {
        let ctl = self.audctl;
        let f = |i: usize| self.audf[i] as u32;
        match ch {
            0 => {
                if ctl & 0x10 != 0 {
                    return None;
                }
                let fast = ctl & 0x40 != 0;
                Some((fast, f(0) + if fast { 4 } else { 1 }))
            }
            1 => {
                if ctl & 0x10 != 0 {
                    let fast = ctl & 0x40 != 0;
                    Some((fast, ((f(1) << 8) | f(0)) + if fast { 7 } else { 1 }))
                } else {
                    Some((false, f(1) + 1))
                }
            }
            2 => {
                if ctl & 0x08 != 0 {
                    return None;
                }
                let fast = ctl & 0x20 != 0;
                Some((fast, f(2) + if fast { 4 } else { 1 }))
            }
            _ => {
                if ctl & 0x08 != 0 {
                    let fast = ctl & 0x20 != 0;
                    Some((fast, ((f(3) << 8) | f(2)) + if fast { 7 } else { 1 }))
                } else {
                    Some((false, f(3) + 1))
                }
            }
        }
    }

    /// Один такт процессора (1.79 МГц).
    pub fn clock(&mut self) {
        // Полиномиальные счётчики идут на полной частоте
        let fb4 = ((self.p4 >> 3) ^ (self.p4 >> 2)) & 1;
        self.p4 = ((self.p4 << 1) | fb4) & 0x0F;
        let fb5 = ((self.p5 >> 4) ^ (self.p5 >> 2)) & 1;
        self.p5 = ((self.p5 << 1) | fb5) & 0x1F;
        let fb9 = ((self.p9 >> 8) ^ (self.p9 >> 4)) & 1;
        self.p9 = ((self.p9 << 1) | fb9) & 0x1FF;
        let fb17 = ((self.p17 >> 16) ^ (self.p17 >> 11)) & 1;
        self.p17 = ((self.p17 << 1) | fb17) & 0x1FFFF;

        // Базовая частота: 64 кГц (÷28) или 15 кГц (÷114)
        self.base_cnt += 1;
        let base_period = if self.audctl & 0x01 != 0 { 114 } else { 28 };
        let base_pulse = if self.base_cnt >= base_period {
            self.base_cnt = 0;
            true
        } else {
            false
        };

        let mut expired = [false; 4];
        for ch in 0..4 {
            let Some((fast, period)) = self.timing(ch) else { continue };
            if !(fast || base_pulse) {
                continue;
            }
            if self.counter[ch] <= 1 {
                self.counter[ch] = period;
                self.expire(ch);
                expired[ch] = true;
            } else {
                self.counter[ch] -= 1;
            }
        }

        // Фильтры высоких частот: канал 1 по каналу 3, канал 2 по каналу 4
        if self.audctl & 0x04 != 0 && expired[2] {
            self.hp[0] = self.out[0];
        }
        if self.audctl & 0x02 != 0 && expired[3] {
            self.hp[1] = self.out[1];
        }
        if expired.iter().any(|&e| e) {
            self.update_level();
        }
    }

    /// Канал досчитал до нуля: обновляем его выход по режиму искажения.
    fn expire(&mut self, ch: usize) {
        let c = self.audc[ch];
        // D7 = 0: полином 5 бит "стробирует" — выход меняется только когда его бит равен 1
        if c & 0x80 != 0 || self.p5 & 1 != 0 {
            if c & 0x20 != 0 {
                self.out[ch] = !self.out[ch]; // чистый тон
            } else if c & 0x40 != 0 {
                self.out[ch] = self.p4 & 1 != 0;
            } else if self.audctl & 0x80 != 0 {
                self.out[ch] = self.p9 & 1 != 0;
            } else {
                self.out[ch] = self.p17 & 1 != 0;
            }
        }
    }

    fn update_level(&mut self) {
        let mut sum = 0u32;
        for ch in 0..4 {
            let c = self.audc[ch];
            let mut on = self.out[ch];
            if ch < 2 && self.audctl & (0x04 >> ch) != 0 {
                on ^= self.hp[ch];
            }
            if c & 0x10 != 0 || on {
                sum += (c & 0x0F) as u32;
            }
        }
        self.level = sum as f32 / 60.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_tone_has_expected_pitch() {
        let mut p = Pokey::new();
        p.write(0x00, 27); // AUDF1: период (27 + 1) * 28 тактов
        p.write(0x01, 0xA8); // чистый тон, громкость 8
        let mut toggles = 0;
        let mut last = p.level();
        for _ in 0..1_789_773 {
            p.clock();
            if (p.level() > 0.0) != (last > 0.0) {
                toggles += 1;
            }
            last = p.level();
        }
        // 1.79 МГц / 28 / 28 / 2 ≈ 1142 Гц
        let hz = toggles as f32 / 2.0;
        assert!((hz - 1142.0).abs() < 60.0, "частота {hz} Гц");
    }

    #[test]
    fn silent_without_volume() {
        let mut p = Pokey::new();
        p.write(0x01, 0xA0);
        for _ in 0..10_000 {
            p.clock();
        }
        assert_eq!(p.level(), 0.0);
    }

    #[test]
    fn random_changes() {
        let mut p = Pokey::new();
        let a = p.read(0x0A);
        for _ in 0..1000 {
            p.clock();
        }
        assert_ne!(a, p.read(0x0A));
    }
}
