//! Функциональный тест 6502 Клауса Дормана (все официальные опкоды + десятичный режим).
//!
//! Файл теста в репозиторий не входит. Скачайте `6502_functional_test.bin` из
//! https://github.com/Klaus2m5/6502_65C02_functional_tests (папка bin_files) и запустите:
//!
//! ```sh
//! A7800_FUNCTIONAL_TEST=путь/к/6502_functional_test.bin cargo test --release --test functional -- --nocapture
//! ```
//!
//! Без переменной окружения тест просто пропускается. В CI файл скачивается автоматически.

use a7800_core::cpu::{Bus, Cpu};

struct Ram(Vec<u8>);

impl Bus for Ram {
    fn read(&mut self, addr: u16) -> u8 {
        self.0[addr as usize]
    }
    fn write(&mut self, addr: u16, val: u8) {
        self.0[addr as usize] = val;
    }
}

#[test]
fn klaus_dormann_functional_test() {
    let Ok(path) = std::env::var("A7800_FUNCTIONAL_TEST") else {
        eprintln!("пропуск: переменная A7800_FUNCTIONAL_TEST не задана");
        return;
    };
    let data = std::fs::read(&path).expect("не удалось прочитать файл теста");
    assert_eq!(data.len(), 0x10000, "ожидается образ на 64 КБ");

    let mut ram = Ram(data);
    let mut cpu = Cpu::new();
    cpu.pc = 0x0400;

    // Тест заканчивается "ловушкой" — переходом на самого себя. Успех — адрес $3469.
    let mut steps: u64 = 0;
    loop {
        let pc = cpu.pc;
        cpu.step(&mut ram);
        steps += 1;
        if cpu.pc == pc {
            break;
        }
        assert!(steps < 500_000_000, "тест не завершился");
    }

    assert_eq!(
        cpu.pc, 0x3469,
        "тест застрял на ${:04X} (A={:02X} X={:02X} Y={:02X} P={:02X}) — ищите этот адрес в 6502_functional_test.lst",
        cpu.pc, cpu.a, cpu.x, cpu.y, cpu.p
    );
    eprintln!("функциональный тест пройден за {steps} инструкций");
}
