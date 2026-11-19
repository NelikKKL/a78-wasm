//! Процессор 6502 (в 7800 это "Sally", 6502C).
//!
//! Реализованы все официальные опкоды и десятичный режим (BCD).
//! Неофициальные опкоды пока выполняются как NOP.

/// Шина, через которую процессор читает и пишет память.
pub trait Bus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, val: u8);
}

const FC: u8 = 0x01;
const FZ: u8 = 0x02;
const FI: u8 = 0x04;
const FD: u8 = 0x08;
const FB: u8 = 0x10;
const FU: u8 = 0x20;
const FV: u8 = 0x40;
const FN: u8 = 0x80;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Imm,
    Zp,
    Zpx,
    Zpy,
    Abs,
    Abx,
    Aby,
    Inx,
    Iny,
    Acc,
}

#[derive(Clone, Copy)]
enum Kind {
    Read,
    Write,
    Rmw,
}

fn cycles(mode: Mode, kind: Kind, crossed: bool) -> u32 {
    use Mode::*;
    match (mode, kind) {
        (Imm, _) | (Acc, _) => 2,
        (Zp, Kind::Rmw) => 5,
        (Zp, _) => 3,
        (Zpx, Kind::Rmw) | (Zpy, Kind::Rmw) => 6,
        (Zpx, _) | (Zpy, _) => 4,
        (Abs, Kind::Rmw) => 6,
        (Abs, _) => 4,
        (Abx, Kind::Rmw) | (Aby, Kind::Rmw) => 7,
        (Abx, Kind::Write) | (Aby, Kind::Write) => 5,
        (Abx, Kind::Read) | (Aby, Kind::Read) => 4 + crossed as u32,
        (Inx, Kind::Rmw) => 8,
        (Inx, _) => 6,
        (Iny, Kind::Write) => 6,
        (Iny, Kind::Rmw) => 8,
        (Iny, Kind::Read) => 5 + crossed as u32,
    }
}

fn read16<T: Bus>(bus: &mut T, addr: u16) -> u16 {
    let lo = bus.read(addr) as u16;
    let hi = bus.read(addr.wrapping_add(1)) as u16;
    lo | (hi << 8)
}

#[derive(Clone, Debug)]
pub struct Cpu {
    pub a: u8,
    pub x: u8,
    pub y: u8,
    pub sp: u8,
    pub pc: u16,
    pub p: u8,
}

impl Default for Cpu {
    fn default() -> Self {
        Self { a: 0, x: 0, y: 0, sp: 0xFD, pc: 0, p: FI | FU }
    }
}

impl Cpu {
    pub fn new() -> Self {
        Self::default()
    }

    /// Сброс: загружает PC из вектора $FFFC.
    pub fn reset<T: Bus>(&mut self, bus: &mut T) {
        self.sp = 0xFD;
        self.p = FI | FU;
        self.pc = read16(bus, 0xFFFC);
    }

    /// Немаскируемое прерывание (в 7800 его выдаёт MARIA по флагу DLI). Возвращает такты.
    pub fn nmi<T: Bus>(&mut self, bus: &mut T) -> u32 {
        self.push16(bus, self.pc);
        self.push(bus, (self.p & !FB) | FU);
        self.p |= FI;
        self.pc = read16(bus, 0xFFFA);
        7
    }

    /// Маскируемое прерывание. Возвращает 0, если прерывания запрещены.
    pub fn irq<T: Bus>(&mut self, bus: &mut T) -> u32 {
        if self.p & FI != 0 {
            return 0;
        }
        self.push16(bus, self.pc);
        self.push(bus, (self.p & !FB) | FU);
        self.p |= FI;
        self.pc = read16(bus, 0xFFFE);
        7
    }

    // ---- вспомогательные операции ----

    fn fetch<T: Bus>(&mut self, bus: &mut T) -> u8 {
        let v = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    fn fetch16<T: Bus>(&mut self, bus: &mut T) -> u16 {
        let lo = self.fetch(bus) as u16;
        let hi = self.fetch(bus) as u16;
        lo | (hi << 8)
    }

    fn push<T: Bus>(&mut self, bus: &mut T, v: u8) {
        bus.write(0x0100 | self.sp as u16, v);
        self.sp = self.sp.wrapping_sub(1);
    }

    fn push16<T: Bus>(&mut self, bus: &mut T, v: u16) {
        self.push(bus, (v >> 8) as u8);
        self.push(bus, v as u8);
    }

    fn pop<T: Bus>(&mut self, bus: &mut T) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        bus.read(0x0100 | self.sp as u16)
    }

