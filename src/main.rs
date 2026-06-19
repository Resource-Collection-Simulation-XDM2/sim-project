use anyhow::Result;
use resource_collection_simulation::app::App;
use resource_collection_simulation::map::{MapPreset, VisualTheme};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);

    // Arg 1: map preset (default: "default").
    let preset: MapPreset = args.next().and_then(|s| s.parse().ok()).unwrap_or(MapPreset::Default);

    // Arg 2: visual theme (default: "default").
    let visual: VisualTheme = args.next().and_then(|s| s.parse().ok()).unwrap_or(VisualTheme::DEFAULT);

    // Arg 3: seed (default: 1).
    let seed: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);

    let mut app = App::new(seed, preset, visual)?;
    app.run()
}
