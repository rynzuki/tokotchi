# tokotchi 🥚→👑

A collectible terminal pet that levels up off the tokens you spend on [Claude Code](https://claude.com/claude-code). No feeding, no chores — it just grows as you burn tokens, evolving through six stages.

```
        ╭─────── ❋ TOKOTCHI ❋ ───────╮
        │                            │
        │           /\_/\            │
        │          ( o.o )           │
        │           > ^ <            │
        │          /|   |\           │
        │           |___|            │
        │                            │
        │          Critter           │
        │           Lv 48            │
        │                            │
        │    ━━━━━━━━━━━━━━━━━━━     │
        │    5.4M / 97.0M → Lv 49    │
        │                            │
        │          Σ 2.31B           │
        ╰─── [r] refresh · [q] quit ─╯
```

> _Replace this ASCII sketch with a real screen recording — see [Demo](#demo)._

## Install

Tokotchi depends on **[claude-token-ledger](https://github.com/rynzuki/claude-token-ledger)** for its token tally. Install that first:

```sh
curl -fsSL https://raw.githubusercontent.com/rynzuki/claude-token-ledger/main/install.sh | sh
```

Then tokotchi:

```sh
git clone https://github.com/rynzuki/tokotchi
cd tokotchi && ./install.sh
```

The installer checks for the ledger dependency and for `python3` + `curses` (Python stdlib), then drops a `tokotchi` launcher on your PATH.

## Run

In a **separate terminal window** (it's a full-screen TUI — it can't share the terminal Claude Code is running in):

```sh
tokotchi
```

`[r]` refreshes the ledger · `[q]` quits.

## Leveling & evolution

The level is a pure function of your all-time token total `Σ`:

```
level = ⌊ √(Σ / 1,000,000) ⌋
```

so it always climbs but naturally slows down. Evolution is gated by level:

| Stage | Levels | ~Tokens |
|---|---|---|
| 🥚 Egg | 1–4 | 0 |
| 👾 Blob | 5–14 | ~25M |
| 🌱 Sprout | 15–29 | ~225M |
| 🐱 Critter | 30–59 | ~900M |
| 🐉 Beast | 60–99 | ~3.6B |
| 👑 Elder | 100+ | ~10B |

The pet also warms in colour from pale cream toward full Claude-clay orange as it grows.

## Statusline integration

`tokotchi level` prints a tab-separated line — `<level>\t<stage>\t<emoji>\t<levelup>` — for a statusline to consume. The 4th field flags a level-up for ~45s after it happens (tracked in `~/.claude/.tokotchi_state.json`), so a bar can flash a celebration. If the ledger dependency is missing, it prints nothing and your statusline just omits the segment.

## Demo

Record a terminal GIF (e.g. with [`vhs`](https://github.com/charmbracelet/vhs) or [`asciinema`](https://asciinema.org)) and drop it here.

## License

MIT © Yannic Oberhausen
