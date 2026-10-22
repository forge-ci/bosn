#!/bin/sh
# Read-only, non-atomic samples. Never follow symlinks outside the cache.
root=/cache
sample() {
    component=$1
    shift
    if [ "$#" -eq 0 ]; then
        printf '%s 0 0\n' "$component"
        return
    fi
    apparent=unknown
    allocated=unknown
    # One invocation per metric deduplicates links across the supplied roots.
    if value=$(du -sbc "$@" 2>/dev/null); then
        value=$(printf '%s\n' "$value" | tail -n 1)
        apparent=${value%%[[:space:]]*}
    fi
    if value=$(du -skc "$@" 2>/dev/null); then
        value=$(printf '%s\n' "$value" | tail -n 1)
        blocks=${value%%[[:space:]]*}
        allocated=$((blocks * 1024))
    fi
    printf '%s %s %s\n' "$component" "$apparent" "$allocated"
}
sample_toolcache() {
    set --
    for path in "$root/toolcache" "$root/toolstore-v1" "$root/.toolstore-v1.enrollment-v1.bolt"; do
        if [ -e "$path" ] || [ -L "$path" ]; then
            set -- "$@" "$path"
        fi
    done
    snapshots=0
    for path in "$root"/.toolcache.native-enrollment-*; do
        token=${path##*/}
        token=${token#.toolcache.native-enrollment-}
        case "$token" in *[!0-9a-f]*) continue ;; esac
        [ "${#token}" -eq 64 ] || continue
        [ -e "$path" ] || [ -L "$path" ] || continue
        snapshots=$((snapshots + 1))
        if [ "$snapshots" -gt 32 ]; then
            printf 'toolcache unknown unknown\n'
            return
        fi
        set -- "$@" "$path"
    done
    sample toolcache "$@"
}
sample total "$root"
for class in tools images actions toolcache actcache; do
    if [ "$class" = toolcache ]; then
        sample_toolcache
        continue
    fi
    if [ -e "$root/$class" ] || [ -L "$root/$class" ]; then
        sample "$class" "$root/$class"
    else
        printf '%s 0 0\n' "$class"
    fi
done
namespaces() {
    for path in "$2"/*; do
        [ -d "$path" ] && [ ! -L "$path" ] || continue
        namespace=${path##*/}
        case "$namespace" in
            *[!0-9a-f]*) continue ;;
        esac
        [ "${#namespace}" -eq 16 ] || continue
        sample "$1:$namespace" "$path"
    done
}
# Never descend through a symlinked store root. Class totals remain inclusive.
if [ -d "$root/actcache" ] && [ ! -L "$root/actcache" ]; then
    namespaces namespace "$root/actcache"
    if [ -d "$root/actcache/cohort-v1" ] && [ ! -L "$root/actcache/cohort-v1" ]; then
        namespaces cohort-v1 "$root/actcache/cohort-v1"
    fi
fi
