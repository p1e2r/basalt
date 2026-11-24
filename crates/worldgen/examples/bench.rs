//! Time each generation stage: cargo run -p basalt-worldgen --release --example bench [n]
use basalt_worldgen::{Generator, Stage};
use std::time::Instant;

fn main() {
    let n: i32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(25);
    let g = Generator::new(12345);
    for stage in [
        Stage::Noise,
        Stage::Surface,
        Stage::Carvers,
        Stage::Features,
    ] {
        let t = Instant::now();
        for i in 0..n {
            let _ = g.generate_stages(i % 5, i / 5, stage);
        }
        println!(
            "{stage:?}: {:.1} ms/chunk (cumulative)",
            t.elapsed().as_secs_f64() * 1000.0 / n as f64
        );
    }
}
