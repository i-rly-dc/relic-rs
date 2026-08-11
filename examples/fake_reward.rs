use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use image::{Rgba, RgbaImage};

const W: u32 = 1920;
const H: u32 = 1080;
const SLOT_W: f32 = 242.0;
const FONT: &str = "/usr/share/fonts/liberation/LiberationSans-Bold.ttf";

const REWARDS: [&[&str]; 4] = [
    &["AKSTILETTO PRIME", "BLUEPRINT"],
    &["BRATON PRIME BARREL"],
    &["FORMA BLUEPRINT"],
    &["SOMA PRIME STOCK"],
];

fn main() {
    let count: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(4)
        .clamp(1, 4);
    let data = std::fs::read(FONT).expect("font not found");
    let font = FontRef::try_from_slice(&data).expect("bad font");
    let mut img = RgbaImage::from_pixel(W, H, Rgba([18, 16, 20, 255]));

    let scale = PxScale::from(22.0);
    let strip_w = SLOT_W * count as f32;
    for (slot, lines) in REWARDS.iter().take(count).enumerate() {
        let cx = (W as f32 - strip_w) / 2.0 + SLOT_W * (slot as f32 + 0.5);
        for (li, line) in lines.iter().enumerate() {
            let y = 0.385 * H as f32 + li as f32 * 24.0;
            draw_text_centered(&mut img, &font, scale, line, cx, y);
        }
    }
    img.save("/tmp/fake_reward.png").expect("save failed");
    println!("wrote /tmp/fake_reward.png ({count} rewards)");
}

fn draw_text_centered(img: &mut RgbaImage, font: &FontRef, scale: PxScale, text: &str, cx: f32, top: f32) {
    let sf = font.as_scaled(scale);
    let width: f32 = text
        .chars()
        .map(|c| sf.h_advance(sf.glyph_id(c)))
        .sum();
    let mut x = cx - width / 2.0;
    let baseline = top + sf.ascent();
    for c in text.chars() {
        let gid = sf.glyph_id(c);
        let glyph = gid.with_scale_and_position(scale, ab_glyph::point(x, baseline));
        x += sf.h_advance(gid);
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px >= 0 && py >= 0 && (px as u32) < W && (py as u32) < H && cov > 0.05 {
                    let v = (255.0 * cov) as u8;
                    let bg = img.get_pixel(px as u32, py as u32).0;
                    let blend = |b: u8| b.saturating_add(((v as u16 * (255 - b as u16)) / 255) as u8);
                    img.put_pixel(
                        px as u32,
                        py as u32,
                        Rgba([blend(bg[0]), blend(bg[1]), blend(bg[2]), 255]),
                    );
                }
            });
        }
    }
}
