fn main() {
    let mut a = std::env::args().skip(1);
    let (inp, out) = (a.next().expect("in"), a.next().expect("out"));
    let f: Vec<f32> = a.map(|s| s.parse().expect("num")).collect();
    let (x, y, w, h) = if f.len() >= 4 { (f[0], f[1], f[2], f[3]) } else { (0.0, 0.0, 1.0, 1.0) };
    let ow = f.get(4).copied().unwrap_or(1400.0) as u32;

    let img = image::open(&inp).expect("open").to_rgb8();
    let (iw, ih) = img.dimensions();
    let (cx, cy) = ((x * iw as f32) as u32, (y * ih as f32) as u32);
    let (cw, ch) = (
        ((w * iw as f32) as u32).min(iw - cx),
        ((h * ih as f32) as u32).min(ih - cy),
    );
    let crop = image::imageops::crop_imm(&img, cx, cy, cw, ch).to_image();
    let scale = ow as f32 / cw as f32;
    let resized = image::imageops::resize(
        &crop,
        (cw as f32 * scale) as u32,
        (ch as f32 * scale).max(1.0) as u32,
        image::imageops::FilterType::CatmullRom,
    );
    resized.save(&out).expect("save");
    println!("{out}: {}x{} from px({cx},{cy} {cw}x{ch})", resized.width(), resized.height());
}
