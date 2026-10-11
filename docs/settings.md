# Settings and the config file

Why the config file and the settings overlay work the way they do. The rules
themselves are in `CLAUDE.md`.

## The file belongs to Mula

`~/.config/mula/config.toml` started as a file the user wrote and Mula only
read: it rebound keys, and nothing ever wrote it back. That stopped working
the moment settings could be changed inside Mula. A setting changed in the
overlay has to survive a restart, so something has to write it, and writing a
file someone also edits by hand means choosing whose version of it wins.

Keeping the user's file intact while changing one value in it is what a
format-preserving editor is for, and it does not answer the hard part: where a
setting goes that the file has never mentioned, what happens to a comment
sitting on a line that changed, and how a default is told from a value the
user chose. So Mula writes the file whole, the way btop does. What is kept
across a write is the values. Comments and formatting written by hand are
not, and that is the price, paid knowingly.

What the user loses is the right to arrange the file, not the right to keep it
somewhere else. A file of their own can be put in place, or symlinked from a
dotfiles repository, and Mula takes it over from its first change.

## A default is written commented out

Every setting and every action is written under a comment saying what it
does, with its default on a commented line below:

```toml
# What a panel is sorted by when it opens: name, time, size, extension
# sort = "name"
sort = "size"
```

A line under the commented one is what replaces the default; a setting left at
its default has no such line. Writing defaults out as values would freeze
them — a default that changes in a later version would never reach a user
whose file already held the old one, because nothing would tell the two
apart. Written as comments, the file still lists everything there is, and
reading it sets nothing it does not have to.

The file is also the documentation of the keys. `config.example.toml` was a
copy of every action at its default, kept equal to the catalogue by a test;
the written file is the same list produced from the catalogue itself, so the
copy went.

## A write goes through a symlink

A file written to a temporary name and renamed over the target is all of the
new text or none of it, which is what a write of a config should be. But
renaming over a symlink replaces the link with a plain file, and the user's
dotfiles quietly stop being the file Mula reads. So the link is resolved
first, as many levels as it has, and the temporary file is written beside
the file at the end of it. A link that points at a file that is not there
yet is followed too, and the file is created where it points.

The temporary file takes the target's permissions before it is renamed, so a
config the user made private stays private.

## A file that cannot be used is never written

A file that cannot be read, is not TOML, or holds something Mula does not
understand costs the user their settings for the run and never the file. Mula
starts on the defaults, says why in a toast, and refuses every write until it
restarts — the same rule the favorites file has. Rewriting from the defaults
would replace whatever the user had in it with a list of comments.

Reading is strict for the same reason. A setting Mula does not know, a section
it has never heard of, a value no setting offers: any of them is an error
rather than a line skipped, because a skipped line is a line the next write
drops. That includes a file written by a newer Mula, which is the case that
matters most — an older one opening it closes it rather than stripping the
settings it cannot read.

It is all or nothing, as the keys were: half a config is a state the user
cannot picture from the file in front of them.

## Written at every change, on the loop

A change in the overlay writes the file before the overlay shows the new
value. When the write fails, the value goes back to what it was and the
failure becomes a toast, so nothing on screen claims a save that did not
happen.

The write happens on the main loop, as the favorites' does and for the same
reasons: it has nothing to be ordered against, the worker's queue might be
carrying a 4 GB copy, and the file is a few kilobytes under the user's home.

## Read once, at startup

Mula does not watch the file. A change made to it by hand while Mula runs is
read at the next start — or overwritten by the next change made in the
overlay, since Mula writes what it holds. Two instances of Mula are the same
case: each writes its own settings, and the last change made wins.

Watching would need a second answer to "whose version wins" for the length of
a run, and the honest answer is the one the file already has: it is Mula's.

## A setting is what a panel opens with

The sort, the dotfiles and the columns each have a key in Browse already, and
those keys change the panel in front of the user for as long as it lasts.
The setting is different: it is what a panel, and every tab opened later,
starts with. The two never cross. Pressing `s` does not write the file — the
cheapest key on the keyboard silently rewriting a config would make every
glance at a different order a decision — and changing the setting is done
in the overlay, deliberately.

A change in the overlay is shown at once, on every pane of both panels, but
only for the setting that changed. Moving the sort setting re-sorts every
pane and leaves each pane's dotfiles as the user toggled them, and the other
way round.

