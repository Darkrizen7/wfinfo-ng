use std::thread::sleep;
use std::time::Duration;
use std::{error::Error, str::FromStr};
use std::{fs::File, thread};
use std::{
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    sync::mpsc::{channel, RecvTimeoutError},
};
use std::{path::PathBuf, sync::mpsc};

use clap::Parser;
use eframe::egui::ColorImage;
use env_logger::{Builder, Env};
use global_hotkey::{hotkey::HotKey, GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use image::DynamicImage;
use levenshtein::levenshtein;
use log::{debug, error, info, warn};
use notify::{watcher, RecursiveMode, Watcher};
use xcap::Window;

use wfinfo::{
    database::Database,
    ocr::{normalize_string, reward_image_to_reward_names_with_rects, OCR},
    overlay::{
        run_overlay, Label, OverlayHandle, OverlayOptions, RelicLabel, RewardLabel, WindowGeometry,
    },
    relic_detection::find_relics,
    screen_text,
    snapit::find_items,
    trace_threshold,
    utils::{fetch_official_relics, fetch_prices_and_items},
    wfinfo_data::item_data::Refinement,
};

fn print_threshold_table(
    advices: &[wfinfo::database::RelicAdvice],
    balanced: f32,
    traces_per_relic: f32,
) {
    println!(
        "Over {} relics, refining when the gain per trace is above:",
        advices.len()
    );
    println!("threshold\ttraces/relic\tplatinum/relic\trefined");
    let mut thresholds = vec![
        0.0, 0.005, 0.01, 0.015, 0.02, 0.025, 0.03, 0.04, 0.05, balanced,
    ];
    thresholds.sort_by(|a, b| a.total_cmp(b));
    for threshold in thresholds {
        let spending = trace_threshold::spending(advices, threshold);
        println!(
            "{:.3}\t\t{:.1}\t\t{:.2}\t\t{:.0}%{}",
            threshold,
            spending.traces_per_relic,
            spending.platinum_per_relic,
            spending.refined_share * 100.0,
            if threshold == balanced {
                "\t<- balanced"
            } else {
                ""
            }
        );
    }
    println!(
        "\nWith {traces_per_relic} traces earned per relic opened, use --trace-threshold {balanced:.3}"
    );
}

/// What to look for on screen
#[derive(Clone, Copy, Debug)]
enum Trigger {
    /// Relic reward choice at the end of a fissure
    Rewards,
    /// Relic selection or refinement screen
    Relics,
    /// Prime parts anywhere on screen, e.g. in the inventory
    Items,
}

fn capture(capturer: &Window) -> DynamicImage {
    let frame = capturer.capture_image().unwrap();
    info!("Captured");
    DynamicImage::ImageRgba8(frame)
}

fn detect(
    image: DynamicImage,
    db: &Database,
    trigger: Trigger,
    trace_threshold: f32,
) -> Vec<Label> {
    match trigger {
        Trigger::Rewards => detect_rewards(image, db)
            .into_iter()
            .map(Label::Reward)
            .collect(),
        Trigger::Relics => detect_relics(image, db, trace_threshold)
            .into_iter()
            .map(Label::Relic)
            .collect(),
        Trigger::Items => detect_items(image, db)
            .into_iter()
            .map(Label::Reward)
            .collect(),
    }
}

fn detect_items(image: DynamicImage, db: &Database) -> Vec<RewardLabel> {
    let items = find_items(&image, db);
    if items.is_empty() {
        warn!("No prime part found on screen");
    }
    items
        .into_iter()
        .map(|found| {
            let item = found.item;
            info!(
                "{}\n\t{}\t{}\t{} sold yesterday{}",
                item.drop_name,
                item.platinum,
                item.ducats as f32 / 10.0,
                item.volume,
                if item.vaulted { "\tvaulted" } else { "" }
            );
            RewardLabel {
                rect: found.rect,
                name: Some(item.drop_name.clone()),
                platinum: item.platinum,
                ducats_platinum: item.ducats as f32 / 10.0,
                volume: item.volume,
                vaulted: item.vaulted,
                best: false,
            }
        })
        .collect()
}

fn detect_relics(image: DynamicImage, db: &Database, trace_threshold: f32) -> Vec<RelicLabel> {
    let relics = find_relics(&image, db);
    if relics.is_empty() {
        warn!("No relic found on screen");
    }
    // A single relic on screen has room for details, otherwise only the selected one does
    let single = relics.len() == 1;
    relics
        .into_iter()
        .map(|relic| {
            let Some(relic_data) = db.relics_of_era(relic.era).unwrap().get(&relic.code) else {
                warn!("{} {}\n\tUnknown drops", relic.era, relic.code);
                return RelicLabel {
                    rect: relic.rect,
                    refinement: relic.refinement,
                    advice: None,
                    detailed: false,
                };
            };
            let advice = db.refinement_advice(relic_data, relic.era, trace_threshold);
            let current = relic.refinement.unwrap_or(Refinement::Intact);
            let values: Vec<_> = advice
                .values
                .iter()
                .map(|value| format!("{:?} {:.1}", value.refinement, value.platinum))
                .collect();
            info!(
                "{} {} ({:?})\n\t{}\n\trefine: {:?}",
                relic.era,
                relic.code,
                relic.refinement,
                values.join("\t"),
                advice.recommendation(current)
            );
            RelicLabel {
                rect: relic.rect,
                refinement: relic.refinement,
                advice: Some(advice),
                detailed: single || relic.selected,
            }
        })
        .collect()
}

fn detect_rewards(image: DynamicImage, db: &Database) -> Vec<RewardLabel> {
    let parts = reward_image_to_reward_names_with_rects(image, None);
    let text: Vec<_> = parts.iter().map(|(s, _rect)| normalize_string(s)).collect();
    debug!("{:#?}", text);

    let items: Vec<_> = text.iter().map(|s| db.find_item(s, None)).collect();

    let best = items
        .iter()
        .map(|item| {
            item.map(|item| {
                item.platinum
                    .max(item.ducats as f32 / 10.0 + item.platinum / 100.0)
            })
            .unwrap_or(0.0)
        })
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|best| best.0);

    for (index, item) in items.iter().enumerate() {
        if let Some(item) = item {
            info!(
                "{}\n\t{}\t{}\t{} sold yesterday{}\t{}",
                item.drop_name,
                item.platinum,
                item.ducats as f32 / 10.0,
                item.volume,
                if item.vaulted { "\tvaulted" } else { "" },
                if Some(index) == best { "<----" } else { "" }
            );
        } else {
            warn!("Unknown item\n\tUnknown");
        }
    }

    items
        .iter()
        .zip(&parts)
        .enumerate()
        .map(|(index, (item, (_text, rect)))| RewardLabel {
            rect: *rect,
            name: item.map(|item| item.drop_name.clone()),
            platinum: item.map_or(0.0, |item| item.platinum),
            ducats_platinum: item.map_or(0.0, |item| item.ducats as f32 / 10.0),
            volume: item.map_or(0.0, |item| item.volume),
            vaulted: item.is_some_and(|item| item.vaulted),
            best: item.is_some() && Some(index) == best,
        })
        .collect()
}

