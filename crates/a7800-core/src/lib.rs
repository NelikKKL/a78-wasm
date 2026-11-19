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
pub mod pokey;
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

/// Снимок внутреннего состояния — для панели отладки и поиска причин, почему игра не идёт.
#[derive(Clone, Debug)]
pub struct DebugInfo {
    pub frames: u64,
    pub nmi_count: u64,
    pub pc: u16,
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub p: u8,
    pub maria_enabled: bool,
    pub ctrl: u8,
    pub dpp: u16,
    pub charbase: u8,
    pub background: u8,
    pub inptctrl: u8,
    pub bios_enabled: bool,
    /// Сколько раз выполнен неофициальный опкод и какой был последним: (адрес, опкод).
    pub unofficial_ops: u32,
    pub last_unofficial: Option<(u16, u8)>,
    /// Процессор завис на опкоде JAM: (адрес, опкод).
    pub jam: Option<(u16, u8)>,
    /// Сколько байтов графики MARIA пропустила (режим не реализован) и какой режим: (RM, WM).
    pub unsupported_gfx_bytes: u32,
    pub last_unsupported_mode: Option<(u8, bool)>,
}

pub struct Emulator {
    cpu: Cpu,
    sys: System,
    frame: FrameBuffer,
    palette: Palette,
    carry: i32,
    frames: u64,
    nmi_count: u64,
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
            nmi_count: 0,
        }
    }

    /// Загружает картридж (.a78 или "сырой" образ) и перезапускает консоль.
    pub fn load_cartridge(&mut self, data: &[u8]) -> Result<CartInfo, LoadError> {
        let cart = Cartridge::from_bytes(data)?;
        let info = CartInfo { title: cart.title.clone(), mapper: cart.mapper, rom_size: cart.rom_len() };
        let pokey_at = cart.pokey_at;
        self.sys.mem.cart = Some(cart);
        self.sys.pokey_base = pokey_at;
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

    pub fn debug_info(&self) -> DebugInfo {
        let m = &self.sys.maria;
        DebugInfo {
            frames: self.frames,
            nmi_count: self.nmi_count,
            pc: self.cpu.pc,
            a: self.cpu.a,
            x: self.cpu.x,
            y: self.cpu.y,
            sp: self.cpu.sp,
            p: self.cpu.p,
            maria_enabled: m.enabled,
            ctrl: m.reg(0x3C),
            dpp: ((m.reg(0x2C) as u16) << 8) | m.reg(0x30) as u16,
            charbase: m.reg(0x34),
            background: m.reg(0x20),
            inptctrl: self.sys.inptctrl(),
            bios_enabled: self.sys.mem.bios_enabled,
            unofficial_ops: self.cpu.unofficial_count,
            last_unofficial: self.cpu.last_unofficial,
            jam: self.cpu.jam,
            unsupported_gfx_bytes: m.unsupported_bytes,
            last_unsupported_mode: m.last_unsupported,
        }
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
        self.sys.take_audio()
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

            self.sys.advance(dma as u32);
            if nmi {
                self.nmi_count += 1;
                let c = self.cpu.nmi(&mut self.sys);
                self.sys.advance(c);
                carry -= c as i32;
            }

            // Процессор получает остаток строки; WSYNC отдаёт её целиком.
            self.sys.wsync = false;
            let mut budget = CYCLES_PER_LINE - dma + carry;
            while budget > 0 {
                let c = self.cpu.step(&mut self.sys) as i32;
                self.sys.advance(c as u32);
                budget -= c;
                if self.sys.wsync {
                    break;
                }
            }
            if self.sys.wsync {
                if budget > 0 {
                    self.sys.advance(budget as u32);
                }
                carry = 0;
            } else {
                carry = budget.max(-CYCLES_PER_LINE);
            }
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

    /// Крошечный ROM, который включает MARIA и рисует зелёную полосу:
    /// 14 зон по 16 строк, у первой флаг DLI (его обработчик увеличивает $1800).
    fn tile_rom() -> Vec<u8> {
        fn put(rom: &mut [u8], addr: usize, bytes: &[u8]) {
            rom[addr - 0xC000..addr - 0xC000 + bytes.len()].copy_from_slice(bytes);
        }
        let (dll, dl, gfx, nmi) = (0xE000usize, 0xE100usize, 0xD000usize, 0xC100usize);
        let mut rom = vec![0xEAu8; 0x4000];

        let code = [
            0x78, 0xD8, 0xA2, 0xFF, 0x9A, // SEI CLD LDX #$FF TXS
            0xA9, 0x07, 0x85, 0x01, // INPTCTRL = 7
            0xA9, 0x92, 0x85, 0x20, // фон
            0xA9, 0xCA, 0x85, 0x23, // P0C3
            0xA9, (dll >> 8) as u8, 0x85, 0x2C, // DPPH
            0xA9, (dll & 0xFF) as u8, 0x85, 0x30, // DPPL
            0xA9, 0x40, 0x85, 0x3C, // CTRL: DMA включён
        ];
        put(&mut rom, 0xC000, &code);
        let spin = 0xC000 + code.len();
        put(&mut rom, spin, &[0x4C, (spin & 0xFF) as u8, (spin >> 8) as u8]);
        put(&mut rom, nmi, &[0xEE, 0x00, 0x18, 0x40]); // INC $1800 ; RTI

        for z in 0..15 {
            let flags = if z == 0 { 0x8F } else { 0x0F };
            // последняя зона указывает на пустой список отображения
            let list = if z == 14 { dl + 0x40 } else { dl };
            put(&mut rom, dll + z * 3, &[flags, (list >> 8) as u8, (list & 0xFF) as u8]);
        }
        put(&mut rom, dll + 15 * 3, &[0, 0, 0]); // следующая запись без DLI
        put(&mut rom, dl + 0x40, &[0, 0]); // пустой список
        // 4-байтовый заголовок: палитра 0, 4 байта графики, x = 20; затем конец списка
        put(&mut rom, dl, &[0x00, 0x1C, (gfx >> 8) as u8, 20, 0x00, 0x00]);
        for r in 0..16 {
            put(&mut rom, gfx + r * 256, &[0xFF; 4]); // все точки — цвет 3
        }
        put(
            &mut rom,
            0xFFFA,
            &[(nmi & 0xFF) as u8, (nmi >> 8) as u8, 0x00, 0xC0, (nmi & 0xFF) as u8, (nmi >> 8) as u8],
        );
        rom
    }

    fn px(emu: &Emulator, x: usize, y: usize) -> [u8; 3] {
        let i = (y * SCREEN_W + x) * 4;
        let p = &emu.frame().pixels;
        [p[i], p[i + 1], p[i + 2]]
    }

    #[test]
    fn maria_draws_display_list_and_fires_dli() {
        let pal = palette::ntsc();
        let mut emu = Emulator::new(Config::default());
        emu.load_cartridge(&tile_rom()).unwrap();
        for _ in 0..3 {
            emu.run_frame();
        }

        // фон и полоса (x = 20 * 2 .. 20 * 2 + 32), с первой по последнюю строку
        assert_eq!(px(&emu, 10, 5), pal[0x92]);
        assert_eq!(px(&emu, 50, 0), pal[0xCA]);
        assert_eq!(px(&emu, 71, 223), pal[0xCA]);
        assert_eq!(px(&emu, 72, 100), pal[0x92]);
        assert_eq!(px(&emu, 50, 239), pal[0x92]); // пустая последняя зона — только фон

        // по одному DLI на кадр
        let d = emu.debug_info();
        assert_eq!(d.nmi_count, 3);
        assert_eq!(emu.sys.mem.peek(0x1800), 3);
        assert_eq!(d.unofficial_ops, 0);
        assert_eq!(d.unsupported_gfx_bytes, 0);
        assert!(d.maria_enabled);
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
