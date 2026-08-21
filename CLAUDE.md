# mula

Dual-pane TUI file manager. Rust + ratatui + crossterm.

`tasks/` holds open work, one file per task, excluded via `.git/info/exclude` —
never committed. This file is what stays, so nothing durable goes in a task file.

## Architecture

- One key table per focus, generating three things: key→message translation,
  the bottom bar, and `?` help. Single source, so they can't drift.
- `Mode` owns the overlay: a dialog is a state, not a field on `App`.
- No vim-like modes for the user. Modality must be visible (bottom bar, `?`).
  Total Commander style keys (F5/F6/F8), not mnemonics.

## Comments

A comment explains *how* the code works, never *why* it was written that way.
Rationale/trade-offs/alternatives go under **Decisions** below, not in source.
- `///` docstrings describe the contract: inputs, output, side effects.
- Don't restate the code. No commented-out code, no `TODO:` (use `tasks/`).
- Words like *because/better/instead of/we chose* signal it belongs in Decisions.

## Working on this codebase

- User-visible text (labels, help, dialogs, toasts) and key tables are written
  by the repo owner, not an agent. Leave placeholder wording alone; flag it
  instead. Logic and widgets are the working area.
- Widgets are dumb: `App` computes content, the widget draws it. Never `&App`.
- Test a widget by rendering into a `Buffer` and asserting on symbols.
- `cargo fmt` and `cargo test` before reporting anything done.

## Tasks

- One task, one file: `bug-`/`feat-`/`chore-` + kebab-case slug.
- The file is the description — `# H1`, then whatever the next person needs.
- Delete the file when done. No "done" markers, git remembers.

## Commits

- Short message: subject line, brief body only if needed.
- No trailers (no Co-Authored-By, no "Generated with", no tool signature).
- One commit = one logical unit, across as many files as needed.
- Whole files, not hunks — stage the file.

## Decisions

**Errors/invariants**
- "Cannot happen" = type-guaranteed or exhaustive match, not "nothing else does
  it yet." Fix fragile invariants at their source instead of wrapping in
  `Result` or swallowing the case.

**Keys/overlays**
- `DialogMsg` never becomes an `Action`; only the result leaves the dialog.
- `show_help` is a `bool`, not a `Mode` variant — it only displays keys.
- At most one overlay, no stack.
- `bar: Option<&'static str>` is itself the "belongs in the bar" flag.
- Key bar is curated, not truncated: fixed set fitting 80 columns (~55 today).
  Rest goes under `?`. Guard with a test over summed `Span::width()`.
- Table order = output order; `resolve` takes the first match.
- Config file shortcuts are a layer over the tables (key→action name), not a
  replacement; `bar`/`help`/order stay in code.

**Marks/operations**
- Marks survive a directory change (yazi style); invisible outside their own
  dir — hence InfoBar count and delete confirmation lists names, not a count.
- InfoBar = derivable from `App` and true while state lasts. Toast = a
  one-off event, recorded nowhere. Nothing non-derivable goes in InfoBar.
- InfoBar row is reserved even empty (avoids pane/cursor shift on marking).
- `pending` is never an action that opens a dialog: `Action::Delete` opens,
  `Action::DeleteMarked` performs.
- Same directory in `transfer` is skipped silently, not an error.
- Operations report summaries (`{ transferred, skipped, total }`), not bare
  errors — so a partial failure can say "Deleted 3 of 5".
- Marks clear only on full batch success, so a partial failure can be retried.
- Confirmation dialogs open on `Choice::No` (harmless answer as default focus).

**Rendering**
- Style goes on `Span`, never `Line` (a `Line` repaints its whole area).
- All `fs::` access behind one boundary (needed by SSH panes, background I/O).
- I/O in threads, not async — `tokio::fs` is just a thread pool over the same
  syscalls. One worker, not a pool: `trash` calls `getmntent`, UB from
  multiple threads on Linux/FreeBSD.
- The worker never touches `App`; it sends `Progress`/`Done { summary }`, the
  main loop refreshes panes and raises a toast.

## ratatui/crossterm gotchas

- `|` on `bitflags` doesn't work in `const` — use `.union()`.
- crossterm has `Display for KeyCode` (`F5`, `Space`, macOS `Return`/`Fwd Del`);
  `Display` on `KeyBinding` adds prefixes — never print `key.code` alone.
- `KeyModifiers::KEYPAD` doesn't exist in this crossterm version; `SUPER`,
  `HYPER`, `META` do.
- Capital letters carry `SHIFT` — bind `plain(Char('d')).shift()`, never
  `Char('D')` directly.
- Terminals add bits; `matches` masks through `RELEVANT`. Debug a dead key by
  logging the `KeyEvent` at `debug` (`logs/mula.log`).
- A terminal with its own tabs claims tab-switching keys (WezTerm:
  `Ctrl+PageUp/Down`, `Ctrl+Tab`/`Ctrl+Shift+Tab`). Plain printable chars can't
  be intercepted this way; free in Browse since every text-reading mode has
  its own key table.
- `Self` in `impl Widget for &Foo` means `&Foo`; associated consts need
  `Foo::<T>::CONST`.
- `Block` doesn't erase what it covers (`set_style` changes colours, not
  glyphs) — every overlay needs a `Clear` in front.
- `Paragraph` can't align vertically — shrink the `Rect` to text height, let
  `Flex::Center` take the rest.
- Measure text width with `Span::width()`, never `str::len()`/`chars().count()`.
- Debug a widget by rendering into a `Buffer` and printing rows symbol by
  symbol — pure function from `Rect` to cells, no terminal needed.
- `Paragraph::line_count` needs the `unstable-rendered-line-info` feature; we
  don't use it. A widget that sizes its own box wraps text itself instead of
  handing `Wrap` to `Paragraph` — same function feeds measurement and drawing.
