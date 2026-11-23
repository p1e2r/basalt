//! List configured feature / placement types that are not implemented yet.
fn main() {
    let g = basalt_worldgen::Generator::new(12345);
    for (k, n) in g.unsupported_features() {
        println!("{n:5} {k}");
    }
}
