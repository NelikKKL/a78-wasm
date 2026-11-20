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

    /// Описание картриджа: схема банков, размер, RAM и POKEY.
    pub fn cart_description(&self) -> String {
        self.emu.cart_description()
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

    /// Текст для панели отладки: регистры, состояние MARIA, счётчики проблем.
    pub fn debug_text(&self) -> String {
        let d = self.emu.debug_info();
        let unofficial = match d.last_unofficial {
            Some((pc, op)) => format!(" (последний: {op:02X} по адресу {pc:04X})"),
            None => String::new(),
        };
        let jam = match d.jam {
            Some((pc, op)) => format!("\n!!! процессор завис на опкоде {op:02X} по адресу {pc:04X}"),
            None => String::new(),
        };
        let unsupported = match d.last_unsupported_mode {
            Some((rm, wm)) => format!(" (RM={rm}, WM={})", wm as u8),
            None => String::new(),
        };
        format!(
            "картридж: {}\n\
             кадр {}, NMI {}\n\
             PC={:04X} A={:02X} X={:02X} Y={:02X} SP={:02X} P={:02X}\n\
             MARIA: {}, CTRL={:02X}, DPP={:04X}, CHARBASE={:02X}, фон={:02X}\n\
             INPTCTRL={:02X}, BIOS: {}\n\
             неофициальных опкодов выполнено: {}{}\n\
             строк с недопустимым режимом чтения MARIA (RM=1): {}{}{}",
            d.cart,
            d.frames,
            d.nmi_count,
            d.pc,
            d.a,
            d.x,
            d.y,
            d.sp,
            d.p,
            if d.maria_enabled { "вкл" } else { "выкл" },
            d.ctrl,
            d.dpp,
            d.charbase,
            d.background,
            d.inptctrl,
            if d.bios_enabled { "вкл" } else { "выкл" },
            d.unofficial_ops,
            unofficial,
            d.unsupported_gfx_bytes,
            unsupported,
            jam,
        )
    }

    // ---- управление ----

    /// `code` — `KeyboardEvent.code`. Возвращает `true`, если клавиша привязана.
    pub fn key_event(&mut self, code: &str, pressed: bool) -> bool {
        self.emu.handle_key(&self.bindings, code, pressed)
    }

    /// Применяет действие напрямую (для геймпада): id вида `p1.up`, `p2.fire1`, `pause`.
    pub fn action_event(&mut self, action_id: &str, pressed: bool) -> bool {
        match Action::from_id(action_id) {
            Some(a) => {
                self.emu.input_mut().apply(a, pressed);
                true
            }
            None => false,
        }
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
