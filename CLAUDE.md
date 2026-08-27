# mula

Dual-pane TUI file manager. Rust + ratatui + crossterm.

`tasks/` holds open work, one file per task, excluded via `.git/info/exclude` —
never committed. This file and `docs/` are what stay, so nothing durable goes
in a task file.

`docs/` holds the reasoning behind the rules below, one file per subsystem:
`background-io.md`, `preview.md`, `keys-overlays.md`, `marks-operations.md`.
Read the one covering what you are about to change. A rule that acquires a
rationale worth keeping puts it there, not here.

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

## Rules

Constraints on new code. Why each one holds is in `docs/`.

These are defaults, not gospel. Where the code and a rule disagree, the code is
the evidence — say so instead of silently following either one. Most of them
are conventions and cannot go stale; the few that rest on a fact about the
world carry a **Holds while** clause, and when that stops being true the rule
is open again, not broken.

**Errors/invariants**
- "Cannot happen" = type-guaranteed or exhaustive match, not "nothing else does
  it yet." Fix fragile invariants at their source instead of wrapping in
  `Result` or swallowing the case.

**Keys/overlays**
- At most one overlay, no stack. **Holds while** nothing needs to open an
  overlay over another one; paginated help is the case that would end it.
- `DialogMsg` never becomes an `Action`; only the result leaves the dialog.
- Key bar is curated, not truncated: a fixed set fitting 80 columns, rest under
  `?`. Guard with a test over summed `Span::width()`, never against a constant
  of our own.

**Marks/operations**
- `pending` is never an action that opens a dialog: `Action::Delete` opens,
  `Action::DeleteMarked` performs. This follows from there being no overlay
  stack; it goes when that goes.
- Nothing non-derivable goes in the InfoBar: it must be computable from `App`
  and true while the state lasts. A one-off event is a toast.
- Operations report summaries (`{ transferred, skipped, total }`), not bare
  errors, so a partial failure can say "Deleted 3 of 5".
- Confirmation dialogs open on `Choice::No`.

**Background I/O**
- All `fs::` access goes behind one boundary.
- Threads, not async. **Holds while** the I/O is local file syscalls, which
  have no non-blocking API; SSH panes are sockets and do.
- One worker for mutations, no pool. **Holds while** a batch stays on one
  device. One `Reader` instance per answer that can be outstanding on its own,
  each with its own generation — the two panels are two of those, not one
  "listing" role.
- A pane keeps its listing while the next one is read, and asks for it once per
  pass of the loop rather than from the action that moved it.
- A job owns a snapshot of its paths, taken when it is queued, and never reads
  `App` again. Nothing is forbidden while a job runs: no modal busy state, no
  read-only mode.
- The worker never touches `App`; it sends `Progress`/`Done { summary }` and
  the main loop acts on them.

**Preview**
- Quick View is a view toggle on `App`, never a `Mode`: browse keys and every
  operation under them keep working.
- What to preview is settled once per pass of the loop, not from each action
  that moves the cursor, the tab or the side.
- A fifo, socket or device is turned away on `symlink_metadata`, never opened.
- What a file is comes from its first bytes; a file that lies about its format
  falls back to a hex dump, not to an error.

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
- Style goes on `Span`, never `Line` — a `Line` repaints its whole area.
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
