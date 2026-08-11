use anyhow::{Context, Result};
use image::RgbImage;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;
use rten_imageproc::{BoundingRect, PointF, RotatedRect, Vec2};

static DETECTION: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/models/text-detection.rten"));
static RECOGNITION: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/models/text-recognition.rten"));

const COLUMN_GAP_FACTOR: f32 = 1.5;

const ROW_OVERLAP: f32 = 0.35;

const MIN_NAME_LEN: usize = 5;

pub struct Ocr {
    engine: OcrEngine,
}

impl Ocr {
    pub fn new() -> Result<Ocr> {
        let detection =
            Model::load_static_slice(DETECTION).context("loading text detection model")?;
        let recognition =
            Model::load_static_slice(RECOGNITION).context("loading text recognition model")?;
        let engine = OcrEngine::new(OcrEngineParams {
            detection_model: Some(detection),
            recognition_model: Some(recognition),
            ..Default::default()
        })
        .map_err(|e| anyhow::anyhow!("creating OCR engine: {e}"))?;
        Ok(Ocr { engine })
    }

    pub fn text(&self, img: &RgbImage) -> Result<String> {
        let source = ImageSource::from_bytes(img.as_raw(), img.dimensions())
            .map_err(|e| anyhow::anyhow!("preparing OCR input: {e}"))?;
        let input = self
            .engine
            .prepare_input(source)
            .map_err(|e| anyhow::anyhow!("preprocessing OCR input: {e}"))?;
        let text = self
            .engine
            .get_text(&input)
            .map_err(|e| anyhow::anyhow!("running OCR: {e}"))?;
        Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
    }

    pub fn read_names(&self, img: &RgbImage) -> Result<Vec<String>> {
        let source = ImageSource::from_bytes(img.as_raw(), img.dimensions())
            .map_err(|e| anyhow::anyhow!("preparing OCR input: {e}"))?;
        let input = self
            .engine
            .prepare_input(source)
            .map_err(|e| anyhow::anyhow!("preprocessing OCR input: {e}"))?;
        let words = self
            .engine
            .detect_words(&input)
            .map_err(|e| anyhow::anyhow!("detecting text: {e}"))?;

        let words = split_tall_boxes(&words);

        if std::env::var_os("RELIC_CHECK_DEBUG_BOXES").is_some() {
            for (i, col) in split_columns(&words).iter().enumerate() {
                eprintln!("column {}: {} words, {} rows", i + 1, col.len(), split_rows(col).len());
                for w in col {
                    let r = w.bounding_rect();
                    eprintln!(
                        "    x {:.0}..{:.0}  y {:.0}..{:.0}  h {:.0}",
                        r.left(),
                        r.right(),
                        r.top(),
                        r.bottom(),
                        r.height()
                    );
                }
            }
        }

        let mut names = Vec::new();
        for column in split_columns(&words) {
            let mut parts: Vec<String> = Vec::new();
            for row in split_rows(&column) {
                let lines = self.engine.find_text_lines(&input, &row);
                let recognized = self
                    .engine
                    .recognize_text(&input, &lines)
                    .map_err(|e| anyhow::anyhow!("recognizing text: {e}"))?;
                parts.extend(recognized.iter().flatten().map(|line| line.to_string()));
            }
            let text = parts.join(" ");
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if text.chars().filter(|c| c.is_alphanumeric()).count() >= MIN_NAME_LEN {
                names.push(text);
            }
        }
        Ok(names)
    }
}

fn split_columns(words: &[RotatedRect]) -> Vec<Vec<RotatedRect>> {
    if words.is_empty() {
        return Vec::new();
    }
    let threshold = COLUMN_GAP_FACTOR * median_height(words);

    let mut sorted: Vec<RotatedRect> = words.to_vec();
    sorted.sort_by(|a, b| a.bounding_rect().left().total_cmp(&b.bounding_rect().left()));

    let mut columns: Vec<Vec<RotatedRect>> = Vec::new();
    let mut current: Vec<RotatedRect> = Vec::new();
    let mut max_right = f32::MIN;
    for word in sorted {
        let rect = word.bounding_rect();
        if !current.is_empty() && rect.left() - max_right > threshold {
            columns.push(std::mem::take(&mut current));
            max_right = f32::MIN;
        }
        max_right = max_right.max(rect.right());
        current.push(word);
    }
    if !current.is_empty() {
        columns.push(current);
    }
    columns
}

fn median_height(words: &[RotatedRect]) -> f32 {
    let mut heights: Vec<f32> = words.iter().map(|w| w.bounding_rect().height()).collect();
    heights.sort_by(f32::total_cmp);
    heights.get(heights.len() / 2).copied().unwrap_or(1.0).max(1.0)
}

