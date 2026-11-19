//! Ввод: кнопки джойстика ProLine, консольные переключатели и "действия",
//! к которым привязываются клавиши (см. `bindings`).

/// Кнопки джойстика 7800: 4 направления и 2 кнопки огня.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    Fire1,
    Fire2,
}

impl Button {
    pub const ALL: [Button; 6] = [
        Button::Up,
        Button::Down,
        Button::Left,
        Button::Right,
        Button::Fire1,
        Button::Fire2,
    ];

    pub fn mask(self) -> u8 {
        1 << (self as u8)
    }

    /// Стабильный строковый идентификатор (используется при сохранении настроек).
    pub fn id(self) -> &'static str {
        match self {
            Button::Up => "up",
            Button::Down => "down",
            Button::Left => "left",
            Button::Right => "right",
            Button::Fire1 => "fire1",
            Button::Fire2 => "fire2",
        }
    }

    pub fn from_id(s: &str) -> Option<Button> {
        Button::ALL.into_iter().find(|b| b.id() == s)
    }
}

/// Всё, к чему можно привязать клавишу.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    /// Кнопка джойстика; `player` — 0 или 1.
    Joy { player: u8, button: Button },
    Reset,
    Select,
    Pause,
    /// Переключатель сложности (0 — левый, 1 — правый); срабатывает при нажатии.
    ToggleDifficulty(u8),
}

impl Action {
    /// Полный список действий — удобно для экрана настройки управления.
    pub fn all() -> Vec<Action> {
        let mut v = Vec::new();
        for player in 0..2u8 {
            for button in Button::ALL {
                v.push(Action::Joy { player, button });
            }
        }
        v.extend([
            Action::Reset,
            Action::Select,
            Action::Pause,
            Action::ToggleDifficulty(0),
            Action::ToggleDifficulty(1),
        ]);
        v
    }

    /// Стабильный идентификатор: `p1.up`, `p2.fire1`, `reset`, `diff0` …
    pub fn id(&self) -> String {
        match *self {
            Action::Joy { player, button } => format!("p{}.{}", player + 1, button.id()),
            Action::Reset => "reset".into(),
            Action::Select => "select".into(),
            Action::Pause => "pause".into(),
            Action::ToggleDifficulty(i) => format!("diff{}", i),
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        match id {
            "reset" => Some(Action::Reset),
            "select" => Some(Action::Select),
            "pause" => Some(Action::Pause),
            "diff0" => Some(Action::ToggleDifficulty(0)),
            "diff1" => Some(Action::ToggleDifficulty(1)),
            _ => {
                let rest = id.strip_prefix('p')?;
                let (p, b) = rest.split_once('.')?;
                let player: u8 = p.parse().ok()?;
                if !(1..=2).contains(&player) {
                    return None;
                }
                Some(Action::Joy { player: player - 1, button: Button::from_id(b)? })
            }
        }
    }
}

/// Текущее состояние всех органов управления.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputState {
    joy: [u8; 2],
    pub reset: bool,
    pub select: bool,
    pub pause: bool,
    /// Переключатели сложности: `true` — позиция A.
    pub difficulty: [bool; 2],
}

impl InputState {
    pub fn set_button(&mut self, player: usize, button: Button, pressed: bool) {
        if let Some(j) = self.joy.get_mut(player) {
            if pressed {
                *j |= button.mask();
            } else {
                *j &= !button.mask();
            }
        }
    }

    pub fn button(&self, player: usize, button: Button) -> bool {
        self.joy.get(player).is_some_and(|j| j & button.mask() != 0)
    }

    pub fn apply(&mut self, action: Action, pressed: bool) {
        match action {
            Action::Joy { player, button } => self.set_button(player as usize, button, pressed),
            Action::Reset => self.reset = pressed,
            Action::Select => self.select = pressed,
            Action::Pause => self.pause = pressed,
            Action::ToggleDifficulty(i) => {
                if pressed {
                    if let Some(d) = self.difficulty.get_mut(i as usize) {
                        *d = !*d;
                    }
                }
            }
        }
    }

    /// Отпускает все кнопки (например, при потере фокуса окном). Переключатели сложности сохраняются.
    pub fn release_all(&mut self) {
        self.joy = [0; 2];
        self.reset = false;
        self.select = false;
        self.pause = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_roundtrip() {
        for a in Action::all() {
            assert_eq!(Action::from_id(&a.id()), Some(a));
        }
        assert_eq!(Action::from_id("p3.up"), None);
        assert_eq!(Action::from_id("мусор"), None);
    }
}
