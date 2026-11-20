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
    /// Сколько раз встретился неофициальный опкод (он выполнен как NOP).
    pub unofficial_count: u32,
    /// Адрес и код последнего неофициального опкода.
    pub last_unofficial: Option<(u16, u8)>,
    /// Процессор "завис" на опкоде JAM/KIL: (адрес, опкод). Выйти можно только сбросом.
    pub jam: Option<(u16, u8)>,
    /// Последние 16 инструкций до первого неофициального опкода (адрес, опкод), старые первыми.
    /// Нужны, чтобы понять, как выполнение попало туда: ушло в данные, не тот банк и т. п.
    pub first_unofficial_trace: Option<Vec<(u16, u8)>>,
    /// То же перед зависанием на JAM.
    pub jam_trace: Option<Vec<(u16, u8)>>,
    history: [(u16, u8); 16],
    hist_pos: usize,
    op_pc: u16,
    op: u8,
}

impl Default for Cpu {
    fn default() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            sp: 0xFD,
            pc: 0,
            p: FI | FU,
            unofficial_count: 0,
            last_unofficial: None,
            jam: None,
            first_unofficial_trace: None,
            jam_trace: None,
            history: [(0, 0); 16],
            hist_pos: 0,
            op_pc: 0,
            op: 0,
        }
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
        self.jam = None;
        self.unofficial_count = 0;
        self.last_unofficial = None;
        self.first_unofficial_trace = None;
        self.jam_trace = None;
        self.hist_pos = 0;
        self.pc = read16(bus, 0xFFFC);
    }

    /// Немаскируемое прерывание (в 7800 его выдаёт MARIA по флагу DLI). Возвращает такты.
    pub fn nmi<T: Bus>(&mut self, bus: &mut T) -> u32 {
        if self.jam.is_some() {
            return 0;
        }
        self.push16(bus, self.pc);
        self.push(bus, (self.p & !FB) | FU);
        self.p |= FI;
        self.pc = read16(bus, 0xFFFA);
        7
    }

    /// Маскируемое прерывание. Возвращает 0, если прерывания запрещены.
    pub fn irq<T: Bus>(&mut self, bus: &mut T) -> u32 {
        if self.p & FI != 0 || self.jam.is_some() {
            return 0;
        }
        self.push16(bus, self.pc);
        self.push(bus, (self.p & !FB) | FU);
        self.p |= FI;
        self.pc = read16(bus, 0xFFFE);
        7
    }

    // ---- вспомогательные операции ----

    fn jam_cpu(&mut self) {
        self.jam = Some((self.op_pc, self.op));
        self.jam_trace = Some(self.recent_instructions());
    }

    /// Последние выполненные инструкции (адрес, опкод), старые первыми; не больше 16.
    pub fn recent_instructions(&self) -> Vec<(u16, u8)> {
        let n = self.hist_pos.min(16);
        (0..n).map(|i| self.history[(self.hist_pos - n + i) & 15]).collect()
    }

    /// Запись для SHA/SHX/SHY/TAS: значение И (старший байт базового адреса + 1);
    /// при переходе через страницу старший байт адреса подменяется значением.
    fn store_high_and<T: Bus>(&mut self, bus: &mut T, addr: u16, index: u8, val: u8) {
        let base = addr.wrapping_sub(index as u16);
        let v = val & ((base >> 8) as u8).wrapping_add(1);
        let target = if (base ^ addr) & 0xFF00 != 0 { ((v as u16) << 8) | (addr & 0x00FF) } else { addr };
        bus.write(target, v);
    }

    fn flag_unofficial(&mut self) {
        self.unofficial_count = self.unofficial_count.wrapping_add(1);
        self.last_unofficial = Some((self.op_pc, self.op));
        if self.first_unofficial_trace.is_none() {
            self.first_unofficial_trace = Some(self.recent_instructions());
        }
    }

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
        if self.jam.is_some() {
            return 2; // завис на JAM: время идёт, процессор стоит
        }
        self.op_pc = self.pc;
        let op = self.fetch(bus);
        self.op = op;
        self.history[self.hist_pos & 15] = (self.op_pc, op);
        self.hist_pos = self.hist_pos.wrapping_add(1);
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
                    _ => self.group3(bus, aaa, bbb),
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
                self.flag_unofficial();
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

    /// ASL ROL LSR ROR STX LDX DEC INC (+ JAM, NOP, SHX)
    fn group2<T: Bus>(&mut self, bus: &mut T, aaa: u8, bbb: u8) -> u32 {
        // JAM: 0x02 0x12 0x22 0x32 0x42 0x52 0x62 0x72 0x92 0xB2 0xD2 0xF2
        if bbb == 4 || (bbb == 0 && aaa < 4) {
            self.flag_unofficial();
            self.jam_cpu();
            return 2;
        }
        // 0x1A 0x3A 0x5A 0x7A 0xDA 0xFA — NOP (0x9A TXS и 0xBA TSX обработаны выше)
        if bbb == 6 {
            self.flag_unofficial();
            return 2;
        }
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
            _ => {
                // bbb == 7
                if aaa == 4 || aaa == 5 {
                    Mode::Aby
                } else {
                    Mode::Abx
                }
            }
        };
        match aaa {
            4 => match mode {
                Mode::Zp | Mode::Abs | Mode::Zpy => {
                    let (a, _) = self.addr(bus, mode);
                    bus.write(a, self.x);
                    cycles(mode, Kind::Write, false)
                }
                Mode::Aby => {
                    // 0x9E SHX abs,Y
                    self.flag_unofficial();
                    let (a, _) = self.addr(bus, mode);
                    let (y, x) = (self.y, self.x);
                    self.store_high_and(bus, a, y, x);
                    cycles(mode, Kind::Write, false)
                }
                _ => {
                    // 0x82 NOP #imm
                    self.flag_unofficial();
                    let _ = self.addr(bus, mode);
                    2
                }
            },
            5 => {
                let (a, crossed) = self.addr(bus, mode);
                let v = bus.read(a);
                self.x = v;
                self.set_nz(v);
                cycles(mode, Kind::Read, crossed)
            }
            _ => {
                if mode == Mode::Imm {
                    // 0xC2 0xE2 — NOP #imm
                    self.flag_unofficial();
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

    /// BIT STY LDY CPY CPX (+ SHY и NOP с операндами)
    fn group0<T: Bus>(&mut self, bus: &mut T, aaa: u8, bbb: u8) -> u32 {
        let mode = match bbb {
            0 => Mode::Imm,
            1 => Mode::Zp,
            3 => Mode::Abs,
            5 => Mode::Zpx,
            7 => Mode::Abx,
            _ => {
                // bbb 2/4/6 — реализованы выше (стек, ветвления, флаги); сюда не попадаем
                self.flag_unofficial();
                return 2;
            }
        };
        // 0x9C SHY abs,X
        if aaa == 4 && mode == Mode::Abx {
            self.flag_unofficial();
            let (a, _) = self.addr(bus, mode);
            let (x, y) = (self.x, self.y);
            self.store_high_and(bus, a, x, y);
            return cycles(mode, Kind::Write, false);
        }
        let valid = match aaa {
            1 => matches!(mode, Mode::Zp | Mode::Abs),
            4 => matches!(mode, Mode::Zp | Mode::Abs | Mode::Zpx),
            5 => true,
            6 | 7 => matches!(mode, Mode::Imm | Mode::Zp | Mode::Abs),
            _ => false,
        };
        if !valid {
            // Неофициальные NOP: 0x04 0x0C 0x14 0x1C 0x34 0x3C 0x44 0x54 0x5C 0x64 0x74 0x7C
            // 0x80 0xD4 0xDC 0xF4 0xFC. Операнд из памяти читается, результат отбрасывается.
            self.flag_unofficial();
            let (a, crossed) = self.addr(bus, mode);
            if mode != Mode::Imm {
                let _ = bus.read(a);
            }
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

    /// Все опкоды с cc = 11 — "составные" неофициальные инструкции:
    /// SLO RLA SRE RRA SAX LAX DCP ISC, а также ANC ALR ARR XAA LAX# AXS SBC#
    /// SHA TAS LAS.
    fn group3<T: Bus>(&mut self, bus: &mut T, aaa: u8, bbb: u8) -> u32 {
        self.flag_unofficial();

        // bbb == 2: операции с непосредственным операндом
        if bbb == 2 {
            let (a, _) = self.addr(bus, Mode::Imm);
            let v = bus.read(a);
            match aaa {
                0 | 1 => {
                    // ANC: A &= imm, C = N
                    self.a &= v;
                    self.set_nz(self.a);
                    self.set_flag(FC, self.a & 0x80 != 0);
                }
                2 => {
                    // ALR: A &= imm, затем LSR A
                    self.a &= v;
                    let r = self.lsr(self.a);
                    self.a = r;
                }
                3 => {
                    // ARR: A &= imm, затем ROR A (флаги C/V особые; десятичный режим не учтён)
                    self.a &= v;
                    let r = (self.a >> 1) | ((self.p & FC) << 7);
                    self.a = r;
                    self.set_nz(r);
                    self.set_flag(FC, r & 0x40 != 0);
                    self.set_flag(FV, ((r >> 6) ^ (r >> 5)) & 1 != 0);
                }
                4 => {
                    // XAA (ANE): нестабильная; берём типичную "магическую" константу 0xEE
                    self.a = (self.a | 0xEE) & self.x & v;
                    self.set_nz(self.a);
                }
                5 => {
                    // LAX #imm (0xAB): нестабильная; упрощённо A = X = imm
                    self.a = v;
                    self.x = v;
                    self.set_nz(v);
                }
                6 => {
                    // AXS (SBX): X = (A & X) - imm, флаги как у CMP
                    let t = self.a & self.x;
                    self.set_flag(FC, t >= v);
                    self.x = t.wrapping_sub(v);
                    self.set_nz(self.x);
                }
                _ => self.sbc(v), // 0xEB — то же, что SBC #imm
            }
            return 2;
        }

        let mode = match bbb {
            0 => Mode::Inx,
            1 => Mode::Zp,
            3 => Mode::Abs,
            4 => Mode::Iny,
            5 => {
                if aaa == 4 || aaa == 5 {
                    Mode::Zpy
                } else {
                    Mode::Zpx
                }
            }
            6 => Mode::Aby,
            _ => {
                // bbb == 7
                if aaa == 4 || aaa == 5 {
                    Mode::Aby
                } else {
                    Mode::Abx
                }
            }
        };

        match aaa {
            4 => {
                // SAX (0x83 0x87 0x8F 0x97), SHA (0x93 0x9F), TAS (0x9B)
                let (a, _) = self.addr(bus, mode);
                match bbb {
                    0 | 1 | 3 | 5 => bus.write(a, self.a & self.x),
                    6 => {
                        self.sp = self.a & self.x;
                        let (y, sp) = (self.y, self.sp);
                        self.store_high_and(bus, a, y, sp);
                    }
                    _ => {
                        let (y, v) = (self.y, self.a & self.x);
                        self.store_high_and(bus, a, y, v);
                    }
                }
                cycles(mode, Kind::Write, false)
            }
            5 => {
                // LAX (A = X = M) и LAS (0xBB: A = X = SP = M & SP)
                let (a, crossed) = self.addr(bus, mode);
                let v = bus.read(a);
                if bbb == 6 {
                    let r = v & self.sp;
                    self.a = r;
                    self.x = r;
                    self.sp = r;
                    self.set_nz(r);
                } else {
                    self.a = v;
                    self.x = v;
                    self.set_nz(v);
                }
                cycles(mode, Kind::Read, crossed)
            }
            _ => {
                // SLO RLA SRE RRA DCP ISC: чтение-изменение-запись + операция над A
                let (a, _) = self.addr(bus, mode);
                let m = bus.read(a);
                let r = match aaa {
                    0 => {
                        let r = self.asl(m);
                        self.a |= r;
                        self.set_nz(self.a);
                        r
                    }
                    1 => {
                        let r = self.rol(m);
                        self.a &= r;
                        self.set_nz(self.a);
                        r
                    }
                    2 => {
                        let r = self.lsr(m);
                        self.a ^= r;
                        self.set_nz(self.a);
                        r
                    }
                    3 => {
                        let r = self.ror(m);
                        self.adc(r);
                        r
                    }
                    6 => {
                        let r = m.wrapping_sub(1);
                        self.cmp(self.a, r);
                        r
                    }
                    _ => {
                        let r = m.wrapping_add(1);
                        self.sbc(r);
                        r
                    }
                };
                bus.write(a, r);
                cycles(mode, Kind::Rmw, false)
            }
        }
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
    fn every_opcode_executes_without_panic() {
        for op in 0..=255u8 {
            let mut ram = Ram(vec![0; 0x10000]);
            ram.0[0x0200] = op;
            let mut cpu = Cpu::new();
            cpu.pc = 0x0200;
            let c = cpu.step(&mut ram);
            assert!((2..=8).contains(&c), "опкод {op:02X}: {c} тактов");
        }
    }

    #[test]
    fn lax_loads_a_and_x() {
        // LDA #$80 ; STA $10 ; LDA #$00 ; LAX $10
        let (cpu, _) = run(&[0xA9, 0x80, 0x85, 0x10, 0xA9, 0x00, 0xA7, 0x10], 4);
        assert_eq!((cpu.a, cpu.x), (0x80, 0x80));
        assert_ne!(cpu.p & FN, 0);
    }

    #[test]
    fn sax_stores_a_and_x() {
        // LDA #$F0 ; LDX #$3C ; SAX $10
        let (_, ram) = run(&[0xA9, 0xF0, 0xA2, 0x3C, 0x87, 0x10], 3);
        assert_eq!(ram.0[0x10], 0x30);
    }

    #[test]
    fn dcp_decrements_then_compares() {
        // LDA #$05 ; STA $10 ; LDA #$04 ; DCP $10
        let (cpu, ram) = run(&[0xA9, 0x05, 0x85, 0x10, 0xA9, 0x04, 0xC7, 0x10], 4);
        assert_eq!(ram.0[0x10], 0x04);
        assert_ne!(cpu.p & FZ, 0);
        assert_ne!(cpu.p & FC, 0);
    }

    #[test]
    fn isc_increments_then_subtracts() {
        // SEC ; LDA #$10 ; STA $10 ; LDA #$20 ; ISC $10
        let (cpu, ram) = run(&[0x38, 0xA9, 0x10, 0x85, 0x10, 0xA9, 0x20, 0xE7, 0x10], 5);
        assert_eq!(ram.0[0x10], 0x11);
        assert_eq!(cpu.a, 0x0F);
    }

    #[test]
    fn slo_and_rra() {
        // LDA #$81 ; STA $10 ; LDA #$01 ; SLO $10
        let (cpu, ram) = run(&[0xA9, 0x81, 0x85, 0x10, 0xA9, 0x01, 0x07, 0x10], 4);
        assert_eq!(ram.0[0x10], 0x02);
        assert_eq!(cpu.a, 0x03);
        assert_ne!(cpu.p & FC, 0);

        // CLC ; LDA #$02 ; STA $10 ; LDA #$10 ; RRA $10
        let (cpu, ram) = run(&[0x18, 0xA9, 0x02, 0x85, 0x10, 0xA9, 0x10, 0x67, 0x10], 5);
        assert_eq!(ram.0[0x10], 0x01);
        assert_eq!(cpu.a, 0x11);
    }

    #[test]
    fn immediate_unofficial_ops() {
        // LDA #$FF ; ANC #$80
        let (cpu, _) = run(&[0xA9, 0xFF, 0x0B, 0x80], 2);
        assert_eq!(cpu.a, 0x80);
        assert_ne!(cpu.p & FC, 0);

        // LDA #$FF ; LDX #$0F ; AXS #$05
        let (cpu, _) = run(&[0xA9, 0xFF, 0xA2, 0x0F, 0xCB, 0x05], 3);
        assert_eq!(cpu.x, 0x0A);
        assert_ne!(cpu.p & FC, 0);

        // SEC ; LDA #$10 ; SBC #$01 (опкод 0xEB)
        let (cpu, _) = run(&[0x38, 0xA9, 0x10, 0xEB, 0x01], 3);
        assert_eq!(cpu.a, 0x0F);
    }

    #[test]
    fn unofficial_nops_have_right_length() {
        // NOP ; NOP zp ; NOP abs ; NOP #imm ; NOP zp,X ; NOP abs,X ; LDA #$7F
        let prog = [0x1A, 0x04, 0x00, 0x0C, 0x00, 0x00, 0x80, 0x00, 0x14, 0x00, 0x1C, 0x00, 0x00, 0xA9, 0x7F];
        let (cpu, _) = run(&prog, 7);
        assert_eq!(cpu.a, 0x7F);
        assert_eq!(cpu.pc, 0x0200 + prog.len() as u16);
    }

    #[test]
    fn trace_before_first_unofficial_opcode() {
        // LDA #1 ; TAX ; NOP-неофициальный (0x1A)
        let (cpu, _) = run(&[0xA9, 0x01, 0xAA, 0x1A], 3);
        let t = cpu.first_unofficial_trace.clone().unwrap();
        assert_eq!(t, vec![(0x0200, 0xA9), (0x0202, 0xAA), (0x0203, 0x1A)]);
        assert_eq!(cpu.unofficial_count, 1);
    }

    #[test]
    fn jam_halts_cpu() {
        let (mut cpu, mut ram) = run(&[0x02, 0xEA], 1);
        assert_eq!(cpu.jam, Some((0x0200, 0x02)));
        let pc = cpu.pc;
        cpu.step(&mut ram);
        assert_eq!(cpu.pc, pc);
        cpu.reset(&mut ram);
        assert_eq!(cpu.jam, None);
    }

    #[test]
    fn binary_sbc_sets_carry() {
        // SEC ; LDA #$10 ; SBC #$01
        let (cpu, _) = run(&[0x38, 0xA9, 0x10, 0xE9, 0x01], 3);
        assert_eq!(cpu.a, 0x0F);
        assert_ne!(cpu.p & FC, 0);
    }
}
