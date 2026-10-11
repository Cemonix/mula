# Mula

A dual-pane terminal file manager: two directories side by side, marks, and the
work — copy, move, delete — on the letter each of them starts with.

Built with [ratatui](https://ratatui.rs).

## Install

```sh
cargo install --locked --git https://github.com/Cemonix/mula
```

`--locked` builds with the dependency versions in `Cargo.lock`, the ones the
tests passed with; without it Cargo picks the newest it can find.

Then:

```sh
mula            # both panels open where you are
mula ~/Music    # both panels open there instead
```

Needs Rust 1.88 or later, Linux or macOS, and a
[Nerd Font](https://www.nerdfonts.com/) for the file icons, the **Mono**
variant, since the others draw a glyph wider than one cell. Without one the
icons are empty boxes and nothing else breaks.

From a clone, `cargo install --locked --path .` does the same thing, and
`cargo build --release` leaves the binary in `target/` instead.

### Staying where you quit

To have your shell end up in the directory you quit Mula in, add one line to
your shell's config:

```sh
eval "$(mula --init zsh)"     # ~/.zshrc
eval "$(mula --init bash)"    # ~/.bashrc
mula --init fish | source     # ~/.config/fish/config.fish
```

`mula` is then a shell function that runs Mula and does the `cd` afterwards. A
program cannot change the directory of the shell that started it, so the `cd`
has to happen in the shell itself; `mula --init zsh` prints the function if you
would rather read it first. Without the line, Mula works as before and leaves
the shell where it was.

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
| `F1` | Help — works in every mode, over whatever is on screen; type to search it |
| `Tab` | Focus the other panel |
| `o` / `Shift+O` | Swap the two panels / send the other panel where this one is |
| `Enter` | Enter a directory, or open a file with what belongs to it |
| `Space` | Mark or unmark the item under the cursor |
| `a` | Mark everything the panel is showing |
| `i` | Invert the marks of what the panel is showing |
| `+` / `-` | Mark / unmark the items shown whose names match a pattern, like `*.jpg` |
| `g` / `Shift+G` | Jump to the first / last item |
| `r` / `Shift+R` | Rename / rename the marked items in `$EDITOR`, one name per line |
| `v` / `e` | Open in `$PAGER` / `$EDITOR` |
| `c` / `m` | Copy / move the marked items into the other panel |
| `l` | Make a symlink to each marked item in the other panel |
| `z` / `u` | Pack the marked items into one archive / unpack the marked archives |
| `n` | Create a file, or a folder if the name ends with `/` |
| `d` | Move the marked items to the trash, naming each one first |
| `Shift+D` | Delete them for good, without the trash |
| `x` | Cancel the running operation |
| `p` | Preview in the opposite panel |
| `,` | Drop a listing column, and bring them all back from the name alone |
| `.` | Show or hide the entries whose names begin with a dot |
| `s` / `Shift+S` | Sort by name, time, size or extension / turn the order around |
| `j` | Go to a path you type: `~` is home, and a relative one starts here |
| `f` | Find by name below this panel |
| `/` | Narrow this listing to the names holding what you type |
| `b` / `Shift+B` | Favorites: go to one / add the directory this panel is in |
| `t` / `w` / `Shift+T` | New tab / close tab / rename tab |
| `[` / `]` | Previous / next tab |
| `Shift+C` | Settings and key bindings, saved as they change |
| `q` | Quit |

## What it does

- **Two panels.** Copy and move always go from the panel you are in to the one
  you are not, so "where does this land" is never a question.
- **Tabs in each panel**, each with its own directory, cursor and marks, and a
  name you can change so a long path is one word.
- **Marks, then an operation.** `Space` marks, `Shift+Up`/`Down` mark and move
  in one keystroke, `a` marks everything shown, `i` inverts it, and `+` / `-`
  mark and unmark by a pattern. Marks survive leaving the directory they were
  made in.
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
  the room. A directory shows what its files add up to, measured in the
  background and kept, so the listing never waits for it and a directory you
  come back to has its numbers at once.
- **Sorting** by name, time, size or extension, each panel on its own:
  newest and largest first, `Shift+S` for the other way. Directories stay on
  top whatever the order.
- **Nothing blocks.** Reads, walks, previews and file operations all run off
  the drawing thread. A 4 GB copy reports progress and a summary — "3 of 5
  copied, 1 skipped" — and `x` cancels it.
- **Every mode says what it can do.** The bottom bar and F1 are generated from
  the same table the keys are resolved from, so neither can drift.

Why each of these works the way it does is in [`docs/`](docs), one file per
subsystem.

## Configuration

`Shift+C` opens the settings: the columns, the order and the dotfiles a panel
opens with, and the browse keys. To rebind an action, put the cursor on it,
press `Enter`, then press the new key. If another action already has that key,
Mula asks before taking it away. `r` puts a row back to its default. A change
shows at once and is saved straight away to `~/.config/mula/config.toml` (or
`$XDG_CONFIG_HOME/mula/config.toml`).

Mula owns that file and writes the whole of it on every change. It lists every
setting and every browse key, each under a comment, with the default on a
commented line and your own value, if you set one, on the line below it:

```toml
[listing]

# What a panel is sorted by when it opens: name, time, size, extension
# sort = "name"
sort = "size"

[keys.browse]

# Copies marked items into the other panel
# transfer.copy = "c"
transfer.copy = ["c", "y"]
```

You can also edit the file by hand. Each action takes one key, a list of keys,
or `[]` for none, and key names are the ones F1 prints. Values you write by hand
are kept the next time Mula writes the file, but your comments and formatting
are not. If the file is a symlink, say from a dotfiles repository, Mula writes
through the link and leaves the link alone.

If Mula can't use the file, because it is broken or holds something Mula doesn't
know, you lose your settings and keys for that run but never the file: Mula
starts on its defaults, says why, and won't write to the file until it restarts.

Only the browse keys can be rebound. The overlays answer to `Esc`, `Enter` and
the arrows.

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
