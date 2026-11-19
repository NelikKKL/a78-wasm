// Крошечный тестовый ROM на 16 КБ ($C000–$FFFF): включает MARIA и рисует три блока
// в режимах 160A (палитры 0 и 1) и 160B. Нужен, чтобы проверять эмулятор без настоящих игр.
// Это наш собственный код, не чужая игра.

export function buildTestRom() {
  const rom = new Uint8Array(0x4000).fill(0xea);
  const put = (addr, bytes) => bytes.forEach((b, i) => (rom[addr - 0xc000 + i] = b));

  const DLL = 0xe000;
  const DL = 0xe100;
  const GFX = 0xd000;
  const NMI = 0xc100;

  const code = [
    0x78, 0xd8, 0xa2, 0xff, 0x9a, //             SEI CLD LDX #$FF TXS
    0xa9, 0x07, 0x85, 0x01, //                   INPTCTRL = 7
    0xa9, 0x92, 0x85, 0x20, //                   фон
    0xa9, 0x1e, 0x85, 0x21, 0xa9, 0x46, 0x85, 0x22, 0xa9, 0xca, 0x85, 0x23, // палитра 0
    0xa9, 0x2e, 0x85, 0x25, 0xa9, 0x66, 0x85, 0x26, 0xa9, 0xaa, 0x85, 0x27, // палитра 1
    0xa9, DLL >> 8, 0x85, 0x2c, 0xa9, DLL & 0xff, 0x85, 0x30, //            DPPH / DPPL
    0xa9, 0x40, 0x85, 0x3c, //                   CTRL: DMA включён
  ];
  put(0xc000, code);
  const spin = 0xc000 + code.length;
  put(spin, [0x4c, spin & 0xff, spin >> 8]); // JMP spin
  put(NMI, [0xee, 0x00, 0x18, 0x40]); //         INC $1800 ; RTI

  // 14 зон по 16 строк = 224 строки; у первой флаг DLI
  for (let z = 0; z < 14; z++) put(DLL + z * 3, [z === 0 ? 0x8f : 0x0f, DL >> 8, DL & 0xff]);

  // Список отображения: два 4-байтовых заголовка (160A), один 5-байтовый (160B), конец
  put(DL, [
    0x00, 0x1c, GFX >> 8, 20, //        палитра 0, 4 байта, x = 20
    0x00, 0x3c, GFX >> 8, 60, //        палитра 1, 4 байта, x = 60
    0x00, 0xc0, GFX >> 8, 0x1c, 100, // 5 байт: режим 160B, x = 100
    0x00, 0x00,
  ]);

  // Графика: по странице на строку зоны, узор сдвигается от строки к строке
  const rol2 = (b, n) => ((b << n) | (b >> (8 - n))) & 0xff;
  for (let r = 0; r < 16; r++) {
    put(GFX + r * 256, [0x1b, 0x6c, 0xb1, 0xc6].map((b) => rol2(b, 2 * (r % 4))));
  }

  // Векторы: NMI, RESET, IRQ
  put(0xfffa, [NMI & 0xff, NMI >> 8, 0x00, 0xc0, NMI & 0xff, NMI >> 8]);
  return rom;
}