The border used to label any order but by-name, because by-name was the one
every pane started in. It now leaves the order the settings ask for
unlabelled, whatever that is: a user who sorts by size by default would
otherwise read "by size" on every pane for good.

## Icons are a look, not what a panel opens with

`[appearance] icons` is the first setting no Browse key changes, and nothing
holds a copy of it. Every frame reads it out of the config, so a change in the
overlay reaches every listing, the preview and the find overlay at once, with
nothing to tell.

Three values. `nerd-font` is what Mula always drew, and needs the font.
`emoji` is for a terminal without one: four glyphs, one for each kind of entry,
since every font stack carries emoji. Only glyphs that are emoji on their own
are used — no variation selector — because a terminal and `Span::width`
disagree about how wide a selector makes a character, and the columns behind
the name are laid out by that width. `off` draws nothing and keeps no room for
it, so a name starts right after the mark.

## One list of settings

`Setting` is an enum, and everything about a setting — its section, its name
in the file, its title and help line in the overlay, its values — is a match
over it. A new setting is a new variant, and the compiler names every place
that has to say something about it. The file, the parser and the overlay all
read the same list, so none of them can know about a setting the others do
not.

The values are `Choice`s: a fixed set of names, written in the file as they
are and stepped through in the overlay.

## A tab per section, and a box that holds still

Each section of the file is a tab of the overlay, and the keys are the last
one; `Tab` and `Shift+Tab` go between them. One long list with headings was
the first shape, and it put three settings above sixty actions — the settings
were what the user came for and the actions were what they scrolled through.

The box does not change size while it is open. It is measured against every
tab at once — the widest row and help line of any of them, the longest list —
and against the widest value of each setting rather than the current one, and
it keeps three lines at the bottom for the help or the question, whichever is
there. The first shape measured what was on screen, and a question longer
than a help line widened the box under the reader's eyes the moment it
appeared. A question longer than three lines takes rows from the list instead,
up to half the box: the box holds still and only what is in it moves.

The cost is a short tab drawn in a box as tall as the longest one, with the
room left empty.

## Config is `Shift+C`

`s` is the sort, and the settings are not its rarer half. The word the overlay
goes by is the one the file goes by, config, and `Shift+C` was free. Reaching
for copy and landing on it opens an overlay that `Esc` closes, which is the
direction of mistake that costs nothing.

## Keys are bound by pressing them

The Keys tab lists every Browse action by its catalogue name — the name the file uses, and the one F1 finds it by — with the keys it
answers to. `Enter` on an action waits for a key, and the key pressed is the
answer. Nothing is typed: a key name typed into a field is a spelling the user
has to know, and a key pressed is the key itself.

While it waits, every key but `Esc` is the answer. `Esc` has to stay the way
out, so it cannot be given to an action this way — the one action that has it
by default, clearing the filter and the marks, gets it back with `r`. F1 is
not an answer either: it is resolved before any mode sees a key, so pressing
it opens the help over the question, which is what F1 does everywhere. A key
the terminal keeps for itself never reaches Mula at all, and the question says
so — WezTerm's `Ctrl+W` is the case that will be met, and from inside Mula it
looks like a key that did nothing.

A captured key is the binding the event would match, with the modifiers
`matches` compares and no others, so a terminal adding bits of its own does
not make a binding nothing can press again. It is refused when it works in
every mode, and when its name does not read back as itself: a key the file
cannot spell would make the next start refuse the file whole.

The answer replaces the action's keys. An action reachable two ways is rare
enough that the second way is the file's to give.

## A key moves, it is not shared

Two actions cannot answer to one key, so a key another action holds is taken
from it — but never in silence. The overlay asks first, naming the key and the
action holding it, and says so when the move would leave that action with no
key at all: an unreachable action is the one consequence that is hard to
notice afterwards. `Enter` takes the key, `Esc` leaves every action as it was.

The question is a state of the overlay, not a dialog over it. It takes the
bottom of the box and the key table, and the list stays where it was; there is
no second overlay to stack, and nothing to land on when it closes but the list
it was asked from.

`r` puts an action's own keys back, and the same question stands between it
and a default key some other action has been given since.

Every change rebuilds the Browse table from the config, so the bar, the help
and the keys the info bar quotes change with it, before the overlay is even
closed.
