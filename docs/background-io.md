# Background I/O

Why the worker and the reader are shaped the way they are. The rules
themselves are in `CLAUDE.md`.

## Threads, not async

Regular files have no non-blocking API. `epoll` and `kqueue` cannot watch
them, so `tokio::fs` is a thread pool running the same blocking syscalls —
the async syntax buys nothing a thread does not already give.

Cancelling a copy is an `AtomicBool` checked between entries either way,
because one `fs::copy` is a single syscall and cannot be interrupted part way.
That is true under async too, so async would not make cancellation finer
grained.

## One worker, not a pool

A single FIFO queue turns "which operation wins" into "which key was pressed
first". Two operations on overlapping paths therefore cannot run at once, and
nothing needs locking to keep them apart.

Parallel I/O on one disk does not pay. Driving two devices at once is what a
second Mula instance is for.

## Reads do not queue behind writes

The serial queue exists for the *order of writes*. Reads change nothing, so
they never need to be ordered against a mutation; the worst case is a listing
that is briefly stale and refreshes anyway.

What kept them out of the queue is that a `Directory::read` sitting behind a
4 GB copy would freeze navigation for as long as the copy runs.

Reading a directory is itself cheap on a local disk — measured on APFS, one
`read_dir` plus `file_type()` per entry:

| directory | time |
| --- | --- |
| 50 000 files | 16–35 ms |
| ~60 entries | 0.03 ms |

The main loop already ticks every 100 ms for toast expiry, so even the
pathological case hides inside one tick. That is why reads stayed synchronous
for as long as they did, and on a local disk moving them buys nothing.

They moved onto the reader anyway, ahead of the network mount and the sleeping
external disk that would have forced it. The cost of the move is not the
thread — that was written and tested already — but a panel that can say "not
yet": two listings held at once, a cursor that must not jump, and a copy whose
target is the directory on screen rather than the one still being read. Paying
that while every answer comes back in a millisecond is what makes it
checkable. Paying it for the first time against a listing that crosses a wire
would mean debugging the panel and the wire at once.

## A listing carries what to read

`read_dir` gives away the name and the type of every entry. The size and the
modification time do not come with them: each is a `stat`, so the columns cost
one syscall per name on top of the single pass over the directory.

Measured again for this, release build, warm cache, 50 000 empty files in one
flat APFS directory, three runs:

| what the listing reads | time |
| --- | --- |
| names and types | 68–72 ms |
| and the metadata | 133–166 ms |

Twice the work, not ten times it — which is worth writing down, because the
guess going in was an order of magnitude. Two things keep the decision
standing anyway. The doubling is measured against a warm cache, where a `stat`
is a lookup in memory; cold, or across a wire, it is a round trip per entry
against one for the directory. And it is a real 65–95 ms on a directory the
main loop wants back inside its 100 ms tick.

So `Listing` carries a `Detail`, and `Directory::read` takes one. `Columns` is
what settles it: the panes ask for metadata while a column is drawn and stop
asking when it is not. Every column is drawn by default — a file manager that
hides the size until you ask for it is answering the wrong question — so the
`stat` is the normal case and dropping a column is the exception that buys it
back. The tree walk behind the find overlay reads names only and always will:
it reads every directory below the root, so it would pay the `stat` for the
whole tree to draw a hit list that has no column to put it in.

The pane compares rather than matches. `Detail` is ordered, and a pane asks
again only when what it holds is *thinner* than what it draws: turning the
columns off draws fewer of them, and re-reading 50 000 files to show less would
be the one cost with nothing on the other side of it. A tab in the background
is not asked at all; the pass that brings it forward is the pass that finds its
listing too thin.

## A job owns a snapshot

A job takes its paths when it is queued and never reads `App` again.

That is what makes restricting the UI unnecessary. Whatever the user changes
afterwards can only turn into a skipped entry, never a crash or a file written
somewhere unexpected — so there is no modal busy state and no read-only mode
to write, and no fourth state to thread through `handle_events` and `draw`.

Refusing a second operation would cost more than accepting it: a refusal needs
its own "something is running" state and a toast, while queueing is a plain
`send`. The queue is *less* code than its prohibition.

## The favorites file is written on the loop

Every `fs::` call in the running app goes to the worker or a reader. The
favorites file is the exception: adding or removing one writes the whole file
from the main loop and waits for it.

The worker is the wrong place for it. It is one FIFO queue kept that way so
that writes to the tree happen in the order the keys were pressed, and a
favorites file has nothing to be ordered against — while a 4 GB copy runs,
the two hundred bytes would sit behind it and the list on screen would be
ahead of the file for minutes. A reader is worse: readers answer, they do not
write.

