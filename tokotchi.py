#!/usr/bin/env python3
"""Tokotchi — a collectible terminal pet that levels off your Claude token usage.

Self-contained (Python 3 stdlib only, no pip installs). The pet's level and
evolution derive entirely from the all-time token total in the ledger
(~/.claude/.token_ledger.json, maintained by token-ledger.sh). View-only for now:
no feeding or playing — it just grows as you burn tokens.

Launch in a SEPARATE terminal window (needs a real TTY — not Claude Code's `!`):
    tokotchi

Depends on claude-token-ledger (github.com/rynzuki/claude-token-ledger) for the
token tally it reads; if that isn't installed, it prints install instructions.
"""

import curses
import json
import math
import os
import random
import subprocess
import sys
import threading
import time

HOME = os.path.expanduser("~")
LEDGER = os.path.join(HOME, ".claude", ".token_ledger.json")
HELPER = os.path.join(HOME, ".claude", "token-ledger.sh")
STATE = os.path.join(HOME, ".claude", ".tokotchi_state.json")  # last level (for level-up detection)
UNIT = 1_000_000          # 1 level-unit = 1M tokens; level = floor(sqrt(total/UNIT))
AUTO_REFRESH_SECS = 20     # background ledger re-scan cadence
CELEBRATE_SECS = 45        # how long the statusline flashes a level-up after it happens
CELEBRATE_TUI_SECS = 8     # how long the live TUI throws a level-up party


LEDGER_REPO = "https://github.com/rynzuki/claude-token-ledger"
NEED_LEDGER = (
    "✦ Tokotchi needs the Claude token-ledger — it's not installed.\n\n"
    "  A small standalone tool that builds the token tally Tokotchi reads.\n\n"
    "  Install it:\n"
    "    curl -fsSL "
    "https://raw.githubusercontent.com/rynzuki/claude-token-ledger/main/install.sh | sh\n\n"
    f"  Repo: {LEDGER_REPO}\n\n"
    "Then run `tokotchi` again.\n"
)


def ledger_available():
    """The token-ledger dependency installs its helper here (see NEED_LEDGER)."""
    return os.path.exists(HELPER)


# ── data ────────────────────────────────────────────────────────────────────
def read_total():
    try:
        with open(LEDGER) as f:
            return sum(int(v) for v in json.load(f).values())
    except Exception:
        return 0


