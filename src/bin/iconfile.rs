//! sweepmac-iconfile — emit the app icon as a PNG (used by the packaging script
//! to build the .icns). Usage: `sweepmac-iconfile <out.png> [size]`.

use std::fs::File;
use std::io::BufWriter;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| {
        eprintln!("usage: sweepmac-iconfile <out.png> [size]");
        std::process::exit(2);
    });
    let size: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1024);

    let rgba = sweepmac::broom_rgba(size, false);
    let file = File::create(&out).expect("create png");
    let w = BufWriter::new(file);
    let mut encoder = png::Encoder::new(w, size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&rgba)
        .expect("png data");
    println!("wrote {out} ({size}x{size})");
}
