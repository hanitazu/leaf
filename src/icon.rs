//! 16×16 Pixel-Blatt – wird für das Fenster-Icon und (über build.rs) für die .exe genutzt.

const PIXELS: [&str; 16] = [
    "................",
    "..........#####.",
    "........##LLLL#.",
    "......##LLLLGG#.",
    ".....#LLLLGGGG#.",
    "....#LLLGGGGVG#.",
    "....#LLGGGGVGG#.",
    "...#LLGGGGVGG#..",
    "...#LGGGGVGGG#..",
    "...#LGGGVGGG#...",
    "...#GGGVGGG#....",
    "...#GGVGG##.....",
    "....#VG##.......",
    "...#V##.........",
    "..#V#...........",
    ".##.............",
];

fn color(c: u8) -> [u8; 4] {
    match c {
        b'#' => [20, 83, 45, 255],
        b'L' => [134, 239, 172, 255],
        b'G' => [34, 197, 94, 255],
        b'V' => [21, 128, 61, 255],
        _ => [0, 0, 0, 0],
    }
}

/// RGBA-Pixel (oben nach unten), nearest-neighbour hochskaliert.
pub fn rgba(size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        let row = PIXELS[y * 16 / size].as_bytes();
        for x in 0..size {
            out.extend_from_slice(&color(row[x * 16 / size]));
        }
    }
    out
}
