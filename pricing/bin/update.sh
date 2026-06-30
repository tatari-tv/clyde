#!/usr/bin/env bash
#
# bin/update.sh - Anthropic pricing.md to JSON pricing map (awk parser).
#
# WHY THIS SCRIPT EXISTS
# ----------------------
# Anthropic does not publish unit-token prices through any programmatic
# API. The Models API returns model metadata; the Cost Report API
# returns admin-scoped aggregate spend. Neither answers "what does an
# input token on Opus 4.7 cost?" The only structured source of truth is
# the page Anthropic maintains as freeform markdown:
#
#   https://platform.claude.com/docs/en/about-claude/pricing.md
#
# Parsing freeform markdown into structured JSON is fragile by nature,
# so this repo runs *two* independent parsers and refuses to ship a
# pricing update when they disagree:
#
#   bin/update.sh  - this file (bash + awk)
#   bin/update.py  - sibling stdlib-Python implementation
#   bin/update     - orchestrator: fetches once, runs both, diffs,
#                    runs regression checks, writes data/pricing.json
#
# The cross-check is the structural defense. The probability that two
# unrelated implementations make the same mistake on the same edge case
# is far lower than either making a mistake alone, so a disagreement is
# the loudest signal we have that the upstream page format has drifted.
#
# This file is meant to be readable. Anthropic's page format will
# change at some point. When it does, the engineer who has to fix this
# script will be reading these comments under time pressure. Be kind
# to that engineer.
#
# CONTRACT
# --------
# Input:  pricing.md content on stdin.
# Output: JSON pricing map on stdout, e.g.
#           {
#             "claude-opus-4-7": {
#               "input_per_mtok": 5,
#               "output_per_mtok": 25,
#               "cache_5m_write_per_mtok": 6.25,
#               "cache_1h_write_per_mtok": 10,
#               "cache_read_per_mtok": 0.5
#             },
#             ...
#           }
# Exit:   0 on success.
#         1 if zero models were parsed, on the assumption that "page
#           format changed" is more likely than "Anthropic deleted all
#           their products." The orchestrator turns any non-zero exit
#           into a failed workflow run with no PR opened.
#
# WHAT THIS SCRIPT DOES NOT DO
# ----------------------------
# - It does not fetch pricing.md (the orchestrator does).
# - It does not write any files (the orchestrator does).
# - It does not wrap the output with schema_version / data_version /
#   min_library_version metadata (the orchestrator does).
# - It does not run regression checks (the orchestrator does).
#
# Keeping this script's responsibility narrow makes it trivially
# fixture-testable: feed it a known markdown snippet, assert the
# expected JSON pricing map.
#
# UPSTREAM PAGE STRUCTURE
# -----------------------
# We parse two pipe-tables out of the markdown:
#
#   1) "Model pricing" - the canonical per-model rate table.
#
#      Header observed today:
#        | Model | Base Input Tokens | 5m Cache Writes | 1h Cache Writes | Cache Hits & Refreshes | Output Tokens |
#
#      Each row is one model, e.g.
#        | Claude Opus 4.7 | $5 / MTok | $6.25 / MTok | $10 / MTok | $0.50 / MTok | $25 / MTok |
#
#      We split each row on `|`. awk is 1-indexed, and split() gives us
#      an empty cell at fields[1] (the slot before the leading bar) and
#      another empty cell at fields[N] (after the trailing bar). The
#      content cells therefore live at:
#
#        fields[2] = model display name ("Claude Opus 4.7")
#        fields[3] = base input price
#        fields[4] = 5-minute cache write price
#        fields[5] = 1-hour cache write price
#        fields[6] = cache read (hits and refreshes) price
#        fields[7] = output price
#
#      If Anthropic ever reorders these columns, every row will parse
#      successfully and produce silently-wrong values. The orchestrator
#      catches this via the >5x magnitude regression check and the
#      cross-parser diff.
#
#   2) "Long context pricing" - only present when one or more models
#      bill at a premium rate above 200K input tokens.
#
#      Header observed historically:
#        | Model | <= 200K input tokens | > 200K input tokens |
#
#      Rows come in pairs per model: an Input row that names the model
#      in fields[2] and carries the > 200K input rate in fields[4],
#      and an Output row whose model field is empty and which carries
#      the > 200K output rate in fields[4].
#
#      As of 2026-04-28 this table is not on the page (Mythos Preview,
#      Opus 4.7, Opus 4.6, and Sonnet 4.6 all bill the full 1M token
#      window at standard rates). The schema reserves *_above_200k
#      Optional fields for the day Anthropic reintroduces a tier.
#
#      Combined-model rows are supported: "Claude Sonnet 4.6 / 4.5 / 4"
#      expands to three ids sharing the same > 200K rates. See the
#      "long context input row" block below.
#
# NORMALIZATION RULES (kept in lockstep with bin/update.py)
# ---------------------------------------------------------
# Display name to canonical id:
#   "Claude Opus 4.6"                     -> "claude-opus-4-6"
#   "Claude Sonnet 3.7 ([deprecated]...)" -> "claude-sonnet-3-7"
#   The deprecation tag and any other parenthesized annotation is
#   stripped before id generation.
#
# Cell value to number:
#   "$5 / MTok"             -> 5
#   "$0.50 / MTok"          -> 0.5
#   "Input: $10 / MTok"     -> 10
#   We always extract the FIRST numeric token from a cell. If Anthropic
#   adopts a "$5-$15 / MTok" range or an "$15 (or $7.50 prepay)" style,
#   this script will silently pick the leftmost number. The regression
#   checks in the orchestrator are the actual guardrails for that.
#
# DETERMINISM
# -----------
# We preserve insertion order via the models[] array so the JSON output
# is identical across runs. Without this, awk's hash-order iteration
# would produce spurious diffs even when the upstream page is
# unchanged, and the cron PR would be noisy.

