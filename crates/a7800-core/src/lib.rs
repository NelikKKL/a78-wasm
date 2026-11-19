//! Ядро эмулятора Atari 7800.
//!
//! Библиотека не зависит от платформы: ни окна, ни звуковой карты, ни клавиатуры.
//! Всё это подключает "хост" — через трейт [`Host`] или прямыми вызовами.
//!
//! Два способа использования.
//!
//! 1. Вручную (так работает веб-версия: JS сам рисует и играет звук):
//!
//! ```ignore
//! let mut emu = Emulator::new(Config::default());
//! emu.load_cartridge(&rom_bytes)?;
//! loop {
//!     bindings.handle_key("KeyZ", true, emu.input_mut());
//!     emu.run_frame();
//!     draw(emu.frame());              // RGBA, 320x224
//!     play(&emu.take_audio());        // моно f32
//! }
//! ```
//!
//! 2. Через [`Host`] — для нативного окна (winit/SDL/minifb) или тестов:
//!
//! ```ignore
//! emu.run(&mut my_window);            // крутится, пока host.should_quit() == false
//! ```

pub mod bindings;
pub mod cart;
pub mod cpu;
pub mod input;
pub mod maria;
pub mod palette;
pub mod riot;
pub mod system;
pub mod tia;

pub use bindings::Bindings;
pub use cart::{Cartridge, LoadError, Mapper};
pub use input::{Action, Button, InputState};
pub use maria::{SCREEN_H, SCREEN_W};
pub use palette::Palette;

use cpu::Cpu;
use system::System;

/// Тактов процессора на строку развёртки (NTSC).
pub const CYCLES_PER_LINE: i32 = 114;
/// Строк в кадре (NTSC).
pub const LINES_PER_FRAME: usize = 262;
/// Первая строка, которую рисует MARIA. Подбирается под конкретные игры.
pub const ACTIVE_START: usize = 16;
/// Частота кадров NTSC.
pub const FRAME_RATE: f64 = 59.92;

#[derive(Clone, Debug)]
pub struct Config {
    /// Частота дискретизации выходного звука.
    pub sample_rate: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self { sample_rate: 48_000 }
    }
}

/// Готовый кадр: RGBA, 8 бит на канал, построчно.
#[derive(Clone)]
pub struct FrameBuffer {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl FrameBuffer {
    fn new() -> Self {
        let mut pixels = vec![0u8; SCREEN_W * SCREEN_H * 4];
        for px in pixels.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Self { width: SCREEN_W, height: SCREEN_H, pixels }
    }
}

/// Сведения о загруженном картридже.
#[derive(Clone, Debug)]
pub struct CartInfo {
    pub title: String,
    pub mapper: Mapper,
    pub rom_size: usize,
}

/// Всё, что ядро ждёт от платформы: окно, звук и ввод.
///
/// Реализуйте этот трейт для вашего окна (web canvas, winit, SDL …),
/// и [`Emulator::run_frame_with`] сам вызовет нужные методы в нужном порядке.
pub trait Host {
    /// Заполнить состояние контроллеров перед кадром (например, через [`Bindings`]).
    fn poll_input(&mut self, input: &mut InputState);
    /// Показать готовый кадр.
    fn present(&mut self, frame: &FrameBuffer);
    /// Отправить новые звуковые сэмплы (моно, f32, `Config::sample_rate`).
    fn queue_audio(&mut self, samples: &[f32]);
    /// Пора выходить из цикла `Emulator::run`.
    fn should_quit(&self) -> bool {
        false
    }
}

pub struct Emulator {
    cpu: Cpu,
    sys: System,
    frame: FrameBuffer,
    palette: Palette,
    carry: i32,
    frames: u64,
}

impl Emulator {
    pub fn new(config: Config) -> Self {
        Self {
            cpu: Cpu::new(),
            sys: System::new(config.sample_rate),
            frame: FrameBuffer::new(),
            palette: palette::ntsc(),
            carry: 0,
            frames: 0,
        }
    }

    /// Загружает картридж (.a78 или "сырой" образ) и перезапускает консоль.
    pub fn load_cartridge(&mut self, data: &[u8]) -> Result<CartInfo, LoadError> {
        let cart = Cartridge::from_bytes(data)?;
        let info = CartInfo { title: cart.title.clone(), mapper: cart.mapper, rom_size: cart.rom_len() };
        self.sys.mem.cart = Some(cart);
        self.reset();
        Ok(info)
    }

    /// Необязательный BIOS (4 или 16 КБ). Сам эмулятор его не содержит.
    pub fn load_bios(&mut self, data: &[u8]) -> Result<(), LoadError> {
        self.sys.mem.set_bios(data)?;
        self.reset();
        Ok(())
    }

    pub fn reset(&mut self) {
        self.sys.reset();
        self.cpu.reset(&mut self.sys);
        self.carry = 0;
    }

