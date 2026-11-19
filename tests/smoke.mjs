// Сквозная проверка собранного wasm: грузим тестовый ROM, гоняем кадры,
// проверяем, что нарисовался фон + цветные блоки и что есть звук.
// Запуск: node tests/smoke.mjs   (после wasm-pack build)
import fs from "node:fs";
import { fileURLToPath } from "node:url";
import { initSync, Console } from "../web/pkg/a7800_web.js";
import { buildTestRom } from "../web/testrom.js";

const wasmPath = fileURLToPath(new URL("../web/pkg/a7800_web_bg.wasm", import.meta.url));
const wasm = initSync({ module: fs.readFileSync(wasmPath) });

const con = new Console(48000);
con.load_cartridge(buildTestRom());

let audio = 0;
for (let i = 0; i < 5; i++) {
  con.run_frame();
  audio += con.take_audio().length;
}

const w = con.frame_width();
const h = con.frame_height();
const px = new Uint32Array(wasm.memory.buffer.slice(con.frame_ptr(), con.frame_ptr() + w * h * 4));
const counts = new Map();
for (const v of px) counts.set(v, (counts.get(v) ?? 0) + 1);
const top = Math.max(...counts.values());

console.log(con.debug_text());
console.log(`цветов: ${counts.size}, фон: ${((top / px.length) * 100).toFixed(0)}%, звук: ${audio} сэмплов`);

const problems = [];
if (counts.size < 4) problems.push("на кадре мало цветов — MARIA ничего не нарисовала");
if (top / px.length < 0.5) problems.push("фон должен занимать больше половины кадра");
if (audio < 3000) problems.push("звука слишком мало");
if (!/NMI 5\b/.test(con.debug_text())) problems.push("ожидался ровно один DLI на кадр (NMI 5 после 5 кадров)");

if (problems.length) {
  console.error("ОШИБКА:\n- " + problems.join("\n- "));
  process.exit(1);
}
console.log("ok");
