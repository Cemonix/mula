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
- Marks clear when a batch is queued, not when it finishes — the user keeps
  marking while it runs. `Done` carries the failed paths and they are marked
  again, so a partial failure is still retryable.
- Confirmation dialogs open on `Choice::No` (harmless answer as default focus).

**Rendering**
- Style goes on `Span`, never `Line` (a `Line` repaints its whole area).

**Background I/O**
- All `fs::` access behind one boundary (needed by SSH panes, background I/O).
- I/O in threads, not async — regular files have no non-blocking API (`epoll`
  and `kqueue` can't watch them), so `tokio::fs` is a thread pool over the same
  blocking syscalls. Cancelling a copy is an `AtomicBool` between entries
  either way, since one `fs::copy` is a single syscall.
- One worker, not a pool. One FIFO queue makes "which operation wins" a
  question of which key was pressed first, so overlapping paths can't conflict
  and nothing needs locking. Parallel I/O on one disk doesn't pay; two devices
  at once is what a second Mula instance is for.
- Every mutation goes through the worker. Reads stay synchronous until SSH
  panes need otherwise — a `Directory::read` queued behind a 4 GB copy would
  freeze navigation. A second worker for reads then, since only writes need
  ordering.
- A job owns a snapshot of its paths, taken when it is queued, and never reads
  `App` again. Whatever the user changes afterwards can only turn into a
  skipped entry, so nothing has to be forbidden while a job runs: no modal
  busy state, no read-only mode.
- The worker never touches `App`; it sends `Progress`/`Done { summary }`, the
  main loop refreshes panes and raises a toast.
- Last wins is the mechanism (`Reader<J: ReadJob>`: generations, channels,
  thread), not the discipline. Folding messages into an answer stays with each
  job — a search's hits are a delta and every live batch is kept, a preview is
  a snapshot and only the newest counts.
- One reader instance per role, each with its own generation. One shared
  counter would cancel a running walk on every cursor move and would hold
  together only because the find overlay happens to be modal.

**Preview**
- Quick View replaces the panel opposite the cursor and is a view toggle on
  `App`, never a `Mode`: browse keys and every operation under them keep
  working. A copy still goes into the hidden panel's directory; if that ever
  bites, show the target path — don't forbid the operation.
- What to preview is settled once per pass of the loop from side, tab and
  cursor, not from each action that moves one of them.
- Cleared to "loading" on request. Panes keep their old listing while reloading
  because blanking one moves the cursor; a preview has no cursor, and a stale
  one draws one file's content under another file's name.
- A fifo, socket or device is turned away on `symlink_metadata` and never
  opened — `File::open` on one blocks until somebody writes, which is never.
- What a file is comes from its first bytes. A file claiming a format it does
  not keep falls back to its hex dump rather than to an error.
- Half blocks (`▀` fg over bg) are the renderer, not a consolation: the only
  block trick with exact per-pixel colour, no terminal detection, and it stays
  inside `Buffer`. The thread sends a bounded RGBA bitmap and the widget fits
  it, so a graphics protocol is a second backend over the same bitmap.
- Transparency is composited at drawing time. What is behind the panel is the
  terminal's own colour and unknowable to the reader, so a cell no part of the
  picture reaches is left unpainted rather than filled with a guess.

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
- `Ctrl+<letter>` is contested ground even beyond terminal defaults — a user's
  own config takes what it likes, and the key then never reaches the app at
  all. Function keys are the safe family, which is what Cancel sits on.
- `Self` in `impl Widget for &Foo` means `&Foo`; associated consts need
  `Foo::<T>::CONST`.
- `Block` doesn't erase what it covers (`set_style` changes colours, not
  glyphs) — every overlay needs a `Clear` in front.
- `Paragraph` can't align vertically — shrink the `Rect` to text height, let
  `Flex::Center` take the rest.
- Measure text width with `Span::width()`, never `str::len()`/`chars().count()`.
- Debug a widget by rendering into a `Buffer` and printing rows symbol by
  symbol — pure function from `Rect` to cells, no terminal needed.
- Only changed cells are written out. Driving the real binary in a pty and
  reading the tail shows nothing once the frame settles — capture across the
  change, not after it.
- `Paragraph::line_count` needs the `unstable-rendered-line-info` feature; we
  don't use it. A widget that sizes its own box wraps text itself instead of
  handing `Wrap` to `Paragraph` — same function feeds measurement and drawing.
