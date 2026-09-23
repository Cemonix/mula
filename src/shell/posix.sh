# Printed by `mula --init bash` and `mula --init zsh`. Put this in your rc file:
#
#     eval "$(mula --init zsh)"
#
# `mula` then runs this function, which runs the real mula and moves the shell
# to the directory it was quit in.
mula() {
    local tmp cwd ret
    tmp="$(mktemp "${TMPDIR:-/tmp}/mula-cwd.XXXXXX")" || return
    command mula --cwd-file="$tmp" "$@"
    ret=$?
    cwd="$(cat -- "$tmp")"
    rm -f -- "$tmp"
    if [ -n "$cwd" ] && [ "$cwd" != "$PWD" ]; then
        cd -- "$cwd"
    fi
    return $ret
}