    pub fn is_ready(&self) -> bool {
        self.sys.mem.cart.is_some()
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    pub fn frame(&self) -> &FrameBuffer {
        &self.frame
    }

    pub fn frames_emulated(&self) -> u64 {
        self.frames
    }

    pub fn input_mut(&mut self) -> &mut InputState {
        &mut self.sys.input
    }

    /// Удобная обёртка: применяет клавишу через привязки. `true`, если клавиша привязана.
    pub fn handle_key(&mut self, bindings: &Bindings, key: &str, pressed: bool) -> bool {
        bindings.handle_key(key, pressed, &mut self.sys.input)
    }

    /// Забирает накопленные звуковые сэмплы.
    pub fn take_audio(&mut self) -> Vec<f32> {
        std::mem::take(&mut self.sys.tia.samples)
    }

    /// Эмулирует один кадр (262 строки).
    pub fn run_frame(&mut self) {
        if !self.is_ready() {
            return;
        }
        self.sys.sync_ports();
        let mut carry = self.carry;

        for line in 0..LINES_PER_FRAME {
            let active = (ACTIVE_START..ACTIVE_START + SCREEN_H).contains(&line);
            self.sys.maria.vblank = !active;
            if line == ACTIVE_START {
                self.sys.maria.start_frame();
            }

            let mut dma = 0i32;
            let mut nmi = false;
            if active {
                let off = (line - ACTIVE_START) * SCREEN_W * 4;
                let row = &mut self.frame.pixels[off..off + SCREEN_W * 4];
                let info = self.sys.maria.render_line(&self.sys.mem, row, &self.palette);
                dma = info.dma_cycles as i32;
                nmi = info.nmi;
            }

            self.sys.riot.tick(dma as u32);
            if nmi {
                let c = self.cpu.nmi(&mut self.sys);
                self.sys.riot.tick(c);
                carry -= c as i32;
            }

            // Процессор получает остаток строки; WSYNC отдаёт её целиком.
            self.sys.wsync = false;
            let mut budget = CYCLES_PER_LINE - dma + carry;
            while budget > 0 {
                let c = self.cpu.step(&mut self.sys) as i32;
                self.sys.riot.tick(c as u32);
                budget -= c;
                if self.sys.wsync {
                    break;
                }
            }
            if self.sys.wsync {
                if budget > 0 {
                    self.sys.riot.tick(budget as u32);
                }
                carry = 0;
            } else {
                carry = budget.max(-CYCLES_PER_LINE);
            }

            // Звук TIA тактируется ~31.4 кГц — это два раза на строку.
            self.sys.tia.tick_audio();
            self.sys.tia.tick_audio();
        }

        self.carry = carry;
        self.frames += 1;
    }

    /// Один кадр через хост: опрос ввода → эмуляция → показ кадра → звук.
    pub fn run_frame_with<H: Host + ?Sized>(&mut self, host: &mut H) {
        host.poll_input(&mut self.sys.input);
        self.run_frame();
        host.present(&self.frame);
        let audio = self.take_audio();
        host.queue_audio(&audio);
    }

    /// Крутит кадры, пока хост не попросит выйти. Темп задаёт хост
    /// (например, блокируясь в `present` до вертикальной синхронизации).
    pub fn run<H: Host + ?Sized>(&mut self, host: &mut H) {
        while !host.should_quit() {
            self.run_frame_with(host);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ROM на 16 КБ: векторы ведут в бесконечный цикл `JMP $C000`.
    fn spin_rom() -> Vec<u8> {
        let mut rom = vec![0xEAu8; 0x4000];
        rom[0] = 0x4C; // JMP $C000
        rom[1] = 0x00;
        rom[2] = 0xC0;
        rom[0x3FFC] = 0x00; // RESET -> $C000
        rom[0x3FFD] = 0xC0;
        rom
    }

    #[test]
    fn runs_frames_and_produces_audio() {
        let mut emu = Emulator::new(Config::default());
        emu.load_cartridge(&spin_rom()).unwrap();
        emu.run_frame();
        emu.run_frame();
        assert_eq!(emu.frames_emulated(), 2);
        assert!(!emu.take_audio().is_empty());
        assert_eq!(emu.frame().pixels.len(), SCREEN_W * SCREEN_H * 4);
    }

    #[test]
    fn host_receives_frame_and_audio() {
        struct Probe {
            frames: u32,
            samples: usize,
        }
        impl Host for Probe {
            fn poll_input(&mut self, _: &mut InputState) {}
            fn present(&mut self, f: &FrameBuffer) {
                assert_eq!(f.width, SCREEN_W);
                self.frames += 1;
            }
            fn queue_audio(&mut self, s: &[f32]) {
                self.samples += s.len();
            }
            fn should_quit(&self) -> bool {
                self.frames >= 3
            }
        }
        let mut emu = Emulator::new(Config::default());
        emu.load_cartridge(&spin_rom()).unwrap();
        let mut host = Probe { frames: 0, samples: 0 };
        emu.run(&mut host);
        assert_eq!(host.frames, 3);
        assert!(host.samples > 0);
    }
}