    fn pop16<T: Bus>(&mut self, bus: &mut T) -> u16 {
        let lo = self.pop(bus) as u16;
        let hi = self.pop(bus) as u16;
        lo | (hi << 8)
    }

    fn set_flag(&mut self, f: u8, on: bool) {
        if on {
            self.p |= f;
        } else {
            self.p &= !f;
        }
    }

    fn set_nz(&mut self, v: u8) {
        self.set_flag(FZ, v == 0);
        self.set_flag(FN, v & 0x80 != 0);
    }

    /// Вычисляет эффективный адрес; второй элемент — был ли переход через страницу.
    fn addr<T: Bus>(&mut self, bus: &mut T, mode: Mode) -> (u16, bool) {
        match mode {
            Mode::Imm => {
                let a = self.pc;
                self.pc = self.pc.wrapping_add(1);
                (a, false)
            }
            Mode::Zp => (self.fetch(bus) as u16, false),
            Mode::Zpx => (self.fetch(bus).wrapping_add(self.x) as u16, false),
            Mode::Zpy => (self.fetch(bus).wrapping_add(self.y) as u16, false),
            Mode::Abs => (self.fetch16(bus), false),
            Mode::Abx => {
                let base = self.fetch16(bus);
                let a = base.wrapping_add(self.x as u16);
                (a, (base ^ a) & 0xFF00 != 0)
            }
            Mode::Aby => {
                let base = self.fetch16(bus);
                let a = base.wrapping_add(self.y as u16);
                (a, (base ^ a) & 0xFF00 != 0)
            }
            Mode::Inx => {
                let zp = self.fetch(bus).wrapping_add(self.x);
                let lo = bus.read(zp as u16) as u16;
                let hi = bus.read(zp.wrapping_add(1) as u16) as u16;
                (lo | (hi << 8), false)
            }
            Mode::Iny => {
                let zp = self.fetch(bus);
                let lo = bus.read(zp as u16) as u16;
                let hi = bus.read(zp.wrapping_add(1) as u16) as u16;
                let base = lo | (hi << 8);
                let a = base.wrapping_add(self.y as u16);
                (a, (base ^ a) & 0xFF00 != 0)
            }
            Mode::Acc => (0, false),
        }
    }

    fn asl(&mut self, v: u8) -> u8 {
        self.set_flag(FC, v & 0x80 != 0);
        let r = v << 1;
        self.set_nz(r);
        r
    }

    fn lsr(&mut self, v: u8) -> u8 {
        self.set_flag(FC, v & 1 != 0);
        let r = v >> 1;
        self.set_nz(r);
        r
    }

    fn rol(&mut self, v: u8) -> u8 {
        let c = self.p & FC;
        self.set_flag(FC, v & 0x80 != 0);
        let r = (v << 1) | c;
        self.set_nz(r);
        r
    }

    fn ror(&mut self, v: u8) -> u8 {
        let c = (self.p & FC) << 7;
        self.set_flag(FC, v & 1 != 0);
        let r = (v >> 1) | c;
        self.set_nz(r);
        r
    }

    fn cmp(&mut self, r: u8, v: u8) {
        self.set_flag(FC, r >= v);
        self.set_nz(r.wrapping_sub(v));
    }

    fn adc(&mut self, v: u8) {
        if self.p & FD != 0 {
            self.adc_dec(v);
        } else {
            self.adc_bin(v);
        }
    }

    fn sbc(&mut self, v: u8) {
        if self.p & FD != 0 {
            self.sbc_dec(v);
        } else {
            self.adc_bin(!v);
        }
    }

    fn adc_bin(&mut self, v: u8) {
        let c = (self.p & FC) as u16;
        let sum = self.a as u16 + v as u16 + c;
        let r = sum as u8;
        self.set_flag(FC, sum > 0xFF);
        self.set_flag(FV, (!(self.a ^ v) & (self.a ^ r) & 0x80) != 0);
        self.a = r;
        self.set_nz(r);
    }

