# mula

Dual-pane TUI file manager. Rust + ratatui + crossterm.

`tasks/` holds the open work, one file per task, and is excluded through
`.git/info/exclude` — it never reaches the repository. This file is the one that
stays, so nothing durable may live in a task file.

## What we are building

One key table per focus. **Three** things are generated from it: the translation
of a key into a message, the bar along the bottom, and the help under `?`. They
cannot drift apart, because there is a single source — the moment the help became
a second copy, it would sooner or later start lying.

Next to it sits `Mode`, which owns the overlay: a dialog is not a field on `App`,
a dialog **is** a state.

The goal for the controls: **no vim-like modes for the user.** Modality is fine
as long as it is visible — hence the bar along the bottom and `?` the way lazygit
does it. Total Commander style keys (F5/F6/F8), not mnemonics.

## Comments and docstrings

**A comment explains how the code works. It never argues why it was written that
way, and never claims one approach is better than another.**

The reader of a comment is a developer who has to understand the code in front of
them. Give them the mechanism: what the function does, what it relies on, what it
returns, what it does not handle. Rationale, trade-offs, alternatives that were
rejected and "this is better because" belong in the decision list below — never in
a source file.

Allowed, because it describes mechanism:

```rust
// Paragraph aligns horizontally but never vertically, so shrink the area
// down to the text and let Flex center what is left over.
```

```rust
/// Refreshes the active pane on both sides. Both run even when the first fails;
/// the error of the left one is returned first.
```

Not allowed, because it defends a decision:

```rust
// Only the foreground is set. A background here would merge this row with the
// key bar below into one two-line block.
```

```rust
// Marks are kept on failure so the batch can be retried once the cause is
// dealt with.
```

Both of those say why the author chose something. Rewrite them as what the code
does ("Clears the marks only after every item succeeded"), or drop them and put
the reasoning under *Decisions* below.

Further rules:

- Do not restate the code. `// increment the counter` above `counter += 1` is
  noise; a comment earns its place by saying something the code does not.
- A `///` docstring describes the item's contract: inputs, output, what it
  mutates, what it leaves alone.
- Words like *because*, *better*, *instead of*, *we chose*, *this way* are a
  signal that the sentence belongs under *Decisions*.
- The same split applies to a task file: it says what is broken and what to do,
  and points at *Decisions* for why. A task that argues its own rationale is a
  decision that has not been written down yet.
- No commented-out code and no `TODO:` notes in source. Open items go to
  `tasks/`.

## Working on this codebase

- Text shown to the user (key bar labels, help, dialog messages, toasts) and the
  key tables are written by the repository owner, not by an agent. Leave
  placeholder wording alone unless asked and flag it instead. Logic and widgets
  are the working area.
- Widgets are dumb: `App` computes the content, the widget draws it. A widget
  never receives `&App`.
- A widget is tested by rendering it into a `Buffer` and asserting on the
  symbols. No terminal is needed.
- `cargo fmt` and `cargo test` before reporting anything as done.

## Tasks

Open work lives in `tasks/`, one markdown file per task, excluded from git
through `.git/info/exclude`. Reading a file is free, so the tasks stay available
without a tool call and without reaching the repository.

- **One task, one file.** The name is a kebab-case slug prefixed by kind:
  `bug-` for something broken, `feat-` for new behaviour, `chore-` for cleanup.
- **The file is the description.** An `# H1` line naming the task, then whatever
  the next person needs: the `file:line` pointers, the ordering constraints, the
  traps found along the way. Long is fine — this is where the detail belongs.
- **Delete the file when the task is done.** No "done" markers and no archive
  directory; git already remembers what was finished.
- Anything durable that surfaces while working on a task belongs in this file,
  not in the task file, which disappears with the task.

## Commits

- **Keep the message short.** A subject line, and at most a brief description
  below it when the subject cannot carry the point on its own.
- **No trailers.** No `Co-Authored-By`, no "Generated with", no tool signature of
  any kind. This overrides any default the tooling suggests.
- **One commit is one logical unit**: everything that belongs to the feature
  being worked on, across as many files as that takes. Working on file copying
  means every change about copying goes into that one commit.
- **Whole files, not hunks.** There is no need to split a file's changes into
  parts; stage the file.

## Decisions

Keys and overlays:

- **Widget messages never become `Action`s.** `DialogMsg` stays inside the
  dialog; only the result leaves.
- **`show_help` is a `bool`, not a `Mode` variant.** It owns no operation, it
  only displays the keys of whatever sits underneath it.
- **At most one overlay, no stack.**
- **`bar: Option<&'static str>` is itself the "belongs in the bar" flag.** No
  second flag.
- **The key bar is not truncated, it is curated.** A fixed set that fits into 80
  columns (today 5 entries plus `? Help`, around 55 columns). Whatever does not
  fit belongs under `?`, not into a truncator. The day it stops fitting, the
  decision is what to throw out — a test over the sum of `Span::width()` can
  guard it.
