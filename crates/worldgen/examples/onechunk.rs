//! Generate one chunk (for debugging): onechunk cx cz
fn main() {
    let a: Vec<i32> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    let g = basalt_worldgen::Generator::new(12345);
    let _ = g.generate_chunk(a[0], a[1]);
}
