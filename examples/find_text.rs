use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use rten::Model;

static DETECTION: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/models/text-detection.rten"));
static RECOGNITION: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/models/text-recognition.rten"));

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: find_text <image> [x y w h]");
    let f: Vec<f32> = args.map(|a| a.parse().expect("bad fraction")).collect();
    let (fx, fy, fw, fh) = if f.len() == 4 {
        (f[0], f[1], f[2], f[3])
    } else {
        (0.0, 0.0, 1.0, 1.0)
    };

    let img = image::open(&path).expect("open image").to_rgb8();
    let (w, h) = img.dimensions();
    let (cx, cy) = ((fx * w as f32) as u32, (fy * h as f32) as u32);
    let (cw, ch) = ((fw * w as f32) as u32, (fh * h as f32) as u32);
    let crop = image::imageops::crop_imm(&img, cx, cy, cw.min(w - cx), ch.min(h - cy)).to_image();

    let engine = OcrEngine::new(OcrEngineParams {
        detection_model: Some(Model::load_static_slice(DETECTION).unwrap()),
        recognition_model: Some(Model::load_static_slice(RECOGNITION).unwrap()),
        ..Default::default()
    })
    .unwrap();
    let source = ImageSource::from_bytes(crop.as_raw(), crop.dimensions()).unwrap();
    let input = engine.prepare_input(source).unwrap();
    let words = engine.detect_words(&input).unwrap();
    let lines = engine.find_text_lines(&input, &words);
    for line in engine.recognize_text(&input, &lines).unwrap().iter().flatten() {
        let r = line.bounding_rect();
        let (lx, ly) = (cx as f32 + r.left() as f32, cy as f32 + r.top() as f32);
        println!(
            "\"{}\"  px({},{} {}x{})  frac x={:.4} y={:.4} w={:.4} h={:.4}",
            line,
            lx as u32,
            ly as u32,
            r.width(),
            r.height(),
            lx / w as f32,
            ly / h as f32,
            r.width() as f32 / w as f32,
            r.height() as f32 / h as f32,
        );
    }
}
