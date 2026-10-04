# WFinfo-ng

A Linux compatible version of the great [WFinfo](https://github.com/WFCD/WFinfo/).

Does support:

- Detecting relic reward screens
- Taking a screenshot the game
- Detecting items
- Displaying platinum values for each item
- X11 & Wayland
- Game in windowed or fullscreen mode

Doesn't support:

- Market integration
- Inventory tracking
- Interactive "snap-it" features

# Prerequisites and Dependencies

- `rust` rustc >= 1.74 & cargo. I recommend installation via [rustup](https://rustup.rs).
- `libxrandr` for taking screenshots
- `tesseract` for OCR processing
- `curl`, `jq` for updating the databases

# Installation

1. Clone this repository
1. Install only reward screen helper: `cargo install --path . --bin wfinfo`
1. Or install all tools: `cargo install --path .`

# Usage

Run the `update.sh` script to download the latest database files.

Find where your game puts it's `EE.log` file. Mine is located at `.local/share/Steam/steamapps/compatdata/230410/pfx/drive_c/users/steamuser/AppData/Local/Warframe/EE.log`.

Now run `wfinfo <path to your EE.log file>` (the path is optional if your EE.log file is in the default location)
This will run the program, immediately taking a screenshot and analyzing it, see section Issues and Workarounds for why.
The program then waits for the reward screen, trying to detect items in the screenshot.

Once items are found, their platinum and ducat values are looked up in the database downloaded previously.
Each item is printed to stdout along with it's platinum and ducat value in platinum (assuming 10:1 conversion).
The highest value item is also indicated with a little arrow.
When the highest value is determined by the ducat value and there is more than one item with the same ducat value, the platinum values are used as a tie breaker.

# Overlay

By default, prices are also shown in game: a transparent, click-through overlay is drawn over the Warframe window and displays the platinum and ducat values right above each reward's name, with the best item highlighted in gold.
Each item also shows how many were sold on warframe.market the previous day (in orange when fewer than 5, it may be hard to sell) and whether it is vaulted.

- `--overlay-duration <seconds>`: how long the prices stay on screen (default 20)
- `--overlay-offset <pixels>`: distance between the prices and the item names (default 10)
- `--no-overlay`: only print prices to the console

The overlay is an X11 window (it runs through XWayland on Wayland sessions). Run the game in borderless fullscreen or windowed mode; with gamescope or exclusive fullscreen the overlay may be hidden, use `--no-overlay` there.

# Relic values and refinement advice

On the Void Relics screen (relic selection before a fissure, or refinement), press `F11` to show above each relic:

- its estimated value in platinum,
- whether refining it is worth the Void Traces, and up to which refinement.

The selected relic also gets the value at every refinement level. Press `F11` again to update the estimates (after scrolling or changing era). They are removed automatically when you leave the screen (its title in the top left corner changes).

Values assume a public squad: you get the best of your reward and the rewards of three players opening random intact relics of the same era.
A refinement is recommended when it gains at least `--trace-threshold` platinum per Void Trace (default 0.02: with 6 to 30 traces earned per fissure and one relic opened per mission, that is about what can be spent without running out).
To recompute the best threshold from today's prices, run `wfinfo --compute-threshold`: it prints how many traces and how much platinum each threshold spends and earns per relic, and the threshold that spends traces as fast as they are earned. Pass `--traces-per-relic` if you earn more or fewer than 18 traces per relic opened. `--auto-threshold` uses that computed threshold instead of `--trace-threshold`.
Use `--relic-hotkey` to pick another key. `relics advice <era> [threshold]` prints the same numbers for every relic of an era.

# Data sources

Downloaded at startup when missing or older than a day, into the temporary directory:

- relic drop tables: the official ones published by Digital Extremes, from [drops.warframestat.us](https://drops.warframestat.us/data/relics.json)
- prices and sales volumes: warframe.market statistics, from [api.warframestat.us/wfinfo/prices](https://api.warframestat.us/wfinfo/prices/)
- ducat values, vaulted status, and relics missing from the official tables: [api.warframestat.us/wfinfo/filtered_items](https://api.warframestat.us/wfinfo/filtered_items/)

# Snap-it

Press `F10` on any screen showing prime parts (inventory, trade, foundry, relic rewards list...) to show their price, ducat value, sales volume and vaulted status above each name.
Press `F10` again to update the prices. They are removed automatically when you leave the screen. Use `--snapit-hotkey` to pick another key.

# Issue and Workarounds

- Due to buffering when the game writes the `EE.log` file, it is possible that WFInfo doesn't pick up the reward screen event until the screen has disappeared. I haven't found a way of getting around the buffered writer.
  If this happens, you can manually trigger the detection by pressing the F12 key.


- If you are using gamescope add the flag `--window-name=gamescope`

# Logging

Using the Environment Variables WFINFO_LOG you can control the output.
There are several levels: error, warn, info, debug, trace, off
[docs](https://docs.rs/env_logger/latest/env_logger/index.html#enabling-logging)
