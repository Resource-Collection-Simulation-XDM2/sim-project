use std::str::FromStr;

use ratatui::style::Color;

use crate::world::{BiomeId, ResourceKind, WorldConfig};

// ---------------------------------------------------------------------------
// Generation presets — control how the world is generated
// ---------------------------------------------------------------------------

/// Pre-built world-generation presets that tweak Perlin-noise parameters.
#[derive(Debug, Clone, Copy)]
pub enum MapPreset {
    /// Balanced mix of open space and obstacles.
    Default,
    /// Tight winding passages and dead-ends.
    Cavern,
    /// Many small obstacle clusters scattered like tree groves.
    Forest,
    /// Large impassable water bodies with narrow land bridges.
    Archipelago,
    /// Wide-open terrain with only sparse rocks.
    Plains,
    /// Small open map for fast integration tests.
    Compact,
}

impl MapPreset {
    /// Convert a preset into its [`WorldConfig`].
    pub fn config(&self) -> WorldConfig {
        let base = WorldConfig::default();
        match self {
            Self::Default => base,
            Self::Cavern => WorldConfig {
                obstacle_threshold: -0.08,
                obstacle_frequency: 0.04,
                ..base
            },
            Self::Forest => WorldConfig {
                obstacle_threshold: 0.10,
                obstacle_frequency: 0.12,
                ..base
            },
            Self::Archipelago => WorldConfig {
                obstacle_threshold: -0.12,
                obstacle_frequency: 0.03,
                ..base
            },
            Self::Plains => WorldConfig {
                obstacle_threshold: 0.35,
                obstacle_frequency: 0.05,
                ..base
            },
            Self::Compact => WorldConfig {
                width: 32,
                height: 20,
                energy_nodes: 4,
                crystal_nodes: 4,
                obstacle_threshold: 0.35,
                obstacle_frequency: 0.05,
                base_safety_radius: 3,
            },
        }
    }
}

