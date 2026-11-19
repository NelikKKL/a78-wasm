//! Тонкая обёртка над `a7800-core` для браузера.
//! Всё "окно" (canvas, звук, клавиатура) живёт в `web/main.js`.

use std::collections::BTreeMap;

use a7800_core::{Action, Bindings, Config, Emulator, SCREEN_H, SCREEN_W};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn init_panic_hook() {
    console_error_panic_hook::set_once();
}

fn js_err(e: impl ToString) -> JsError {
    JsError::new(&e.to_string())
}

#[wasm_bindgen]
pub struct Console {
    emu: Emulator,
    bindings: Bindings,
}

#[wasm_bindgen]
impl Console {
    #[wasm_bindgen(constructor)]
    pub fn new(sample_rate: u32) -> Console {
        Console { emu: Emulator::new(Config { sample_rate }), bindings: Bindings::default() }
    }

    /// Загружает картридж, возвращает название из заголовка (может быть пустым).
    pub fn load_cartridge(&mut self, data: &[u8]) -> Result<String, JsError> {
        let info = self.emu.load_cartridge(data).map_err(js_err)?;
        Ok(info.title)
    }

    pub fn load_bios(&mut self, data: &[u8]) -> Result<(), JsError> {
        self.emu.load_bios(data).map_err(js_err)
    }

    pub fn reset(&mut self) {
        self.emu.reset();
    }

    pub fn run_frame(&mut self) {
        self.emu.run_frame();
    }

    /// Указатель на RGBA-кадр в памяти wasm (без копирования).
    pub fn frame_ptr(&self) -> *const u8 {
        self.emu.frame().pixels.as_ptr()
    }

    pub fn frame_width(&self) -> u32 {
        SCREEN_W as u32
    }

    pub fn frame_height(&self) -> u32 {
        SCREEN_H as u32
    }

    /// Звук за прошедшие кадры (Float32Array, моно).
    pub fn take_audio(&mut self) -> Vec<f32> {
        self.emu.take_audio()
    }

    // ---- управление ----

    /// `code` — `KeyboardEvent.code`. Возвращает `true`, если клавиша привязана.
    pub fn key_event(&mut self, code: &str, pressed: bool) -> bool {
        self.emu.handle_key(&self.bindings, code, pressed)
    }

    pub fn is_bound(&self, code: &str) -> bool {
        self.bindings.lookup(code).is_some()
    }

    pub fn release_all(&mut self) {
        self.emu.input_mut().release_all();
    }

    /// Список id всех действий: `["p1.up", …, "reset", "diff1"]`.
    pub fn actions_json(&self) -> String {
        let ids: Vec<String> = Action::all().iter().map(Action::id).collect();
        serde_json::to_string(&ids).unwrap_or_default()
    }

    /// Текущие привязки: `{"KeyZ": "p1.fire1", …}`.
    pub fn bindings_json(&self) -> String {
        let map: BTreeMap<String, String> = self.bindings.to_pairs().into_iter().collect();
        serde_json::to_string(&map).unwrap_or_default()
    }

    pub fn set_bindings_json(&mut self, json: &str) -> Result<(), JsError> {
        let map: BTreeMap<String, String> = serde_json::from_str(json).map_err(js_err)?;
        self.bindings = Bindings::from_pairs(map);
        Ok(())
    }

    /// Переназначает действие на клавишу. Возвращает id действия, у которого
    /// отобрали эту клавишу (или пустую строку).
    pub fn rebind(&mut self, action_id: &str, code: &str) -> Result<String, JsError> {
        let action = Action::from_id(action_id).ok_or_else(|| js_err("неизвестное действие"))?;
        Ok(self.bindings.rebind(action, code).map(|a| a.id()).unwrap_or_default())
    }

    pub fn reset_bindings(&mut self) {
        self.bindings = Bindings::default();
    }
}