    fn adc_dec(&mut self, v: u8) {
        let a = self.a as u16;
        let m = v as u16;
        let c = (self.p & FC) as u16;
        let bin = (a + m + c) as u8;
        let mut lo = (a & 0x0F) + (m & 0x0F) + c;
        if lo >= 0x0A {
            lo = ((lo + 0x06) & 0x0F) + 0x10;
        }
        let mut sum = (a & 0xF0) + (m & 0xF0) + lo;
        self.set_flag(FN, sum & 0x80 != 0);
        self.set_flag(FV, (!(a ^ m) & (a ^ sum) & 0x80) != 0);
        self.set_flag(FZ, bin == 0);
        if sum >= 0xA0 {
            sum += 0x60;
        }
        self.set_flag(FC, sum >= 0x100);
        self.a = sum as u8;
    }

    fn sbc_dec(&mut self, v: u8) {
        let a = self.a as i16;
        let m = v as i16;
        let c = (self.p & FC) as i16;
        let bin = a - m - (1 - c);
        let mut lo = (a & 0x0F) - (m & 0x0F) + c - 1;
        if lo < 0 {
            lo = ((lo - 6) & 0x0F) - 0x10;
        }
        let mut sum = (a & 0xF0) - (m & 0xF0) + lo;
        if sum < 0 {
            sum -= 0x60;
        }
        let r = bin as u8;
        self.set_flag(FC, bin >= 0);
        self.set_flag(FV, ((self.a ^ v) & (self.a ^ r) & 0x80) != 0);
        self.set_nz(r);
        self.a = sum as u8;
    }

    // ---- выполнение одной инструкции ----

    /// Выполняет одну инструкцию и возвращает число потраченных тактов.
    pub fn step<T: Bus>(&mut self, bus: &mut T) -> u32 {
        let op = self.fetch(bus);
        match op {
            0x00 => {
                // BRK
                self.pc = self.pc.wrapping_add(1);
                self.push16(bus, self.pc);
                self.push(bus, self.p | FB | FU);
                self.p |= FI;
                self.pc = read16(bus, 0xFFFE);
                7
            }
            0x20 => {
                // JSR
                let target = self.fetch16(bus);
                self.push16(bus, self.pc.wrapping_sub(1));
                self.pc = target;
                6
            }
            0x40 => {
                // RTI
                self.p = (self.pop(bus) & !FB) | FU;
                self.pc = self.pop16(bus);
                6
            }
            0x60 => {
                // RTS
                self.pc = self.pop16(bus).wrapping_add(1);
                6
            }
            0x4C => {
                self.pc = self.fetch16(bus);
                3
            }
            0x6C => {
                // JMP (ind) с известной ошибкой NMOS на границе страницы
                let ptr = self.fetch16(bus);
                let lo = bus.read(ptr) as u16;
                let hi = bus.read((ptr & 0xFF00) | (ptr.wrapping_add(1) & 0x00FF)) as u16;
                self.pc = lo | (hi << 8);
                5
            }
            0x08 => {
                self.push(bus, self.p | FB | FU);
                3
            }
            0x28 => {
                self.p = (self.pop(bus) & !FB) | FU;
                4
            }
            0x48 => {
                self.push(bus, self.a);
                3
            }
            0x68 => {
                let v = self.pop(bus);
                self.a = v;
                self.set_nz(v);
                4
            }
            0x88 => {
                self.y = self.y.wrapping_sub(1);
                self.set_nz(self.y);
                2
            }
            0xC8 => {
                self.y = self.y.wrapping_add(1);
                self.set_nz(self.y);
                2
            }
            0xCA => {
                self.x = self.x.wrapping_sub(1);
                self.set_nz(self.x);
                2
            }
            0xE8 => {
                self.x = self.x.wrapping_add(1);
                self.set_nz(self.x);
                2
            }
            0xA8 => {
                self.y = self.a;
                self.set_nz(self.y);
                2
            }
            0xAA => {
                self.x = self.a;
                self.set_nz(self.x);
                2
            }
            0x98 => {
                self.a = self.y;
                self.set_nz(self.a);
                2
            }
            0x8A => {
                self.a = self.x;
                self.set_nz(self.a);
                2
            }
            0xBA => {
                self.x = self.sp;
                self.set_nz(self.x);
                2
            }
            0x9A => {
                self.sp = self.x;
                2
            }
            0x18 => {
                self.p &= !FC;
                2
            }
            0x38 => {
                self.p |= FC;
                2
            }
            0x58 => {
                self.p &= !FI;
                2
            }
            0x78 => {
                self.p |= FI;
                2
            }
            0xB8 => {
                self.p &= !FV;
                2
            }
            0xD8 => {
                self.p &= !FD;
                2
            }
            0xF8 => {
                self.p |= FD;
                2
            }
            0xEA => 2,
            op if op & 0x1F == 0x10 => {
                // Ветвления: биты 7-6 выбирают флаг, бит 5 — ожидаемое значение
                let off = self.fetch(bus) as i8;
                let flag = match op >> 6 {
                    0 => FN,
                    1 => FV,
                    2 => FC,
                    _ => FZ,
                };
                let want = (op >> 5) & 1 == 1;
                let mut cyc = 2;
                if (self.p & flag != 0) == want {
                    let old = self.pc;
                    self.pc = old.wrapping_add(off as i16 as u16);
                    cyc += 1;
                    if (old ^ self.pc) & 0xFF00 != 0 {
                        cyc += 1;
                    }
                }
                cyc
            }
            _ => {
                let aaa = op >> 5;
                let bbb = (op >> 2) & 7;
                match op & 3 {
                    1 => self.group1(bus, aaa, bbb),
                    2 => self.group2(bus, aaa, bbb),
                    0 => self.group0(bus, aaa, bbb),
                    _ => 2, // неофициальные опкоды cc=11 — пока NOP
                }
            }
        }
    }

