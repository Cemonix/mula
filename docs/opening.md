# Opening

Why handing the terminal to another program is shaped the way it is. The rules
themselves are in `CLAUDE.md`.

## The action asks, the loop hands over

`dispatch` has no terminal and giving it one for a single action would put it
in the way of every other. So `Action::Open` only writes down what to open, and
`run` carries it out at the end of the pass — the same shape as the preview,
which is settled once per pass rather than by each action that moves the
cursor.

The end of the pass rather than the start, because a request can be raised by
anything in it, not only by the key that was pressed.

## Two shapes, not one

`Launch::Handover` gives the program the screen, the cursor and cooked mode,
and waits. `Launch::Detached` gives it none of them and does not wait. They are
opposites in every respect, which is why the choice is a named enum rather than
something read off the opener at each use.

A picture viewer started the first way would be three bugs at once: Mula frozen
until the window is closed, the program's startup warnings written over the
drawn frame, and the program killed by SIGHUP when the terminal it was tied to
goes away. So a detached program gets `/dev/null` for all three of its standard
streams and `setsid` for a session of its own.

Nothing waits for a detached program, but something has to ask about it. A
child that has exited and that nobody ever asked about stays in the process
table. `App` keeps the `Child` values only for that: one `try_wait` each per
pass, which returns at once, and the ones that have ended are dropped.

Dropping a `Child` on Unix kills nothing, so quitting Mula leaves them running.
That is the point of detaching them.

## What opens what

Two rules, which is all an association table amounts to here:

| what the listing found | opener |
| --- | --- |
| a directory | the pane enters it |
| a regular file whose first bytes hold no NUL | `$EDITOR`, handed the terminal |
| any other regular file | the system's opener, detached |
| a fifo, a socket, a device | nothing, and a toast |

F3 and F4 override it: the user named the program, so nothing is sniffed.

## Signals

This is the part that is easy to get wrong and expensive to get wrong.

With raw mode off, the terminal driver turns Ctrl-C into SIGINT and Ctrl-\ into
SIGQUIT, and sends them to *every process in the foreground group*. The child
is in Mula's group, so without care one Ctrl-C in an editor kills the file
manager too and leaves the terminal in whatever state the editor had it in.

What we do is what POSIX specifies for `system(3)`: the caller ignores SIGINT
and SIGQUIT for as long as it waits, and puts the previous dispositions back
afterwards. `Ignored` is that, as a guard, so an early return cannot skip it.

The other half is less obvious. A disposition of *ignore* survives `exec`,
unlike a handler, which is reset. Ignoring the two signals without more would
therefore hand the child Mula's deafness, and the editor could not be
interrupted at all. `pre_exec` puts both back to their default in the child,
between `fork` and `exec`, using nothing but `signal` — which is on the list of
calls that are safe there.

`signal` rather than `sigaction`: the caveat that separates them is what
happens to a *handler* when it runs, and neither disposition here is a handler.
Restoring what `signal` handed back is what a `system` implementation does. It
cannot faithfully restore a three-argument handler installed by `sigaction`,
which is fine while nothing installs one for these two signals — the terminal
resize crossterm watches is SIGWINCH, which is not one of them.

SIGCHLD is left alone. `system(3)` blocks it so that a caller's handler cannot
reap the child first; nothing here installs one, and `Command::status` waits on
its own pid.

### Why not job control

The shell-grade version gives the child its own process group and hands it the
terminal with `tcsetpgrp`. `rustix` exposes every call for it, so this was a
real choice rather than a missing tool.

Its one advantage over the above is that Ctrl-Z would stop only the child. That
is not the win it looks like: `Child::wait` does not ask for stopped children,
so Mula would go on waiting for a program the user just suspended — a stuck
app, where the plain version merely stops both and resumes both on `fg`. And
taking the terminal back afterwards means `tcsetpgrp` from a background group,
which raises SIGTTOU and has to be ignored, so the signal handling does not go
away either.

## Nothing is handed to a shell

`$EDITOR` is split on whitespace: the first word is the program, the rest are
arguments. The file's path is then one more element of `argv`.

There is no quoting to get wrong, because there is no shell to quote for. A
file called `; rm -rf ~` is a file name, and a name with spaces needs nothing
done to it.

## The path is absolute

Nothing marks the end of the options, so a path is the one argument a program
could still misread. Mula's paths start at `env::current_dir()` and grow by
`read_dir`, so every one of them begins with `/` and none can look like an
option.

That is a property of the listing rather than of this module, which is why it
is written down: making paths relative somewhere else would quietly turn a file
named `-i` into a flag.

## Taking the screen back

`Terminal::clear` is the documented way to force a full repaint, but it first
asks the terminal where its cursor is — a query written to the screen and
answered on standard input, the same input the keys arrive on.

`Terminal::resize` reaches the same place: it resets the back buffer, so the
next frame draws every cell. It needs a size, which comes from an ioctl on the
device rather than from the terminal, and it costs no round trip. Passing the
size that was just read also picks up a window resized while the program ran.

## Entering asks one question

Enter does not consult `DirEntryKind`. It asks the reader to list the entry,
and the reader answers with the listing or with what the entry is instead. One
round trip, and symlinks come out right because they were actually followed
rather than guessed at from a listing that reports every link as a link.

That is why `Listed::NotADirectory` is a variant rather than an
`io::ErrorKind::NotADirectory`: for a pane that is entering something, "it is a
file" is the answer, not a failure.

`Intent` on the request is what keeps a refresh out of it. A pane catching up
with the disk goes through the same call, and a directory replaced by a file
between two passes must not open an editor nobody asked for.

## What is not checked

F3 and F4 hand over whatever is under the cursor, a directory aside. A fifo
given to `$PAGER` will block it — the listing reports fifos, sockets and
devices as `File`, and telling them apart would need a `symlink_metadata` the
panel has not done.

Enter does check, because it was already reading. F3 and F4 do not, because the
user named both the program and the file, and because Ctrl-C now gets out of
it — which is what the signal work above buys.
