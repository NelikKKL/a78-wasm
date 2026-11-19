//! Палитра 7800: 256 цветов = 16 оттенков × 16 яркостей.
//!
//! Таблица генерируется через YIQ и является приближением. Чтобы получить
//! точные цвета конкретного телевизора, подмените её через `Emulator::set_palette`.

pub type Palette = [[u8; 3]; 256];

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

pub fn ntsc() -> Palette {
    let mut out = [[0u8; 3]; 256];
    for (c, slot) in out.iter_mut().enumerate() {
        let hue = (c >> 4) as f32;
        let lum = (c & 0x0F) as f32 / 15.0;
        let y = 0.04 + 0.96 * lum;
        let (i, q) = if hue == 0.0 {
            (0.0, 0.0)
        } else {
            let theta = ((hue - 1.0) * 24.0 - 35.0).to_radians();
            let sat = 0.32;
            (sat * theta.cos(), sat * theta.sin())
        };
        let r = y + 0.956 * i + 0.621 * q;
        let g = y - 0.272 * i - 0.647 * q;
        let b = y - 1.106 * i + 1.703 * q;
        *slot = [to_u8(r), to_u8(g), to_u8(b)];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_ramp_is_monotonic() {
        let p = ntsc();
        for l in 1..16 {
            assert!(p[l][0] >= p[l - 1][0]);
        }
        assert!(p[0x0F][0] > 240);
    }
}