What makes the synchronous write affordable is its size. It is a handful of
paths, written whole rather than appended to, into a file nothing else holds
open; the cost is one `write` of well under a page, on the same loop that
already waits 100 ms for a key. The reason reads moved off the loop — a
directory on a wire, or a disk that has to spin up first — does not reach a
file under `$XDG_STATE_HOME`.

**Holds while** the list stays small and local. A favorites file that grew
into something the user syncs from somewhere far away would be a read like
any other.

The list and the file agree, which is the other half of writing at once: a
write that fails puts the list back the way it was and the failure becomes a
toast, so nothing on screen claims to have been saved. A file that could not
be *read* at startup closes writing for the run — the user's own list is
still in it, and rewriting it from an empty list would be the one way this
feature could take something away.

## The worker never touches `App`

It sends `Progress` and `Done { summary }`. The main loop refreshes panes and
raises the toast. Keeping the worker on one side of the channel is what lets a
job be tested without an `App` at all.

## Last wins is the mechanism, not the discipline

`Reader<J: ReadJob>` owns the generation counter, the pair of channels and the
thread. What it cannot own is how messages fold into an answer, because that
differs per job:

- a **search** sends hits as a *delta*, so every live batch has to be kept
- a **preview** sends a *snapshot*, so only the newest one counts

Folding therefore stays with each job.

## One reader per outstanding answer

Each reader gets its own generation counter. One shared counter would cancel a
running tree walk on every cursor move, and would only hold together because
the find overlay happens to be modal today — an accident, not a guarantee.

The unit is not the *kind* of read but how many answers can be outstanding at
once. Find and preview are one each: there is one overlay, and there is one
cursor. The panels are two, and `refresh_panes` asks both in the same breath —
sharing a counter there would raise the generation on the second request and
drop the first panel's answer, leaving that panel waiting for a generation
that never comes. Not a slow panel: a stuck one. It is worth saying that this
was never about remote panels; it was already true of two local directories
being refreshed after a copy.

## A refresh keeps the entry, a move keeps nothing

`set_directory` fits the old selection to the new listing by index. For a pane
being sent somewhere else that is all there is to go on: nothing in the new
directory is the entry the cursor stood on. Reading the *same* directory again
is a different question, and the index is the wrong answer to it — a job that
deletes a file above the cursor shifts every index behind it, and the cursor
walks down one row per deletion.

So `refresh` names the path under the cursor as the request's `focus`, and the
answer puts the cursor back on it. The index stays as the fallback, and the
entry that falls back to it is the one that went away — usually the file the
user just deleted, where landing on whatever took its place is what was wanted
anyway.

Synchronously this was almost invisible. The answer now comes from the reading
thread, so there is a real gap between the question and the redraw, and the
jump is on screen long enough to see.

## The filter sits above the read, not inside it

A pane holds the listing it was given and a *view*: the indices of the entries
that are on screen, in the order they are drawn. The cursor is an index into
the view and never into the listing, so everything that counts entries counts
what the user can see.

Filtering while reading would have been simpler, and for hidden files alone it
would have been right — the toggle is pressed rarely, and one read of the disk
per press is a fair price for not touching index arithmetic at all. It is the
search that cannot pay it. Search is typed, letter by letter, and a read of the
disk per keystroke is not a thing to build. Since both criteria narrow the same
listing, writing the filter at read time would have meant writing it twice and
deleting one.

The two differ in how long they live, not in how they work. Hidden files are a
setting the pane keeps; a search is thrown away. Both come out as the same list
of indices, so no combination of them has anywhere to disagree.

`rebuild_view` is the only place either the listing or the criteria change, and
it refits the cursor every time. That is what the funnel is for: `select_prev`
and `select_next` wrap with `%`, and a cursor left behind on a view that just
emptied divides by zero. Nothing today can empty a view that has a parent
entry in it, which is a reason to be careful rather than a reason to relax —
the parent is only there when there is a directory above.

The parent is exempt from filtering for its own reason. Its path is the
directory above, whose name may itself begin with a dot, and hiding it would
take away the only way out of `~/.config/mula`.

The pattern is a glob rather than a regular expression, and the reason is that
it is matched on every keystroke. A regex is invalid for most of its own
prefixes — `*` alone will not compile — so a live filter built on one spends
half its time in an error state, while half a glob is still a glob. `.` being
"any character" in a language whose subject is file names is the second reason:
`.log` would quietly find `catalog` too. Regular expressions would earn their
keep in the tree search, which is confirmed with Enter and reads thousands of
names; they earn nothing here.

A filter drops when the pane moves to a different directory and survives a
refresh of the same one. The two are told apart by the path, not by the reason
for the read: a filter is about the names in front of you, and carrying it
along would open the next directory already narrowed on a word chosen for the
last one — while a background job finishing is no reason to lose what was
typed.