    /// ORA AND EOR ADC STA LDA CMP SBC
    fn group1<T: Bus>(&mut self, bus: &mut T, aaa: u8, bbb: u8) -> u32 {
        const MODES: [Mode; 8] = [
            Mode::Inx,
            Mode::Zp,
            Mode::Imm,
            Mode::Abs,
            Mode::Iny,
            Mode::Zpx,
            Mode::Aby,
            Mode::Abx,
        ];
        let mode = MODES[bbb as usize];
        if aaa == 4 {
            if mode == Mode::Imm {
                // 0x89: неофициальный NOP #imm
                self.pc = self.pc.wrapping_add(1);
                return 2;
            }
            let (a, _) = self.addr(bus, mode);
            bus.write(a, self.a);
            return cycles(mode, Kind::Write, false);
        }
        let (a, crossed) = self.addr(bus, mode);
        let v = bus.read(a);
        match aaa {
            0 => {
                self.a |= v;
                self.set_nz(self.a);
            }
            1 => {
                self.a &= v;
                self.set_nz(self.a);
            }
            2 => {
                self.a ^= v;
                self.set_nz(self.a);
            }
            3 => self.adc(v),
            5 => {
                self.a = v;
                self.set_nz(v);
            }
            6 => self.cmp(self.a, v),
            _ => self.sbc(v),
        }
        cycles(mode, Kind::Read, crossed)
    }

    /// ASL ROL LSR ROR STX LDX DEC INC
    fn group2<T: Bus>(&mut self, bus: &mut T, aaa: u8, bbb: u8) -> u32 {
        let mode = match bbb {
            0 => Mode::Imm,
            1 => Mode::Zp,
            2 => Mode::Acc,
            3 => Mode::Abs,
            5 => {
                if aaa == 4 || aaa == 5 {
                    Mode::Zpy
                } else {
                    Mode::Zpx
                }
            }
            7 => {
                if aaa == 5 {
                    Mode::Aby
                } else {
                    Mode::Abx
                }
            }
            _ => return 2, // JAM и прочее неофициальное
        };
        match aaa {
            4 => {
                if !matches!(mode, Mode::Zp | Mode::Abs | Mode::Zpy) {
                    let _ = self.addr(bus, mode);
                    return 2;
                }
                let (a, _) = self.addr(bus, mode);
                bus.write(a, self.x);
                cycles(mode, Kind::Write, false)
            }
            5 => {
                let (a, crossed) = self.addr(bus, mode);
                let v = bus.read(a);
                self.x = v;
                self.set_nz(v);
                cycles(mode, Kind::Read, crossed)
            }
            _ => {
                if mode == Mode::Imm || (mode == Mode::Acc && aaa >= 6) {
                    let _ = self.addr(bus, mode);
                    return 2;
                }
                let (a, _) = self.addr(bus, mode);
                let v = if mode == Mode::Acc { self.a } else { bus.read(a) };
                let r = match aaa {
                    0 => self.asl(v),
                    1 => self.rol(v),
                    2 => self.lsr(v),
                    3 => self.ror(v),
                    6 => {
                        let r = v.wrapping_sub(1);
                        self.set_nz(r);
                        r
                    }
                    _ => {
                        let r = v.wrapping_add(1);
                        self.set_nz(r);
                        r
                    }
                };
                if mode == Mode::Acc {
                    self.a = r;
                } else {
                    bus.write(a, r);
                }
                cycles(mode, Kind::Rmw, false)
            }
        }
    }

