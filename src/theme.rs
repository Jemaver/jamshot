//! Кольори інтерфейсу: акцент і світла/темна тема беруться з налаштувань COSMIC.

use crate::gfx::Rgba;

pub struct Theme {
    pub accent: Rgba,
    pub dark: bool,
}

fn num_after(s: &str, key: &str) -> Option<f32> {
    let i = s.find(key)? + key.len();
    let rest = s[i..].trim_start();
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

impl Theme {
    pub fn load() -> Theme {
        let home = std::env::var("HOME").unwrap_or_default();
        let base = format!("{home}/.config/cosmic");
        let dark = std::fs::read_to_string(format!("{base}/com.system76.CosmicTheme.Mode/v1/is_dark"))
            .map(|s| s.trim() != "false")
            .unwrap_or(true);
        let dir = if dark { "Dark" } else { "Light" };
        let accent = std::fs::read_to_string(format!("{base}/com.system76.CosmicTheme.{dir}/v1/accent"))
            .ok()
            .and_then(|s| {
                let b = &s[s.find("base:")?..];
                let f = |k| num_after(b, k).map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
                Some([f("red:")?, f("green:")?, f("blue:")?, 255])
            })
            .unwrap_or([0x63, 0xD0, 0xDF, 255]);
        Theme { accent, dark }
    }

    pub fn glass(&self) -> Rgba {
        if self.dark { [30, 32, 36, 158] } else { [248, 249, 251, 190] }
    }
    pub fn edge(&self) -> Rgba {
        if self.dark { [255, 255, 255, 26] } else { [0, 0, 0, 22] }
    }
    pub fn ink(&self) -> Rgba {
        if self.dark { [236, 238, 241, 255] } else { [28, 30, 34, 255] }
    }
    pub fn ink_dim(&self) -> Rgba {
        if self.dark { [163, 169, 177, 255] } else { [92, 98, 106, 255] }
    }
    pub fn hover(&self) -> Rgba {
        if self.dark { [255, 255, 255, 20] } else { [0, 0, 0, 14] }
    }
    pub fn selected_bg(&self) -> Rgba {
        if self.dark { [255, 255, 255, 31] } else { [0, 0, 0, 24] }
    }
    pub fn tip_bg(&self) -> Rgba {
        if self.dark { [20, 22, 26, 235] } else { [255, 255, 255, 245] }
    }
    /// Колір іконки на акцентній підкладці — темний або світлий за яскравістю акценту.
    pub fn on_accent(&self) -> Rgba {
        let a = self.accent;
        let l = 0.2126 * a[0] as f32 + 0.7152 * a[1] as f32 + 0.0722 * a[2] as f32;
        if l > 140.0 { [11, 42, 46, 255] } else { [255, 255, 255, 255] }
    }
}
