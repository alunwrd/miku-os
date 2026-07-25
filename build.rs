use std::path::Path;

fn main() {
    generate_miku_libs_table();

    let font_path = Path::new("assets/JetBrainsMono-Regular.ttf");
    if !font_path.exists() {
        println!("cargo:warning=JetBrainsMono-Regular.ttf not found, using fallback");
        generate_empty_font();
        return;
    }

    let font_data = std::fs::read(font_path).unwrap();
    let font = fontdue::Font::from_bytes(
        font_data,
        fontdue::FontSettings {
            scale: 16.0,
            ..Default::default()
        },
    )
    .unwrap();

    let mut output = String::new();
    output.push_str("pub const CHAR_WIDTH: usize = 9;\n");
    output.push_str("pub const CHAR_HEIGHT: usize = 16;\n");
    output.push_str("pub const GLYPH_COUNT: usize = 95;\n\n");
    output.push_str("pub static GLYPHS: [([u8; CHAR_WIDTH * CHAR_HEIGHT], u8); GLYPH_COUNT] = [\n");

    for c in 0x20u8..=0x7Eu8 {
        let ch = c as char;
        let (metrics, bitmap) = font.rasterize(ch, 16.0);

        let mut glyph = [0u8; 9 * 16];

        let x_offset = if metrics.width < 9 {
            (9 - metrics.width) / 2
        } else {
            0
        };
        let y_offset = if metrics.height < 16 {
            let baseline = 13i32;
            let top = baseline - metrics.ymin as i32 - metrics.height as i32;
            top.max(0) as usize
        } else {
            0
        };

        for row in 0..metrics.height.min(16) {
            for col in 0..metrics.width.min(9) {
                let dst_y = row + y_offset;
                let dst_x = col + x_offset;
                if dst_y < 16 && dst_x < 9 {
                    glyph[dst_y * 9 + dst_x] = bitmap[row * metrics.width + col];
                }
            }
        }

        let width = metrics.advance_width.round() as u8;
        output.push_str(&format!("    ({:?}, {}),\n", glyph, width.max(9)));
    }

    output.push_str("];\n\n");
    output.push_str("#[inline]\n");
    output.push_str(
        "pub fn get_glyph(c: char) -> Option<&'static ([u8; CHAR_WIDTH * CHAR_HEIGHT], u8)> {\n",
    );
    output.push_str("    let idx = c as usize;\n");
    output.push_str("    if idx >= 0x20 && idx <= 0x7E {\n");
    output.push_str("        Some(&GLYPHS[idx - 0x20])\n");
    output.push_str("    } else {\n");
    output.push_str("        None\n");
    output.push_str("    }\n");
    output.push_str("}\n");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("font_data.rs");
    std::fs::write(dest, output).unwrap();

    println!("cargo:rerun-if-changed=assets/JetBrainsMono-Regular.ttf");
    println!("cargo:rerun-if-changed=build.rs");
}

// Generate the MIKU_LIBS preload table (src/ldso.rs includes it) from the
// shared library manifest, so the kernel can never go out of sync with the
// set of libraries the builder produces.
fn generate_miku_libs_table() {
    let list = Path::new("src/lib/mikulibs/libs.list");
    let src = std::fs::read_to_string(list)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", list.display()));

    let mut output = String::from("pub static MIKU_LIBS: &[MikuLib] = &[\n");
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let name = line.split(':').next().unwrap().trim();
        output.push_str(&format!(
            "    MikuLib {{ name: \"{name}.so\", bytes: include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/src/lib/mikulibs/libs/{name}.so\")) }},\n"
        ));
    }
    output.push_str("];\n");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(Path::new(&out_dir).join("miku_libs.rs"), output).unwrap();
    println!("cargo:rerun-if-changed=src/lib/mikulibs/libs.list");
}

fn generate_empty_font() {
    let mut output = String::new();
    output.push_str("pub const CHAR_WIDTH: usize = 9;\n");
    output.push_str("pub const CHAR_HEIGHT: usize = 16;\n");
    output.push_str("pub const GLYPH_COUNT: usize = 0;\n\n");
    output.push_str("pub static GLYPHS: [([u8; CHAR_WIDTH * CHAR_HEIGHT], u8); 0] = [];\n\n");
    output.push_str("#[inline]\n");
    output.push_str(
        "pub fn get_glyph(_c: char) -> Option<&'static ([u8; CHAR_WIDTH * CHAR_HEIGHT], u8)> {\n",
    );
    output.push_str("    None\n");
    output.push_str("}\n");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = std::path::Path::new(&out_dir).join("font_data.rs");
    std::fs::write(dest, output).unwrap();
}