impl FromStr for MapPreset {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "default" | "d" => Ok(Self::Default),
            "cavern" | "cave" | "c" => Ok(Self::Cavern),
            "forest" | "f" => Ok(Self::Forest),
            "archipelago" | "archi" | "a" => Ok(Self::Archipelago),
            "plains" | "open" | "p" => Ok(Self::Plains),
            "compact" | "small" => Ok(Self::Compact),
            other => Err(format!(
                "unknown preset '{other}'. try: default, cavern, forest, archipelago, plains, compact"
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Visual themes — control how the world is displayed
// ---------------------------------------------------------------------------

/// Controls the characters and colors used to render every element on the map.
///
/// Obstacle characters are drawn from a palette (with biome-based variety)
/// so the same logical `Cell::Obstacle` can appear as different glyphs
/// in coherent terrain patches (lakes, forests, mountains).
#[derive(Debug, Clone)]
pub struct VisualTheme {
    pub name: &'static str,

    // Character palettes — obstacles use multiple entries for terrain variety.
    obstacle_palette: &'static [&'static str],
    pub energy_char: &'static str,
    pub crystal_char: &'static str,
    pub base_char: &'static str,
    pub scout_char: &'static str,
    pub collector_char: &'static str,
    pub ground_char: &'static str,
    pub fog_char: &'static str,

    // Color palette.
    pub obstacle_color: Color,
    pub energy_color: Color,
    pub crystal_color: Color,
    pub base_color: Color,
    pub scout_color: Color,
    pub collector_color: Color,
    pub ground_color: Color,
    pub fog_color: Color,

    /// How many terminal columns each map cell occupies.
    /// `1` for single-width glyphs (default, retro, symbols).
    /// `2` for double-width emojis — single-width chars are padded with a space
    /// so the grid never shifts.
    pub cell_width: u8,

    /// Per-glyph colors for obstacle variety (same length as `obstacle_palette`).
    /// If empty, falls back to `obstacle_color` for all obstacle cells.
    obstacle_color_palette: &'static [Color],
}

impl VisualTheme {
    /// The default theme matching the project spec.
    pub const DEFAULT: Self = Self {
        name: "default",
        obstacle_palette: &["O"],
        energy_char: "E",
        crystal_char: "C",
        base_char: "#",
        scout_char: "x",
        collector_char: "o",
        ground_char: " ",
        fog_char: ".",
        obstacle_color: Color::LightCyan,
        energy_color: Color::Green,
        crystal_color: Color::LightMagenta,
        base_color: Color::LightGreen,
        scout_color: Color::Red,
        collector_color: Color::Magenta,
        ground_color: Color::DarkGray,
        fog_color: Color::Black,
        cell_width: 1,
        obstacle_color_palette: &[],
    };

    /// Retro ASCII theme — uses uppercase letters only.
    pub const RETRO: Self = Self {
        name: "retro",
        obstacle_palette: &["#", "%", "&", "@"],
        energy_char: "E",
        crystal_char: "C",
        base_char: "H",
        scout_char: "S",
        collector_char: "R",
        ground_char: " ",
        fog_char: "·",
        obstacle_color: Color::White,
        energy_color: Color::Yellow,
        crystal_color: Color::Cyan,
        base_color: Color::Green,
        scout_color: Color::Red,
        collector_color: Color::Blue,
        ground_color: Color::DarkGray,
        fog_color: Color::Black,
        cell_width: 1,
        obstacle_color_palette: &[Color::White, Color::Yellow, Color::LightRed, Color::LightCyan],
    };

    /// Single-width Unicode symbols — safe for any terminal, no grid shifting.
    pub const SYMBOLS: Self = Self {
        name: "symbols",
        obstacle_palette: &["◆", "▲", "■", "●"],
        energy_char: "⚡",
        crystal_char: "♦",
        base_char: "⌂",
        scout_char: "●",
        collector_char: "○",
        ground_char: "·",
        fog_char: "░",
        obstacle_color: Color::Gray,
        energy_color: Color::Yellow,
        crystal_color: Color::LightMagenta,
        base_color: Color::LightGreen,
        scout_color: Color::Red,
        collector_color: Color::LightBlue,
        ground_color: Color::DarkGray,
        fog_color: Color::Rgb(30, 30, 40),
        cell_width: 1,
        obstacle_color_palette: &[Color::Gray, Color::LightGreen, Color::LightCyan, Color::LightRed],
    };

    /// Full-color emoji theme — renders real emojis (🪨 🌳 💧).
    ///
    /// Each cell occupies **2 terminal columns**. Single-width characters
    /// are padded with a space so the grid never shifts. Your terminal needs
    /// to be wide enough (`96 × 2 = 192` columns for the default map) or use
    /// a narrower preset like `cavern` with a smaller font.
    pub const EMOJI: Self = Self {
        name: "emoji",
        obstacle_palette: &["🪨", "🌳", "💧"],
        energy_char: "⚡",
        crystal_char: "💎",
        base_char: "🏠",
        scout_char: "🔍",
        collector_char: "🤖",
        ground_char: "·",
        fog_char: "░",
        obstacle_color: Color::Gray,
        energy_color: Color::Yellow,
        crystal_color: Color::LightMagenta,
        base_color: Color::LightGreen,
        scout_color: Color::Red,
        collector_color: Color::LightBlue,
        ground_color: Color::DarkGray,
        fog_color: Color::Rgb(30, 30, 40),
        cell_width: 2,
        obstacle_color_palette: &[Color::Gray, Color::Green, Color::LightBlue],
    };

    /// Pick an obstacle character based on biome identifier for terrain variety.
    ///
    /// The biome value (0..=255) maps to palette entries, producing coherent
    /// terrain patches — low values for water/wet terrain, mid values for
    /// vegetation/forest, high values for rock/mountain.
    pub fn obstacle_char(&self, biome: BiomeId) -> &'static str {
        if self.obstacle_palette.is_empty() {
            return "O";
        }
        let idx = (biome as usize).wrapping_mul(self.obstacle_palette.len()) >> 8;
        self.obstacle_palette[idx]
    }

    /// Biome-aware color for the obstacle at the given biome identifier.
    pub fn obstacle_color_for(&self, biome: BiomeId) -> Color {
        if self.obstacle_color_palette.is_empty() {
            return self.obstacle_color;
        }
        let idx = (biome as usize).wrapping_mul(self.obstacle_color_palette.len()) >> 8;
        self.obstacle_color_palette[idx]
    }

    /// Character + color for a resource of the given kind.
    pub fn resource_display(&self, kind: ResourceKind) -> (&'static str, Color) {
        match kind {
            ResourceKind::Energy => (self.energy_char, self.energy_color),
            ResourceKind::Crystal => (self.crystal_char, self.crystal_color),
        }
    }
}

// ---------------------------------------------------------------------------
// Lookup helpers for CLI
// ---------------------------------------------------------------------------

impl FromStr for VisualTheme {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "default" | "d" => Ok(Self::DEFAULT),
            "retro" | "r" => Ok(Self::RETRO),
            "symbols" | "sym" | "s" => Ok(Self::SYMBOLS),
            "emoji" | "e" => Ok(Self::EMOJI),
            other => Err(format!(
                "unknown visual theme '{other}'. try: default, retro, symbols, emoji"
            )),
        }
    }
}
