#!/bin/zsh
# Run a bounded Ralph loop with the local Codex CLI.
#
# Usage:
#   scripts/ralph_codex.sh <iterations>
#
# State and open topics live in progress.md. Each Codex invocation must select
# and complete at most one actionable topic before updating that file.

set -euo pipefail

if [[ $# -ne 1 || ! "$1" =~ ^[1-9][0-9]*$ ]]; then
  echo "Usage: $0 <positive-iterations>" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROGRESS="$ROOT/progress.md"
SPOTATUI="$ROOT/../spotatui-player-performance"
CODEX_BIN="$(command -v codex || true)"

[[ -f "$PROGRESS" ]] || {
  echo "error: missing $PROGRESS" >&2
  exit 1
}

[[ -n "$CODEX_BIN" ]] || {
  echo "error: codex is not on PATH" >&2
  exit 1
}

CODEX_ARGS=(
  --ask-for-approval never
  --no-alt-screen
  exec
  --cd "$ROOT"
  --model gpt-5.6-luna
  --config 'model_reasoning_effort="xhigh"'
  --sandbox workspace-write
)

# rust-player's Cargo configuration may patch to this sibling checkout.
if [[ -d "$SPOTATUI" ]]; then
  CODEX_ARGS+=(--add-dir "$SPOTATUI")
fi

# workspace-write can leave Git metadata read-only even when source files are
# writable; commits need the repository's index and lock file as well.
CODEX_ARGS+=(--add-dir "$ROOT/.git")

for ((i = 1; i <= $1; i++)); do
  echo "==> Codex iteration $i/$1"

  if result=$("$CODEX_BIN" \
    "${CODEX_ARGS[@]}" <<'PROMPT'
You are working in the rust-player repository.

Read AGENTS.md and progress.md before changing anything. Treat progress.md as
the task ledger: choose the highest-priority actionable item under “Open
topics”, and work on exactly one item in this iteration.

Requirements:

1. Implement the smallest correct change for that one item. Preserve unrelated
   existing modifications and untracked files; never reset, clean, or overwrite
   work you did not create in this iteration.
2. Respect rust-player's MacOS, Windows, and Linux support, performance goals,
   and the repository's instruction not to use GUI/browser verification unless
   explicitly requested.
3. Run focused tests/checks, then broader relevant Rust checks when practical.
4. Update progress.md with what changed, verification results, and the current
   open topics. Keep it factual and concise.
5. Selectively stage only files belonging to this iteration. Do not stage
   unrelated pre-existing changes. Commit the implementation and progress
   update with a Conventional Commit message. Do not create a PR or push.

If no actionable engineering topic remains, do not make speculative changes;
output <promise>COMPLETE</promise> in your final response. If progress is
blocked on a human decision, credentials, hardware, or an explicitly forbidden
verification step, record that in progress.md and output
<promise>BLOCKED</promise>.
PROMPT
  ); then
    printf '%s\n' "$result"
  else
    status=$?
    printf '%s\n' "$result" >&2
    echo "Codex failed in iteration $i (exit $status)." >&2
    exit "$status"
  fi

  if [[ "$result" == *"<promise>COMPLETE</promise>"* ]]; then
    echo "Progress complete after $i iteration(s)."
    exit 0
  fi

  if [[ "$result" == *"<promise>BLOCKED</promise>"* ]]; then
    echo "Progress blocked after $i iteration(s)." >&2
    exit 2
  fi
done

echo "Iteration limit reached without completion."
