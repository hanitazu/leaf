// Erzeugt leaf.ico aus dem Pixel-Blatt und bettet es in die .exe ein.
#[path = "src/icon.rs"]
#[allow(dead_code)]
mod icon;

fn ico() -> Vec<u8> {
    let sizes = [16usize, 24, 32, 48, 64, 128, 256];
    let mut images = Vec::new();
    for &s in &sizes {
        let px = icon::rgba(s);
        let mask_row = s.div_ceil(32) * 4;
        let mut d = Vec::new();
        // BITMAPINFOHEADER
        for v in [40u32, s as u32, (s * 2) as u32] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&32u16.to_le_bytes());
        for v in [0u32, (s * s * 4 + mask_row * s) as u32, 0, 0, 0, 0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        for y in (0..s).rev() {
            for x in 0..s {
                let i = (y * s + x) * 4;
                d.extend_from_slice(&[px[i + 2], px[i + 1], px[i], px[i + 3]]);
            }
        }
        d.extend(std::iter::repeat_n(0u8, mask_row * s));
        images.push((s, d));
    }
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (s, d) in &images {
        let b = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[b, b, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(d.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += d.len();
    }
    for (_, d) in &images {
        out.extend_from_slice(d);
    }
    out
}

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("leaf.ico");
    std::fs::write(&out, ico()).unwrap();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(out.to_str().unwrap());
        res.set("FileDescription", "Leaf");
        res.set("ProductName", "Leaf");
        if let Err(e) = res.compile() {
            println!("cargo:warning=Icon konnte nicht eingebettet werden: {e}");
        }
    }
}
