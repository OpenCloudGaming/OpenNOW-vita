//! The player's starred games, kept on the memory card.
//!
//! Stores enough of each game to *draw its row*, not just its id. That matters because the catalog
//! only pages in 1000 of the account's ~5800 titles: a favourite past that cut-off is not in
//! `games` at all, so with an id alone there would be nothing to render and the player would have
//! to search for it - which is the entire thing favourites exist to avoid.
//!
//! One JSON object per line rather than one JSON array. A corrupt or half-written line then costs
//! that single favourite instead of the whole list.

use serde::{Deserialize, Serialize};

use super::catalog::GameSummary;

const STORE_DIR: &str = "ux0:data/opennow-vita";
const STORE_PATH: &str = "ux0:data/opennow-vita/favorites.txt";

/// A starred game, as much of it as is needed to show a row without the catalog loaded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FavoriteGame {
    pub app_id: String,
    pub title: String,
    #[serde(default)]
    pub cover_url: Option<String>,
    #[serde(default)]
    pub store: Option<String>,
}

impl FavoriteGame {
    fn from_summary(game: &GameSummary) -> Self {
        Self {
            app_id: game.app_id.clone(),
            title: game.title.clone(),
            cover_url: game.cover_url.clone(),
            store: game.store.clone(),
            // `last_played` is deliberately not kept: it changes with use, and the server's copy is
            // always the truthful one.
        }
    }

    /// Rebuilds a catalog entry from the stored record, for a favourite the catalog never paged in.
    pub fn to_summary(&self) -> GameSummary {
        GameSummary {
            app_id: self.app_id.clone(),
            title: self.title.clone(),
            cover_url: self.cover_url.clone(),
            store: self.store.clone(),
            last_played: None,
            search_key: self.title.to_lowercase(),
        }
    }
}

/// Reads the starred games, skipping any line that will not parse.
pub fn load() -> Vec<FavoriteGame> {
    let Ok(text) = std::fs::read_to_string(STORE_PATH) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| match serde_json::from_str::<FavoriteGame>(line) {
            Ok(favorite) => Some(favorite),
            Err(error) => {
                eprintln!("Skipping unreadable favorite: {error}");
                None
            }
        })
        .collect()
}

fn save(favorites: &[FavoriteGame]) {
    if std::fs::create_dir_all(STORE_DIR).is_err() {
        return;
    }
    let contents = favorites
        .iter()
        .filter_map(|favorite| serde_json::to_string(favorite).ok())
        .collect::<Vec<_>>()
        .join("\n");
    if let Err(error) = std::fs::write(STORE_PATH, contents) {
        eprintln!("Could not persist favorites: {error}");
    }
}

/// Stars or unstars one game, returning the updated list.
pub fn toggle(game: &GameSummary) -> Vec<FavoriteGame> {
    let mut favorites = load();
    match favorites
        .iter()
        .position(|favorite| favorite.app_id == game.app_id)
    {
        Some(index) => {
            favorites.remove(index);
        }
        // Newest first, so the most recently starred game leads the list.
        None => favorites.insert(0, FavoriteGame::from_summary(game)),
    }
    save(&favorites);
    favorites
}

/// The starred ids, for the per-frame "is this one starred?" check the list makes on every row.
pub fn ids(favorites: &[FavoriteGame]) -> std::collections::BTreeSet<String> {
    favorites
        .iter()
        .map(|favorite| favorite.app_id.clone())
        .collect()
}
