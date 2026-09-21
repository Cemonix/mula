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

## A transfer flattens

Marks are absolute and outlive a directory change, so one batch can hold items
from several directories at once. `transfer_item` joins the destination with
the item's own file name and nothing else, so every item lands directly in the
target directory whatever it was nested in.

Keeping the structure would need a root to keep it relative to, and there is
none. Two marks in `A` and `A/sub` share `A`, but a mark put back from a failed
batch can sit anywhere the user has been, and the common ancestor of those
is `/`.

What follows from it is that a batch can collide with itself: `A/x.txt` and
`A/sub/x.txt` both want to land on `dest/x.txt`.

## A collision is answered once, and the answer rides on the job

Nothing is forbidden while a job runs — no modal busy state — so the user may
have a dialog of their own open when a collision is met. A worker that could
ask in flight would be the one thing in the app wanting two overlays at once.

Asked before the batch or after it, the user is in Browse and it is an ordinary
dialog. Before is still wrong: jobs run serially, and the jobs ahead change the
destination between queueing and starting. An answer given at queue time is
about a world that no longer holds by the time the job runs.

So the job runs, transfers what does not collide, and brings the collisions
back in its `Outcome`. The main loop asks, and a follow-up job carries the
answer. That job takes its own snapshot when *it* starts, so a destination that
changed again is met with a fresh question rather than a stale answer.

## What may be overwritten is what was there when the job started

The pre-walk that weighs the tree also records which destinations already
exist, and only those may be overwritten. Anything appearing afterwards —
including what the job itself has just written — is never overwritten, whatever
the answer was.

Without it a batch that flattens quietly eats its own output: `A/x.txt` and
`A/sub/x.txt` both land on `dest/x.txt`, and "overwrite" would leave one file
with no telling which.

## Directories merge, they never replace

Names are unique within a directory, so at each name there is exactly one pair
— what the source has, and what the destination has or has not. Four cases and
no more:

| source | destination | |
| --- | --- | --- |
| anything | nothing | copy it, nothing to ask |
| directory | directory | descend, and pair up a level down |
| file | file | collision, the policy answers |
| directory | file, or the reverse | collision that cannot be overwritten |

Directory on directory is a descent. Written once for files, "overwrite" is
`remove_recursive` and then a copy, and `remove_recursive` on a directory is
`remove_dir_all`: it takes the files that were only in the destination, which
the user was never shown. Merging cannot be assembled out of two steps the user
could take on their own; replacing can — delete, then copy.

A symlink in the destination is a collision rather than a descent, which is
what `symlink_metadata` is for: descending into a symlinked directory writes
outside the tree it was pointed at.

A type mismatch is never overwritten even under "overwrite". The user answered
about files standing where files stand, not about a directory taking the place
of a file.

## Nothing is asked when nothing is at stake

Two directories of the same name whose contents do not clash merge in silence.
Nothing is overwritten and nothing is lost, so there is no question to put.
Asking anyway only teaches the user to press Enter without reading.

## Keep both, rather than a name typed by hand

Four answers — overwrite, skip, keep both, cancel — and each of them is a
setting on the job that one button carries. A typed rename would be the only
one needing a prompt, and one per colliding item at that; keep both answers the
same need for a whole batch at once.

It is also the only answer that touches nothing already there, so the existing
cleanup of a partial copy is all the safety it needs.

The number goes where `file_stem` and `extension` split the name, which is at
the last dot: `notes.tar.gz` becomes `notes.tar(1).gz`. Every file manager
without a table of double extensions does the same. A free number is found by
*creating* — `File::create_new`, `fs::create_dir` — never by asking whether a
name is free and then taking it.

## `pending` never opens a dialog

`Action::Delete` opens the confirmation; `ConfirmTarget::Delete` performs it.
If `pending` could itself be an action that opens a dialog, confirming one
dialog could open another, and there is no stack to hold them.

## Confirmation dialogs open on `Choice::No`

The harmless answer takes the default focus, so Enter on reflex cannot delete
anything.

## The trash is never a fallback

Deleting means the trash (`d`); permanent deletion is `Shift+D`, a deliberate
choice. "If the trash fails, delete for real" would take the safety net away
at exactly the moment someone was relying on it — a failure to reach the trash
is a failure to delete, and it is reported as one.

This rule was written before any of it was built, lost in a pass that condensed
`CLAUDE.md` (`848ec26`), and put back here once `feat-trash-delete` turned out
to be resting on it. Today `Shift+D` is still the only key that deletes for
good.

Two things about the `trash` crate that the task was wrong about, both checked
against 5.2.6 rather than remembered:

**The thread-safety objection is dead.** `CLAUDE.md` once justified "one
worker, not a pool" with `trash` calling `getmntent`. The crate holds its own
`Mutex` around the mount-table calls; what it warns about is *other* threads in
the process calling `getmntent` directly, which nothing here does — Mula's only
libc is `signal`, `termios` and a `mkfifo` in a test. On macOS the question
does not arise at all: `src/macos/mod.rs` never reads the mount table, and the
`getmntinfo` path is gated to the BSDs. The one-worker rule now rests on write
ordering alone, which is where `background-io.md` already has it.

**macOS has no good answer, and the default is the opposite of what was
assumed.** `DeleteMethod::Finder` is the default, not `NsFileManager`:

| | Finder | NsFileManager |
| --- | --- | --- |
| "Put Back" | yes | first item per process only |
| how | `osascript` subprocess | `trashItemAtURL` |
| costs | automation permission prompt, sound, slower | none |

`Finder` builds its AppleScript by interpolating the path into the source
(`tell application "Finder" to delete { POSIX file "…" }`). That is not a shell
— it is one `argv` element — so the opening rule survives it literally. But a
filename reaching another language as source text is the shape that rule exists
to prevent, and Mula has nothing else like it. The crate does escape, and
percent-encodes a path that is not UTF-8.

`NsFileManager` is what `move_to_trash` sets. A permission the user has to
grant before `d` works — and which fails the key outright if they decline —
costs more than "Put Back" is worth, and macOS only records Put Back for the
first item a process trashes anyway, so what is given up is closer to nothing
than the table suggests. The file lands in the trash either way; dragging it
out is unaffected. It is one line in `move_to_trash` if that judgement turns
out wrong.

Linux is uninteresting by comparison: `freedesktop.rs` writes `.trashinfo` and
restoring is the desktop's business.

## The question waits for a free screen

A job ends whenever it ends. The user may be halfway through a delete
confirmation of their own at that moment, and the answer to "what about these
collisions?" cannot take the screen from a dialog already on it — pressing
Enter would then answer a question the user never read.

So the pairs wait on `App` as `asking`, and `ask_pending` puts the question on
the first pass of the loop that finds `Mode::Browse`. Nothing is stacked: there
is still at most one overlay, and the second question simply has not been asked
yet.

This is the case `keys-overlays.md` named as the one that would end the no-stack
rule — a collision met inside a running transfer. It does not end it, because
the collision is not answered in flight. The worker finishes, reports, and the
question is put afterwards like any other.

## Overwrite is asked once and applied twice

The first pass of a batch carries `OnCollision::Refuse`, which is not an answer
but the absence of one: it writes the pair down and moves on. The follow-up job
carries the answer and only the pairs it was about.

The follow-up takes its own snapshot when it starts. That is what makes
"overwrite" reach a file the first batch put in the way: by then it is a file
that was already there, and the rule about not eating a batch's own output does
not reach across jobs.