    /// BIT STY LDY CPY CPX (+ неофициальные NOP с операндами)
    fn group0<T: Bus>(&mut self, bus: &mut T, aaa: u8, bbb: u8) -> u32 {
        let mode = match bbb {
            0 => Mode::Imm,
            1 => Mode::Zp,
            3 => Mode::Abs,
            5 => Mode::Zpx,
            7 => Mode::Abx,
            _ => return 2,
        };
        let valid = match aaa {
            1 => matches!(mode, Mode::Zp | Mode::Abs),
            4 => matches!(mode, Mode::Zp | Mode::Abs | Mode::Zpx),
            5 => true,
            6 | 7 => matches!(mode, Mode::Imm | Mode::Zp | Mode::Abs),
            _ => false,
        };
        if !valid {
            let (_, crossed) = self.addr(bus, mode);
            return cycles(mode, Kind::Read, crossed);
        }
        if aaa == 4 {
            let (a, _) = self.addr(bus, mode);
            bus.write(a, self.y);
            return cycles(mode, Kind::Write, false);
        }
        let (a, crossed) = self.addr(bus, mode);
        let v = bus.read(a);
        match aaa {
            1 => {
                self.set_flag(FZ, self.a & v == 0);
                self.set_flag(FN, v & 0x80 != 0);
                self.set_flag(FV, v & 0x40 != 0);
            }
            5 => {
                self.y = v;
                self.set_nz(v);
            }
            6 => self.cmp(self.y, v),
            _ => self.cmp(self.x, v),
        }
        cycles(mode, Kind::Read, crossed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ram(Vec<u8>);

    impl Bus for Ram {
        fn read(&mut self, a: u16) -> u8 {
            self.0[a as usize]
        }
        fn write(&mut self, a: u16, v: u8) {
            self.0[a as usize] = v;
        }
    }

    fn run(prog: &[u8], steps: usize) -> (Cpu, Ram) {
        let mut ram = Ram(vec![0; 0x10000]);
        ram.0[0x0200..0x0200 + prog.len()].copy_from_slice(prog);
        let mut cpu = Cpu::new();
        cpu.pc = 0x0200;
        for _ in 0..steps {
            cpu.step(&mut ram);
        }
        (cpu, ram)
    }

    #[test]
    fn load_transfer_store() {
        // LDA #$42 ; TAX ; INX ; STA $10
        let (cpu, ram) = run(&[0xA9, 0x42, 0xAA, 0xE8, 0x85, 0x10], 4);
        assert_eq!(cpu.x, 0x43);
        assert_eq!(ram.0[0x10], 0x42);
    }

    #[test]
    fn decimal_adc() {
        // SED ; CLC ; LDA #$19 ; ADC #$28
        let (cpu, _) = run(&[0xF8, 0x18, 0xA9, 0x19, 0x69, 0x28], 4);
        assert_eq!(cpu.a, 0x47);
    }

    #[test]
    fn branch_loop() {
        // LDX #3 ; loop: DEX ; BNE loop
        let (cpu, _) = run(&[0xA2, 0x03, 0xCA, 0xD0, 0xFD], 1 + 3 * 2);
        assert_eq!(cpu.x, 0);
        assert_eq!(cpu.pc, 0x0205);
    }

    #[test]
    fn jsr_rts() {
        let mut prog = vec![0u8; 0x20];
        prog[0] = 0x20; // JSR $0210
        prog[1] = 0x10;
        prog[2] = 0x02;
        prog[3] = 0xEA;
        prog[0x10] = 0x60; // RTS
        let (cpu, _) = run(&prog, 2);
        assert_eq!(cpu.pc, 0x0203);
    }

    #[test]
    fn binary_sbc_sets_carry() {
        // SEC ; LDA #$10 ; SBC #$01
        let (cpu, _) = run(&[0x38, 0xA9, 0x10, 0xE9, 0x01], 3);
        assert_eq!(cpu.a, 0x0F);
        assert_ne!(cpu.p & FC, 0);
    }
}
