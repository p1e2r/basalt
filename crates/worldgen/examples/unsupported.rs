//! List configured feature / placement types that are not implemented yet (`unsupported [nether]`).
fn main() {
    let nether = std::env::args().nth(1).as_deref() == Some("nether");
    let g = if nether {
        basalt_worldgen::Generator::new_dim(12345, basalt_worldgen::Dimension::Nether)
    } else {
        basalt_worldgen::Generator::new(12345)
    };
    if std::env::args().nth(2).as_deref() == Some("steps") {
        for (i, st) in g.feature_steps().iter().enumerate() {
            println!("step {i}: {}", st.join(" "));
        }
    }
    for (k, n) in g.unsupported_features() {
        println!("{n:5} {k}");
    }
}
