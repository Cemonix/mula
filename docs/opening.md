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

## What is not checked

A fifo, a socket or a device handed to `$PAGER` will block it. The listing
reports all three as `File` — `Directory::read` only separates directories and
symlinks — so turning them away would need a `symlink_metadata` the panel has
not done, and doing it here would be a blocking read on the main thread.

F3 and F4 are explicit: the user named the program and named the file. If it
blocks, Ctrl-C now gets out of it, which is exactly what the signal work above
buys. What the preview does instead — refusing on the metadata it already had
to read — is the right thing for a preview, which nobody asked for.
