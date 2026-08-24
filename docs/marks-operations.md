# Marks and operations

Why marking and batch operations behave the way they do. The rules themselves
are in `CLAUDE.md`.

## Marks survive a directory change

Yazi's behaviour: a mark is an absolute path in a set on the `Tab`, so leaving
the directory does not drop it.

The cost is that a mark is invisible once you are outside its own directory.
That is why the InfoBar carries a count, and why the delete confirmation lists
**names rather than a number** — a bare "delete 5 items" would be asking the
user to confirm something they cannot see.

## InfoBar versus toast

Two different things, and the line between them is what may go in each:

- **InfoBar** — derivable from `App`, and true for as long as the state lasts.
  A mark count, a queue length, a running job.
- **Toast** — a one-off event, recorded nowhere and gone when it expires.

Anything not derivable from `App` cannot go in the InfoBar, because there
would be nothing to redraw it from on the next frame.

The row stays reserved even when empty. Letting it collapse would shift the
panes and move the cursor the moment the first item is marked.

## Marks clear when a batch is queued

Not when it finishes. The user goes on marking while the copy runs, and a
batch that cleared marks on completion would wipe whatever was marked in the
meantime.

`Done` carries the paths that failed and those are marked again. Marks are
absolute paths, so this works even if the user walked somewhere else while the
job ran — which is what makes a partial failure retryable rather than
something to reconstruct by hand.

## Summaries, not bare errors

An operation returns `{ transferred, skipped, total }`. A bare `Result` can
only say that a batch failed; a summary can say "Deleted 3 of 5", which is the
difference between a report and an alarm.

The two denominators do not always agree, and that is deliberate: the summary
counts items, while a transfer's progress bar fills by bytes. They reach their
end at different moments because they are measuring different things.

## `pending` never opens a dialog

`Action::Delete` opens the confirmation; `Action::DeleteMarked` performs it.
If `pending` could itself be an action that opens a dialog, confirming one
dialog could open another, and there is no stack to hold them.

## Confirmation dialogs open on `Choice::No`

The harmless answer takes the default focus, so Enter on reflex cannot delete
anything.
