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

Reads stay synchronous for now. Not because they are ordered, but because a
`Directory::read` sitting in the mutation queue behind a 4 GB copy would
freeze navigation for as long as the copy runs.

Reading a directory is itself cheap on a local disk — measured on APFS, one
`read_dir` plus `file_type()` per entry:

| directory | time |
| --- | --- |
| 50 000 files | 16–35 ms |
| ~60 entries | 0.03 ms |

The main loop already ticks every 100 ms for toast expiry, so even the
pathological case hides inside one tick. What changes the answer is a network
mount, a sleeping external disk or an SSH pane, where every entry costs a
round trip. That is when directory reads move onto the reader.

## A job owns a snapshot

A job takes its paths when it is queued and never reads `App` again.

That is what makes restricting the UI unnecessary. Whatever the user changes
afterwards can only turn into a skipped entry, never a crash or a file written
somewhere unexpected — so there is no modal busy state and no read-only mode
to write, and no fourth state to thread through `handle_events` and `draw`.

Refusing a second operation would cost more than accepting it: a refusal needs
its own "something is running" state and a toast, while queueing is a plain
`send`. The queue is *less* code than its prohibition.

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

## One reader instance per role

Each role gets its own generation counter. One shared counter would cancel a
running tree walk on every cursor move, and would only hold together because
the find overlay happens to be modal today — an accident, not a guarantee.
