# Archives

Why packing and unpacking are shaped the way they are. The rules themselves
are in `CLAUDE.md`.

## Two things under one name

Packing and unpacking are jobs like any other, and they were built. Entering
an archive as if it were a directory is a different problem and is not built;
`tasks/feat-archive-panes.md` says why it waits.

The short of it is that a `DirEntry` carries an `Arc<Path>` and an entry inside
an archive is not anywhere. Inventing a path out of the archive and the name
inside it would pass the type and then be handed to `$EDITOR`, to a
`MutationOp` and to the preview, and the rule that paths handed to another
program are absolute would stop meaning anything. What is needed instead is a
listing whose entries are not local paths — which is the same thing SSH panes
need, down to transfers with one side elsewhere and a preview that has to
fetch bytes rather than open a file. Building it twice, once for archives with
a shape that only archives could use, would mean throwing the first one away.

What can be had without any of that is in this file.

## The stack has no C in it

`zip 8.6`, `tar 0.4` and `flate2 1.1` are the whole of it, and none of them
compiles a line of C: flate2's default backend is Rust, and `zip`'s other
codecs, where they are wanted later, are Rust too — `libbz2-rs-sys` for bzip2
and `lzma-rust2` for xz, both pulled in by cargo features rather than by a
build script.

That is not the same as no `unsafe`. `zlib-rs` alone holds several hundred
blocks of it. The difference is that `unsafe` in Rust is scoped, can be run
under Miri, and is covered by RUSTSEC advisories that `cargo audit` reads,
while a vendored C source tree is none of those. The practical half matters
more: nothing in the build wants a C toolchain, so `cargo install mula` works
on a machine that has none and cross-compiling needs nothing extra.

The one codec that would break this is zstd, whose only Rust implementation
(`ruzstd`) decompresses and does not compress. When `.tar.zst` is wanted, the
honest shape is to read it and not offer to write it.

Calling `bsdtar` or `7z` as a process would not break the rule about shells —
that rule is about a shell, not about `argv` — but it would cost the progress
per entry and the cancellation between entries, which is exactly what the
worker already gives. So the codecs are in process.

## A batch is its archives

The first design of this expected trouble: a zip keeps an index and can say
how many entries it holds, while a `tar.gz` cannot say until it has been read
to the end, so `Progress` would have needed an item count it did not know.

That trouble comes from counting the wrong thing. The items of a batch are
what was marked — for a transfer the marked files, for an unpack the marked
archives — and the entries inside are a level below, the way the files inside
a marked directory are. `Observer::entry_copied` already carries that level:
it moves the bar and puts the name of the entry being written on screen, while
the count stays on the batch. So an unpack of three archives is `0 / 3`, the
bar is filled by how far into the archive files the job has read, and the name
changes per entry. `Progress` needed no change at all.

For packing the batch is one archive, however many items go into it, so the
count is `0 / 1` and the bar carries everything. The weight is what the items
weigh on disk, which `tree_size` already walks for a transfer.

That a compressed archive is measured by position in the *input* stream rather
than by bytes written is what makes the two formats behave alike. A zip's
central directory gives a per-entry compressed size; a tar has none, so the
reader under the decompressor counts what has been taken out of the file and
each entry reports the growth since the last one.

## Unpacking never asks about a collision

An unpack writes into a staging directory beside the destination and gives it
its name with one `rename` at the end. The name is the archive's, less its
suffix; when that name is taken it takes a numbered one, by the same
`claim_free_name` a transfer's "keep both" uses.

So nothing that was already on the disk is ever written to, and there is no
question to put. This is the "nothing is asked when nothing is at stake" rule
reaching further than it does for transfers, and it is worth being clear that
it is a choice: Total Commander unpacks into the target directory and asks
about what it finds there.

Merging an archive into a tree you already have is then two steps — unpack,
then F5 the contents — and that is the argument for it rather than against.
`marks-operations.md` defends merging over replacing on the grounds that
merging cannot be assembled out of steps the user could take alone. Here it
can, and the second step is the transfer, which already has the whole
collision machinery: the pre-walk, the four answers, and the question put
after the job rather than during it. Writing a second one inside the unpack
would have meant a `Transfer` pair whose `from` is not a path.

Two more things follow from the staging directory. An unpack is now all of an
archive or none of it — a cancelled or failed one takes what it had written
with it, which a transfer does not manage. And every name inside it is one the
run itself created, so "never overwrite the job's own output" comes down to
refusing an archive's second use of a name, which is one `create_new`.

The wrapper is lifted at the end: a staging directory holding one directory
and nothing else is renamed by *its* name, so `notes.tar.gz` holding `notes/`
unpacks as `notes/` and not as `notes/notes/`. Doing it at the end rather than
from the index is what makes it work for a tar, which cannot be asked what it
holds without being read.

## The name says which format

Packing asks for one thing, the archive's name, and reads the format off its
suffix. Total Commander puts a format picker in the dialog; a suffix answers
the same question in a field that has to be filled in anyway, and it keeps the
prompt to the one text field `Mode::Input` already carries. A second field
would have meant teaching `Prompt` about cursors and scrolling in two places,
and a second overlay is not available at all.

A name asking for a format Mula does not write stops before anything is
queued, with the marks still standing, so the key can be pressed again.

## Where a path from an archive is checked

Unpacking is the only place in Mula where a file name comes from somewhere
other than the filesystem. An entry can call itself `../../.ssh/authorized_keys`,
can be absolute, or can be a symlink pointing out of the tree that a later
entry then writes through.

Both crates check containment themselves — `ZipFile::enclosed_name` refuses an
absolute name, a NUL and a `..` that climbs out, and `tar`'s `unpack_in`
canonicalises the destination's parent for every entry — and neither is relied
on alone.

The check that matters is the pair of `make_way` and `inside`. `make_way`
creates the directories an entry needs one component at a time and refuses to
walk through anything already there that is not a directory, which is what
keeps a symlink from ever being a component of a path inside the staging
directory. `inside` can then work lexically, spelling a link's target out
against where the link sits rather than resolving it — which it has to,
because a link may point at something that does not exist yet and there is
nothing to canonicalise. The two hold each other up: lexical reasoning is
sound exactly because no component is a link.

The rest is smaller. Permission bits are masked to the low nine, which is
where setuid, setgid and the sticky bit are not; `tar` does the same masking
itself with `set_preserve_permissions(false)`. Anything that is not a file, a
directory or a link is left out. An encrypted zip is left out too — reading
one means asking for a password in the middle of a running job, which is the
one question the overlay rules have no room for.

A zip bomb needs no special answer. The central directory states the
uncompressed sizes, so the bar shows the absurd total before anything is
written, and `x` stops the job between entries like any other.

## The keys are letters

`CLAUDE.md` asks for Total Commander's function keys and the task for this
work proposed Alt+F5 and Alt+F9. The code says otherwise: every entry in
`BROWSE_ACTIONS` is a letter, from `c` and `m` to `d` and `n`. Following the
rule here would have put two actions in a family nothing else is in, so
packing is `z` and unpacking is `u`.

## `Kind::Archive` is not in the listing yet

Sniffing an archive lives in `fs::archive::format` and is called by the
preview and by the worker. `listing::Kind` was left alone, because what it is
for is deciding what opens an entry — and until entering an archive means
something, Enter on one should go on doing what it does, which is handing it
to the system opener. The variant belongs there on the day that changes, not
before.
