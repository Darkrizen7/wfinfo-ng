use image::DynamicImage;
use levenshtein::levenshtein;
use log::debug;

use crate::{
    database::Database, ocr::PartRect, screen_text::image_to_words,
    wfinfo_data::item_data::Refinement,
};

const ERAS: [&str; 4] = ["Lith", "Meso", "Neo", "Axi"];

/// A relic name found on screen
#[derive(Clone, Debug)]
pub struct RelicOnScreen {
    pub era: &'static str,
    /// Code as used by the database, e.g. "A1"
    pub code: String,
    /// `None` when no refinement text was found next to the name
    pub refinement: Option<Refinement>,
    /// Where the name is, in pixels of the analyzed image
    pub rect: PartRect,
    /// Name shown in the "Possible Rewards" panel of the selected relic
    pub selected: bool,
}

fn letters(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_lowercase()
}

fn parse_era(text: &str) -> Option<&'static str> {
    let text = letters(text);
    ERAS.into_iter()
        .find(|era| levenshtein(&era.to_lowercase(), &text) <= era.len() / 4)
}

fn parse_refinement(text: &str) -> Option<Refinement> {
    let text = letters(text);
    [
        ("intact", Refinement::Intact),
        ("exceptional", Refinement::Exceptional),
        ("flawless", Refinement::Flawless),
        ("radiant", Refinement::Radiant),
    ]
    .into_iter()
    .find(|(name, _)| levenshtein(name, &text) <= name.len() / 4)
    .map(|(_, refinement)| refinement)
}

/// Relic codes are a letter followed by a number; fixes the usual OCR confusions.
///
/// `followed_by_relic` tells whether the word "Relic" comes next, which confirms a relic name
/// even when its code is missing from the downloaded data.
fn parse_code(
    text: &str,
    database: &Database,
    era: &str,
    followed_by_relic: bool,
) -> Option<String> {
    let text: String = text.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let mut chars = text.chars();
    let letter = match chars.next()?.to_ascii_uppercase() {
        '0' => 'O',
        '1' => 'I',
        '5' => 'S',
        '8' => 'B',
        letter => letter,
    };
    let digits: String = chars
        .map(|c| match c {
            'O' | 'o' | 'D' | 'Q' => '0',
            'I' | 'l' | 'i' | '|' | 'L' | 'T' | 't' => '1',
            'G' | 'b' => '6',
            'g' => '9',
            'S' | 's' => '5',
            'B' => '8',
            'Z' | 'z' => '2',
            c => c,
        })
        .collect();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let code = format!("{letter}{digits}");
    if database.relic_exists(era, &code) {
        return Some(code);
    }

    // A character too many or misread, e.g. "E1" read as "ET1": only trust a single candidate
    let close: Vec<_> = database
        .relic_codes(era)
        .into_iter()
        .filter(|known| levenshtein(known, &code) == 1 && known.starts_with(letter))
        .collect();
    if let [known] = close[..] {
        return Some(known.to_owned());
    }

    // Probably a relic newer than the downloaded data
    (followed_by_relic && digits.len() <= 2).then_some(code)
}

/// Finds all "<Era> <Code>" relic names in a screenshot, along with their refinement when written nearby
pub fn find_relics(image: &DynamicImage, database: &Database) -> Vec<RelicOnScreen> {
    let words = image_to_words(image);
    debug!(
        "OCR words: {:?}",
        words.iter().map(|w| &w.text).collect::<Vec<_>>()
    );

    let mut relics: Vec<RelicOnScreen> = words
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0].line == pair[1].line)
        .filter_map(|(index, pair)| {
            let era = parse_era(&pair[0].text)?;
            let followed_by_relic = words
                .get(index + 2)
                .is_some_and(|word| word.line == pair[1].line && letters(&word.text) == "relic");
            let code = parse_code(&pair[1].text, database, era, followed_by_relic)?;
            let (first, second) = (pair[0].rect, pair[1].rect);
            let top = first.y.min(second.y);
            let bottom = (first.y + first.height).max(second.y + second.height);
            let selected = words[index..]
                .iter()
                .take_while(|word| word.line == pair[0].line)
                .any(|word| letters(&word.text) == "possible");
            Some(RelicOnScreen {
                era,
                code,
                refinement: None,
                selected,
                rect: PartRect {
                    x: first.x,
                    y: top,
                    width: second.x + second.width - first.x,
                    height: bottom - top,
                },
            })
        })
        .collect();

    // Attach each refinement word to the closest relic name
    for word in &words {
        let Some(refinement) = parse_refinement(&word.text) else {
            continue;
        };
        let center = |rect: &PartRect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let (word_x, word_y) = center(&word.rect);
        let closest = relics
            .iter_mut()
            .map(|relic| {
                let (x, y) = center(&relic.rect);
                let distance = (x - word_x).hypot(y - word_y);
                (distance / relic.rect.height, relic)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        // Only trust refinements written within a few lines of the name
        if let Some((distance, relic)) = closest {
            if distance < 4.0 && relic.refinement.is_none() {
                relic.refinement = Some(refinement);
            }
        }
    }

    relics
}
