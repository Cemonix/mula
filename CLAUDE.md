# mula

Dual-pane TUI file manager. Rust + ratatui + crossterm.

`tasks/` holds open work, one file per task, excluded via `.git/info/exclude` —
never committed. This file and `docs/` are what stay, so nothing durable goes
in a task file.

`docs/` holds the reasoning behind the rules below, one file per subsystem:
`background-io.md`, `preview.md`, `keys-overlays.md`, `marks-operations.md`,
`opening.md`.
Read the one covering what you are about to change. A rule that acquires a
rationale worth keeping puts it there, not here.

## Architecture

- One key table per focus, generating three things: key→message translation,
  the bottom bar, and the F1 help. Single source, so they can't drift.
- `Mode` owns the overlay: a dialog is a state, not a field on `App`.
- No vim-like modes for the user. Modality must be visible (bottom bar, F1).
  Total Commander style keys (F1/F5/F6/F8), not mnemonics.

## Comments

A comment explains *how* the code works, never *why* it was written that way.
Rationale/trade-offs/alternatives go under **Decisions** below, not in source.
- `///` docstrings describe the contract: inputs, output, side effects.
- Don't restate the code. No commented-out code, no `TODO:` (use `tasks/`).
- Words like *because/better/instead of/we chose* signal it belongs in Decisions.

## Working on this codebase

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
- At most one overlay, no stack. A question with nowhere to go waits on `App`
  until a pass of the loop finds `Mode::Browse`, rather than taking the screen
  from a dialog already on it. Help is not that case either: it draws over a
  mode and describes it, so the mode stays alive underneath and nothing is
  stacked. **Holds while** no dialog has to be answered before the one under
  it — waiting is only enough because a collision is never asked in flight.
- `DialogMsg` never becomes an `Action`; only the result leaves the dialog.
- Key bar is curated, not truncated: a fixed set fitting 80 columns beside the
  help hint every mode draws, rest under F1. Guard with a test over summed
  `Span::width()`, never against a constant of our own.
- An overlay is sized to the terminal and scrolls, never to a constant of ours.
  Guard against 80x24 — the smallest that counts — by walking the whole table
  and asserting every entry can be reached.
- Browse is a catalogue, not a table: an action carries its name, its default
  keys, its bar label and its help, and a config replaces the keys alone. The
  config names actions and gives them keys, never the reverse — moving an
  action is one line that way, and it is what gives `validate` a duplicate to
  find. A config that cannot be used falls back whole, never line by line.
- `Display` and `FromStr` on `KeyBinding` are one spelling, so what F1 prints
  is what a config can say. Guard it by round-tripping every table's keys.
- A key working in every mode lives in `GLOBAL_KEYS`, not in each table.
  Resolved before the mode's, so it needs the invariant `validate` cannot give:
  no mode may bind a global key. Nothing printable can be global while a mode
  reads text, which is why help is F1.

**Marks/operations**
- Asking and doing are two things, and only the asking half is an `Action`.
  `Action::Delete` opens the dialog; `ConfirmTarget::Delete` carries it out and
  no key table can name it. Same shape as `InputTarget`. This does not come
  from there being no overlay stack — a confirmation needs a terminal step
  whatever the overlays do — it comes from a key never being allowed to reach
  past the question.
- Nothing non-derivable goes in the InfoBar: it must be computable from `App`
  and true while the state lasts. A one-off event is a toast. Derivable is not
  enough — the bar carries what the screen does not already show. Marks can be
  in a directory that was left, a queue and a progress bar are invisible by
  nature; a filter you can see the result of is not.
- Operations report summaries (`{ transferred, skipped, total }`), not bare
  errors, so a partial failure can say "Deleted 3 of 5".
- Confirmation dialogs open on `Choice::No`.
- Deleting means the trash (F8); permanent deletion is Shift+F8, and the trash
  is never a fallback — failing to reach it is a failure to delete, reported as
  one. On macOS that is `NsFileManager` rather than our own move, so what the
  Finder offers to put back is what Mula took away.
- A transfer flattens: every item lands under its own name, whatever it was
  nested in. **Holds while** marks are absolute and may come from any
  directory, which leaves no root to keep a structure relative to.
- A collision is answered once per batch and the answer rides on the job as a
  setting. Never asked in flight — nothing is forbidden while a job runs, so
  that is the one question that could land on a dialog the user opened. Never
  asked at queue time either: the jobs ahead move the destination first.
