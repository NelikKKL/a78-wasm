//! Привязка клавиш к действиям.
//!
//! Ключ — это произвольная строка, которую выдаёт окно/хост. В вебе это
//! `KeyboardEvent.code` ("KeyZ", "ArrowUp" …), в нативном окне — что угодно своё.
//! Ядро ничего не знает о конкретной платформе.

use std::collections::BTreeMap;

use crate::input::{Action, Button, InputState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bindings {
    map: BTreeMap<String, Action>,
}

impl Default for Bindings {
    fn default() -> Self {
        Self::default_layout()
    }
}

impl Bindings {
    pub fn empty() -> Self {
        Self { map: BTreeMap::new() }
    }

    /// Раскладка по умолчанию: игрок 1 — стрелки + Z/X, игрок 2 — WASD + F/G.
    pub fn default_layout() -> Self {
        let mut b = Self::empty();
        for (key, button) in [
            ("ArrowUp", Button::Up),
            ("ArrowDown", Button::Down),
            ("ArrowLeft", Button::Left),
            ("ArrowRight", Button::Right),
            ("KeyZ", Button::Fire1),
            ("KeyX", Button::Fire2),
        ] {
            b.bind(key, Action::Joy { player: 0, button });
        }
        for (key, button) in [
            ("KeyW", Button::Up),
            ("KeyS", Button::Down),
            ("KeyA", Button::Left),
            ("KeyD", Button::Right),
            ("KeyF", Button::Fire1),
            ("KeyG", Button::Fire2),
        ] {
            b.bind(key, Action::Joy { player: 1, button });
        }
        b.bind("Enter", Action::Reset);
        b.bind("ShiftRight", Action::Select);
        b.bind("KeyP", Action::Pause);
        b.bind("Digit1", Action::ToggleDifficulty(0));
        b.bind("Digit2", Action::ToggleDifficulty(1));
        b
    }

    /// Добавляет привязку (у одного действия может быть несколько клавиш).
    /// Возвращает действие, которое раньше было на этой клавише.
    pub fn bind(&mut self, key: impl Into<String>, action: Action) -> Option<Action> {
        self.map.insert(key.into(), action)
    }

    /// Заменяет все клавиши действия на одну новую — это и есть "переназначить".
    /// Возвращает действие, у которого отобрали клавишу (чтобы UI мог предупредить).
    pub fn rebind(&mut self, action: Action, key: impl Into<String>) -> Option<Action> {
        self.map.retain(|_, a| *a != action);
        self.map.insert(key.into(), action)
    }

    pub fn unbind_key(&mut self, key: &str) -> Option<Action> {
        self.map.remove(key)
    }

    pub fn lookup(&self, key: &str) -> Option<Action> {
        self.map.get(key).copied()
    }

    pub fn keys_for(&self, action: Action) -> Vec<&str> {
        self.map
            .iter()
            .filter(|(_, a)| **a == action)
            .map(|(k, _)| k.as_str())
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, Action)> {
        self.map.iter().map(|(k, a)| (k.as_str(), *a))
    }

    /// Обрабатывает нажатие/отпускание клавиши. Возвращает `true`, если клавиша привязана.
    pub fn handle_key(&self, key: &str, pressed: bool, input: &mut InputState) -> bool {
        match self.lookup(key) {
            Some(action) => {
                input.apply(action, pressed);
                true
            }
            None => false,
        }
    }

    /// Пары (клавиша, id действия) — формат для сохранения в JSON/файл на стороне хоста.
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        self.map.iter().map(|(k, a)| (k.clone(), a.id())).collect()
    }

    /// Обратная операция. Неизвестные id молча пропускаются.
    pub fn from_pairs<K: Into<String>, V: AsRef<str>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        let mut b = Self::empty();
        for (k, v) in pairs {
            if let Some(a) = Action::from_id(v.as_ref()) {
                b.map.insert(k.into(), a);
            }
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebind_moves_key() {
        let mut b = Bindings::default();
        let fire1 = Action::Joy { player: 0, button: Button::Fire1 };
        assert_eq!(b.lookup("KeyZ"), Some(fire1));

        // KeyX раньше был Fire2 — его отберут
        let displaced = b.rebind(fire1, "KeyX");
        assert_eq!(displaced, Some(Action::Joy { player: 0, button: Button::Fire2 }));
        assert_eq!(b.lookup("KeyZ"), None);

        let mut input = InputState::default();
        assert!(b.handle_key("KeyX", true, &mut input));
        assert!(input.button(0, Button::Fire1));
        assert!(b.handle_key("KeyX", false, &mut input));
        assert!(!input.button(0, Button::Fire1));
        assert!(!b.handle_key("KeyQ", true, &mut input));
    }

    #[test]
    fn pairs_roundtrip() {
        let mut b = Bindings::default();
        b.rebind(Action::Pause, "Space");
        let restored = Bindings::from_pairs(b.to_pairs());
        assert_eq!(b, restored);
    }
}
