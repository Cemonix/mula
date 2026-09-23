# Printed by `mula --init fish`. Put this in ~/.config/fish/config.fish:
#
#     mula --init fish | source
#
# `mula` then runs this function, which runs the real mula and moves the shell
# to the directory it was quit in.
function mula
    set -l dir /tmp
    set -q TMPDIR; and set dir $TMPDIR
    set -l tmp (mktemp "$dir/mula-cwd.XXXXXX"); or return
    command mula --cwd-file=$tmp $argv
    set -l ret $status
    set -l cwd (cat -- $tmp)
    rm -f -- $tmp
    if test -n "$cwd"; and test "$cwd" != "$PWD"
        cd -- $cwd
    end
    return $ret
end
