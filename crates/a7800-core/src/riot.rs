//! RIOT (MOS 6532): два 8-битных порта, интервальный таймер и 128 байт RAM.

pub struct Riot {
    ram: [u8; 128],
    swcha_out: u8,
    ddra: u8,
    swchb_out: u8,
    ddrb: u8,
    /// Что "подаёт" внешний мир на порт A (джойстики, активный уровень — низкий).
    pub swcha_in: u8,
    /// Что подаёт внешний мир на порт B (консольные переключатели).
    pub swchb_in: u8,
    timer: i64,
    interval: i64,
    underflow: bool,
}

impl Default for Riot {
    fn default() -> Self {
        Self::new()
    }
}

impl Riot {
    pub fn new() -> Self {
        Self {
            ram: [0; 128],
            swcha_out: 0,
            ddra: 0,
            swchb_out: 0,
            ddrb: 0,
            swcha_in: 0xFF,
            swchb_in: 0xFF,
            timer: 0,
            interval: 1024,
            underflow: false,
        }
    }

    /// Двигает таймер на `cycles` тактов процессора.
    pub fn tick(&mut self, cycles: u32) {
        let was_running = self.timer >= 0;
        self.timer -= cycles as i64;
        if was_running && self.timer < 0 {
            self.underflow = true;
        }
    }

    /// Выведена ли линия порта B в ноль (бит `mask` — выход со значением 0).
    /// Так игра включает режим двух кнопок на PB2 (джойстик 0) и PB4 (джойстик 1).
    pub fn port_b_driven_low(&self, mask: u8) -> bool {
        self.ddrb & mask != 0 && self.swchb_out & mask == 0
    }

    pub fn ram_read(&self, addr: u16) -> u8 {
        self.ram[(addr & 0x7F) as usize]
    }

    pub fn ram_write(&mut self, addr: u16, val: u8) {
        self.ram[(addr & 0x7F) as usize] = val;
    }

    /// Чтение регистров ($0280–$02FF).
    pub fn read(&mut self, addr: u16) -> u8 {
        if addr & 0x04 == 0 {
            match addr & 3 {
                0 => (self.swcha_out & self.ddra) | (self.swcha_in & !self.ddra),
                1 => self.ddra,
                2 => (self.swchb_out & self.ddrb) | (self.swchb_in & !self.ddrb),
                _ => self.ddrb,
            }
        } else if addr & 1 == 0 {
            // INTIM
            self.underflow = false;
            if self.timer >= 0 {
                (self.timer / self.interval) as u8
            } else {
                (self.timer & 0xFF) as u8
            }
        } else {
            // INSTAT
            (self.underflow as u8) << 7
        }
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        if addr & 0x04 == 0 {
            match addr & 3 {
                0 => self.swcha_out = val,
                1 => self.ddra = val,
                2 => self.swchb_out = val,
                _ => self.ddrb = val,
            }
        } else if addr & 0x10 != 0 {
            // TIM1T / TIM8T / TIM64T / T1024T
            self.interval = [1, 8, 64, 1024][(addr & 3) as usize];
            self.timer = val as i64 * self.interval;
            self.underflow = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_counts_down_then_underflows() {
        let mut r = Riot::new();
        r.write(0x0296, 10); // TIM64T = 10
        r.tick(64 * 3);
        assert_eq!(r.read(0x0284), 7);
        r.tick(64 * 7 + 1);
        assert_eq!(r.read(0x0285) & 0x80, 0x80);
        assert_eq!(r.read(0x0284), 0xFF);
    }
}
