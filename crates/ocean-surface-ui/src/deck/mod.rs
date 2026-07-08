//! Context Deck — right-side reveal-on-intent panels (see
//! docs/OCEAN_DESKTOP_NORTH_STAR.md). Each panel is a self-contained module;
//! mounting into the app shell is integrator-owned (app.rs), never done here.
//! Panel styling uses `deck-*` class names consolidated in styles/deck.css.

pub mod browser;
pub mod files;
pub mod repo;

/// Which deck panel is revealed. The shell shows at most ONE panel at a
/// time (reveal-on-intent — a permanent multi-panel sidebar is exactly the
/// control sprawl the design doc forbids).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeckPanel {
    Files,
    Repo,
    Browser,
}

impl DeckPanel {
    /// Title shown in the deck bar.
    pub fn title(self) -> &'static str {
        match self {
            DeckPanel::Files => "Files",
            DeckPanel::Repo => "Repo",
            DeckPanel::Browser => "Browser",
        }
    }
}
