# Mula

A dual-pane terminal file manager, in the Total Commander tradition: two
directories side by side, marks, and the work — copy, move, delete — on the
function keys where muscle memory expects them.

Built with [ratatui](https://ratatui.rs) and
[crossterm](https://github.com/crossterm-rs/crossterm).

## What it does

**Two panels, two directories.** `Tab` moves the cursor between them. Copy and
move always go from the panel you are in to the one you are not, so "where does
this land" is never a question the dialog has to ask.

**Tabs in each panel.** Every tab keeps its own directory, its own cursor and
its own marks, and can be renamed so a long path is one word on the tab bar.

**Marks, then an operation.** `Space` marks the item under the cursor;
`Shift+Up`/`Shift+Down` mark and move in one keystroke, `Alt+Up`/`Alt+Down`
unmark the same way, and `Esc` clears the panel. F5, F6 and F8 then act on
everything marked. Delete names every item it is about to remove rather than
counting them — marks can be made in a directory you have since left, and a
count is not something you can check against what you remember marking.

**Size and date, by default.** A listing shows the name, the size and the
modification time — the three things you open two panels side by side to
compare. `c` drops a column when you would rather have the room, and brings
them all back from the name alone. It is a cycle rather than a setting because
the columns are only paid for while they are drawn: names come free with
reading a directory, sizes and dates are a `stat` per entry, and dropping them
drops those syscalls with them.

**Quick View.** `v` replaces the panel opposite the cursor with the contents of
whatever the cursor is on: text as text, a directory as its listing, an image
as an image, and anything else as a hex dump. What a file *is* comes from its
first bytes, never from its extension, so a file that lies about its format
falls back to the dump rather than to an error. Quick View is a view toggle,
not a mode — every browse key and every operation goes on working underneath
it.

**Images, drawn properly where the terminal allows it.** Mula asks the terminal
what it can do before the first frame. A terminal that speaks the Kitty
graphics protocol gets real pixels; everything else gets half-block rendering,
which is exact to the colour and needs nothing from the terminal at all.

**Find.** `/` walks the tree below the panel for a name, showing hits as they
arrive rather than at the end. `Enter` on a hit opens the folder holding it
with the cursor already on it. The walk skips `.git`, `node_modules` and
`target`, stops at sixteen levels and two hundred hits, and can be cancelled
while it runs.

**Opening a file follows from what the file is.** `Enter` on a directory enters
it; on a text file it opens `$EDITOR`, handed the terminal; on anything else it
hands the file to the system opener, detached, so a picture viewer neither
freezes Mula nor writes over the frame. F3 and F4 are the named exceptions —
you asked for `$PAGER` or `$EDITOR`, so nothing is sniffed.

**A collision is one question for the whole batch.** A transfer moves
everything that lands on a free name, then asks once about the rest —
overwrite, skip, or keep both under a new name. It asks afterwards rather than
during, because nothing is forbidden while a job runs and a question arriving
mid-copy would land on a dialog you opened yourself. Directories merge instead
of replacing each other, so copying a folder onto one of the same name never
takes the files that were only in the destination, and a batch can never
overwrite what it has just written itself.

**Nothing blocks.** Directory reads, tree walks, previews and the file
operations all run off the drawing thread. A copy of 4 GB does not stop you
navigating, and it reports progress and a summary — "3 of 5 copied, 1 skipped"
— rather than a bare error. F9 cancels the running operation.

**Every mode says what it can do.** The bar along the bottom carries the keys
that matter in whatever is on screen, and `F1` lists all of them, including the
ones the bar had no room for. Both are generated from the same key table the
keys themselves are resolved from, so neither can drift.

## Requirements

- A Unix-like system. Mula uses POSIX process and signal handling directly and
  is not built for Windows.
- A Rust toolchain new enough for edition 2024 (1.85 or later).
- A [Nerd Font](https://www.nerdfonts.com/) in your terminal, for the file
  icons. Install the **Mono** variant specifically: the other variants draw the
  glyph wider than one cell and it overflows into the column beside it. Without
  a Nerd Font at all the icons show up as empty boxes and nothing else breaks.

## Building

```sh
cargo build --release
./target/release/mula
```

Mula opens both panels in the directory it was started from.

## Keys

The running app is the authority: press **F1** for the full list of what works
where you are standing. These are the ones worth knowing before you start.

| Key | |
| --- | --- |
| `F1` | Help — works in every mode, over whatever is on screen |
| `Tab` | Focus the other panel |
| `Enter` | Enter a directory, or open a file with what belongs to it |
| `Space` | Mark or unmark the item under the cursor |
| `g` / `Shift+G` | Jump to the first / last item |
| `F2` | Rename |
| `F3` / `F4` | Open in `$PAGER` / `$EDITOR` |
| `F5` / `F6` | Copy / move the marked items into the other panel |
| `F7` | Create a file, or a folder if the name ends with `/` |
| `F8` | Delete the marked items, naming each one first |
| `F9` | Cancel the running operation |
| `v` | Quick View in the opposite panel |
| `c` | Drop a listing column, and bring them all back from the name alone |
| `/` | Find by name below this panel |
| `t` / `w` / `r` | New tab / close tab / rename tab |
| `[` / `]` | Previous / next tab |
| `q` | Quit |

The tab family sits on plain letters rather than on `Ctrl+PageUp` and friends
because terminals with their own tabs claim those first — WezTerm takes
`Ctrl+PageUp`/`Ctrl+PageDown`, `Ctrl+Tab` and `Ctrl+W` before the app ever sees
them. Printable characters cannot be intercepted that way, and every mode that
reads text has a key table of its own, so they are free here.

## Configuration

`~/.config/mula/config.toml` (or `$XDG_CONFIG_HOME/mula/config.toml`) rebinds
the browse keys. Mula never writes it for you; nothing is created and nothing
changes until you put one there.

[`config.example.toml`](config.example.toml) in this repository is every action
written out at the key it already has — copy it over and keep the lines you
want to change. A test holds it to the defaults, so it cannot quietly stop
being true.

A config names an *action* and gives it the keys that should reach it — one
key, a list of them, or an empty list to leave it unreachable:

```toml
[keys.browse]
transfer.copy = ["y", "F5"]   # copy on y as well as F5
entry.delete  = "d"           # delete on d, and no longer on F8
tab.close     = []            # nothing closes a tab
```

Anything the file does not mention keeps the key it has, so a config is as
short as the changes in it. What a rebound action is called in the bar, what it
says under F1 and where it sits in both are not the config's to decide — an
action carries them wherever it moves.

Key names are the ones F1 prints: `F5`, `Tab`, `Enter`, `Backspace`, `Delete`,
`Esc`, `Space`, `PageUp`, `PageDown`, `Home`, `End`, `Insert`, the four arrows,
and any single character. `Ctrl+`, `Alt+` and `Shift+` go in front, in that
order. A capital letter carries Shift whether the name says so or not, so `G`
and `Shift+G` are one key.

The actions:

| | |
| --- | --- |
| `cursor.up` `cursor.down` `cursor.first` `cursor.last` | Move the cursor |
| `panel.toggle` `panel.parent` `panel.find` | Focus the other panel, leave for the one above, find by name |
| `entry.open` `entry.view` `entry.edit` | Open under the cursor, in `$PAGER`, in `$EDITOR` |
| `entry.rename` `entry.create` `entry.delete` | Rename, create, delete |
| `mark.toggle` `mark.up` `mark.down` `mark.clear` | Mark under the cursor, mark and move, clear the panel |
| `unmark.up` `unmark.down` | Unmark and move |
| `transfer.copy` `transfer.move` | Copy or move the marked items across |
| `tab.new` `tab.close` `tab.rename` `tab.previous` `tab.next` | Tabs |
| `view.quick` `view.columns` | Quick View, listing columns |
| `job.cancel` `app.quit` | Cancel the running operation, quit |

A config Mula cannot use costs you your keys and nothing else: the defaults
stand, and the reason appears as an error the moment it starts. Two actions
cannot share a key, and no action can take `F1` — it is resolved ahead of the
browse table and would never arrive.

Only the browse keys are configurable. The overlays answer to `Esc`, `Enter`
and the arrows, which are not keys anyone needs to move.

## Environment

| | |
| --- | --- |
| `$VISUAL`, `$EDITOR` | What F4 and `Enter` on a text file open, in that order, falling back to `vi`. Split on whitespace, so `code -w` works; nothing reaches a shell. |
| `$PAGER` | What F3 opens, falling back to `less`. |
| `$TZ` | Which zone the date column is drawn in, as everywhere else on the system. |
| `RUST_LOG` | Log filter, e.g. `RUST_LOG=debug`. Nothing is logged without it: a file manager is run from every directory there is, and one that logs by default leaves a trail of them behind. |
| `XDG_STATE_HOME` | Where the log goes, under `mula/`. Unset, that is `~/Library/Logs/mula` on macOS and `~/.local/state/mula` elsewhere. |
| `XDG_CONFIG_HOME` | Where `mula/config.toml` is looked for. Unset, that is `~/.config` — on macOS too, unlike the log: a config is a file you edit and carry between machines. |

## Not there yet

- **Remote panels.** Managing files on a server over SSH is the reason the
  directory reads were moved off the drawing thread ahead of needing to be.
  When it arrives, Mula will not authenticate anything itself.
- **Settings in the config file.** It rebinds keys and nothing else yet; the
  column choice and the file icons are still decided in the running app and
  forgotten on exit.

## Contributing

`docs/` holds the reasoning behind the design, one file per subsystem:
background I/O, previews, keys and overlays, marks and operations, opening.
`CLAUDE.md` holds the rules those documents argue for. Read the one covering
what you are about to change.

`cargo fmt` and `cargo test` before opening a pull request.

## License

GPL-3.0-only. See [LICENSE](LICENSE).
