# Mula

A dual-pane terminal file manager: two directories side by side, marks, and the
work — copy, move, delete — on the letter each of them starts with.

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

Needs Rust 1.88 or later, Linux or macOS, and a
[Nerd Font](https://www.nerdfonts.com/) for the file icons, the **Mono**
variant, since the others draw a glyph wider than one cell. Without one the
icons are empty boxes and nothing else breaks.

From a clone, `cargo install --path .` does the same thing, and
`cargo build --release` leaves the binary in `target/` instead.

Windows is not a target. Mula reaches for POSIX process and signal handling
directly — a detached opener gets `/dev/null` and a session of its own,
`$EDITOR` is handed the terminal with the default signal handlers put back, and
a symlink is copied as a symlink — and under WSL none of that needs a
counterpart. Set `OPENER=wslview` and `Enter` hands a file to Windows.

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
| `r` | Rename |
| `v` / `e` | Open in `$PAGER` / `$EDITOR` |
| `c` / `m` | Copy / move the marked items into the other panel |
| `z` / `u` | Pack the marked items into one archive / unpack the marked archives |
| `n` | Create a file, or a folder if the name ends with `/` |
| `d` | Move the marked items to the trash, naming each one first |
| `Shift+D` | Delete them for good, without the trash |
| `x` | Cancel the running operation |
| `p` | Preview in the opposite panel |
| `,` | Drop a listing column, and bring them all back from the name alone |
| `.` | Show or hide the entries whose names begin with a dot |
| `f` | Find by name below this panel |
| `/` | Narrow this listing to the names holding what you type |
| `b` / `Shift+B` | Favorites: go to one / add the directory this panel is in |
| `t` / `w` / `Shift+T` | New tab / close tab / rename tab |
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
- **Deleting means the trash.** `d` names every item before it goes; `Shift+D`
  deletes for good. A trash that cannot be reached is a failed delete, never a
  silent fall back to permanent deletion.
- **A collision is one question for the whole batch** — overwrite, skip, or
  keep both — asked after everything else has moved. Directories merge rather
  than replacing each other.
- **Archives.** `z` packs the marked items into one archive — zip, tar or
  tar.gz, chosen by the suffix you type — and `u` unpacks the marked ones into
  the other panel. An unpack goes into a directory of its own and takes a
  numbered name when that one is taken, so it never writes over anything, and
  a cancelled one leaves nothing behind.
- **Preview.** `p` fills the opposite panel with whatever the cursor is on:
  text, a listing, an archive's contents, an image, or a hex dump. What a file
  is comes from its first bytes, so one that lies about its format falls back
  to the dump.
- **Images drawn properly** — real pixels where the terminal speaks the Kitty
  graphics protocol, exact half-blocks everywhere else.
- **Find and filter.** `f` walks the tree below the panel and shows hits as they
  arrive; `/` narrows the listing as you type, with `*` and `?`, reading no
  directory twice.
- **Favorites.** `Shift+B` writes the directory you are in down, `b` opens the
  list and `Enter` sends the panel there. One list for both panels, kept in a
  file of its own between runs. A favorite that has gone away says so and
  leaves the panel where it is.
- **Size and date columns**, with `,` to drop one when you would rather have
  the room.
- **Nothing blocks.** Reads, walks, previews and file operations all run off
  the drawing thread. A 4 GB copy reports progress and a summary — "3 of 5
  copied, 1 skipped" — and `x` cancels it.
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
transfer.copy = ["c", "y"]    # copy on y as well as c
entry.delete  = "Delete"      # delete on Delete, and no longer on d
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
| `$VISUAL`, `$EDITOR` | What `e` and `Enter` on a text file open, in that order, falling back to `vi`. Split on whitespace, so `code -w` works; nothing reaches a shell. |
| `$PAGER` | What `v` opens, falling back to `less`. |
| `$OPENER` | What `Enter` hands a file the terminal cannot show, falling back to `open` on macOS and `xdg-open` elsewhere. Started detached, so Mula keeps the screen. |
| `$TZ` | Which zone the date column is drawn in. |
| `RUST_LOG` | Log filter, e.g. `RUST_LOG=debug`. Nothing is logged without it. |
| `XDG_STATE_HOME` | Where the log and the favorites go, under `mula/`. Unset, the log is `~/Library/Logs/mula` on macOS and the favorites `~/Library/Application Support/mula`; elsewhere both are `~/.local/state/mula`. |
| `XDG_CONFIG_HOME` | Where `mula/config.toml` is looked for. Unset, that is `~/.config`, on macOS too. |

## Not there yet

- **Remote panels.** Managing files on a server over SSH is the reason the
  directory reads were moved off the drawing thread ahead of needing to be.
- **Walking into an archive.** `p` shows what one holds and `u` gets it out,
  but Enter on an archive does not open it as a directory. Doing that means
  entries that are not local paths — the same thing a remote panel needs, so
  the two wait for one another.
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
