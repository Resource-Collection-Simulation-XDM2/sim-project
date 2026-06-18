use anyhow::Result;
use rand::Rng;
use resource_collection_simulation::app::App;
use resource_collection_simulation::map::{MapPreset, VisualTheme};
use resource_collection_simulation::simulation::RevealMode;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);

    // Arg 1: map preset (default: "default").
    let preset: MapPreset = args.next().and_then(|s| s.parse().ok()).unwrap_or(MapPreset::Default);

    // Arg 2: visual theme (default: "default").
    let visual: VisualTheme = args.next().and_then(|s| s.parse().ok()).unwrap_or(VisualTheme::DEFAULT);

    // Arg 3: seed (number) or reveal mode ("reveal" / "nofog").
    let arg3 = args.next();
    let (seed, reveal_mode) = match arg3.as_deref() {
        Some("reveal") => (rand::rng().random(), RevealMode::Full),
        Some("nofog") => (rand::rng().random(), RevealMode::TerrainOnly),
        Some(s) => {
            if let Ok(n) = s.parse::<u64>() {
                let mode = match args.next().as_deref() {
                    Some("reveal") => RevealMode::Full,
                    Some("nofog") => RevealMode::TerrainOnly,
                    _ => RevealMode::Normal,
                };
                (n, mode)
            } else {
                (rand::rng().random(), RevealMode::Normal)
            }
        }
        None => (rand::rng().random(), RevealMode::Normal),
    };

    let mut app = App::new(seed, preset, visual, reveal_mode).await?;
    app.run().await
}