def refresh_ledger():
    """Re-scan transcripts into the ledger (~1s). Safe to call from a thread."""
    try:
        subprocess.run(["sh", HELPER, "update"], timeout=30,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except Exception:
        pass


def level_for(total):
    return max(1, math.isqrt(max(0, total) // UNIT))


def progress(total):
    """(fraction 0..1, tokens_into_level, tokens_needed_for_level) for current level."""
    lvl = level_for(total)
    floor_t = (lvl * lvl) * UNIT
    next_t = ((lvl + 1) * (lvl + 1)) * UNIT
    span = next_t - floor_t
    into = max(0, total - floor_t)
    return (min(1.0, into / span) if span else 0.0), into, span


def humanize(n):
    n = float(n)
    if n >= 1e9:  return f"{n/1e9:.2f}B"
    if n >= 1e6:  return f"{n/1e6:.1f}M"
    if n >= 1e3:  return f"{n/1e3:.1f}K"
    return f"{int(n)}"


# ── evolution stages ─────────────────────────────────────────────────────────
# Each stage: (min_level, name, color_key, [frame0_lines, frame1_lines]).
# frame1 is the "blink" frame. Keep art the same width/height across both frames.
STAGES = [
    (1, "Egg", "egg", [
        ["  ___ ", " /   \\", "|     |", "|     |", " \\___/"],
        ["  ___ ", " /   \\", "| . . |", "|     |", " \\___/"],
    ]),
    (5, "Blob", "blob", [
        [" _____ ", "/     \\", "| o o |", "|  ω  |", "\\_____/"],
        [" _____ ", "/     \\", "| - - |", "|  ω  |", "\\_____/"],
    ]),
    (15, "Sprout", "sprout", [
        ["   ,   ", "  (|)  ", " /o o\\ ", "( \\_/ )", " \\___/ "],
        ["   ,   ", "  (|)  ", " /- -\\ ", "( \\_/ )", " \\___/ "],
    ]),
    (30, "Critter", "critter", [
        [" /\\_/\\ ", "( o.o )", " > ^ < ", "/|   |\\", " |___| "],
        [" /\\_/\\ ", "( -.- )", " > ^ < ", "/|   |\\", " |___| "],
    ]),
    (60, "Beast", "beast", [
        ["  __/\\__  ", " ( o  o ) ", "<   VV   >", " \\ ~~~~ / ", " /|    |\\ "],
        ["  __/\\__  ", " ( -  - ) ", "<   VV   >", " \\ ~~~~ / ", " /|    |\\ "],
    ]),
    (100, "Elder", "elder", [
        ["✦  __/\\__  ✦", "  ( ◕  ◕ )  ", " <   WW   > ", "✦ \\ ~~~~ / ✦", "  /|    |\\  "],
        ["✦  __/\\__  ✦", "  ( ─  ─ )  ", " <   WW   > ", "✦ \\ ~~~~ / ✦", "  /|    |\\  "],
    ]),
]

# Claude palette (256-color): warm "clay" accent (~#CC785C) on cream, muted taupe
# for secondary text. The evolution stages ramp through the same warm family, so
# the pet visibly "warms up" toward full Claude clay as it grows.
COLORS = {
    # evolution ramp — pale cream → tan → gold → clay → coral → bright clay
    "egg": 223, "blob": 180, "sprout": 179, "critter": 173, "beast": 209, "elder": 215,
    # ui chrome
    "accent": 173,   # clay — title + card border
    "cream": 223,    # level number
    "muted": 244,    # secondary text / footer
    "faint": 239,    # empty xp track
    "bar": 173,      # filled xp
    "spark": 230,    # pale gold — celebration sparkles
}

PANEL_W = 38
PANEL_H = 19
CREATURE_TOP = 1   # interior row where the creature's band begins
CREATURE_BAND = 6  # rows reserved for the creature (art is 5 tall + 1 for the bob)

# Ambient/celebration sparkle glyphs (twinkle over each particle's short life).
SPARKLE_CHARS = ["·", "✦", "✧", "⋆", "✳"]


def stage_for(level):
    chosen = STAGES[0]
    for st in STAGES:
        if level >= st[0]:
            chosen = st
    return chosen


def next_stage(level):
    """First stage the pet hasn't reached yet, or None if it's fully evolved."""
    for st in STAGES:
        if st[0] > level:
            return st
    return None


# One emoji per evolution stage — used by the `level` CLI / statusline.
STAGE_EMOJI = {
    "Egg": "🥚", "Blob": "👾", "Sprout": "🌱",
    "Critter": "🐱", "Beast": "🐉", "Elder": "👑",
}


# ── level CLI (consumed by the statusline) ───────────────────────────────────
def _read_state():
    try:
        with open(STATE) as f:
            d = json.load(f)
        return int(d.get("level", 0)), float(d.get("celebrate_until", 0.0))
    except Exception:
        return None


def _write_state(level, celebrate_until):
    try:
        tmp = STATE + ".tmp"
        with open(tmp, "w") as f:
            json.dump({"level": int(level), "celebrate_until": float(celebrate_until)}, f)
        os.replace(tmp, STATE)
    except Exception:
        pass


def cli_level():
    """Print '<level>\\t<stage>\\t<emoji>\\t<levelup 0|1>' for the statusline.

    Recomputes the level from the ledger every call and flags a level-up for
    CELEBRATE_SECS after the stored level increases (persisted in STATE).
    Prints nothing if the ledger dependency is absent, so a consuming statusline
    simply omits the pet segment."""
    if not ledger_available():
        return
    total = read_total()
    cur = level_for(total)
    name = stage_for(cur)[1]
    emoji = STAGE_EMOJI.get(name, "•")
    now = time.time()

    st = _read_state()
    if st is None:
        # first ever call — record the level, never celebrate retroactively
        _write_state(cur, 0.0)
        celeb = 0.0
    else:
        last, celeb = st
        if cur > last:
            celeb = now + CELEBRATE_SECS
            _write_state(cur, celeb)
        elif cur < last:
            celeb = 0.0
            _write_state(cur, celeb)
        # unchanged level → no write; celeb window (if any) keeps ticking down

    levelup = 1 if now < celeb else 0
    sys.stdout.write(f"{cur}\t{name}\t{emoji}\t{levelup}\n")


# ── rendering ────────────────────────────────────────────────────────────────
def _add(stdscr, y, x, text, attr=0):
    try:
        stdscr.addstr(y, x, text, attr)
    except curses.error:
        pass  # off-screen / bottom-right cell — ignore


def _box(stdscr, top, left, h, w, attr):
    _add(stdscr, top, left, "╭" + "─" * (w - 2) + "╮", attr)
    for r in range(1, h - 1):
        _add(stdscr, top + r, left, "│", attr)
        _add(stdscr, top + r, left + w - 1, "│", attr)
    _add(stdscr, top + h - 1, left, "╰" + "─" * (w - 2) + "╯", attr)


# ── ambient sparkles ─────────────────────────────────────────────────────────
# Particles live only in the creature band (interior rows 0..BAND), so the status
# rows below never flicker. Each is [row, col, char, age, ttl]; they twinkle
# dim→bright→dim over their short life. Denser during a level-up celebration.
def step_particles(state, band_rows, iw, celebrating):
    ps = state["particles"]
    for p in ps:
        p[3] += 1
    ps[:] = [p for p in ps if p[3] < p[4]]

    spawn_chance = 0.85 if celebrating else 0.22
    cap = 26 if celebrating else 10
    if len(ps) < cap and random.random() < spawn_chance:
        row = random.randint(0, band_rows)
        col = random.randint(0, iw - 1)
        char = random.choice(SPARKLE_CHARS)
        ttl = random.randint(10, 22)
        ps.append([row, col, char, 0, ttl])


def render_particles(stdscr, state, top, left, cp, celebrating):
    key = "spark" if celebrating else None
    for row, col, char, age, ttl in state["particles"]:
        phase = age / ttl
        attr = curses.A_BOLD if 0.33 <= phase <= 0.66 else curses.A_DIM
        color = cp(key) if key else cp(state["ckey"])
        _add(stdscr, top + 1 + row, left + 1 + col, char, color | attr)


def draw(stdscr, state, frame):
    stdscr.erase()
    h, w = stdscr.getmaxyx()

    total = state["total"]
    lvl = level_for(total)
    _, name, ckey, frames = stage_for(lvl)
    state["ckey"] = ckey
    frac, into, span = progress(total)
    celebrating = time.time() < state.get("celebrate_until", 0.0)

    def cp(key):
        return curses.color_pair(state["pairs"].get(key, 0))

    # too small for the card → compact one-liner
    if h < PANEL_H + 1 or w < PANEL_W + 1:
        _add(stdscr, 0, 0, f"Tokotchi · Lv {lvl} · Σ {humanize(total)}  (resize for full view)",
             cp("accent"))
        stdscr.refresh()
        return

    top = (h - PANEL_H) // 2
    left = (w - PANEL_W) // 2
    iw = PANEL_W - 2  # interior width

    def panel(row, text, attr=0):
        """Center `text` on interior `row` (0-based, under the top border)."""
        col = left + 1 + max(0, (iw - len(text)) // 2)
        _add(stdscr, top + 1 + row, col, text, attr)

    _box(stdscr, top, left, PANEL_H, PANEL_W, cp("accent"))

    # title inset on the top border
    title = " ❋ TOKOTCHI ❋ "
    _add(stdscr, top, left + (PANEL_W - len(title)) // 2, title, cp("accent") | curses.A_BOLD)

    # sparkles first, so the creature always renders on top of them
    step_particles(state, CREATURE_TOP + CREATURE_BAND, iw, celebrating)
    render_particles(stdscr, state, top, left, cp, celebrating)

    # creature — the ONLY thing the bob moves, kept inside its reserved band
    blink = (frame % 36) in (0, 1)
    art = frames[1] if blink else frames[0]
    bob = 1 if (frame // 7) % 2 == 0 else 0
    glow = curses.A_BOLD if (name == "Elder" or celebrating) else 0
    for i, line in enumerate(art):
        panel(CREATURE_TOP + bob + i, line, cp(ckey) | glow)

    # status block — fixed rows, never affected by the bob
    base = CREATURE_TOP + CREATURE_BAND + 1   # interior row 8
    if celebrating:
        pulse = curses.A_BOLD if (frame // 4) % 2 == 0 else curses.A_DIM
        panel(base, "✦ LEVEL UP! ✦", cp("spark") | pulse)
    else:
        panel(base, name, cp(ckey) | curses.A_BOLD)
    panel(base + 1, f"Lv {lvl}", cp("cream") | curses.A_BOLD)

    # xp bar, drawn as filled + empty tracks
    bar_w = iw - 6
    filled = int(round(frac * bar_w))
    barrow = top + 1 + base + 3
    barcol = left + 1 + 3
    _add(stdscr, barrow, barcol, "━" * filled, cp("bar") | curses.A_BOLD)
    _add(stdscr, barrow, barcol + filled, "━" * (bar_w - filled), cp("faint"))
    panel(base + 4, f"{humanize(into)} / {humanize(span)} → Lv {lvl + 1}", cp("muted"))

    # next-evolution preview (or a max-form badge for the Elder)
    nxt = next_stage(lvl)
    evo = f"Next: {nxt[1]} at Lv {nxt[0]}" if nxt else "✦ fully evolved ✦"
    panel(base + 5, evo, cp("muted"))

    panel(base + 7, f"Σ {humanize(total)}", cp(ckey))

    # footer inset on the bottom border (auto-refreshes every AUTO_REFRESH_SECS)
    foot = " [q] quit "
    _add(stdscr, top + PANEL_H - 1, left + (PANEL_W - len(foot)) // 2, foot, cp("muted"))
    stdscr.refresh()


# ── main loop ────────────────────────────────────────────────────────────────
def run(stdscr):
    curses.curs_set(0)
    stdscr.nodelay(True)
    stdscr.timeout(100)  # ~10 fps

    has_color = curses.has_colors()
    if has_color:
        curses.start_color()
        try:
            curses.use_default_colors()
        except curses.error:
            pass
    pairs = {}
    if has_color:
        idx = 1
        for key, fg in COLORS.items():
            try:
                curses.init_pair(idx, fg if fg < curses.COLORS else 7, -1)
                pairs[key] = idx
                idx += 1
            except curses.error:
                pass

    total0 = read_total()
    state = {
        "total": total0,
        "pairs": pairs,
        "syncing": True,
        "particles": [],
        "ckey": stage_for(level_for(total0))[2],
        "celebrate_until": 0.0,
    }

    # initial + periodic background refresh (never blocks the animation)
    stop = threading.Event()

    def worker(once=False):
        refresh_ledger()
        new_total = read_total()
        if level_for(new_total) > level_for(state["total"]):
            state["celebrate_until"] = time.time() + CELEBRATE_TUI_SECS
        state["total"] = new_total
        state["syncing"] = False

    threading.Thread(target=worker, daemon=True).start()

    def auto():
        while not stop.wait(AUTO_REFRESH_SECS):
            state["syncing"] = True
            worker()
    threading.Thread(target=auto, daemon=True).start()

    frame = 0
    try:
        while True:
            draw(stdscr, state, frame)
            frame += 1
            try:
                ch = stdscr.getch()
            except curses.error:
                ch = -1
            if ch in (ord("q"), ord("Q"), 27):
                break
            if ch == curses.KEY_RESIZE:
                stdscr.erase()
    finally:
        stop.set()


NEED_TTY = (
    "Tokotchi needs a real terminal — it can't run inside Claude Code's `!` prompt\n"
    "or any captured shell.\n\n"
    "Open a SEPARATE terminal window (Terminal, iTerm, Warp…) that is a normal shell,\n"
    "and run:\n\n    tokotchi\n"
)


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "level":
        cli_level()
        return
    if not ledger_available():
        print(NEED_LEDGER)
        return
    if not (sys.stdin.isatty() and sys.stdout.isatty()):
        print(NEED_TTY)
        return
    try:
        curses.wrapper(run)
    except KeyboardInterrupt:
        pass
    except curses.error as e:
        print(f"Tokotchi couldn't start the terminal UI ({e}).\n" + NEED_TTY)


if __name__ == "__main__":
    main()