set -euo pipefail

awk '
BEGIN {
    in_model_table = 0
    in_long_table  = 0
    model_count    = 0
    long_models_count = 0
}

# -- Header detection ------------------------------------------------
# Detect the start of the Model pricing table. We require the literal
# "Model" plus both "Input" and a Cache column word; this is far less
# fragile than matching the exact full header string and still
# discriminates against unrelated tables on the page.
/^\| Model/ && /Input/ && /[Cc]ache/ {
    in_model_table = 1
    in_long_table  = 0
    next
}

# Detect the start of the Long context pricing table. The "200K"
# substring in the header is currently unique to this table.
/^\| Model/ && /200K/ {
    in_long_table  = 1
    in_model_table = 0
    next
}

# -- Boilerplate row handling ---------------------------------------
# Pipe-table separator rows look like |---|---|---|. Skip them so they
# do not confuse the data-row matchers below.
/^\|[-: |]+\|$/ { next }

# A line that does not start with `|` is outside any table, so we drop
# our table-state flags. This handles blank lines, prose paragraphs,
# and the transitions between tables uniformly.
/^[^|]/ {
    in_model_table = 0
    in_long_table  = 0
}

# -- Helpers --------------------------------------------------------

# extract_number: pull the first numeric token from a table cell.
# Replaces every non-digit-non-dot character with a space, trims, and
# splits on whitespace. This collapses "$5 / MTok", "Input: $10/MTok",
# "  $0.50 / MTok " all to their leading number. Returns 0 when no
# number is found, which lets the magnitude regression check upstream
# notice that something parsed as zero.
function extract_number(s,    tmp, parts, n) {
    tmp = s
    gsub(/[^0-9.]/, " ", tmp)
    gsub(/^ +| +$/, "", tmp)
    n = split(tmp, parts, / +/)
    if (n > 0 && parts[1] != "") return parts[1] + 0
    return 0
}

# to_model_id: canonical id from a cleaned display name. The Rust side
# (claude_pricing::pricing::normalize_model_id) expects exactly this
# shape, so do not adjust without coordinating with src/pricing.rs.
function to_model_id(name,    id) {
    id = tolower(name)
    gsub(/^ +| +$/, "", id)
    gsub(/ /, "-", id)
    gsub(/\./, "-", id)
    return id
}