fn split_tall_boxes(words: &[RotatedRect]) -> Vec<RotatedRect> {
    let median = median_height(words);
    words
        .iter()
        .flat_map(|word| {
            let rect = word.bounding_rect();
            let rows = (rect.height() / median).round().max(1.0) as usize;
            if rows < 2 {
                return vec![*word];
            }
            let slice = rect.height() / rows as f32;
            (0..rows)
                .map(|i| {
                    RotatedRect::new(
                        PointF::from_yx(
                            rect.top() + (i as f32 + 0.5) * slice,
                            (rect.left() + rect.right()) / 2.0,
                        ),
                        Vec2::from_yx(-1.0, 0.0),
                        rect.width(),
                        slice,
                    )
                })
                .collect()
        })
        .collect()
}

fn split_rows(words: &[RotatedRect]) -> Vec<Vec<RotatedRect>> {
    if words.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<RotatedRect> = words.to_vec();
    sorted.sort_by(|a, b| a.bounding_rect().top().total_cmp(&b.bounding_rect().top()));

    let mut rows: Vec<Vec<RotatedRect>> = Vec::new();
    let mut current: Vec<RotatedRect> = Vec::new();
    let (mut top, mut bottom) = (f32::MAX, f32::MIN);
    for word in sorted {
        let rect = word.bounding_rect();
        if !current.is_empty() {
            let overlap = bottom.min(rect.bottom()) - top.max(rect.top());
            let shorter = rect.height().min(bottom - top).max(1.0);
            if overlap / shorter < ROW_OVERLAP {
                rows.push(std::mem::take(&mut current));
                (top, bottom) = (f32::MAX, f32::MIN);
            }
        }
        top = top.min(rect.top());
        bottom = bottom.max(rect.bottom());
        current.push(word);
    }
    if !current.is_empty() {
        rows.push(current);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{split_columns, split_rows, split_tall_boxes};
    use rten_imageproc::{PointF, RotatedRect, Vec2};

    fn word(x0: f32, x1: f32, h: f32) -> RotatedRect {
        RotatedRect::new(
            PointF::from_yx(h / 2.0, (x0 + x1) / 2.0),
            Vec2::from_yx(-1.0, 0.0),
            x1 - x0,
            h,
        )
    }

    fn three_cards() -> Vec<RotatedRect> {
        vec![
            word(0.0, 30.0, 20.0),
            word(38.0, 100.0, 20.0),
            word(108.0, 160.0, 20.0),
            word(240.0, 290.0, 20.0),
            word(298.0, 380.0, 20.0),
            word(460.0, 500.0, 20.0),
            word(508.0, 570.0, 20.0),
        ]
    }

    #[test]
    fn splits_at_card_gaps_not_word_gaps() {
        let columns = split_columns(&three_cards());
        assert_eq!(columns.len(), 3);
        assert_eq!(columns.iter().map(Vec::len).collect::<Vec<_>>(), [3, 2, 2]);
    }

    #[test]
    fn derives_reward_count_without_assuming_four() {
        assert_eq!(split_columns(&[]).len(), 0);
        assert_eq!(split_columns(&three_cards()[..3]).len(), 1);
        assert_eq!(split_columns(&three_cards()[..5]).len(), 2);
    }

    #[test]
    fn threshold_scales_with_resolution() {
        let scaled: Vec<RotatedRect> = three_cards()
            .iter()
            .map(|r| {
                let b = <RotatedRect as rten_imageproc::BoundingRect>::bounding_rect(r);
                word(b.left() * 4.0, b.right() * 4.0, b.height() * 4.0)
            })
            .collect();
        assert_eq!(split_columns(&scaled).len(), 3);
    }

    #[test]
    fn wrapped_name_stays_one_column() {
        let wrapped = vec![
            word(0.0, 60.0, 20.0),
            word(68.0, 110.0, 20.0),
            word(10.0, 80.0, 20.0),
        ];
        assert_eq!(split_columns(&wrapped).len(), 1);
    }

    #[test]
    fn input_order_does_not_matter() {
        let mut shuffled = three_cards();
        shuffled.reverse();
        assert_eq!(split_columns(&shuffled).len(), 3);
    }

    fn merged_wrapped_column() -> Vec<RotatedRect> {
        vec![
            box_tlbr(24.0, 572.0, 113.0, 781.0),
            box_tlbr(67.0, 778.0, 114.0, 940.0),
            box_tlbr(23.0, 792.0, 65.0, 896.0),
        ]
    }

    fn box_tlbr(top: f32, left: f32, bottom: f32, right: f32) -> RotatedRect {
        RotatedRect::new(
            PointF::from_yx((top + bottom) / 2.0, (left + right) / 2.0),
            Vec2::from_yx(-1.0, 0.0),
            right - left,
            bottom - top,
        )
    }

    #[test]
    fn splits_double_height_boxes_into_rows() {
        let merged = merged_wrapped_column();
        assert_eq!(split_rows(&merged).len(), 1, "merged box bridges both rows");
        let fixed = split_tall_boxes(&merged);
        assert_eq!(fixed.len(), 4, "tall box becomes two single-row boxes");
        assert_eq!(split_rows(&fixed).len(), 2);

        assert_eq!(split_columns(&fixed).len(), 1);
    }

    #[test]
    fn leaves_normal_boxes_alone() {
        let cards = three_cards();
        assert_eq!(split_tall_boxes(&cards).len(), cards.len());
    }
}
