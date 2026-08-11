use image::{Rgb, RgbImage};

fn main() {
    let mut a = std::env::args().skip(1);
    let (inp, out) = (a.next().expect("in"), a.next().expect("out"));
    let img = image::open(&inp).expect("open").to_rgb8();
    let (w, h) = img.dimensions();
    let mut o = RgbImage::new(w, h);
    for (x, y, p) in img.enumerate_pixels() {
        let [r, g, b] = p.0;
        let (r_i, g_i, b_i) = (r as i32, g as i32, b as i32);

        let is_text = r_i > 190 && (70..=190).contains(&g_i) && b_i < 110 && r_i - b_i > 120;
        let v = if is_text { r } else { 0 };
        o.put_pixel(x, y, Rgb([v, v, v]));
    }
    o.save(&out).expect("save");
    println!("wrote {out}");
}
