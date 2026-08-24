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
