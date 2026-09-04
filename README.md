# Mula

A dual-pane terminal file manager in the Total Commander tradition: two
directories side by side, marks, and the work — copy, move, delete — on the
function keys where muscle memory expects them.

Built with [ratatui](https://ratatui.rs).

## Install

```sh
cargo install --git https://github.com/Cemonix/mula
```

Then:

```sh
mula            # both panels open where you are
mula ~/Music    # both panels open there instead
```

Needs Rust 1.88 or later, Linux or macOS — Windows is on the list, see
[Not there yet](#not-there-yet) — and a [Nerd Font](https://www.nerdfonts.com/)
for the file icons, the **Mono** variant, since the others draw a glyph wider
than one cell. Without one the icons are empty boxes and nothing else breaks.

From a clone, `cargo install --path .` does the same thing, and
`cargo build --release` leaves the binary in `target/` instead.

## Keys

Press **F1** while it runs for everything that works where you are standing.
These are the ones worth knowing first.

| Key | |
| --- | --- |
| `F1` | Help — works in every mode, over whatever is on screen |
| `Tab` | Focus the other panel |
| `Enter` | Enter a directory, or open a file with what belongs to it |
| `Space` | Mark or unmark the item under the cursor |
| `a` | Mark everything the panel is showing |
| `g` / `Shift+G` | Jump to the first / last item |
| `F2` | Rename |
| `F3` / `F4` | Open in `$PAGER` / `$EDITOR` |
| `F5` / `F6` | Copy / move the marked items into the other panel |
| `F7` | Create a file, or a folder if the name ends with `/` |
| `F8` | Move the marked items to the trash, naming each one first |
| `Shift+F8` | Delete them for good, without the trash |
| `F9` | Cancel the running operation |
| `v` | Quick View in the opposite panel |
| `c` | Drop a listing column, and bring them all back from the name alone |
| `.` | Show or hide the entries whose names begin with a dot |
| `/` | Find by name below this panel |
| `f` | Narrow this listing to the names holding what you type |
| `t` / `w` / `r` | New tab / close tab / rename tab |
| `[` / `]` | Previous / next tab |
| `q` | Quit |

## What it does

- **Two panels.** Copy and move always go from the panel you are in to the one
  you are not, so "where does this land" is never a question.
- **Tabs in each panel**, each with its own directory, cursor and marks, and a
  name you can change so a long path is one word.
- **Marks, then an operation.** `Space` marks, `Shift+Up`/`Down` mark and move
  in one keystroke, `a` marks everything shown. Marks survive leaving the
  directory they were made in.
- **Deleting means the trash.** F8 names every item before it goes; Shift+F8
  deletes for good. A trash that cannot be reached is a failed delete, never a
  silent fall back to permanent deletion.
- **A collision is one question for the whole batch** — overwrite, skip, or
  keep both — asked after everything else has moved. Directories merge rather
  than replacing each other.
- **Quick View.** `v` fills the opposite panel with whatever the cursor is on:
  text, a listing, an image, or a hex dump. What a file is comes from its first
  bytes, so one that lies about its format falls back to the dump.
- **Images drawn properly** — real pixels where the terminal speaks the Kitty
  graphics protocol, exact half-blocks everywhere else.
- **Find and filter.** `/` walks the tree below the panel and shows hits as they
  arrive; `f` narrows the listing as you type, with `*` and `?`, reading no
  directory twice.
- **Size and date columns**, with `c` to drop one when you would rather have
  the room.
- **Nothing blocks.** Reads, walks, previews and file operations all run off
  the drawing thread. A 4 GB copy reports progress and a summary — "3 of 5
  copied, 1 skipped" — and F9 cancels it.
- **Every mode says what it can do.** The bottom bar and F1 are generated from
  the same table the keys are resolved from, so neither can drift.

Why each of these works the way it does is in [`docs/`](docs), one file per
subsystem.

## Configuration

`~/.config/mula/config.toml` (or `$XDG_CONFIG_HOME/mula/config.toml`) rebinds
the browse keys. Nothing is created for you and nothing changes until you put a
file there.

```toml
[keys.browse]
transfer.copy = ["y", "F5"]   # copy on y as well as F5
entry.delete  = "d"           # delete on d, and no longer on F8
tab.close     = []            # nothing closes a tab
```

[`config.example.toml`](config.example.toml) is every action written out at the
key it already has — copy it and keep the lines you want to change. Key names
are the ones F1 prints. Anything the file leaves out keeps the key it has, and
a config Mula cannot use costs you your keys and nothing else: the defaults
stand and the reason appears at startup.

Only the browse keys are configurable. The overlays answer to `Esc`, `Enter`
and the arrows.

## Environment

| | |
| --- | --- |
| `$VISUAL`, `$EDITOR` | What F4 and `Enter` on a text file open, in that order, falling back to `vi`. Split on whitespace, so `code -w` works; nothing reaches a shell. |
| `$PAGER` | What F3 opens, falling back to `less`. |
| `$TZ` | Which zone the date column is drawn in. |
| `RUST_LOG` | Log filter, e.g. `RUST_LOG=debug`. Nothing is logged without it. |
| `XDG_STATE_HOME` | Where the log goes, under `mula/`. Unset, that is `~/Library/Logs/mula` on macOS and `~/.local/state/mula` elsewhere. |
| `XDG_CONFIG_HOME` | Where `mula/config.toml` is looked for. Unset, that is `~/.config`, on macOS too. |

## Not there yet

- **Windows.** Mula reaches for POSIX process and signal handling directly: a
  detached opener gets `/dev/null` and a session of its own, `$EDITOR` is
  handed the terminal with the default signal handlers put back, and a symlink
  is copied as a symlink. Each of those needs a counterpart before the panels
  could open there.
- **Remote panels.** Managing files on a server over SSH is the reason the
  directory reads were moved off the drawing thread ahead of needing to be.
- **Settings in the config file.** It rebinds keys and nothing else; the column
  choice and the icons are decided in the running app and forgotten on exit.

## Contributing

[`docs/`](docs) holds the reasoning behind the design, one file per subsystem;
`CLAUDE.md` holds the rules those documents argue for. Read the one covering
what you are about to change.

`cargo fmt` and `cargo test` before opening a pull request — as an ordinary
user, since three tests revoke a permission and expect to be refused.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Mula is free software: you can redistribute it and modify it under the terms
of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version.
