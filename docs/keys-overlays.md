# Keys and overlays

Why the key tables and the overlay states look the way they do. The rules
themselves are in `CLAUDE.md`.

## `show_help` is a `bool`, not a `Mode`

Help only displays keys. It reads nothing, it decides nothing, and it takes no
input of its own, so it does not need a state — a flag is the whole of it.

This holds only while help fits on one screen. Paginating it would give it
arrow keys and an Esc of its own, and at that point it becomes a `Mode` like
any other overlay. That is a decision to make deliberately, not to drift into.

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
what is underneath is a pane, always. A stack would need a rule for what each
key means at each depth, and nothing here has ever wanted one.

## `DialogMsg` never becomes an `Action`

A dialog's messages describe moving around inside the dialog. Only its
*result* leaves. Letting `DialogMsg` widen into `Action` would put dialog
navigation into the same table as file operations, and then every key handler
would have to know which of the two it was looking at.
