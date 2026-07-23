#!/usr/bin/env bash
# Throwaway Phase 0 verification: asserts every field path documented in
# fixtures/efficiency/README.md resolves in its fixture, and that
# bash_command_failures is a strict subset of tool_errors (no double-count).
# Not shipped machinery -- delete once Phase 3's real extractor tests cover
# the same ground against these fixtures.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$DIR"

FAILED=0

check() {
    local desc="$1"
    local actual="$2"
    local expected="$3"
    if [[ "$actual" == "$expected" ]]; then
        echo "  OK   $desc (=$actual)"
    else
        echo "  FAIL $desc (got $actual, want $expected)"
        FAILED=1
    fi
}

echo "=== tool-errors.jsonl ==="
tool_errors=$(jq -s '[.[] | select((.message.content? | type) == "array") | .message.content[]
    | select(.type=="tool_result" and .is_error==true)] | length' tool-errors.jsonl)
bash_failures=$(jq -s '[.[]
    | select((.message.content? // []) | any(.type=="tool_result" and .is_error==true))
    | select((.toolUseResult | type) == "string")
    | select(.toolUseResult | test("^Error: Exit code [0-9]+"))
    ] | length' tool-errors.jsonl)
check "tool_errors count" "$tool_errors" "2"
check "bash_command_failures count" "$bash_failures" "1"
if [[ "$bash_failures" -gt "$tool_errors" ]]; then
    echo "  FAIL bash_command_failures ($bash_failures) exceeds tool_errors ($tool_errors) -- not a subset"
    FAILED=1
elif [[ "$bash_failures" -eq "$tool_errors" ]]; then
    echo "  FAIL bash_command_failures == tool_errors -- fixture doesn't prove the strict subset (need a non-bash error too)"
    FAILED=1
else
    echo "  OK   bash_command_failures ($bash_failures) is a strict subset of tool_errors ($tool_errors)"
fi
healthy_uncounted=$(jq -s '[.[] | select((.message.content? | type) == "array") | .message.content[]
    | select(.tool_use_id=="toolu_bashok00000000000000001")
    | select(.is_error == null)] | length' tool-errors.jsonl)
check "healthy bash call has no is_error key (never counted)" "$healthy_uncounted" "1"

echo "=== interrupts.jsonl ==="
structured=$(jq -s '[.[] | select((.toolUseResult|type)=="object" and .toolUseResult.interrupted == true)] | length' interrupts.jsonl)
text=$(jq -s '[.[] | select((.message.content? | type) == "array") | .message.content[]
    | select(.type=="text")
    | select(.text=="[Request interrupted by user]" or .text=="[Request interrupted by user for tool use]")
    ] | length' interrupts.jsonl)
negative=$(jq -s '[.[] | select((.toolUseResult|type)=="object" and .toolUseResult.interrupted == false)] | length' interrupts.jsonl)
check "interrupts_structured count" "$structured" "1"
check "interrupts_text count" "$text" "2"
check "negative control (interrupted:false) present and excluded" "$negative" "1"

echo "=== compaction.jsonl ==="
boundaries=$(jq -s '[.[] | select(.subtype=="compact_boundary")] | length' compaction.jsonl)
triggers=$(jq -s '[.[] | select(.subtype=="compact_boundary") | .compactMetadata.trigger] | sort | join(",")' compaction.jsonl)
fields_present=$(jq -s '[.[] | select(.subtype=="compact_boundary")
    | select((.compactMetadata.preTokens|type)=="number"
        and (.compactMetadata.postTokens|type)=="number"
        and (.compactMetadata.durationMs|type)=="number")
    ] | length' compaction.jsonl)
check "compact_boundary record count" "$boundaries" "2"
check "trigger values" "$triggers" "\"auto,manual\""
check "preTokens/postTokens/durationMs all numeric" "$fields_present" "2"

echo "=== turn-duration.jsonl ==="
turns=$(jq -s '[.[] | select(.subtype=="turn_duration")] | length' turn-duration.jsonl)
positive=$(jq -s '[.[] | select(.subtype=="turn_duration") | select(.durationMs > 0)] | length' turn-duration.jsonl)
check "turn_duration record count" "$turns" "7"
check "all durationMs positive" "$positive" "7"

echo "=== usage.jsonl ==="
usage_turns=$(jq -s '[.[] | select(.message.usage != null)] | length' usage.jsonl)
only_5m=$(jq -s '[.[] | select(.message.usage.cache_creation.ephemeral_5m_input_tokens > 0
    and .message.usage.cache_creation.ephemeral_1h_input_tokens == 0)] | length' usage.jsonl)
only_1h=$(jq -s '[.[] | select(.message.usage.cache_creation.ephemeral_1h_input_tokens > 0
    and .message.usage.cache_creation.ephemeral_5m_input_tokens == 0)] | length' usage.jsonl)
check "usage-bearing turn count" "$usage_turns" "2"
check "5m-only cache-write turn present" "$only_5m" "1"
check "1h-only cache-write turn present" "$only_1h" "1"

echo "=== clean-session.jsonl ==="
clean_errors=$(jq -s '[.[] | select((.message.content? | type) == "array") | .message.content[]
    | select(.type=="tool_result" and .is_error==true)] | length' clean-session.jsonl)
clean_interrupts=$(jq -s '[.[] | select((.toolUseResult|type)=="object" and .toolUseResult.interrupted == true)] | length' clean-session.jsonl)
clean_text_interrupts=$(jq -s '[.[] | select((.message.content? | type) == "array") | .message.content[]
    | select(.type=="text")
    | select(.text=="[Request interrupted by user]" or .text=="[Request interrupted by user for tool use]")
    ] | length' clean-session.jsonl)
clean_compactions=$(jq -s '[.[] | select(.subtype=="compact_boundary")] | length' clean-session.jsonl)
clean_turn_durations=$(jq -s '[.[] | select(.subtype=="turn_duration")] | length' clean-session.jsonl)
clean_tokens=$(jq -s '[.[] | select(.message.usage != null) | .message.usage.output_tokens] | add' clean-session.jsonl)
check "tool_errors" "$clean_errors" "0"
check "interrupts_structured" "$clean_interrupts" "0"
check "interrupts_text" "$clean_text_interrupts" "0"
check "compactions" "$clean_compactions" "0"
check "turn_duration records" "$clean_turn_durations" "0"
if [[ "$clean_tokens" -gt 0 ]]; then
    echo "  OK   real (nonzero) usage tokens present despite clean behavioral record ($clean_tokens output tokens)"
else
    echo "  FAIL clean-session fixture has no usage tokens at all -- not representative"
    FAILED=1
fi

echo ""
if [[ "$FAILED" -eq 0 ]]; then
    echo "All fixture assertions passed."
    exit 0
else
    echo "One or more fixture assertions FAILED."
    exit 1
fi
