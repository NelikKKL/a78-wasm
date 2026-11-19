// "Окно" эмулятора в браузере: рисует кадр, играет звук, ловит клавиши.
// Сама эмуляция — в wasm (crates/a7800-web → crates/a7800-core).
import init, { Console } from "./pkg/a7800_web.js";

const SAMPLE_RATE = 48000;
const FRAME_MS = 1000 / 59.92;
const LS_BINDINGS = "a7800.bindings";

const canvas = document.getElementById("screen");
const g = canvas.getContext("2d");
const statusEl = document.getElementById("status");
const controlsEl = document.getElementById("controls");

let wasm, con, audio;
let loaded = false;
let paused = false;
let capturing = null; // { id } — ждём клавишу для переназначения
let last = 0;
let acc = 0;
let nextAudio = 0;

// ---------- подписи ----------

const DIRS = { up: "вверх", down: "вниз", left: "влево", right: "вправо", fire1: "огонь 1", fire2: "огонь 2" };
const SYSTEM = {
  reset: "Reset",
  select: "Select",
  pause: "Pause",
  diff0: "Сложность, левая",
  diff1: "Сложность, правая",
};

function actionLabel(id) {
  const m = id.match(/^p(\d)\.(\w+)$/);
  if (m) return `Игрок ${m[1]}: ${DIRS[m[2]] ?? m[2]}`;
  return SYSTEM[id] ?? id;
}

function keyLabel(code) {
  const arrows = { ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→" };
  return arrows[code] ?? code.replace(/^Key|^Digit/, "");
}

function setStatus(text) {
  statusEl.textContent = text;
}

// ---------- звук ----------

function ensureAudio() {
  try {
    audio ??= new AudioContext({ sampleRate: SAMPLE_RATE });
    if (audio.state === "suspended") audio.resume();
  } catch {
    audio = null;
  }
}

function pushAudio(samples) {
  if (!audio || audio.state !== "running" || samples.length === 0) return;
  const t = audio.currentTime;
  if (nextAudio < t + 0.01) nextAudio = t + 0.06; // буфер опустел — пересинхронизация
  if (nextAudio - t > 0.25) return; // накопилась задержка — выбрасываем кусок
  const buf = audio.createBuffer(1, samples.length, SAMPLE_RATE);
  buf.copyToChannel(samples, 0);
  const src = audio.createBufferSource();
  src.buffer = buf;
  src.connect(audio.destination);
  src.start(nextAudio);
  nextAudio += buf.duration;
}

// ---------- кадры ----------

function draw() {
  const w = con.frame_width();
  const h = con.frame_height();
  // Память wasm может расти, поэтому вид создаём заново на каждый кадр
  const pixels = new Uint8ClampedArray(wasm.memory.buffer, con.frame_ptr(), w * h * 4);
  g.putImageData(new ImageData(pixels, w, h), 0, 0);
}

function tick(now) {
  requestAnimationFrame(tick);
  const dt = Math.min(now - last, 100);
  last = now;
  if (!loaded || paused) return;

  acc += dt;
  let ran = false;
  while (acc >= FRAME_MS) {
    con.run_frame();
    pushAudio(con.take_audio());
    acc -= FRAME_MS;
    ran = true;
  }
  if (ran) draw();
}

// ---------- загрузка файлов ----------

async function readFile(file) {
  return new Uint8Array(await file.arrayBuffer());
}

async function loadRom(file) {
  ensureAudio();
  try {
    const title = con.load_cartridge(await readFile(file));
    loaded = true;
    paused = false;
    setStatus(title || file.name);
  } catch (e) {
    setStatus(`Не удалось открыть «${file.name}»: ${e.message ?? e}`);
  }
}

async function loadBios(file) {
  try {
    con.load_bios(await readFile(file));
    setStatus(`BIOS загружен: ${file.name}`);
  } catch (e) {
    setStatus(`BIOS не подошёл: ${e.message ?? e}`);
  }
}

// ---------- управление ----------

function saveBindings() {
  localStorage.setItem(LS_BINDINGS, con.bindings_json());
}

function buildControls() {
  const actions = JSON.parse(con.actions_json());
  const keysFor = {};
  for (const [code, id] of Object.entries(JSON.parse(con.bindings_json()))) {
    (keysFor[id] ??= []).push(code);
  }

  controlsEl.replaceChildren();
  for (const id of actions) {
    const row = document.createElement("div");
    row.className = "row";

    const name = document.createElement("span");
    name.textContent = actionLabel(id);

    const btn = document.createElement("button");
    btn.type = "button";
    btn.textContent = (keysFor[id] ?? []).map(keyLabel).join(", ") || "не назначено";
    btn.addEventListener("click", () => {
      capturing = { id };
      btn.textContent = "нажмите клавишу…";
      btn.classList.add("listening");
    });

    row.append(name, btn);
    controlsEl.append(row);
  }
}

window.addEventListener("keydown", (e) => {
  if (!con) return;

  if (capturing) {
    e.preventDefault();
    const { id } = capturing;
    capturing = null;
    if (e.code !== "Escape") {
      const displaced = con.rebind(id, e.code);
      saveBindings();
      if (displaced) {
        setStatus(`Клавиша ${keyLabel(e.code)} теперь у «${actionLabel(id)}», а у «${actionLabel(displaced)}» её больше нет.`);
      }
    }
    buildControls();
    return;
  }

  if (e.ctrlKey || e.metaKey || e.altKey) return;
  if (e.repeat) {
    if (con.is_bound(e.code)) e.preventDefault();
    return;
  }
  if (con.key_event(e.code, true)) {
    ensureAudio();
    e.preventDefault();
  }
});

window.addEventListener("keyup", (e) => {
  if (con && !capturing && con.key_event(e.code, false)) e.preventDefault();
});

window.addEventListener("blur", () => con?.release_all());

// ---------- запуск ----------

async function main() {
  wasm = await init();
  con = new Console(SAMPLE_RATE);

  const saved = localStorage.getItem(LS_BINDINGS);
  if (saved) {
    try {
      con.set_bindings_json(saved);
    } catch {
      localStorage.removeItem(LS_BINDINGS);
    }
  }
  buildControls();

  document.getElementById("rom").addEventListener("change", (e) => {
    const f = e.target.files[0];
    if (f) loadRom(f);
  });
  document.getElementById("bios").addEventListener("change", (e) => {
    const f = e.target.files[0];
    if (f) loadBios(f);
  });
  document.getElementById("reset").addEventListener("click", () => con.reset());
  document.getElementById("pause").addEventListener("click", (e) => {
    paused = !paused;
    e.target.textContent = paused ? "Продолжить" : "Пауза";
  });
  document.getElementById("defaults").addEventListener("click", () => {
    con.reset_bindings();
    saveBindings();
    buildControls();
  });

  // Перетаскивание файла на страницу
  window.addEventListener("dragover", (e) => {
    e.preventDefault();
    document.body.classList.add("drag");
  });
  window.addEventListener("dragleave", () => document.body.classList.remove("drag"));
  window.addEventListener("drop", (e) => {
    e.preventDefault();
    document.body.classList.remove("drag");
    const f = e.dataTransfer?.files?.[0];
    if (f) loadRom(f);
  });

  requestAnimationFrame(tick);
}

main().catch((e) => setStatus(`Не удалось запустить эмулятор: ${e.message ?? e}`));
