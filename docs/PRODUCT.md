# PRODUCT.md

register: product

## What this is

Portreaper is a macOS + Windows tray utility that finds processes listening on
TCP ports and tells the developer **which ones are orphaned dev-server
"zombies"** (a `vite` / `node` / `cargo run` whose launching shell died), so
they can be killed with confidence. It is not a generic port viewer: the
verdict is the product. Two further detections round out the core: **orphaned
dev processes that hold no port at all** (an `electron-vite` Electron main left
behind when its parent dies — invisible to a port scan) and **duplicate dev
servers of the same project** (a forgotten earlier launch holding the port
hostage; flagged `Possible`, never swept).

## Users

Developers, mid coding session. The canonical visit: a port is "already in
use", they click the tray icon, find the ghost on :5173, kill it, close the
window. Dwell time under 15 seconds. They live in dark terminals and IDEs.

## Tone

Calm, factual, confident. The app makes a serious claim ("this process is safe
to kill") and must read like a tool that earned that claim: plain language
verdicts, evidence available on demand, no alarmism. Bilingual zh/en with equal
care.

## Strategic principles

1. **Answer first, evidence second.** The UI states the conclusion in one
   human sentence ("its launching terminal is gone"); raw signals (reason
   codes, parent chain, paths) live one click deeper.
2. **The zombie list IS the interface.** Suspects are visually segregated from
   healthy listeners; an empty zombie section is a success state worth showing.
3. **Danger color is a verdict, never decoration.** Red appears only on
   zombie classifications and kill affordances.
4. **Never interrupt trust.** Destructive actions always confirm; whitelisted
   ("starred") processes are never flagged; "Possible" tier is never swept.

## Anti-references

- Activity-monitor clones: ten equal-weight columns of metrics nobody reads.
- Hacker-neon dashboards: glow, gradients, rainbow badges on every cell.
- Enterprise dashboards: KPI pills, hero metrics, card grids.
