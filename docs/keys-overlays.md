# Keys and overlays

Why the key tables and the overlay states look the way they do. The rules
themselves are in `CLAUDE.md`.

## Help is not a `Mode`

Help draws over whatever mode is running and lists *that mode's* keys, so the
mode has to stay alive underneath it. Turning help into a `Mode` would replace
the thing it is describing.

It used to be a bare `bool` on the argument that it takes no input of its own.
That stopped being true when it grew a scroll: it now has an offset, its own
key table, and keys that mean something different from the ones it is drawing.
None of that makes it a mode. It is one flag with state, read before `mode` is
looked at, and every key that it does not bind closes it.

The scroll came from the box being sized to the terminal instead of to a
constant. A fixed height is a guess at how tall the reader's terminal is, and
the number of rows is the user's to decide now that a config can add keys —
no constant can be right about either.

## A catalogue, not a table

`BROWSE_ACTIONS` is the list; the table `resolve` walks is built from it at
startup. An entry carries the name a config calls it by, its default keys, its
bar label and its help line, and a config replaces the keys and nothing else.

The direction matters. Mapping a key to an action would be the shape every
editor's config has, but it answers the wrong question. What a user sits down
to do is *move* an action — copy onto `y` — and key-to-action cannot say that
in one line: the old key still resolves, so a second entry has to unbind it,
which needs a sentinel value that means nothing. Action-to-keys says it in one
line, and an empty list is the unbinding with nothing invented for it.

The other half is that it gives `validate` something to check. Keys are unique
in a TOML table by construction, so key-to-action could not produce a duplicate
if it tried, and the function would have stayed dead. Two actions claiming one
key is a mistake a user can really make, and now it is caught.

Only Browse is configurable. The overlays run on `Esc`, `Enter` and the arrows,
and a table nobody wants to move is not worth a format.

Nothing about a rebound action changes but the key: it keeps its label, its
help line and its place in both listings, because those never left the
catalogue. Only the first key of an action carries the bar label — two keys
would otherwise draw `F5 Copy  y Copy`.

A config that cannot be used costs the user their keys and never their file
manager: the defaults stand and the reason arrives as a toast. It is all or
nothing rather than line by line, because half a config is a state the user
cannot picture from the file in front of them.

## One spelling for a key

`Display` and `FromStr` on `KeyBinding` are the same names, so what F1 prints
is what a config can say. That took the three codes crossterm renames on macOS
— `Delete` for Backspace, `Fwd Del` for Delete, `Return` for Enter — away from
it: a config written on a Mac has to load on Linux, and a help overlay naming a
key the config would reject is worse than a native spelling is good.

A test walks every table and round-trips every key through its own name. A key
that fails it is one nobody could bind.

## The key bar is curated, not truncated

The bar carries a fixed set of keys chosen to fit 80 columns, and everything
else lives under `?`.

The guard test sums `Span::width()` against 80 because **80 columns is an
outside fact**, not a number we picked. A test that measures a list against a
constant we raise whenever the list outgrows it proves nothing — it is a
tautology, and it stays green right up until the thing it guards is broken.

Any budget test in this codebase should measure against something it cannot
move.

## At most one overlay

No stack. An overlay draws a `Clear` and takes the screen; when it closes,
what is underneath is a pane, always.

The reason is not keys. "The topmost overlay takes every key" would be a
complete rule and would cost nothing. The reason is that *cancelling can name
its destination*: four places in `App` write `self.mode = Mode::Browse`, and
they can only do that because there is nowhere else to land. A stack turns each
of them into a pop and takes that knowledge away from them.

Nothing has ever wanted the stack. The case that would want it is a dialog that
has to open a dialog, and `feat-overwrite-on-transfer` is where to expect it.

## Globals

A global is a key that works in every mode. `GLOBAL_KEYS` is one table, tried
before the mode's own, and F1 is the only entry.

It is one table rather than a help entry in each of the four, because four
copies of one key drift and because the modes that read text cannot carry it at
all: `?` in a prompt is a character being typed. That is also why the key is a
function key. Nothing printable can be global while any mode reads text.

What a global needs and `validate` cannot give is a check **across** tables: no
mode may bind a global key, since it would never see it. `validate` compares
entries inside one table, so this is a second test, over every table there is.

Two things follow from being global rather than modal. The keybar draws the
hint into every mode's row, so the bar of a dialog is now measured beside it
against the same 80 columns. And the help overlay lists the globals under the
mode's own keys — it claims to list every key working right now, and the key
that opened it is one of those.

The overlay resolves its own keys before the globals, so F1 closes what F1
opened rather than reopening it.

`Esc` is the next global anyone will reach for, and it is the one that will
hurt: `Cancel` in every overlay, and in Browse a cascade — the filter, then the
marks. The cascade came from a user pressing `Esc` to get rid of a filter
without being told to, which is the strongest evidence a binding gets. Order
matters and only in one direction: marks can stand for work done across several
directories, so the cheaper thing to lose goes first, and the second press is
still there for the rest. Making `Esc` global means taking it away from both.

## `DialogMsg` never becomes an `Action`

A dialog's messages describe moving around inside the dialog. Only its
*result* leaves. Letting `DialogMsg` widen into `Action` would put dialog
navigation into the same table as file operations, and then every key handler
would have to know which of the two it was looking at.

## Asking and doing are two things

`Action::Delete` opens the confirmation; `ConfirmTarget::Delete` performs it.
The split looks like a workaround for having no overlay stack, and the rule
used to say so, but it isn't one: whatever the overlays do, the answer to a
confirmation has to dispatch something that does *not* ask again, or the
dialog reopens forever.

What matters is which of the two halves a key can reach. `Mode::Input` had it
right first — `InputTarget` is its own type, so no key resolves to "rename this
tab, skipping the prompt". `Mode::Confirm` carried an `Action`, and the only
thing keeping `QuitAnyway` off a key was that nobody had written the binding.
The config would have handed that binding to the user: a name in the catalogue
is a name a stranger's config file can ask for.
