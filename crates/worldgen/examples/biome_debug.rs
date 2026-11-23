//! Print the closest biome parameter entries at a quart position: biome_debug qx qy qz
fn main() {
    let a: Vec<i32> = std::env::args().skip(1).map(|s| s.parse().unwrap()).collect();
    let g = basalt_worldgen::Generator::new(12345);
    g.debug_biome(a[0], a[1], a[2]);
}
