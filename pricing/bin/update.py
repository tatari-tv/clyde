#!/usr/bin/env python3
"""bin/update.py - Anthropic pricing.md to JSON pricing map (Python parser).

Sibling of bin/update.sh in the dual-parser cross-check pipeline that
bin/update orchestrates. Stdlib only on purpose: no venv, no
requirements.txt, no pyproject.toml, no uv. Any Python 3.10+ on PATH
runs this script directly.

CONTRACT
    Input:  pricing.md content on stdin.
    Output: JSON pricing map on stdout, e.g.
              {
                "claude-opus-4-7": {
                  "input_per_mtok": 5.0,
                  ...
                },
                ...
              }
    Exit:   0 on success.
            1 if zero models were parsed (the orchestrator turns a
              non-zero exit into a failed workflow with no PR opened).

The shape of the output JSON, the model-id normalization, and the
number-extraction rules are kept in lockstep with bin/update.sh. The
orchestrator compares the two outputs structurally (5 == 5.0), so the
fact that this parser emits floats and the awk parser emits raw numeric
literals is intentional and harmless.

See bin/update.sh's header comment for the documented layout of the
upstream pricing tables and the field-index conventions both parsers
rely on. This file mirrors the same conventions in Python idiom.
"""

from __future__ import annotations

import json
import re
import sys
from typing import Iterable

# Long-context cache rate multipliers, applied off the > 200K input
# rate when the long-context table is present. These mirror the
# constants in bin/update.sh and the values Anthropic documents under
# "Prompt caching" on the pricing page.
CACHE_5M_MULTIPLIER = 1.25
CACHE_1H_MULTIPLIER = 2.0
CACHE_READ_MULTIPLIER = 0.1

# Header detection. We require the literal "Model" plus distinguishing
# substrings so we never confuse the model-pricing or long-context
# tables with unrelated pipe-tables on the page (e.g. tool-use overhead
# tables).
MODEL_HEADER_RE = re.compile(r"^\| Model.*Input.*[Cc]ache")
LONG_HEADER_RE = re.compile(r"^\| Model.*200K")

# Pipe-table separator rows ("|---|---|---|") need to be skipped so
# they do not match any data-row pattern below.
SEPARATOR_RE = re.compile(r"^\|[-: |]+\|$")

# Any line that does not start with `|` ends the current table.
NON_TABLE_RE = re.compile(r"^[^|]")


def to_model_id(name: str) -> str:
    """Canonical id from a cleaned display name.

    The Rust side (claude_pricing::pricing::normalize_model_id) expects
    exactly this shape, so do not adjust without coordinating with
    src/pricing.rs.

        "Claude Opus 4.6"  ->  "claude-opus-4-6"
    """
    return name.strip().lower().replace(" ", "-").replace(".", "-")


def clean_model_name(raw: str) -> str:
    """Strip the markdown noise that occasionally wraps a model name.

    Two patterns observed in the upstream table:
        "Claude Sonnet 3.7 ([deprecated](/docs/...))"
        "**Claude Opus 4.7**"

    We drop everything from the first opening parenthesis onward, then
    remove emphasis markers, then trim. The order matters: trim runs
    last so it removes any whitespace left behind by the parenthetical
    removal.
    """
    name = re.sub(r" *\(.*", "", raw)
    name = name.replace("*", "")
    return name.strip()


def extract_number(cell: str) -> float:
    """Pull the first numeric token from a table cell.

        "$5 / MTok"           ->  5.0
        "$0.50 / MTok"        ->  0.5
        "Input: $10 / MTok"   ->  10.0

    Returns 0.0 when no number is found; the orchestrator's magnitude
    regression check upstream will notice if a real rate suddenly
    parses as zero.
    """
    cleaned = re.sub(r"[^0-9.]", " ", cell).strip()
    if not cleaned:
        return 0.0
    first = cleaned.split()[0]
    try:
        return float(first)
    except ValueError:
        return 0.0


def split_pipe_row(line: str) -> list[str]:
    """Split a pipe-table row, preserving awk's 1-based field layout.

    Python's str.split('|') gives [empty_lead, ...cells, empty_trail],
    matching what awk's split($0, fields, "|") yields at fields[1..N].
    We prepend a sentinel "" at index 0 so callers can index 1-based
    using the same field numbers documented in bin/update.sh.

        | Claude Opus 4.7 | $5 / MTok | $25 / MTok |
            -> ["",                       # f[0] sentinel
                "",                       # f[1] empty pre-bar cell
                " Claude Opus 4.7 ",      # f[2] model display name
                " $5 / MTok ",            # f[3] base input
                " $25 / MTok ",           # f[4] (here, output)
                ""]                       # f[5] empty post-bar cell
    """
    return [""] + line.split("|")


