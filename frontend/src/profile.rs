//! what Pocket Native knows about a game, titles/<title id>/profile.toml:
//! its name and the options the menu lists under "This game". a game without
//! a profile runs with none.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct Profile {
    pub name: String,
    #[serde(default, rename = "option")]
    pub options: Vec<GameOption>,
    /// the advanced settings the game needs, until the player changes them.
    #[serde(default)]
    pub advanced: Option<crate::settings::Advanced>,
}

/// an option the menu turns on and off.
#[derive(Debug, Deserialize)]
pub struct GameOption {
    pub name: String,
    /// on until the player turns it off.
    #[serde(default)]
    pub default: bool,
    /// what the menu shows for off and on.
    #[serde(default = "on_off")]
    pub values: [String; 2],
    /// what the menu says about it under the list.
    #[serde(default)]
    pub help: String,
    /// draws left out while it is off, as ZAKURO_DROP_DRAWS takes them.
    #[serde(default)]
    pub drops_when_off: Vec<String>,
    /// the bottom screen drawn with every picture while it is on.
    #[serde(default)]
    pub bottom_full_rate_when_on: bool,
}

fn on_off() -> [String; 2] {
    ["Off".to_owned(), "On".to_owned()]
}

/// POCKET_NATIVE_TITLES, or titles beside the program.
pub fn titles_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("POCKET_NATIVE_TITLES") {
        return dir.into();
    }
    let program = std::env::current_exe().ok();
    program.and_then(|program| program.parent().map(|dir| dir.join("titles"))).unwrap_or_else(|| "titles".into())
}

/// the profile for a title, when there is one that reads.
pub fn find(titles: &Path, title_id: u64) -> Option<Profile> {
    let path = titles.join(format!("{title_id:016X}")).join("profile.toml");
    let text = std::fs::read_to_string(&path).ok()?;
    match toml::from_str(&text) {
        Ok(profile) => Some(profile),
        Err(error) => {
            log::warn!("{} is not readable, {error}", path.display());
            None
        }
    }
}
