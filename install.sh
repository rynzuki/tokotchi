#!/bin/sh
# Install tokotchi: put a `tokotchi` launcher on your PATH.
# Requires the claude-token-ledger dependency (checked below).
set -e

REPO_DIR=$(cd "$(dirname "$0")" && pwd)
BIN_DIR="$HOME/.local/bin"
LEDGER_HELPER="$HOME/.claude/token-ledger.sh"

# ── dependency: claude-token-ledger ──────────────────────────────────────────
if [ ! -e "$LEDGER_HELPER" ]; then
  cat <<EOF
! tokotchi depends on claude-token-ledger, which isn't installed.

  Install it first:
    curl -fsSL https://raw.githubusercontent.com/rynzuki/claude-token-ledger/main/install.sh | sh

  Repo: https://github.com/rynzuki/claude-token-ledger

  Then re-run this installer.
EOF
  exit 1
fi

# ── runtime: python3 + curses ────────────────────────────────────────────────
if ! command -v python3 >/dev/null 2>&1; then
  echo "! python3 is required (brew install python / from python.org)"; exit 1
fi
if ! python3 -c 'import curses' >/dev/null 2>&1; then
  echo "! python3 is present but its 'curses' module is missing — tokotchi can't run."; exit 1
fi

# ── launcher on PATH (repo path baked in) ────────────────────────────────────
mkdir -p "$BIN_DIR"
cat > "$BIN_DIR/tokotchi" <<EOF
#!/bin/sh
exec python3 "$REPO_DIR/tokotchi.py" "\$@"
EOF
chmod +x "$BIN_DIR/tokotchi"
echo "+ installed launcher: $BIN_DIR/tokotchi -> $REPO_DIR/tokotchi.py"

case ":$PATH:" in
  *":$BIN_DIR:"*) : ;;
  *) echo "! add $BIN_DIR to your PATH, then open a new shell" ;;
esac

echo "done — run 'tokotchi' in a separate terminal window."
