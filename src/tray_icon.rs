use ksni::Icon;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    #[default]
    Default,
    Left,
    Right,
    UpsideDown,
}

impl Orientation {
    pub const OPTIONS: [(Self, &'static str); 4] = [
        (Self::Default, "Обычная"),
        (Self::Left, "Набок влево"),
        (Self::Right, "Набок вправо"),
        (Self::UpsideDown, "Вверх ногами"),
    ];

    pub fn load() -> Self {
        crate::history::state_path()
            .and_then(|path| std::fs::read(path.with_file_name("tray-icon.json")).ok())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    pub fn save(self) -> anyhow::Result<()> {
        if let Some(path) = crate::history::state_path() {
            crate::history::save_json(&path.with_file_name("tray-icon.json"), &self)?;
        }
        Ok(())
    }

    pub fn pixmaps(self, low: bool) -> Vec<Icon> {
        [1, 2].into_iter().map(|scale| self.pixmap(scale, low)).collect()
    }

    fn pixmap(self, scale: i32, low: bool) -> Icon {
        const SIZE: i32 = 24;
        const BORDER: [u8; 4] = [255, 236, 246, 245];
        const BACKGROUND: [u8; 4] = [255, 29, 43, 55];
        let (fill_end, fill) = if low { (7, [255, 244, 189, 90]) } else { (14, [255, 112, 209, 172]) };
        let size = SIZE * scale;
        let mut data = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let (x, y) = (x / scale, y / scale);
                let (x, y) = match self {
                    Self::Left => (SIZE - 1 - y, x),
                    Self::Right => (y, SIZE - 1 - x),
                    Self::UpsideDown => (SIZE - 1 - x, SIZE - 1 - y),
                    Self::Default => (x, y),
                };
                let color = match (x, y) {
                    (5..=14, 9..=14) if x <= fill_end => fill,
                    (4..=17, 8..=15) => BACKGROUND,
                    (2..=19, 6..=17) | (20..=21, 10..=13) => BORDER,
                    _ => [0; 4],
                };
                data.extend_from_slice(&color);
            }
        }
        Icon { width: size, height: size, data }
    }
}
