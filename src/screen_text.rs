use std::{sync::Mutex, time::Instant};

use image::{imageops::FilterType, DynamicImage, GrayImage, Luma, Rgb};
use lazy_static::lazy_static;
use log::debug;
use rayon::{prelude::*, ThreadPool, ThreadPoolBuilder};
use tesseract::Tesseract;

use crate::ocr::{detect_theme, PartRect};

/// Tesseract engines read text bands in parallel, more barely helps and costs memory
const ENGINE_COUNT: usize = 4;
/// Tesseract misreads UI sized text (e.g. "A9" as "A5") unless it is enlarged
const UPSCALE: u32 = 2;
/// Rows with fewer text colored pixels are noise
const MIN_ROW_HITS: u32 = 2;

lazy_static! {
    static ref ENGINES: Mutex<Vec<Tesseract>> = Mutex::new(Vec::new());
    static ref POOL: ThreadPool = ThreadPoolBuilder::new()
        .num_threads(ENGINE_COUNT)
        .build()
        .expect("Failed to create OCR thread pool");
}

fn new_engine() -> Tesseract {
    Tesseract::new(None, Some("eng")).expect("Could not initialize Tesseract")
}

/// Creates the OCR engines ahead of time so that the first analysis isn't slower
pub fn warm_up() {
    let engines: Vec<_> = POOL.install(|| {
        (0..ENGINE_COUNT)
            .into_par_iter()
            .map(|_| new_engine())
            .collect()
    });
    ENGINES.lock().unwrap().extend(engines);
}

/// Releases the OCR engines, call before exiting
pub fn shut_down() {
    ENGINES.lock().unwrap().clear();
}

#[derive(Clone, Debug)]
pub(crate) struct Word {
    pub(crate) text: String,
    pub(crate) rect: PartRect,
    /// Words with the same value are on the same line
    pub(crate) line: (u32, u32, u32),
}

/// Titles such as the selected relic's name are written in white, whatever the theme
fn is_white(pixel: Rgb<u8>) -> bool {
    let [r, g, b] = pixel.0;
    r.min(g).min(b) >= 215 && r.max(g).max(b) - r.min(g).min(b) <= 25
}

/// Keeps only the pixels in the UI text color: black text on a white background, without icons
fn filter_text(image: &DynamicImage) -> GrayImage {
    let theme = detect_theme(image);
    debug!("Theme: {theme:?}");
    let rgb = image.to_rgb8();
    let mut filtered = GrayImage::from_pixel(rgb.width(), rgb.height(), Luma([255]));
    let width = rgb.width() as usize;
    filtered
        .par_chunks_mut(width)
        .zip(rgb.par_chunks(width * 3))
        .for_each(|(filtered_row, row)| {
            for (filtered, pixel) in filtered_row.iter_mut().zip(row.chunks_exact(3)) {
                let pixel = Rgb([pixel[0], pixel[1], pixel[2]]);
                if theme.threshold_filter(pixel) || is_white(pixel) {
                    *filtered = 0;
                }
            }
        });
    filtered
}

/// Vertical ranges of rows containing text, with some margin
fn text_bands(filtered: &GrayImage) -> Vec<(u32, u32)> {
    let margin = (filtered.height() / 200).max(4);
    let mut bands: Vec<(u32, u32)> = Vec::new();
    for (y, row) in filtered.rows().enumerate() {
        let hits = row.filter(|pixel| pixel.0[0] == 0).count() as u32;
        if hits < MIN_ROW_HITS {
            continue;
        }
        let (top, bottom) = (y as u32, y as u32 + 1);
        match bands.last_mut() {
            Some(band) if top <= band.1 + margin => band.1 = bottom,
            _ => bands.push((top, bottom)),
        }
    }
    bands
        .into_iter()
        .map(|(top, bottom)| {
            (
                top.saturating_sub(margin),
                (bottom + margin).min(filtered.height()),
            )
        })
        .collect()
}

fn read_band(filtered: &GrayImage, band_index: usize, (top, bottom): (u32, u32)) -> Vec<Word> {
    let height = bottom - top;
    let band = image::imageops::crop_imm(filtered, 0, top, filtered.width(), height).to_image();
    let band = image::imageops::resize(
        &band,
        band.width() * UPSCALE,
        height * UPSCALE,
        FilterType::Triangle,
    );

    let engine = ENGINES.lock().unwrap().pop().unwrap_or_else(new_engine);
    let mut engine = engine
        .set_frame(
            band.as_raw(),
            band.width() as i32,
            band.height() as i32,
            1,
            band.width() as i32,
        )
        .expect("Failed to set image")
        .recognize()
        .expect("Failed to recognize text");
    let tsv = engine.get_tsv_text(0).unwrap_or_default();
    ENGINES.lock().unwrap().push(engine);

    let scale = UPSCALE as f32;
    tsv.lines()
        .filter_map(|line| {
            // level page block paragraph line word left top width height confidence text
            let columns: Vec<_> = line.split('\t').collect();
            if columns.len() < 12 || columns[0] != "5" {
                return None;
            }
            let number = |index: usize| columns[index].parse::<f32>().ok();
            let text = columns[11].trim();
            if text.is_empty() {
                return None;
            }
            Some(Word {
                text: text.to_owned(),
                rect: PartRect {
                    x: number(6)? / scale,
                    y: number(7)? / scale + top as f32,
                    width: number(8)? / scale,
                    height: number(9)? / scale,
                },
                line: (
                    band_index as u32,
                    number(2)? as u32 * 1000 + number(3)? as u32,
                    number(4)? as u32,
                ),
            })
        })
        .collect()
}

/// Reads all the words written in the UI text color, along with their position
pub(crate) fn image_to_words(image: &DynamicImage) -> Vec<Word> {
    let start = Instant::now();
    let filtered = filter_text(image);
    let bands = text_bands(&filtered);
    debug!(
        "Filtered in {} ms, {} text bands",
        start.elapsed().as_millis(),
        bands.len()
    );
    let words: Vec<Word> = POOL.install(|| {
        bands
            .par_iter()
            .enumerate()
            .flat_map(|(index, band)| read_band(&filtered, index, *band))
            .collect()
    });
    debug!(
        "Read {} words in {} ms",
        words.len(),
        start.elapsed().as_millis()
    );
    words
}

/// Reads the screen title in the top left corner (e.g. "VOID RELICS/REFINEMENT"), keeping only
/// lowercase letters. Empty when there is no title.
pub fn read_title(image: &DynamicImage) -> String {
    let (width, height) = (image.width() as f32, image.height() as f32);
    let title = image
        .crop_imm(
            (width * 0.03) as u32,
            (height * 0.025) as u32,
            (width * 0.47) as u32,
            (height * 0.075) as u32,
        )
        .to_rgb8();
    let title = image::imageops::resize(
        &title,
        title.width() * UPSCALE,
        title.height() * UPSCALE,
        FilterType::Triangle,
    );

    let engine = ENGINES.lock().unwrap().pop().unwrap_or_else(new_engine);
    let mut engine = engine
        .set_frame(
            title.as_raw(),
            title.width() as i32,
            title.height() as i32,
            3,
            3 * title.width() as i32,
        )
        .expect("Failed to set image");
    let text = engine.get_text().unwrap_or_default();
    ENGINES.lock().unwrap().push(engine);

    text.chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_lowercase()
}
