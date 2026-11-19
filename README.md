# atari7800-rs

Эмулятор Atari 7800 на Rust: библиотека-ядро + веб-версия (WebAssembly).

```
crates/
  a7800-core/   ядро (без зависимостей, без привязки к платформе)
  a7800-web/    обёртка wasm-bindgen
web/            страница: canvas, звук, клавиши, переназначение управления
.github/workflows/ci.yml   тесты → сборка wasm → деплой на GitHub Pages
```

## Библиотека (`a7800-core`)

Ядро ничего не знает об окне, звуке и клавиатуре. Хост подключается двумя способами.

**Вручную** (так работает веб-версия):

```rust
let mut emu = Emulator::new(Config::default());
emu.load_cartridge(&rom)?;
let bindings = Bindings::default();

emu.handle_key(&bindings, "KeyZ", true);   // нажатие
emu.run_frame();
let rgba = &emu.frame().pixels;            // 320x224, RGBA
let audio = emu.take_audio();              // моно f32
```

**Через трейт `Host`** (нативное окно, тесты): реализуйте `poll_input`,
`present`, `queue_audio`, `should_quit` и вызовите `emu.run(&mut host)`.

### Привязка клавиш

`Bindings` сопоставляет строку-клавишу (в вебе это `KeyboardEvent.code`) с `Action`:
кнопка джойстика игрока 1/2, Reset, Select, Pause, переключатели сложности.

```rust
let mut b = Bindings::default();
b.rebind(Action::Joy { player: 0, button: Button::Fire1 }, "Space");
let saved = b.to_pairs();                  // [("Space", "p1.fire1"), …] → хоть в JSON, хоть в файл
let b = Bindings::from_pairs(saved);
```

## Сборка

```sh
cargo test -p a7800-core                   # тесты ядра
cargo install wasm-pack
wasm-pack build crates/a7800-web --target web --release --out-dir ../../web/pkg
python3 -m http.server -d web 8080         # открыть http://localhost:8080
```

GitHub Pages: Settings → Pages → Source: **GitHub Actions**. После пуша в `main` страница выкладывается сама.
Закоммитьте `Cargo.lock` после первой успешной сборки.

## Проверка процессора

- Юнит-тесты (`cargo test -p a7800-core`): каждый из 256 опкодов выполняется без паники, плюс проверки неофициальных инструкций и длины NOP.
- Функциональный тест Клауса Дормана (`crates/a7800-core/tests/functional.rs`): CI скачивает `6502_functional_test.bin` и гоняет его.
  Если он упрётся, в сообщении будет адрес, на котором застрял процессор — по нему в `.lst` файле теста видно, какая инструкция ведёт себя неверно.
  Шаг помечен `continue-on-error`, пока не убедимся, что тест проходит.
- Если игра зависла на опкоде JAM, «Отладка» на странице покажет адрес и код.

## Проверка без настоящих игр

Кнопка «Тестовый экран» на странице грузит крошечный ROM из `web/testrom.js`: должен получиться
синий фон с тремя полосами разных цветов. В CI тот же ROM прогоняется через собранный wasm
(`tests/smoke.mjs`), так что поломку конвейера CPU → MARIA → кадр → звук видно сразу.
Если на настоящей игре чёрный экран, откройте «Отладку» на странице: там PC, состояние MARIA,
счётчики неофициальных опкодов и нереализованных режимов графики.

## Что готово

- Процессор 6502C: все 256 опкодов (официальные и неофициальные: LAX, SAX, DCP, ISC, SLO, RLA, SRE, RRA, ANC, ALR, ARR, AXS, SHA/SHX/SHY/TAS, LAS, все NOP, JAM), десятичный режим, NMI.
  Нестабильные опкоды (XAA, LAX #imm, SHA/TAS) сделаны по типичному поведению железа, ARR в десятичном режиме упрощён.
- Карта памяти, RIOT (порты, таймер, RAM), WSYNC.
- MARIA: список зон и списки отображения, режимы 160A, 160B, 320A, символьный режим, DLI.
- Звук TIA (2 канала, упрощённые формы волн).
- Картриджи: `.a78`, линейная ROM, SuperGame, RAM на $4000.
- Ввод: два джойстика (2 кнопки), консольные переключатели, переназначение клавиш.

## Чего нет (по важности)

1. Проверка на реальных играх и тестовых ROM — этот код ещё ни разу не запускался на игре.
2. MARIA: режимы 320B/C/D, Kangaroo, рамка, colorkill, точный тайминг DMA.
3. POKEY, YM2151, банки Activision/Absolute/SOUPER.
4. PAL, точная палитра, сохранения, геймпады.

BIOS и игры в репозиторий не входят: берите свои. BIOS не обязателен — большинство игр запускается без него.
