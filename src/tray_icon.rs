use serde::{Deserialize, Serialize};
use std::path::Path;

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
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Style {
    Color,
    #[default]
    Monochrome,
}

impl Style {
    pub const OPTIONS: [(Self, &'static str); 2] = [
        (Self::Color, "Обычный"),
        (Self::Monochrome, "Монохром"),
    ];
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Appearance {
    pub orientation: Orientation,
    pub style: Style,
    pub language: crate::i18n::Language,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SavedAppearance {
    Current(Appearance),
    OrientationOnly(Orientation),
}

impl Appearance {
    pub fn load() -> Self {
        crate::history::state_path()
            .and_then(|path| std::fs::read(path.with_file_name("tray-icon.json")).ok())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .map(|saved| match saved {
                SavedAppearance::Current(appearance) => appearance,
                SavedAppearance::OrientationOnly(orientation) => Self { orientation, ..Self::default() },
            })
            .unwrap_or_default()
    }

    pub fn save(self) -> anyhow::Result<()> {
        if let Some(path) = crate::history::state_path() {
            crate::history::save_json(&path.with_file_name("tray-icon.json"), &self)?;
        }
        Ok(())
    }

    pub fn icon_name(self, low: bool) -> String {
        let orientation = match self.orientation {
            Orientation::Default => "normal",
            Orientation::Left => "left",
            Orientation::Right => "right",
            Orientation::UpsideDown => "inverted",
        };
        let style = if self.style == Style::Color { "_color" } else { "" };
        let level = if low { "_low" } else { "" };
        // KDE uses hyphen-separated prefixes to look up fallback icons.
        format!("battery_life_tray_{orientation}{style}{level}-symbolic")
    }
}

pub fn theme_path() -> String {
    let installed = std::env::current_exe().ok()
        .and_then(|exe| exe.parent()?.join("../share/battery-life/icons").canonicalize().ok());
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/tray-icons");
    installed.unwrap_or(source).to_str().map(str::to_owned)
        .filter(|path| Path::new(path).is_dir()).unwrap_or_default()
}
