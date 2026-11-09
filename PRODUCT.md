# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

The shipping surface is KodAct's terminal UI (Rust, cell-based renderer). Design work is delivered as
HTML mocks of that terminal, so `web` here means the mock, never a separate web product.

## Stack

Design deliverables: static HTML, CSS and JavaScript in one file (the brief asked for an interactive
HTML mock). Product: the existing Rust terminal UI in `crates/kodact-cli/src/tui/`.

## Users

The user's own engineering team, using KodAct all day as their coding agent: long interactive
sessions in Linux and WSL terminals, on company model endpoints (`quick-thinking`, `deep-thinking`,
served through an internal OpenAI-compatible gateway), with company MCP servers such as Jira and
GitHub. They direct the agent, approve its actions, read its answers and diffs, and come back to a
session after stepping away.

## Product Purpose

KodAct is a terminal coding agent whose goal is the best measured task success per model, at the
lowest cost and wall time (`docs/GOALS.md`). The terminal UI is where people direct it and watch it
work. Success for the UI: at any moment the person knows what KodAct is doing and what it needs from
them, and a long session stays easy to scan.

## Positioning

KodAct is measured, not assumed: every harness change is checked on its eval suite and on
Terminal-Bench reports. It runs with any model endpoint, including company and local ones, and stays
compatible with the reference CLI's flags, stream-json protocol, settings, hooks, tools, skills and
plugins, so it can replace it.

## Operating Context

- Long sessions in a terminal emulator, often inside tmux, on Linux or WSL, behind a corporate VPN
  and proxy.
- One turn mixes streamed answers, tool calls (shell, file reads and edits, MCP tools), permission
  prompts, background commands, context compaction and KodAct's own reminders.
- The person types prompts, `/` commands, `@` file mentions and `!` shell lines, switches permission
  modes, interrupts and rewinds.

## Capabilities and Constraints

- Terminal-true: everything is drawn in a monospace character grid with ANSI colour (24-bit, with
  256- and 16-colour fallbacks, and `NO_COLOR`), Unicode box and block glyphs, and bold, dim,
  italic, underline and reverse. No images and no proportional type.
- Redraws must stay cheap: long outputs and fast streaming may not lag the UI.
- Today the UI has a welcome screen, a transcript of turns, folded tool results, a permission
  dialog, a `/` command menu, a shortcuts panel and a status bar with model, context meter and
  cost. Markdown rendering covers tables, headings, bold and inline code; lists, italic, links,
  quotes and code colours are not rendered yet.

## Brand Commitments

- The name KodAct as a pixel-art wordmark ("Kod" in paper, "Act" in lime); the welcome
  page shows the name alone, without the tile mark (the user's choice).
- The lime accent as KodAct's signal colour (the input box, the mode, the prompt mark).
- Every command and shortcut works as today (`/` commands, `@` files, `!` shell, `?` shortcuts, the
  key bindings).
- KodAct names only in the UI; nothing borrowed from the reference CLI's naming.
- The design must not look like the reference CLI or the other popular terminal coding agents (the user's brief).

## Evidence on Hand

- Screenshots of the current UI, taken from the real binary.
- Real transcripts of Terminal-Bench runs and the eval suite. Demonstration sessions in mocks are
  synthetic and must say so; no invented customers, benchmarks or claims.

## Product Principles

1. State at a glance: thinking, running a tool, waiting for the person, or done is never in doubt.
2. A long session stays scannable: what ran, what changed and what failed can be found fast.
3. Calm and fast: motion only carries state, nothing lags, nothing performs.
4. Terminal-true: every detail can ship in the real terminal UI.
5. Honest status: errors, costs, limits and KodAct's own interventions are shown plainly.

## Accessibility & Inclusion

Colour never carries state alone: every state also has a glyph and a word, and the design holds in
16 colours and with `NO_COLOR`. Text keeps legible contrast for all-day reading.
