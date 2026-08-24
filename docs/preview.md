# Preview

Why Quick View works the way it does. The rules themselves are in `CLAUDE.md`.

## A view toggle, never a `Mode`

Quick View replaces the panel opposite the cursor. Making it a `Mode` would
take the browse keys away with it; as a toggle on `App`, every operation keeps
working while a preview is up.

The consequence is that a copy still goes into the hidden panel's directory.
If that ever bites, the fix is to show the target path — not to forbid the
operation.

## Settled once per pass

What to preview is a function of the focused side, its active tab and where
the cursor is. It is worked out once per pass of the loop rather than from
each action that moves one of those three, so there is one place that can be
wrong instead of five that can disagree.

## Cleared to "loading", unlike a pane

A pane keeps its old listing while reloading, because blanking it moves the
cursor. A preview has no cursor to lose, and a stale one is actively
misleading: it draws one file's content under another file's name.

## What a file is comes from its first bytes

Not from its extension. A file claiming a format it does not keep falls back
to its hex dump rather than to an error — the hex dump is always a truthful
answer, and "this is not really a PNG" is something the bytes already say.

A fifo, socket or device is turned away on `symlink_metadata` and never
opened. `File::open` on a fifo blocks until somebody writes to it, which for a
file manager is never.

## Half blocks are the renderer, not a consolation

`▀` with a foreground over a background is the only block trick that gives
exact per-pixel colour. It needs no terminal detection and it stays inside
`Buffer`, which means the widget is still a pure function from `Rect` to cells
and can be tested by rendering into a buffer.

The thread sends a bounded RGBA bitmap and the widget fits it to the area. A
graphics protocol (Kitty, Sixel) is therefore a second backend over the same
bitmap, not a rewrite.

## A placement is state, not drawing

`Buffer` is declarative: draw the same thing every pass and ratatui works out
what changed. A graphics protocol is not. A placement is an object the terminal
holds and goes on drawing under whatever text arrives later, and sending the
same one twice leaves two.

Ratatui cannot help here, because no cell of a picture it never drew ever
changes. So the screen is reconciled the way `Buffer` is diffed, one level up:
what should be on it against what is. Everything that would otherwise be a
separate case — the cursor moving, the panel changing sides, the window being
resized, an overlay opening, Quick View closing — becomes the same comparison
of two `Placement`s, and two passes that agree write nothing at all.

Nothing writes because they agree is what keeps a bitmap off the wire sixty
times a second. It is the property to hold on to; the rest follows from it.

A placement is identified by a number that turns on every answer the reader
gives, rather than by the path it came from. The same file read twice is two
pictures, and the second one has to reach the screen.

It also outlives the process that made it, so it is taken back on the way out
— including the way out through a panic, ahead of the hook that restores the
terminal, while the screen it was placed on is still the one in front.

## The widget says where, the surface says how

Writing escape sequences from inside `render` would end the widget being a
function from a `Rect` to cells. So it stays one and gives back a second
answer: the area it wants a picture in. Nothing else changes hands.

Fitting a bitmap to an area is the same arithmetic whichever backend draws it,
which is why it stays in the widget rather than moving to `App` — half blocks
measure in half-pixels and a protocol measures in cell pixels, and that is the
whole of the difference.

The cells under a picture are cleared rather than left alone. They have to be
blank, and they have to be *known* to be blank: what ratatui takes for
unchanged it never writes out, so anything left there would show through the
moment the picture goes.

## What can be drawn is not what a terminal has

`Protocol` names what the panel knows how to write, not what terminals offer.
A protocol detected but unimplemented would blank the half blocks and put
nothing in their place, so it must not be nameable until it can be written.

## Transparency is composited at drawing time

What sits behind the panel is the terminal's own colour, and the reader thread
cannot know it. So a cell that no part of the picture reaches is left
unpainted rather than filled with a guess.

## Shrinking with `thumbnail`, not `resize(Triangle)`

At 4000 → 512 px, `thumbnail` takes less than half the time, and an area
average is what a reduction that large wants anyway.

Worth recording because the guess was wrong: in a debug build the resize cost
three times the PNG decode, not the other way round. The intuition that
decoding dominates does not survive measurement here.
