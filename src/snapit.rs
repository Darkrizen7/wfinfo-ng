use image::DynamicImage;
use log::debug;

use crate::{
    database::{Database, Item},
    ocr::{normalize_string, PartRect},
    screen_text::{image_to_words, Word},
};

/// An item name found on screen
#[derive(Clone, Debug)]
pub struct ItemOnScreen<'db> {
    pub item: &'db Item,
    /// Where the name is, in pixels of the analyzed image
    pub rect: PartRect,
}

/// Item names have at most this many lines, e.g. "Octavia Prime / Systems / Blueprint"
const MAX_NAME_LINES: usize = 4;

#[derive(Clone, Debug)]
struct Segment {
    text: String,
    rect: PartRect,
}

fn union(a: PartRect, b: PartRect) -> PartRect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    PartRect {
        x,
        y,
        width: (a.x + a.width).max(b.x + b.width) - x,
        height: (a.y + a.height).max(b.y + b.height) - y,
    }
}

/// Splits tesseract lines where words are far apart: neighboring tiles share a line
fn segments(words: &[Word]) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut previous: Option<&Word> = None;
    for word in words {
        let continues = previous.is_some_and(|previous| {
            let gap = word.rect.x - (previous.rect.x + previous.rect.width);
            previous.line == word.line && gap < previous.rect.height * 1.5
        });
        match segments.last_mut() {
            Some(segment) if continues => {
                segment.text.push(' ');
                segment.text.push_str(&word.text);
                segment.rect = union(segment.rect, word.rect);
            }
            _ => segments.push(Segment {
                text: word.text.clone(),
                rect: word.rect,
            }),
        }
        previous = Some(word);
    }
    // Stray characters read in icons would glue unrelated names together
    segments.retain(|segment| normalize_string(&segment.text).len() >= 3);
    segments
}

/// Groups segments written right below each other, as in a multi-line tile name
fn stacks(mut segments: Vec<Segment>) -> Vec<Vec<Segment>> {
    segments.sort_by(|a, b| a.rect.y.total_cmp(&b.rect.y));
    let mut stacks: Vec<Vec<Segment>> = Vec::new();
    for segment in segments {
        let center = segment.rect.x + segment.rect.width / 2.0;
        // OCR boxes are loose, lines of a name may even overlap a bit
        let below = stacks
            .iter_mut()
            .filter_map(|stack| {
                let last = stack.last().unwrap().rect;
                let last_center = last.x + last.width / 2.0;
                let gap = segment.rect.y - (last.y + last.height);
                let height = last.height.max(segment.rect.height);
                let aligned =
                    (center - last_center).abs() < last.width.max(segment.rect.width) / 2.0;
                (aligned && (-height * 0.5..height * 1.2).contains(&gap))
                    .then_some((gap.abs(), stack))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, stack)| stack);
        match below {
            Some(stack) => stack.push(segment),
            None => stacks.push(vec![segment]),
        }
    }
    stacks
}

/// Finds the names of tradeable prime parts anywhere on screen, e.g. in the inventory
pub fn find_items<'db>(image: &DynamicImage, database: &'db Database) -> Vec<ItemOnScreen<'db>> {
    let words = image_to_words(image);
    let stacks = stacks(segments(&words));
    debug!(
        "Text stacks: {:?}",
        stacks
            .iter()
            .map(|stack| stack
                .iter()
                .map(|s| format!(
                    "{} @{:.0},{:.0} {:.0}x{:.0}",
                    s.text, s.rect.x, s.rect.y, s.rect.width, s.rect.height
                ))
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    );

    stacks
        .iter()
        .filter_map(|stack| {
            // A tile name may share its stack with unrelated text (counts, labels): try every run of lines
            (0..stack.len())
                .flat_map(|start| {
                    (start + 1..=stack.len().min(start + MAX_NAME_LINES))
                        .map(move |end| &stack[start..end])
                })
                .filter_map(|lines| {
                    let text: String = lines.iter().map(|line| line.text.as_str()).collect();
                    let needle = normalize_string(&text);
                    let (item, distance) = database.find_item_with_distance(&needle)?;
                    // Non prime versions only lack this word, e.g. "Akbolto Blueprint": the text must
                    // contain it, or be long enough to contain a badly read one
                    let lowercase = needle.to_lowercase();
                    let mentions_prime = lowercase.contains("prim") || lowercase.contains("rime");
                    let full_length = item.drop_name.replace(' ', "").len();
                    if !mentions_prime && needle.len() + 3 < full_length {
                        return None;
                    }
                    let tolerance = item.drop_name.len() / 4;
                    let tradeable = item.drop_name.contains("Prime") && item.platinum > 0.0;
                    (distance <= tolerance && tradeable).then(|| {
                        let rect = lines.iter().map(|line| line.rect).reduce(union).unwrap();
                        // Prefer the run covering the most text: "Bronco Prime Receiver" over
                        // "Bronco Prime", even when a few of its letters were misread
                        let score = needle.len() as isize - distance as isize;
                        (score, ItemOnScreen { item, rect })
                    })
                })
                .max_by_key(|(score, _)| *score)
                .map(|(_, found)| found)
        })
        .collect()
}