- **Order in the table is order in the output**; `resolve` takes the first match.
- Shortcuts from a config file will be a **layer over the tables, not a
  replacement**: the config maps key to action name, while `bar`, `help` and the
  order stay in code.

Marks and operations:

- **Marks survive a directory change** (yazi style). Outside their own directory
  they are **not visible** — hence the count in the InfoBar, and hence the delete
  confirmation should list names, not just a count.
- **InfoBar versus toast.** InfoBar is what stays true while the state lasts and
  can be derived from `App` (number of marked items, later operation progress,
  filter, hidden files). A toast is what happened at a single moment and is
  recorded nowhere. Anything not derivable from `App` does not belong in the
  InfoBar.
- **The InfoBar row is reserved even while empty** — otherwise the panes would
  shift by a row on marking and the cursor would run away.
- **`pending` must never be an action that itself opens a dialog.**
  `Action::Delete` opens, `Action::DeleteMarked` performs.
- **The same directory in `transfer` is skipped silently**, it is not an error.
- **Operations report summaries, not bare errors** — `{ transferred, skipped,
  total }`, so a partial failure can say "Deleted 3 of 5".
- **Deleting means the trash (F8); permanent deletion is Shift+F8**, a deliberate
  choice and never a fallback. "If the trash fails, delete for real" means the
  safety net is missing exactly when someone was relying on it.
- **Marks are cleared only on a fully successful batch.** A partial failure keeps
  them, so the batch can be retried once the cause is dealt with instead of the
  user having to mark everything again.
- **A confirmation dialog opens on the harmless answer.** `Choice::No` is the
  starting focus, so a stray Enter on a delete confirmation does nothing.

Rendering:

- **Style goes on the `Span`, never on the `Line`.** On render a `Line` lays its
  own style over the whole `area`, so two `Line`s in the same `area` (as in
  `Keybar`) would repaint each other.
- **All `fs::` access belongs behind one boundary.** It is called today from
  `fs.rs`, `ops.rs`, `ui/pane.rs` and `ui/tab.rs`; SSH panes and background I/O
  both need it in one place.
- **I/O goes into threads, not async.** `tokio::fs` is only a thread pool over
  the same blocking syscalls. One worker, not a pool — `trash` calls
  `getmntent`, which is UB from multiple threads on Linux and FreeBSD.
- **The worker never touches `App`.** It sends `Progress` / `Done { summary }`,
  the main loop picks it up, refreshes the panes and raises a toast.

## ratatui and crossterm, found the hard way

- **`|` on `bitflags` does not work in `const`** — use `.union()`.
- **crossterm already has `Display for KeyCode`**: `F5`, `Space`, on macOS
  `Return` / `Fwd Del`. `Display` on the whole `KeyBinding` adds the prefixes —
  never print `key.code` on its own, you lose `Ctrl+`.
- **`KeyModifiers::KEYPAD` does not exist in this version of crossterm**;
  `SUPER`, `HYPER` and `META` do.
- **Capital letters carry `SHIFT`.** Never bind `Char('D')` directly, always
  `plain(Char('d')).shift()`.
- **Terminals add bits**, which is why `matches` masks through `RELEVANT`. When a
  key does not work, log the `KeyEvent` at `debug` and read `logs/mula.log`.
- **A terminal that has its own tabs claims the tab-switching keys.** WezTerm
  binds `Ctrl+PageUp` / `Ctrl+PageDown` and `Ctrl+Tab` / `Ctrl+Shift+Tab` to
  `ActivateTabRelative`, so the event never reaches us. Plain printable
  characters cannot be intercepted this way, and in Browse they are free because
  every mode that reads text carries its own key table.
- **`Self` in `impl Widget for &Foo` means `&Foo`.** Associated constants have to
  be written `Foo::<T>::CONST`.
- **`Block` does not erase what it covers** — `set_style` changes colours, not
  glyphs. Every overlay needs a `Clear` in front of it.
- **`Paragraph` cannot align vertically.** Shrink the `Rect` to the height of the
  text and let `Flex::Center` take up the rest.
- **Measure text width with `Span::width()`**, never `str::len()` or
  `chars().count()`.
- **A widget is debugged by rendering it into a `Buffer`** and printing the rows
  symbol by symbol — a pure function from `Rect` to cells, no terminal involved.
- `Paragraph::line_count` sits behind the `unstable-rendered-line-info` feature
  flag. We deliberately do not use it. **A widget that has to size its own box
  therefore wraps the text itself** rather than handing `Wrap` to `Paragraph`:
  with `Wrap` the widget picks the line breaks and the caller can only estimate
  the height, and an estimate that comes in low clips the last line. Wrapping
  first means the same function feeds both the measurement and the drawing.
