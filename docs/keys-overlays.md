# Keys and overlays

Why the key tables and the overlay states look the way they do. The rules
themselves are in `CLAUDE.md`.

## A key is the first letter of what it does

Browse ran on function keys first — rename, view, edit, copy, move, new,
delete, cancel on F2 to F9 in that order. The order is the thing that was
wrong with it. It is not a fact about the actions, it is a fact about a row of
keys, so the only way to know which one copies is to have learned the row; the
labels in the bar are read off the screen every time instead of recalled. A
letter is recalled: `c` copy, `m` move, `f` find, `v` view, `e` edit, `n` new,
`d` delete, `r` rename.

Twenty-six letters and more actions than that, so the interesting part is the
collisions. Three of them mattered.

`f` was the filter and `/` was find. One `f` cannot be both, and find is the
one a user names with a word — so find took `f`, and the filter took `/`,
which is what typing into a listing means everywhere else. The filter loses
nothing by it: the key that is hard to guess is the key for the action nobody
looks up.

`c` was the column cycle and is now copy. The columns did not move to
`Shift+C`, even though the letter fits, because the two directions of that
mistake do not cost the same: reaching for the columns and landing on `c`
starts copying a batch, and reaching for copy and landing on `Shift+C` drops a
column. Shift is a weak guard against the expensive direction. `,` sits beside
`.`, which already toggles dotfiles, so the two view toggles are two
neighbouring keys and neither pretends to be a first letter.

`v` was the preview and is now the pager, which is what `v` says. That left
the preview needing a word of its own, and it had one already: `ui::preview`,
`fs::preview`, this directory's `preview.md`. "Quick View" was the last name
in the codebase that came from somewhere else, so the feature is called the
preview now and the key is `p`.

What is left on a function key is F1, and it is there because it cannot be
printable — see **Globals** below. Nothing else needs to be.

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
would otherwise draw `c Copy  y Copy`.

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
else lives under F1.

The guard test measures against 80 because **80 columns is an outside fact**,
not a number we picked. A test that measures a list against a constant we
raise whenever the list outgrows it proves nothing — it is a tautology, and it
stays green right up until the thing it guards is broken.

Any budget test in this codebase should measure against something it cannot
move.

What it measures is the row the widget drew: it renders into a `Buffer` and
reads the cells back. Adding the layout up a second time in the test was the
first shape, and it went stale the moment the help hint moved from the right
edge into the row — the arithmetic still agreed with itself and no longer with
`render`, which had gained a separator the test knew nothing about. A test that
re-implements what it checks can only catch the thing it was told about.

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

The hint leads the row rather than sitting right-aligned at the far end of it,
which is where it started. Right-aligned was the honest drawing of "this key
is not one of the mode's" — and on a wide terminal it put the one key a user
has to find if they know nothing else as far from the other keys as the row
allows, with empty cells between. Leading the row costs the same columns and
is read first.

The overlay resolves its own keys before the globals, so F1 closes what F1
opened rather than reopening it.

`Esc` is the next global anyone will reach for, and it is the one that will
hurt: `Cancel` in every overlay, and in Browse a cascade — the filter, then the
marks. The cascade came from a user pressing `Esc` to get rid of a filter
without being told to, which is the strongest evidence a binding gets. Order
matters and only in one direction: marks can stand for work done across several
directories, so the cheaper thing to lose goes first, and the second press is
still there for the rest. Making `Esc` global means taking it away from both.

## The favorites are one list, and the overlay does not own it

Both panels read the same list. Per-panel lists were the first shape asked
for, and the thing they cost is the thing they are for: a directory added on
the left is not there on the right, so it is added twice and then the two
halves drift. Which panel is involved is answered by where the cursor is when
the overlay opens, not by which list was drawn. Tabs settle it too — "per
panel" with tabs underneath immediately asks why not per tab, and there is no
answer that a single list does not already give.

The list lives on `App` and the mode carries only the cursor into it. It
outlives every time the overlay is opened and it is written to a file, which
is state no overlay should be holding; `Finder` keeps its hits because they
belong to the search that ended when the overlay closed.

Nothing is asked before a favorite is taken off. A confirmation exists to
stand between the user and something they cannot get back, and this removes a
line from a list — the directory is untouched. It is also the case that asking
would want a dialog over an overlay, which there is no stack for; that is a
consequence and not the reason.

Adding is `Shift+B` in Browse and not a key inside the overlay, so the path
being added is the one on the screen in front of the user. Nothing is typed:
a favorite is a path, and the day it grows a name of its own the overlay takes
a `TextInput` of its own the way `Finder` has one, rather than opening a
prompt over itself.

`b` and `Shift+B` rather than a `Ctrl+<letter>`: Ctrl is contested ground, and
a terminal or a user's own config can take the key before Mula ever sees it.
Printable characters in Browse cannot be intercepted that way.

## The favorites box is sized to the list

Not to a share of the screen. A share is a guess twice over: three favorites
in a box covering half the terminal say nothing about what is in them, and a
list of forty would be cut off at the same percentage on a tall terminal that
had room for all of it.

So the box asks for its widest path and a row per favorite, inside a band of
the terminal: a quarter to four fifths of it. Below the floor a list of two is
a box too small to read as a list; above the ceiling a list of eighty covers
the panels it was opened over. Both ends are shares rather than numbers of
cells, for the same reason the box is not a fixed size — and what the ceiling
cuts off scrolls, which is what the walk of every entry at 80x24 guards.

Everything it measures is measured: the widest path and the title with
`Span::width`, the borders and the padding off the `Block` itself, the status
line at the widest it can print for a list of that length.

A path too long for the box keeps its last three components behind an
ellipsis. Cutting the tail, which is what `ui::clip` does everywhere else,
would take exactly the part that tells one favorite from another: they are
paths, they share their beginnings, and the name at the end is the one the
user wrote the favorite down for.

## An empty overlay names the key that fills it

The line under an empty list says which key adds a directory, and it reads the
key out of the built Browse table the way the info bar reads the cancel key.
Spelling it into the sentence would be a fourth place a key is written down,
and the config can move it.

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
