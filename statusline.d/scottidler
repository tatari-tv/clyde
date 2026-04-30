#!/usr/bin/env bash
# Claude Code custom status line - two-line powerline style
set -euo pipefail

# Cross-platform timeout: GNU timeout → Homebrew gtimeout → perl fallback
_timeout() {
  local secs=$1; shift
  if command -v timeout &>/dev/null; then
    timeout "$secs" "$@"
  elif command -v gtimeout &>/dev/null; then
    gtimeout "$secs" "$@"
  else
    perl -e 'alarm shift @ARGV; exec @ARGV' "$secs" "$@"
  fi
}

DATA=$(cat)

# Parse all fields in one jq call
eval "$(echo "$DATA" | jq -r '
  @sh "MODEL=\(.model.display_name // .model.id // "?")",
  @sh "USED_PCT=\(.context_window.used_percentage // "")",
  @sh "CTX_SIZE=\(.context_window.context_window_size // 0)",
  @sh "SESSION_COST=\(.cost.total_cost_usd // 0)",
  @sh "DURATION_MS=\(.cost.total_duration_ms // 0)",
  @sh "LINES_ADDED=\(.cost.total_lines_added // 0)",
  @sh "LINES_REMOVED=\(.cost.total_lines_removed // 0)",
  @sh "CWD=\(.workspace.current_dir // .cwd // "")",
  @sh "TOK_IN=\(.context_window.total_input_tokens // 0)",
  @sh "TOK_OUT=\(.context_window.total_output_tokens // 0)"
')"

# Shorten model name
MODEL=$(echo "$MODEL" | sed -e 's/ ([^)]*context[^)]*)//g' -e 's/Opus [0-9.]*/opus/' -e 's/Sonnet [0-9.]*/sonnet/' -e 's/Haiku [0-9.]*/haiku/')

# Format context window size
if [[ "$CTX_SIZE" -ge 1000000 ]]; then
    CTX_WIN="$(awk "BEGIN{printf \"%.0fM\",$CTX_SIZE/1000000}")"
elif [[ "$CTX_SIZE" -ge 1000 ]]; then
    CTX_WIN="$(awk "BEGIN{printf \"%.0fK\",$CTX_SIZE/1000}")"
else
    CTX_WIN="$CTX_SIZE"
fi

# --- Format tokens ---
fmt_tok() {
    local t="$1"
    if [[ $t -ge 1000000 ]]; then
        awk -v t="$t" 'BEGIN{printf "%.1fM", t/1000000}'
    elif [[ $t -ge 1000 ]]; then
        awk -v t="$t" 'BEGIN{printf "%.0fK", t/1000}'
    else
        echo "$t"
    fi
}
TOK_IN_FMT=$(fmt_tok "$TOK_IN")
TOK_OUT_FMT=$(fmt_tok "$TOK_OUT")

# --- ANSI helpers ---
RST=$'\033[0m'
BOLD=$'\033[1m'
fg()  { printf '\033[38;5;%sm' "$1"; }
bg()  { printf '\033[48;5;%sm' "$1"; }
fgr() { printf '\033[38;2;%s;%s;%sm' "$1" "$2" "$3"; }
bgr() { printf '\033[48;2;%s;%s;%sm' "$1" "$2" "$3"; }

# --- Load color scheme ---
SCHEME="${CLAUDE_COLORSCHEME:-catppuccin-mocha}"
SCHEME_DIR="${CLAUDE_COLORSCHEME_DIR:-$HOME/.claude/statusline.d}"
SCHEME_FILE="${SCHEME_DIR}/${SCHEME}.sh"

if [[ "$SCHEME" =~ ^[a-zA-Z0-9_-]+$ ]] && [[ -f "$SCHEME_FILE" ]]; then
    source "$SCHEME_FILE"
else
    S0="30;30;46"; S1="49;50;68"; S2="69;71;90"; S3="88;91;112"
    ACCENT_PRIMARY="137;180;250"; ACCENT_OK="166;227;161"
    ACCENT_WARN="249;226;175"; ACCENT_CAUTION="250;179;135"
    ACCENT_ERROR="243;139;168"; ACCENT_COST="148;226;213"
    ACCENT_COST_SECONDARY="249;226;175"; ACCENT_MUTED="147;153;178"
    TEXT="205;214;244"; SUBTEXT="186;194;222"
fi

PL=$'\ue0b0'
PREV_BG=""
OUT=""

# seg <text> <bg_rgb> <fg_rgb>
seg() {
    local text="$1" sbg="$2" sfg="$3"
    IFS=';' read -r br bg_ bb <<< "$sbg"
    if [[ -n "$PREV_BG" ]]; then
        IFS=';' read -r pr pg pb <<< "$PREV_BG"
        OUT+="$(bgr "$br" "$bg_" "$bb")$(fgr "$pr" "$pg" "$pb")${PL}${RST}"
    fi
    IFS=';' read -r fr fg_ fb <<< "$sfg"
    OUT+="$(bgr "$br" "$bg_" "$bb")$(fgr "$fr" "$fg_" "$fb") ${text}${RST}"
    PREV_BG="$sbg"
}

end_seg() {
    if [[ -n "$PREV_BG" ]]; then
        IFS=';' read -r pr pg pb <<< "$PREV_BG"
        OUT+="$(fgr "$pr" "$pg" "$pb")${PL}${RST}"
    fi
}

# --- Cost via ccu ---
TODAY_COST=$(_timeout 5 ccu today --total 2>/dev/null || echo "0")
WEEK_COST=$(_timeout 5 ccu weekly --total -w 1 2>/dev/null || echo "0")
MONTH_COST=$(_timeout 5 ccu monthly --total -m 1 2>/dev/null || echo "0")

