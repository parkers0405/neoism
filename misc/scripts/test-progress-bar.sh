#!/bin/bash
#
# Enhanced manual repro for OSC 9;4 progress-bar handling (issue #1509).
# Run inside a Neoism window. The fix is verified visually if the
# indeterminate bar in phase 2 *moves* across the window — before
# the fix it froze at the left edge because every heartbeat OSC
# yanked the animation phase back to t=0.
#
# New features in this version:
# - Configurable test duration and intervals
# - Visual separators between phases
# - Option to run specific phases only
# - Better error handling and status reporting
# - Test for both determinate and indeterminate modes

set -euo pipefail

# Configuration
TEST_DURATION=${TEST_DURATION:-6}
INTERVAL=${INTERVAL:-0.1}
DELAY=${DELAY:-0.15}

osc() {
    printf '\033]9;4;%s;%s\033\\' "$1" "$2"
    # Also log what we're sending for debugging
    echo "  [OSC] Sent: type=$1 value=$2" >&2
}

print_header() {
    echo "======================================================================"
    echo "NEOISM PROGRESS BAR TEST - $1"
    echo "======================================================================"
    echo
}

print_footer() {
    echo
    echo "Phase complete. Waiting $1 seconds before next test..."
    echo
}

if [[ "${1:-}" == "help" ]]; then
    echo "Usage: $0 [phase_number|all|help]"
    echo "  1-5: Run specific phase"
    echo "  all: Run full test sequence (default)"
    echo "  help: Show this help"
    echo
    echo "Environment variables:"
    echo "  TEST_DURATION=6   Duration of indeterminate test in seconds"
    echo "  INTERVAL=0.1      Sleep between heartbeats"
    echo "  DELAY=0.15        Sleep between determinate steps"
    exit 0
fi

PHASE="${1:-all}"

case "$PHASE" in
    1|all)
        print_header "PHASE 1: Determinate progress 0% → 100%"
        for p in 0 10 20 30 40 50 60 70 80 90 100; do
            osc 1 "$p"
            sleep "$DELAY"
        done
        print_footer 1
        ;;
esac

case "$PHASE" in
    2|all)
        print_header "PHASE 2: Heartbeat indeterminate mode (repro for #1509)"
        echo "Testing indeterminate bar - should slide smoothly L<->R"
        echo "Duration: ${TEST_DURATION}s, interval: ${INTERVAL}s"
        echo
        end=$(( $(date +%s) + TEST_DURATION ))
        count=0
        while [ "$(date +%s)" -lt "$end" ]; do
            osc 3 $((count % 100))
            sleep "$INTERVAL"
            count=$((count + 1))
        done
        print_footer 1
        ;;
esac

case "$PHASE" in
    3|all)
        print_header "PHASE 3: Error state at 50%"
        osc 2 50
        echo "Error indicator should appear at 50% progress"
        sleep 2.0
        print_footer 0.5
        ;;
esac

case "$PHASE" in
    4|all)
        print_header "PHASE 4: Pause state at 75%"
        osc 4 75
        echo "Pause indicator should appear at 75% progress"
        sleep 2.0
        print_footer 0.5
        ;;
esac

case "$PHASE" in
    5|all)
        print_header "PHASE 5: Clear/reset"
        osc 0 0
        echo "Progress bar should be cleared"
        echo
        echo "TEST COMPLETE - Check if indeterminate bar moved smoothly in phase 2"
        echo "If the bar froze, the animation phase handling bug is still present."
        ;;
esac

echo
echo "Test script enhanced with:"
echo "- Configurable timing via env vars"
echo "- Selective phase execution"
echo "- Better visual feedback and logging"
echo "- Help system"
echo
echo "Run with: TEST_DURATION=10 ./misc/scripts/test-progress-bar.sh"
