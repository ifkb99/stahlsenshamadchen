#!/usr/bin/env bash
#
# Run every dev tour and report which ones pass.
#
#   scripts/dev/run-tours.sh              # all of them
#   scripts/dev/run-tours.sh infantry     # just the ones whose name matches
#   STAHL_HEADLESS=1 scripts/dev/run-tours.sh   # no display, no GPU
#
# A tour is a test: `expect` and a timed-out `until` make the game exit
# nonzero, so a red tour here is a real regression in the presentation layer.
# This exists because that property was not being enforced by anything —
# `cargo test --workspace` does not run them and neither does CI — and three
# tours were quietly assumed broken for weeks when in fact they had only ever
# been *invoked* wrongly.
#
# Which is the other reason this exists. Each tour needs its own boot
# environment (a map, sometimes a seed), and that used to live in a prose
# comment at the top of the file. Running one by hand meant reading the
# comment first, and running the set meant reading ten of them. Every tour
# now declares its own on a `#!env` line and this reads it, so there is one
# source of truth and no way to run `infantry-tour` on the wrong map.
#
# These drive the real Bevy app, so normally they need a display and a GPU.
# `STAHL_HEADLESS=1` supplies neither: it runs each tour under `xvfb-run` with
# Mesa's lavapipe, a software Vulkan device, which is what lets a machine with
# no screen — a CI runner — check the presentation layer at all. It works
# (`battle-tour` and `after-action` were run through it), and it is **roughly
# an order of magnitude slower**: everything a tour waits on is paced by
# animation, animation is paced by frames, and llvmpipe draws this scene at a
# handful of frames a second. Budget accordingly before putting it on a push
# trigger — see TODO under Tooling.
set -uo pipefail
cd "$(dirname "$0")/../.."

filter="${1:-}"

# The software-rendering path, assembled once. `-a` picks a free display
# number so two runs cannot collide, and pinning the ICD stops Vulkan finding
# a real GPU that is present but headless.
headless=()
if [[ -n "${STAHL_HEADLESS:-}" ]]; then
    lvp=/usr/share/vulkan/icd.d/lvp_icd.json
    if [[ ! -f $lvp ]]; then
        echo "STAHL_HEADLESS needs Mesa's lavapipe ICD at $lvp" >&2
        echo "  Debian/Ubuntu: apt-get install mesa-vulkan-drivers xvfb" >&2
        exit 1
    fi
    headless=(xvfb-run -a -s "-screen 0 1280x800x24" env "VK_ICD_FILENAMES=$lvp")
fi
# Once, up front: ten cargo invocations otherwise each print a build line and
# the first one hides a compile error inside a tour's output.
cargo build -p stahlsenshamädchen || exit 1

pass=0
fail=0
failed=()
for script in scripts/dev/*.txt; do
    name="$(basename "$script" .txt)"
    if [[ -n "$filter" && "$name" != *"$filter"* ]]; then
        continue
    fi
    # The tour's own declared boot environment.
    env_line="$(grep -m1 '^#!env' "$script" | sed 's/^#!env *//')"
    printf '%-18s ' "$name"
    out="$(timeout 600 "${headless[@]}" env STAHL_PRESENT=immediate STAHL_DEBUG=1 \
        $env_line STAHL_SCRIPT="$script" \
        cargo run -q -p stahlsenshamädchen 2>&1)"
    code=$?
    fails="$(printf '%s' "$out" | grep -c 'SCRIPT FAIL')"
    if [[ $code -eq 0 && $fails -eq 0 ]]; then
        echo "ok"
        pass=$((pass + 1))
    else
        if [[ $code -eq 124 ]]; then
            echo "TIMED OUT"
        else
            echo "FAILED ($fails)"
        fi
        printf '%s\n' "$out" | grep -E 'SCRIPT FAIL|panicked' | sed 's/^/    /'
        fail=$((fail + 1))
        failed+=("$name")
    fi
done

echo
echo "$pass passed, $fail failed"
if [[ $fail -gt 0 ]]; then
    echo "red: ${failed[*]}"
    exit 1
fi