def parse(stream: Iterable[str]) -> dict[str, dict[str, float]]:
    """Stream-parse pricing.md and return the canonical pricing map."""
    in_model = False
    in_long = False

    # Track ids in insertion order so JSON output is deterministic
    # across runs. The orchestrator's diff would otherwise see noise
    # from Python dict iteration order on Python versions that did not
    # preserve insertion order (3.6 and earlier). 3.7+ guarantee
    # insertion order, but we make it explicit to match the awk parser.
    ordered_ids: list[str] = []
    seen: set[str] = set()

    rates: dict[str, dict[str, float]] = {}
    long_input: dict[str, float] = {}
    long_output: dict[str, float] = {}

    # Buffer of ids waiting for the long-context Output row to pair
    # with. Reset on every long-context Input row.
    pending_long_pair: list[str] = []

    for raw in stream:
        line = raw.rstrip("\n")

        if MODEL_HEADER_RE.search(line):
            in_model, in_long = True, False
            continue
        if LONG_HEADER_RE.search(line):
            in_model, in_long = False, True
            continue
        if SEPARATOR_RE.match(line):
            continue
        if NON_TABLE_RE.match(line):
            in_model, in_long = False, False
            continue

        # -- Model pricing data row ------------------------------------
        # Match rows whose first content cell starts with "Claude" so
        # we never pick up explanatory rows that may share the table.
        if in_model and line.lstrip().startswith("|"):
            f = split_pipe_row(line)
            if len(f) >= 8 and clean_model_name(f[2]).startswith("Claude"):
                model_raw = clean_model_name(f[2])
                mid = to_model_id(model_raw)
                if mid not in seen:
                    seen.add(mid)
                    ordered_ids.append(mid)
                rates[mid] = {
                    "input_per_mtok": extract_number(f[3]),
                    "output_per_mtok": extract_number(f[7]),
                    "cache_5m_write_per_mtok": extract_number(f[4]),
                    "cache_1h_write_per_mtok": extract_number(f[5]),
                    "cache_read_per_mtok": extract_number(f[6]),
                }
                continue

        # -- Long-context input row ------------------------------------
        # The model cell contains "Claude"; the > 200K input rate is in
        # fields[4]. Combined-model rows like "Claude Sonnet 4.6 / 4.5
        # / 4" expand to three ids sharing the prefix of the first
        # part. This mirrors bin/update.sh's expansion exactly.
        if in_long and line.startswith("|") and "Claude" in line:
            f = split_pipe_row(line)
            if len(f) < 5:
                continue
            model_raw = clean_model_name(f[2])
            value = extract_number(f[4])

            if " / " in model_raw:
                parts = model_raw.split(" / ")
                base_name = parts[0]
                prefix = base_name.rsplit(" ", 1)[0]
                ids = [to_model_id(base_name)]
                for suffix in parts[1:]:
                    ids.append(to_model_id(f"{prefix} {suffix.strip()}"))
            else:
                ids = [to_model_id(model_raw)]

            pending_long_pair = ids
            for i in ids:
                long_input[i] = value
            continue

        # -- Long-context output row -----------------------------------
        # Empty model cell, "Output" in the row. Pairs with whichever
        # ids the previous Input row stashed in pending_long_pair.
        if in_long and line.startswith("|") and "Output" in line:
            f = split_pipe_row(line)
            if len(f) < 5:
                continue
            value = extract_number(f[4])
            for i in pending_long_pair:
                long_output[i] = value
            continue

    # -- Assemble the output map ---------------------------------------
    out: dict[str, dict[str, float]] = {}
    for mid in ordered_ids:
        entry = dict(rates[mid])
        if mid in long_input and long_input[mid] > 0:
            li = long_input[mid]
            lo = long_output.get(mid, 0.0)
            entry["input_per_mtok_above_200k"] = li
            entry["output_per_mtok_above_200k"] = lo
            entry["cache_5m_write_per_mtok_above_200k"] = li * CACHE_5M_MULTIPLIER
            entry["cache_1h_write_per_mtok_above_200k"] = li * CACHE_1H_MULTIPLIER
            entry["cache_read_per_mtok_above_200k"] = li * CACHE_READ_MULTIPLIER
        out[mid] = entry

    return out


def main() -> int:
    rates = parse(sys.stdin)
    if not rates:
        print(
            "bin/update.py: parsed zero models - page format may have changed",
            file=sys.stderr,
        )
        return 1
    json.dump(rates, sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