- What a job may overwrite is what existed when it started, taken in its own
  pre-walk. What the job itself wrote is never overwritten, whatever was
  answered — a flattened batch can collide with itself.
- Directory on directory is a descent, never an overwrite; a type mismatch and
  a symlink in the destination are collisions. `remove_recursive` never stands
  behind "overwrite".
- Nothing is asked when nothing is at stake: a merge with no clashing leaf runs
  in silence.

**Opening**
- The action writes down what to open; the loop hands the terminal over, once
  per pass. Only `run` holds the terminal.
- Two launch shapes, never one. A detached program gets `/dev/null` and its own
  session, is never waited for, and is `try_wait`ed once a pass so it cannot
  stay in the process table.
- What opens a file follows from what the file is, not from which key was
  pressed. F3 and F4 are the named exceptions.
- Entering asks the reader one question. `Listed::NotADirectory` is an answer,
  not a failure, and only `Intent::Enter` acts on it.
- Nothing reaches a shell. `$EDITOR` splits on whitespace and the path is one
  element of `argv`.
- Paths handed to another program are absolute. **Holds while** listings grow
  from `env::current_dir()`; a relative one could reach a program as an option.
- Mula ignores SIGINT/SIGQUIT while another program owns the terminal, and
  `pre_exec` puts both back to their default in that program.

**Background I/O**
- All `fs::` access goes behind one boundary.
- A listing carries what to read. Metadata is a `stat` per entry, so a caller
  that draws no column asks for none, and a pane reads again only when what it
  holds is thinner than what it draws, never when it is thicker.
- Threads, not async. **Holds while** the I/O is local file syscalls, which
  have no non-blocking API; SSH panes are sockets and do.
- One worker for mutations, no pool. **Holds while** a batch stays on one
  device. One `Reader` instance per answer that can be outstanding on its own,
  each with its own generation — the two panels are two of those, not one
  "listing" role.
- What is on screen is a view — indices into the listing — and the cursor
  indexes the view, never the listing. Filtering happens above the read, so a
  criterion typed letter by letter costs no disk. One function rebuilds the
  view and refits the cursor, and every change to listing or criteria goes
  through it: `select_prev`/`select_next` wrap with `%` and would divide by
  zero on a view that emptied under a cursor. The parent entry is never
  filtered out — it is the way out of a directory whose own name is hidden.
- A pane keeps its listing while the next one is read, and asks for it once per
  pass of the loop rather than from the action that moved it.
- A refresh keeps the cursor on the path it stands on, falling back to the
  index when that entry is gone. Only a refresh can: a pane sent somewhere else
  has no path to keep.
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
- Capital letters carry `SHIFT` *and* arrive uppercase — bind
  `plain(Char('D')).shift()`, never `Char('D')` on its own and never
  `Char('d')).shift()`. crossterm's `char_code_to_event` sets `SHIFT` from
  `c.is_uppercase()` and leaves the letter as typed. **Holds while** we don't
  push the Kitty keyboard flags: with alternate keys reported, crossterm moves
  the shifted char into the code and clears `SHIFT` again.
- Terminals add bits; `matches` masks through `RELEVANT`. Debug a dead key by
  logging the `KeyEvent` at `debug` (`logs/mula.log`).
- A terminal with its own tabs claims tab-switching keys (WezTerm:
  `Ctrl+PageUp/Down`, `Ctrl+Tab`/`Ctrl+Shift+Tab`, and `Ctrl+W` closes its tab).
  Plain printable chars can't be intercepted this way; free in Browse since
  every text-reading mode has its own key table. The tab family sits on
  `t`/`w`/`r`/`[`/`]` for exactly that reason.
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
- `Terminal::clear` asks the terminal where its cursor is — a query answered on
  the same standard input the keys arrive on. `Terminal::resize` resets the
  back buffer too and costs no round trip.
- `Paragraph::line_count` needs the `unstable-rendered-line-info` feature; we
  don't use it. A widget that sizes its own box wraps text itself instead of
  handing `Wrap` to `Paragraph` — same function feeds measurement and drawing.
- `ListState::select_last` stores `usize::MAX` and waits for a render to cut it
  down. Anything reading the selection earlier in the pass — the preview does —
  finds no entry there. Work the index out from the listing instead.
