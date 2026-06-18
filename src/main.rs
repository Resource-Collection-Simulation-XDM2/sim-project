use anyhow::Result;
use resource_collection_simulation::app::App;

#[tokio::main]
async fn main() -> Result<()> {
    // Use a fixed seed for deterministic maps; change to random for variety.
    let seed: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let mut app = App::new(seed)?;
    app.run()
}
