//! Minimal `std` program: `println!`, `args()`, and `var()`

fn main() {
    println!("hello from std on MikuOS");

    // Owned `String`s, which means the allocator and the formatter
    let mut names: Vec<String> = Vec::new();
    for (i, a) in std::env::args().enumerate() {
        names.push(format!("argv[{i}] = {a}"));
    }
    println!("{} argument(s)", names.len());
    for n in &names {
        println!("  {n}");
    }

    // Environment through `std`
    match std::env::var("PATH") {
        Ok(v) => println!("PATH = {v}"),
        Err(e) => println!("PATH unavailable: {e}"),
    }

    eprintln!("and this goes to stderr");
}