fn log_watcher(path: PathBuf, event_sender: mpsc::Sender<Trigger>) {
    debug!("Path: {}", path.display());
    let mut position = File::open(&path)
        .unwrap_or_else(|_| panic!("Couldn't open file {}", path.display()))
        .seek(SeekFrom::End(0))
        .unwrap();

    thread::spawn(move || {
        debug!("Position: {}", position);

        let (tx, rx) = mpsc::channel();
        let mut watcher = watcher(tx, Duration::from_millis(100)).unwrap();
        watcher
            .watch(&path, RecursiveMode::NonRecursive)
            .unwrap_or_else(|_| panic!("Failed to open EE.log file: {}", path.display()));

        loop {
            match rx.recv() {
                Ok(notify::DebouncedEvent::Write(_)) => {
                    let mut f = File::open(&path).unwrap();
                    f.seek(SeekFrom::Start(position)).unwrap();

                    let mut reward_screen_detected = false;

                    let reader = BufReader::new(f.by_ref());
                    for line in reader.lines() {
                        let line = match line {
                            Ok(line) => line,
                            Err(err) => {
                                error!("Error reading line: {}", err);
                                continue;
                            }
                        };
                        // debug!("> {:?}", line);
                        if line.contains("Pause countdown done")
                            || line.contains("Got rewards")
                            || line.contains("Created /Lotus/Interface/ProjectionRewardChoice.swf")
                        {
                            reward_screen_detected = true;
                        }
                    }

                    if reward_screen_detected {
                        info!("Detected, waiting...");
                        sleep(Duration::from_millis(1500));
                        event_sender.send(Trigger::Rewards).unwrap();
                    }

                    position = f.metadata().unwrap().len();
                    debug!("Log position: {}", position);
                }
                Ok(_) => {}
                Err(err) => {
                    error!("Error: {:?}", err);
                }
            }
        }
    });
}

