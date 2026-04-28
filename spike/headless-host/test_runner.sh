#!/usr/bin/env bash
# 10-run stability test for the headless plugin hosting spike.
# Pass/fail criteria from the spec:
#   1. Renders WAV out (non-zero output file)
#   2. State recall PASS (exit code 0, not 2)
#   3. No crash on 10 consecutive runs

set -euo pipefail

BINARY="${1:-./build/HeadlessHost}"
OUT_DIR="${2:-/tmp/spike_runs}"
mkdir -p "$OUT_DIR"

if [[ ! -x "$BINARY" ]]; then
    echo "Binary not found: $BINARY"
    echo "Build first: cd build && cmake .. -DCMAKE_BUILD_TYPE=Release && cmake --build . -j\$(sysctl -n hw.ncpu)"
    exit 1
fi

# ── Plugin discovery ───────────────────────────────────────────────────────
VST3_SYS="/Library/Audio/Plug-Ins/VST3"
VST3_USER="$HOME/Library/Audio/Plug-Ins/VST3"

find_vst3() {
    local pattern="$1"
    find "$VST3_SYS" "$VST3_USER" -maxdepth 2 -name "*.vst3" -path "*$pattern*" 2>/dev/null | head -1
}

PRO_Q=$(find_vst3 "Pro-Q 3")
# Waves uses a shell model — one .vst3 bundles every Waves plugin.
# We use the latest WaveShell and ask for SSL Comp by name via the :: syntax.
WAVES_SHELL=$(find_vst3 "WaveShell1-VST3" | sort -V | tail -1)
VALHALLA=$(find_vst3 "ValhallaRoom")

echo "=== Plugin discovery ==="
echo "  FabFilter Pro-Q 3: ${PRO_Q:-NOT FOUND}"
echo "  Waves shell:       ${WAVES_SHELL:-NOT FOUND}"
echo "  Valhalla Room:     ${VALHALLA:-NOT FOUND}"
echo ""

CHAIN=()
[[ -n "${PRO_Q:-}" ]]        && CHAIN+=("$PRO_Q")
[[ -n "${WAVES_SHELL:-}" ]]  && CHAIN+=("${WAVES_SHELL}::SSL Comp")
[[ -n "${VALHALLA:-}" ]]     && CHAIN+=("$VALHALLA")

if [[ ${#CHAIN[@]} -eq 0 ]]; then
    echo "WARNING: no plugins found — running tone passthrough only"
fi

echo "=== Chain ==="
for c in "${CHAIN[@]}"; do echo "  $c"; done
echo ""

# ── 10-run loop ────────────────────────────────────────────────────────────
echo "=== 10-run stability test ==="
PASS=0
FAIL=0
PARTIAL=0

for i in $(seq 1 10); do
    OUT="$OUT_DIR/run_$(printf '%02d' $i).wav"
    printf "  Run %2d/10 ... " "$i"

    set +e
    "$BINARY" "$OUT" "${CHAIN[@]}" > "$OUT_DIR/run_$(printf '%02d' $i).log" 2>&1
    CODE=$?
    set -e

    if [[ $CODE -eq 0 ]]; then
        PASS=$((PASS + 1))
        echo "PASS"
    elif [[ $CODE -eq 2 ]]; then
        PARTIAL=$((PARTIAL + 1))
        echo "PARTIAL (state recall failed)"
    else
        FAIL=$((FAIL + 1))
        echo "FAIL (exit $CODE) — see $OUT_DIR/run_$(printf '%02d' $i).log"
    fi
done

echo ""
echo "=== Results: $PASS pass / $PARTIAL partial / $FAIL fail (target: 10/10 pass) ==="

# ── Determinism check ─────────────────────────────────────────────────────
FIRST_WAV="$OUT_DIR/run_01.wav"
if [[ -f "$FIRST_WAV" ]] && [[ $PASS -ge 2 ]]; then
    echo ""
    echo "=== Output determinism ==="
    FIRST_HASH=$(shasum -a 256 "$FIRST_WAV" | cut -d' ' -f1)
    ALL_SAME=true
    for i in $(seq 2 10); do
        f="$OUT_DIR/run_$(printf '%02d' $i).wav"
        [[ -f "$f" ]] || continue
        H=$(shasum -a 256 "$f" | cut -d' ' -f1)
        if [[ "$FIRST_HASH" != "$H" ]]; then
            echo "  WARNING: run_$(printf '%02d' $i).wav differs from run_01 (non-deterministic)"
            ALL_SAME=false
        fi
    done
    [[ "$ALL_SAME" == true ]] && echo "  All outputs identical — deterministic PASS"
fi

# ── Final verdict ─────────────────────────────────────────────────────────
echo ""
if [[ $FAIL -eq 0 && $PARTIAL -eq 0 ]]; then
    echo "SPIKE VERDICT: PASS — Phase 3 headless rendering is feasible"
    exit 0
elif [[ $FAIL -eq 0 ]]; then
    echo "SPIKE VERDICT: PARTIAL — loads and renders, state recall needs investigation"
    exit 2
else
    echo "SPIKE VERDICT: FAIL — $FAIL/10 runs crashed; Phase 3 timeline needs rework"
    exit 1
fi