# clean_model_name: strip the markdown noise that occasionally wraps a
# model display name in a pipe-table cell. Two patterns observed:
#   - "Claude Sonnet 3.7 ([deprecated](/docs/...))"
#   - "**Claude Opus 4.7**" or "*Claude Sonnet 4.6*"
# We drop everything from the first opening parenthesis onward, then
# remove emphasis markers. The trailing/leading-whitespace trim runs
# last so it works even when the parenthetical removal leaves a
# trailing space.
function clean_model_name(raw,    name) {
    name = raw
    sub(/ *\(.*/, "", name)
    gsub(/\*+/, "", name)
    gsub(/^ +| +$/, "", name)
    return name
}

# -- Model pricing rows ---------------------------------------------
# Match data rows in the model pricing table. We look for "Claude" at
# the start of the model cell (after the leading bar and optional
# whitespace) so we do not pick up the header or any explanatory rows
# that may live alongside the table.
in_model_table && /^\| *Claude/ {
    n = split($0, fields, "|")

    model_raw = clean_model_name(fields[2])

    # Collapse date-tiered intro/standard rows (e.g. "Claude Sonnet 5
    # [through August 31, 2026]" and "Claude Sonnet 5 starting
    # September 1, 2026") to the base "Claude <Family> <Version>" so both
    # map to one id. Kept in lockstep with the DATE_TIER_RE in bin/update.py.
    if (match(model_raw, /^Claude [A-Za-z]+ [0-9][0-9.]*/)) {
        model_raw = substr(model_raw, RSTART, RLENGTH)
    }

    id        = to_model_id(model_raw)

    # First row wins. A new id is appended in insertion order and
    # captures its rates here; the second (post-intro) row for that model hits
    # the same id and is ignored, so the introductory rate listed first
    # is the one kept. Field-index conventions are documented in the
    # header; if the table reorders columns, these are the lines to fix.
    if (!(id in seen)) {
        seen[id]              = 1
        model_count           = model_count + 1
        models[model_count]   = id

        input[id]      = extract_number(fields[3])
        cache_5m[id]   = extract_number(fields[4])
        cache_1h[id]   = extract_number(fields[5])
        cache_read[id] = extract_number(fields[6])
        output[id]     = extract_number(fields[7])
    }
    next
}

# -- Long context input row -----------------------------------------
# Match the per-model Input row inside the long-context table. The
# model cell contains "Claude" and the > 200K rate sits in fields[4].
#
# Anthropic groups variants in a single row when their pricing is
# identical, e.g. "Claude Sonnet 4.6 / 4.5 / 4". We split on " / " and
# expand to three ids sharing the same prefix ("Claude Sonnet"). The
# first part of the split is always the fully-qualified name; the
# remaining parts are bare version suffixes that get re-prefixed.
in_long_table && /^\|/ && /Claude/ {
    n = split($0, fields, "|")
    model_raw      = clean_model_name(fields[2])
    long_input_val = extract_number(fields[4])

    long_models_count = 0

    if (model_raw ~ / \/ /) {
        num_parts = split(model_raw, parts, / \/ /)

        # parts[1] is the full name "Claude Sonnet 4.6"; strip its last
        # word to recover the prefix "Claude Sonnet".
        base_name = parts[1]
        prefix    = base_name
        sub(/ [^ ]+$/, "", prefix)

        long_models_count   = num_parts
        long_models[1]      = to_model_id(base_name)
        for (j = 2; j <= num_parts; j++) {
            variant = prefix " " parts[j]
            gsub(/^ +| +$/, "", variant)
            long_models[j] = to_model_id(variant)
        }
    } else {
        long_models_count = 1
        long_models[1]    = to_model_id(model_raw)
    }

    # Stash the > 200K input rate for each id. The pairing Output row
    # (matched by the next block) will read long_models[] back to know
    # which ids to attribute its > 200K output rate to.
    for (j = 1; j <= long_models_count; j++) {
        long_input[long_models[j]] = long_input_val
    }
    next
}

# -- Long context output row ----------------------------------------
# The Output row has an empty model cell and contains the literal
# "Output". It pairs with the most recent Input row by reusing the
# long_models[] buffer left there.
in_long_table && /^\|/ && /Output/ {
    n = split($0, fields, "|")
    long_output_val = extract_number(fields[4])
    for (j = 1; j <= long_models_count; j++) {
        long_output[long_models[j]] = long_output_val
    }
}

# -- JSON emission --------------------------------------------------
END {
    if (model_count == 0) {
        print "bin/update.sh: parsed zero models - page format may have changed" > "/dev/stderr"
        exit 1
    }

    print "{"
    for (i = 1; i <= model_count; i++) {
        id = models[i]
        if (i > 1) printf ",\n"

        printf "  \"%s\": {\n", id
        printf "    \"input_per_mtok\": %s,\n",            input[id]
        printf "    \"output_per_mtok\": %s,\n",           output[id]
        printf "    \"cache_5m_write_per_mtok\": %s,\n",   cache_5m[id]
        printf "    \"cache_1h_write_per_mtok\": %s,\n",   cache_1h[id]
        printf "    \"cache_read_per_mtok\": %s",          cache_read[id]

        # Optional > 200K fields. Anthropic publishes the input and
        # output premium rates directly; the cache premium rates are
        # derived using the same multipliers Anthropic documents under
        # "Prompt caching" (1.25x base for 5m write, 2x for 1h write,
        # 0.1x for cache read). bin/update.py uses identical constants;
        # any divergence would be caught by the cross-parser diff.
        if (id in long_input && long_input[id] > 0) {
            li = long_input[id]
            lo = long_output[id]
            printf ",\n    \"input_per_mtok_above_200k\": %s",          li
            printf ",\n    \"output_per_mtok_above_200k\": %s",         lo
            printf ",\n    \"cache_5m_write_per_mtok_above_200k\": %s", li * 1.25
            printf ",\n    \"cache_1h_write_per_mtok_above_200k\": %s", li * 2.0
            printf ",\n    \"cache_read_per_mtok_above_200k\": %s",     li * 0.1
        }

        printf "\n  }"
    }
    printf "\n}\n"
}
'