Showing dot files gets no InfoBar segment. It would have passed the rule as it
stood — derivable from `App`, true while the state lasts — but the bar carries
what the screen does not already say, and dot files being shown is a thing you
are looking at. The columns have no segment for the same reason. The filter does get one, for the reason the dot files do not: a filtered
listing looks exactly like a short directory, and what is being filtered on
appears nowhere else. It is drawn from the moment the filter key is pressed,
before anything is typed, since that is the only sign the keys have gone
somewhere else — the filter draws no overlay, because watching the listing
narrow is the entire point of it.

Marks need nothing from any of this. They are absolute paths in a set on the
`Tab`, so an entry filtered off the screen stays marked, the same way a mark
survives leaving its directory. What that does mean is that a copy acts on marks
the filter is hiding — consistent with how marks already behave, but closer
together in time, and the delete dialog naming every item is what catches it.

## The order sits above the read too

Sorting is the third thing the view is built from, after the dot files and the
filter, and for the same reason: pressing `s` reorders what is already held,
and the listing underneath stays in the order `Directory::new` gave it. That
order is also what makes the default free — a stable sort over indices that
are already in name order within each kind finds one run and stops.

A key that compares metadata asks for it. `Sort::detail` is ordered alongside
the columns', and the panel reads to whichever of the two wants more, so a
panel showing names alone and sorted by time still gets its `stat`. Until that
listing lands every entry compares equal on the key and falls back to its name,
which is the order the pane was already showing.

The order belongs to the pane, like the dot files, rather than to `App`, like
the columns. Downloads by time beside a source tree by name is the ordinary
case; columns are about how wide the screen is, which both panels share. It
outlives a change of directory, since unlike a filter it says nothing about the
names in front of you.

Directories stay on top whatever the key, and the size key orders them by what
they measured to — never by their own `st_size`, which is the size of their
record rather than of what they hold. One not measured yet goes after the ones
that were, and moves into place when its total arrives. The border says
which order a pane is in whenever it is not the one every pane starts in — a
list sorted by time looks like a list in no order at all.

## A directory's size is a walk, not a column's detail

Nothing stores what a directory holds; the only way to know is to read every
entry below it. Measured on an M-series Mac, APFS, release build:

| tree | entries | one thread, not read lately | one thread, again | four threads |
| --- | --- | --- | --- | --- |
| `~/.cargo` | 71 000 | 0.50 s | 0.23 s | 0.10 s |
| `~/Programming` | 282 000 | 4.1 s | 1.0 s | 0.47 s |

Eight or sixteen threads measured no faster than four: past that the
filesystem is what is being waited on, not the CPU. Hence a pool of four,
shared by both panels, and rayon for it — a walk is recursive and uneven, which
is the shape work stealing is for. It is a pool of *reads*; "one worker, no
pool" is about the order of writes and does not reach it.

It does not go on the `Detail` ladder. `Detail` is a price per entry that the
number of names predicts; a walk is a price per *tree* that nothing on the
listing predicts, and a pane that asks again whenever its listing is thinner
than what it draws would start walking trees every time a column came back.
So it has a reader of its own per panel, beside the one for listings, and the
listing arrives at once with the totals filling in behind it.

Three things make it affordable:

- **Every directory a walk finishes is reported**, not only the rows that were
  asked about. Measuring `~/Programming` also measures `~/Programming/Rust` and
  everything under it, and `App` keeps every total for both panels, so walking
  down into what was just measured shows its numbers at once.
- **A walk that is cancelled keeps what it finished.** Leaving a directory
  stops the walk at its next directory, and the subtrees already done were
  reported on the way.
- **Only what is on screen is measured**: the rows the view holds, which
  leaves out the dot directories — `~/Library` alone would cost more than
  everything visible in `~`.

A total goes stale with any change below it, and the directory above does not
find out — `mtime` moves only for a change of its own entries. So entering a
directory measures every row again, showing the kept totals meanwhile; a pane
that stays asks only for what nobody knows. After one of Mula's own jobs the
paths it touched are forgotten with every directory above them, the running
walks are restarted — one begun before the job finished would put the old
totals back — and the rows that lost their numbers are asked for at once.

The number is what the files add up to (`du -A`), not the blocks they take
(`du`): the files in the same column show their `st_size`, and a column that
meant two things would be read as one. A file with several hard links counts
once in every directory that holds any of them. Counting every link was the
first version, on the guess that hard links are rare; `~/Programming` held
173 000 of them — cargo links what it builds twice under `target/`, pnpm links
`node_modules` — and came out 56 GB against `du -A`'s 43. Each directory
therefore carries the linked files of its subtree by inode and its parent
merges them, which keeps every level exact and independent of the order the
threads finished in.
