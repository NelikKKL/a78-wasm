//! TIA в 7800 отвечает за звук (2 канала). Видео делает MARIA.
//!
//! Модель упрощённая: формы волн строятся на полиномиальных счётчиках
//! (LFSR 4/5/9 бит) и делителях, а не по точным таблицам TIA.
//! Высота тона верна, тембр шумовых режимов приблизительный.

/// Частота тактирования звука TIA (≈ 3.579545 МГц / 114).
pub const AUDIO_CLOCK_HZ: f32 = 31_399.5;

#[derive(Clone)]
struct Channel {
    audc: u8,
    audf: u8,
    audv: u8,
    count: u8,
    out: bool,
    step: u32,
    p4: u8,
    p5: u8,
    p9: u16,
}

impl Channel {
    fn new() -> Self {
        Self { audc: 0, audf: 0, audv: 0, count: 0, out: false, step: 0, p4: 0x0F, p5: 0x1F, p9: 0x1FF }
    }

    fn poly4(&mut self) -> bool {
        let fb = ((self.p4 >> 3) ^ (self.p4 >> 2)) & 1;
        self.p4 = ((self.p4 << 1) | fb) & 0x0F;
        self.p4 & 1 != 0
    }

    fn poly5(&mut self) -> bool {
        let fb = ((self.p5 >> 4) ^ (self.p5 >> 2)) & 1;
        self.p5 = ((self.p5 << 1) | fb) & 0x1F;
        self.p5 & 1 != 0
    }

    fn poly9(&mut self) -> bool {
        let fb = ((self.p9 >> 8) ^ (self.p9 >> 4)) & 1;
        self.p9 = ((self.p9 << 1) | fb) & 0x1FF;
        self.p9 & 1 != 0
    }

    /// Один такт 31.4 кГц.
    fn clock(&mut self) {
        if self.count == 0 {
            self.count = self.audf & 0x1F;
            self.step_wave();
        } else {
            self.count -= 1;
        }
    }

    fn step_wave(&mut self) {
        self.step = self.step.wrapping_add(1);
        let s = self.step;
        match self.audc & 0x0F {
            0 | 11 => self.out = true,
            1 => self.out = self.poly4(),
            2 => {
                if s % 31 >= 13 {
                    self.out = self.poly4();
                }
            }
            3 => {
                if self.poly5() {
                    self.out = self.poly4();
                }
            }
            4 | 5 => self.out = !self.out,
            6 | 10 => self.out = s % 31 >= 13,
            7 | 9 => self.out = self.poly5(),
            8 => self.out = self.poly9(),
            12 | 13 => self.out = (s / 3) % 2 == 0,
            14 => self.out = (s / 3) % 31 >= 13,
            15 => {
                if s % 3 == 0 {
                    self.out = self.poly5();
                }
            }
            _ => {}
        }
    }

    fn level(&self) -> u8 {
        if self.out {
            self.audv & 0x0F
        } else {
            0
        }
    }
}

pub struct Tia {
    ch: [Channel; 2],
    sample_rate: f32,
    phase: f32,
    last_in: f32,
    dc: f32,
    /// Накопленные моно-сэмплы f32 в диапазоне примерно [-1, 1].
    pub samples: Vec<f32>,
}

impl Tia {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            ch: [Channel::new(), Channel::new()],
            sample_rate: sample_rate as f32,
            phase: 0.0,
            last_in: 0.0,
            dc: 0.0,
            samples: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.ch = [Channel::new(), Channel::new()];
        self.phase = 0.0;
        self.last_in = 0.0;
        self.dc = 0.0;
        self.samples.clear();
    }

    /// Запись в регистры звука: $15 AUDC0, $16 AUDC1, $17 AUDF0, $18 AUDF1, $19 AUDV0, $1A AUDV1.
    pub fn write(&mut self, reg: u8, val: u8) {
        match reg {
            0x15 => self.ch[0].audc = val & 0x0F,
            0x16 => self.ch[1].audc = val & 0x0F,
            0x17 => self.ch[0].audf = val & 0x1F,
            0x18 => self.ch[1].audf = val & 0x1F,
            0x19 => self.ch[0].audv = val & 0x0F,
            0x1A => self.ch[1].audv = val & 0x0F,
            _ => {}
        }
    }

    /// Один такт звука. Вызывается дважды на строку развёртки.
    pub fn tick_audio(&mut self) {
        for c in &mut self.ch {
            c.clock();
        }
        let mix = (self.ch[0].level() + self.ch[1].level()) as f32 / 30.0;

        // Простой фильтр, убирающий постоянную составляющую
        let y = mix - self.last_in + 0.995 * self.dc;
        self.last_in = mix;
        self.dc = y;

        self.phase += self.sample_rate / AUDIO_CLOCK_HZ;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.samples.push(y);
        }
    }
}