fn hotkey_watcher(hotkeys: Vec<(HotKey, Trigger)>, event_sender: mpsc::Sender<Trigger>) {
    debug!("watching hotkeys: {hotkeys:?}");
    thread::spawn(move || {
        let manager = GlobalHotKeyManager::new().unwrap();
        for (hotkey, _trigger) in &hotkeys {
            manager.register(*hotkey).unwrap();
        }

        while let Ok(event) = GlobalHotKeyEvent::receiver().recv() {
            debug!("{:?}", event);
            if event.state != HotKeyState::Pressed {
                continue;
            }
            if let Some((_hotkey, trigger)) =
                hotkeys.iter().find(|(hotkey, _)| hotkey.id() == event.id)
            {
                event_sender.send(*trigger).unwrap();
            }
        }
    });
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Arguments {
    /// Path to the `EE.log` file located in the game installation directory
    ///
    /// Most likely located at `~/.local/share/Steam/steamapps/compatdata/230410/pfx/drive_c/users/steamuser/AppData/Local/Warframe/EE.log`
    game_log_file_path: Option<PathBuf>,
    /// Warframe Window Name
    ///
    /// some systems may require the window name to be specified (e.g. when using gamescope)
    #[arg(short, long, default_value = "Warframe")]
    window_name: String,
    /// Only print prices to the console, don't show the in-game overlay
    #[arg(long)]
    no_overlay: bool,
    /// How many seconds the overlay keeps prices on screen
    #[arg(long, default_value_t = 20.0)]
    overlay_duration: f32,
    /// Vertical distance in pixels between the prices and the item names
    #[arg(long, default_value_t = 10.0)]
    overlay_offset: f32,
    /// Minimum platinum gained per Void Trace for a relic refinement to be recommended
    #[arg(long, default_value_t = 0.02)]
    trace_threshold: f32,
    /// Compute the trace threshold from today's prices at startup instead of using --trace-threshold
    #[arg(long)]
    auto_threshold: bool,
    /// Print how many traces and how much platinum each threshold spends and earns, then exit
    #[arg(long)]
    compute_threshold: bool,
    /// Void Traces earned per relic opened, used to compute the threshold
    #[arg(long, default_value_t = trace_threshold::AVERAGE_TRACES_PER_MISSION)]
    traces_per_relic: f32,
    /// Hotkey to analyze the relics on screen (selection or refinement screen)
    #[arg(long, default_value = "F11")]
    relic_hotkey: String,
    /// Hotkey to price every prime part on screen, e.g. in the inventory
    #[arg(long, default_value = "F10")]
    snapit_hotkey: String,
    /// Analyze this screenshot instead of watching the game (for testing the overlay)
    #[arg(long, hide = true)]
    test_image: Option<PathBuf>,
    /// Treat the test image as a relic screen instead of a reward screen
    #[arg(long, hide = true)]
    test_relics: bool,
    /// Look for prime parts anywhere in the test image instead of a reward screen
    #[arg(long, hide = true)]
    test_snapit: bool,
    /// Print the screen title read from the test image
    #[arg(long, hide = true)]
    test_title: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    let default_log_path = PathBuf::from_str(&std::env::var("HOME").unwrap()).unwrap().join(PathBuf::from_str(".local/share/Steam/steamapps/compatdata/230410/pfx/drive_c/users/steamuser/AppData/Local/Warframe/EE.log")?);
    let log_path = arguments.game_log_file_path.unwrap_or(default_log_path);
    let window_name = arguments.window_name;
    let env = Env::default()
        .filter_or("WFINFO_LOG", "info")
        .write_style_or("WFINFO_STYLE", "always");
    Builder::from_env(env)
        .format_timestamp(None)
        .format_level(false)
        .format_module_path(false)
        .format_target(false)
        .init();

    // Ready before the first F10/F11 press
    thread::spawn(screen_text::warm_up);

    let (prices, items) = fetch_prices_and_items()?;
    let mut db = Database::load_from_file(Some(&prices), Some(&items));
    match fetch_official_relics().and_then(|path| db.load_official_relics(&path)) {
        Ok(()) => {}
        Err(err) => warn!("Using relic data from WFInfo only: {err:#}"),
    }

    info!("Loaded database");

    let mut trace_threshold = arguments.trace_threshold;
    if arguments.compute_threshold || arguments.auto_threshold {
        let advices = trace_threshold::all_relic_advice(&db);
        let balanced = trace_threshold::balanced_threshold(&advices, arguments.traces_per_relic);
        if arguments.compute_threshold {
            print_threshold_table(&advices, balanced, arguments.traces_per_relic);
            screen_text::shut_down();
            return Ok(());
        }
        info!("Trace threshold: {balanced:.3} platinum per trace");
        trace_threshold = balanced;
    }

    if let Some(test_image) = arguments.test_image {
        let image = image::open(test_image)?;
        if arguments.test_title {
            println!("Title: {:?}", screen_text::read_title(&image));
            screen_text::shut_down();
            return Ok(());
        }
        let geometry = WindowGeometry {
            x: 0,
            y: 0,
            width: image.width(),
            height: image.height(),
        };
        let background = ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            &image.to_rgba8(),
        );
        let trigger = if arguments.test_relics {
            Trigger::Relics
        } else if arguments.test_snapit {
            Trigger::Items
        } else {
            Trigger::Rewards
        };
        let labels = detect(image, &db, trigger, trace_threshold);
        drop(OCR.lock().unwrap().take());
        screen_text::shut_down();
        if arguments.no_overlay {
            return Ok(());
        }
        let options = OverlayOptions {
            geometry,
            display_duration: Duration::from_secs_f32(arguments.overlay_duration),
            vertical_offset: arguments.overlay_offset,
            background: Some(background),
        };
        run_overlay(options, move |overlay| overlay.show(labels))?;
        return Ok(());
    }

    let windows = Window::all()?;
    let Some(warframe_window) = windows.into_iter().find(|x| x.title() == window_name) else {
        return Err("Warframe window not found".into());
    };

    debug!(
        "Capture source resolution: {:?}x{:?}",
        warframe_window.width(),
        warframe_window.height()
    );

    let geometry = WindowGeometry {
        x: warframe_window.x(),
        y: warframe_window.y(),
        width: warframe_window.width(),
        height: warframe_window.height(),
    };

    let (event_sender, event_receiver) = channel();

    log_watcher(log_path, event_sender.clone());
    hotkey_watcher(
        vec![
            ("F12".parse()?, Trigger::Rewards),
            (arguments.relic_hotkey.parse()?, Trigger::Relics),
            (arguments.snapit_hotkey.parse()?, Trigger::Items),
        ],
        event_sender,
    );

    if arguments.no_overlay {
        detection_loop(event_receiver, warframe_window, db, trace_threshold, None);
        return Ok(());
    }

    let options = OverlayOptions {
        geometry,
        display_duration: Duration::from_secs_f32(arguments.overlay_duration),
        vertical_offset: arguments.overlay_offset,
        background: None,
    };
    // The overlay window has to live on the main thread, detection moves to its own thread
    run_overlay(options, move |overlay| {
        thread::spawn(move || {
            detection_loop(
                event_receiver,
                warframe_window,
                db,
                trace_threshold,
                Some(overlay),
            )
        });
    })?;
    Ok(())
}