# --- Format duration ---
DS=$((DURATION_MS/1000)); DM=$((DS/60)); DH=$((DM/60)); DM=$((DM%60))
[[ $DH -gt 0 ]] && DUR="${DH}h${DM}m" || DUR="${DM}m"

# --- Token burn rate (tok/h) ---
TOTAL_TOKENS=$((TOK_IN + TOK_OUT))
if [[ $DURATION_MS -gt 30000 && $TOTAL_TOKENS -gt 0 ]]; then
    TOK_HR=$(awk -v t="$TOTAL_TOKENS" -v d="$DURATION_MS" 'BEGIN{printf "%.0f", t / (d / 3600000)}')
    if [[ $TOK_HR -ge 1000000 ]]; then
        TOK_HR_FMT="$(awk -v t="$TOK_HR" 'BEGIN{printf "%.1fM", t/1000000}')/h"
    elif [[ $TOK_HR -ge 1000 ]]; then
        TOK_HR_FMT="$(awk -v t="$TOK_HR" 'BEGIN{printf "%.0fK", t/1000}')/h"
    else
        TOK_HR_FMT="${TOK_HR}/h"
    fi
else
    TOK_HR_FMT="..."
fi

# --- Context color ---
CTX_FG=$ACCENT_OK
if [[ -n "$USED_PCT" && "$USED_PCT" != "null" ]]; then
    case $(awk -v p="$USED_PCT" 'BEGIN{if(p>=70)print 4;else if(p>=60)print 3;else if(p>=50)print 2;else print 1}') in
        4) CTX_FG=$ACCENT_ERROR ;;
        3) CTX_FG=$ACCENT_CAUTION ;;
        2) CTX_FG=$ACCENT_WARN ;;
    esac
    CTX="${USED_PCT}%"
else
    CTX="..."
fi

# --- Format costs ---
fc() { awk -v c="$1" 'BEGIN{if(c<10)printf"%.2f",c;else if(c<100)printf"%.1f",c;else printf"%.0f",c}'; }
S_COST=$(echo | fc "$SESSION_COST")
T_COST=$(echo | fc "$TODAY_COST")
W_COST=$(echo | fc "$WEEK_COST")
M_COST=$(echo | fc "$MONTH_COST")

# --- Git branch ---
GIT_BRANCH=""
if [[ -n "$CWD" ]] && git -C "$CWD" rev-parse --is-inside-work-tree &>/dev/null; then
    GIT_BRANCH=$(git -C "$CWD" rev-parse --abbrev-ref HEAD 2>/dev/null || echo "")
    if [[ -n "$GIT_BRANCH" ]]; then
        [[ -n "$(git -C "$CWD" status --porcelain 2>/dev/null | head -1)" ]] && GIT_BRANCH+="*"
    fi
fi

# --- Hostname ---
HOSTNAME_SHORT=$(hostname -s 2>/dev/null || echo "?")

# --- Shorten CWD ---
DISPLAY_CWD="${CWD/#$HOME/\~}"

# --- Inline color helpers ---
bgr_split() { IFS=';' read -r r g b <<< "$1"; bgr "$r" "$g" "$b"; }
fgr_split() { IFS=';' read -r r g b <<< "$1"; fgr "$r" "$g" "$b"; }

# Line background and foreground colors
L1_A="94;129;172"   # muted blue bg
L1_B="163;190;140"  # soft green bg
L2_A="235;203;139"  # warm sand bg
L2_B="191;97;106"   # muted red bg
L1_A_FG="15;25;45"  # darker blue text
L1_B_FG="25;45;15"  # darker green text
L2_A_FG="60;45;10"  # darker yellow text
L2_B_FG="50;10;15"  # darker red text

# =====================
# LINE 1: CWD | hostname | git branch +/- | model(ctx)
# =====================
seg "${DISPLAY_CWD} " "$L1_B" "$L1_B_FG"
seg "🌐 ${HOSTNAME_SHORT} " "$L1_A" "$L1_A_FG"

if [[ -n "$GIT_BRANCH" ]]; then
    seg " ${GIT_BRANCH} " "$L1_B" "$L1_B_FG"
    if [[ "$LINES_ADDED" != "0" || "$LINES_REMOVED" != "0" ]]; then
        seg " $(fgr_split $ACCENT_OK)+${LINES_ADDED}$(fgr_split $ACCENT_ERROR)-${LINES_REMOVED} " "$L1_A" "$L1_A_FG"
    fi
fi

end_seg

# =====================
# LINE 2: model | tokens | burn rate | used% | costs | duration
# =====================
PREV_BG=""
OUT+="\n"

seg "${MODEL}(${CTX_WIN}) " "$L2_B" "$L2_B_FG"
seg "${CTX} " "$L2_A" "$L2_A_FG"
seg "↓${TOK_IN_FMT} ↑${TOK_OUT_FMT} " "$L2_B" "$L2_B_FG"
seg "🔥${TOK_HR_FMT} " "$L2_A" "$L2_A_FG"
seg "\$${M_COST}$(fgr_split $L2_B_FG)|$(fgr_split $L2_B_FG)\$${W_COST}$(fgr_split $L2_B_FG)|$(fgr_split $L2_B_FG)\$${T_COST}$(fgr_split $L2_B_FG)|$(fgr_split $L2_B_FG)\$${S_COST} " "$L2_B" "$L2_B_FG"
seg "${DUR} " "$L2_A" "$L2_A_FG"
end_seg

echo -e "$OUT"