/// How often the screen title is checked while relic or item estimates are shown
const TITLE_CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// Consecutive different titles before the estimates are removed, to ignore short popups
const TITLE_CHANGES_TO_CLOSE: u32 = 2;

/// Fuzzy title comparison, the spaced out capitals of titles are often misread
fn same_title(reference: &str, current: &str) -> bool {
    levenshtein(reference, current) <= (reference.len() / 3).max(3)
}

/// Screen the shown estimates belong to, to remove them once the player leaves it
struct WatchedScreen {
    title: String,
    changes: u32,
}

fn detection_loop(
    event_receiver: mpsc::Receiver<Trigger>,
    warframe_window: Window,
    db: Database,
    trace_threshold: f32,
    overlay: Option<OverlayHandle>,
) {
    let mut watched: Option<WatchedScreen> = None;
    loop {
        let trigger = match event_receiver.recv_timeout(TITLE_CHECK_INTERVAL) {
            Ok(trigger) => trigger,
            Err(RecvTimeoutError::Timeout) => {
                let (Some(overlay), Some(screen)) = (&overlay, &mut watched) else {
                    continue;
                };
                let title = screen_text::read_title(&capture(&warframe_window));
                if same_title(&screen.title, &title) {
                    screen.changes = 0;
                    continue;
                }
                screen.changes += 1;
                debug!("Screen title changed: {:?} -> {title:?}", screen.title);
                if screen.changes >= TITLE_CHANGES_TO_CLOSE {
                    info!("Left the screen, removing estimates");
                    overlay.show_until_replaced(Vec::new());
                    watched = None;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };

        info!("Capturing ({trigger:?})");
        let image = capture(&warframe_window);
        let title = screen_text::read_title(&image);
        let labels = detect(image, &db, trigger, trace_threshold);
        let Some(overlay) = &overlay else {
            continue;
        };
        match trigger {
            Trigger::Rewards => overlay.show(labels),
            // Relic and item estimates stay while the player is on the same screen, the hotkey
            // updates them, or clears them when nothing is found anymore
            Trigger::Relics | Trigger::Items => {
                watched = (!labels.is_empty() && !title.is_empty())
                    .then_some(WatchedScreen { title, changes: 0 });
                overlay.show_until_replaced(labels)
            }
        }
    }

    drop(OCR.lock().unwrap().take());
    screen_text::shut_down();
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;
    use std::fs::read_to_string;

    use image::io::Reader;
    use indexmap::IndexMap;
    use rayon::prelude::*;
    use tesseract::Tesseract;
    use wfinfo::ocr::detect_theme;
    use wfinfo::ocr::extract_parts;
    use wfinfo::ocr::reward_image_to_reward_names;
    use wfinfo::testing::Label;

    use super::*;

    #[test]
    fn title_comparison() {
        // Titles as read from real screenshots
        assert!(same_title("voidrelicsrefinement", "vidrelicsrefinements"));
        assert!(same_title("inventorysell", "inventorysell"));
        assert!(!same_title("voidrelicsrefinement", "inventorysell"));
        assert!(!same_title("inventorysell", ""));
        assert!(!same_title("voidrelicsrefinement", "aeifvifissurerewards"));
    }

    #[test]
    fn single_image() {
        let image = Reader::open(format!("test-images/{}.png", 1))
            .unwrap()
            .decode()
            .unwrap();
        let text = reward_image_to_reward_names(image, None);
        let text = text.iter().map(|s| normalize_string(s));
        println!("{:#?}", text);
        let db = Database::load_from_file(None, None);
        let items: Vec<_> = text.map(|s| db.find_item(&s, None)).collect();
        println!("{:#?}", items);

        assert_eq!(
            items[0].expect("Didn't find an item?").drop_name,
            "Octavia Prime Systems Blueprint"
        );
        assert_eq!(
            items[1].expect("Didn't find an item?").drop_name,
            "Octavia Prime Blueprint"
        );
        assert_eq!(
            items[2].expect("Didn't find an item?").drop_name,
            "Tenora Prime Blueprint"
        );
        assert_eq!(
            items[3].expect("Didn't find an item?").drop_name,
            "Harrow Prime Systems Blueprint"
        );
    }

    // #[test]
    #[allow(dead_code)]
    fn wfi_images_exact() {
        let labels: IndexMap<String, Label> =
            serde_json::from_str(&read_to_string("WFI test images/labels.json").unwrap()).unwrap();
        for (filename, label) in labels {
            let image = Reader::open("WFI test images/".to_string() + &filename)
                .unwrap()
                .decode()
                .unwrap();
            let text = reward_image_to_reward_names(image, None);
            let text: Vec<_> = text.iter().map(|s| normalize_string(s)).collect();
            println!("{:#?}", text);

            let db = Database::load_from_file(None, None);
            let items: Vec<_> = text.iter().map(|s| db.find_item(s, None)).collect();
            println!("{:#?}", items);
            println!("{}", filename);

            let item_names = items
                .iter()
                .map(|item| item.map(|item| item.drop_name.clone()));

            for (result, expectation) in item_names.zip(label.items) {
                if expectation.is_empty() {
                    assert_eq!(result, None)
                } else {
                    assert_eq!(result, Some(expectation))
                }
            }
        }
    }

    #[test]
    fn wfi_images_99_percent() {
        let labels: BTreeMap<String, Label> =
            serde_json::from_str(&read_to_string("WFI test images/labels.json").unwrap()).unwrap();
        let total = labels.len();
        let success_count: usize = labels
            .into_par_iter()
            .map(|(filename, label)| {
                let image = Reader::open("WFI test images/".to_string() + &filename)
                    .unwrap()
                    .decode()
                    .unwrap();
                let text = reward_image_to_reward_names(image, None);
                let text: Vec<_> = text.iter().map(|s| normalize_string(s)).collect();
                println!("{:#?}", text);

                let db = Database::load_from_file(None, None);
                let items: Vec<_> = text.iter().map(|s| db.find_item(s, None)).collect();
                println!("{:#?}", items);
                println!("{}", filename);

                let item_names = items
                    .iter()
                    .map(|item| item.map(|item| item.drop_name.clone()));

                if item_names.zip(label.items).all(|(result, expectation)| {
                    expectation == result.unwrap_or_else(|| "".to_string())
                }) {
                    1
                } else {
                    0
                }
            })
            .sum();

        let success_rate = success_count as f32 / total as f32;
        assert!(success_rate > 0.95, "Success rate: {success_rate}");
    }

    // #[test]
    #[allow(dead_code)]
    fn images() {
        let tests = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];
        for i in tests {
            let image = Reader::open(format!("test-images/{}.png", i))
                .unwrap()
                .decode()
                .unwrap();

            let theme = detect_theme(&image);
            println!("Theme: {:?}", theme);

            let parts = extract_parts(&image, theme);

            let mut ocr =
                Tesseract::new(None, Some("eng")).expect("Could not initialize Tesseract");
            for part in parts {
                let buffer = part.as_flat_samples_u8().unwrap();
                ocr = ocr
                    .set_frame(
                        buffer.samples,
                        part.width() as i32,
                        part.height() as i32,
                        3,
                        3 * part.width() as i32,
                    )
                    .expect("Failed to set image");
                let text = ocr.get_text().expect("Failed to get text");
                println!("{}", text);
            }
            println!("=================");
        }
    }
}
