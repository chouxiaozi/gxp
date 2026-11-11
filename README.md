# gxp

[![CI](https://github.com/cianlabs/gxp/actions/workflows/ci.yml/badge.svg)](https://github.com/cianlabs/gxp/actions/workflows/ci.yml)
[![Release](https://github.com/cianlabs/gxp/actions/workflows/release.yml/badge.svg)](https://github.com/cianlabs/gxp/actions/workflows/release.yml)
[![crates.io](https://img.shields.io/crates/v/gxp.svg)](https://crates.io/crates/gxp)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT_OR_Apache--2.0-blue.svg)](LICENSE-MIT)

[English](README.md) | [简体中文](README.zh-CN.md)

**grep-aligned query language for LLM agents** — one DSL for the log- and code-searching workflows of LLM agents. Single Rust binary, zero persistent index.

> Not a strict superset of grep: grep short flags are rejected, and equivalent features use native syntax instead (`context 3` ≈ `-C`, `files` ≈ `-l`, `count` ≈ `-c`).

## Why

In log triage, performance analysis, and code understanding, LLM agents typically shuttle between `rg` + `jq` + `ast-grep`: three syntaxes, multi-layer pipes, three output formats. gxp collapses those workflows into one DSL with **self-contained output** — JSON carries `path` / `line_no` / `line` / captures, so downstream tools never have to reconstruct context from chat history:

| Workflow | Traditional toolchain | gxp |
|---|---|---|
| Error triage | rg + manual aggregation + context stitching | `gxp 'level in [ERROR] \| context 3' app.log` |
| Slow-query Top 20 | rg + jq + sort pipes | `gxp '"checkout" && cost_ms>800 \| rank top 20 by cost_ms desc' *.log` |
| AST structural match | ast-grep + jq | `gxp 'ast(rs, "$OBJ.unwrap()")' src/` |
| Time-window tracing | rg substring approximation (error-prone) | `gxp '_ts between "2026-10-20T12:00:00Z" and "2026-10-20T12:30:00Z"' app.log` |
| Exception grouping | rg + jq group | `gxp 'ERROR \| group by _tag' app.log` |

In a [16-task agent A/B experiment](docs/agent-ab-experiment-en.md), both arms solved 16/16 tasks; the gxp arm used **9% fewer tokens** (249k vs 274k). Raw tool calls were 32 vs 25 — gxp's extra calls were entirely new-DSL discovery cost (`--help`, syntax trials, calls eaten by rate limits) plus voluntary `--verify` runs, while the rg+jq arm paid for dual-engine cross-validation; effective query rounds were equal (≈13 vs ≈13). Most lopsided task: random sampling took gxp **3 calls** (`| sample 20`); the rg+jq arm hand-wrote reservoir sampling in jq — 5 calls, 47.8k tokens, the most expensive single task of the experiment.

## Quick start

```bash
# ① One-liner (Linux / macOS / Windows Git Bash; picks the right prebuilt package + sha256 verify)
curl -fsSL https://github.com/cianlabs/gxp/raw/main/install.sh | sh

# ② One-liner + Claude Code skill (syntax cheat-sheet + 8 workflow templates into ~/.claude/skills/gxp/)
curl -fsSL https://github.com/cianlabs/gxp/raw/main/install.sh | sh -s -- --with-skill

# ③ From source (Rust toolchain required; l3-structural is on by default)
cargo install --locked gxp
```

Prebuilt packages for Linux / Windows / macOS arm64 / macOS x64 (each with a sha256) are also on the [Releases](https://github.com/cianlabs/gxp/releases) page.

> **Windows PowerShell note**: queries containing inner double quotes (`level="ERROR"`, `ast(rs, "PATTERN")`) hit shell quoting pitfalls — PS 5.1 strips inner quotes (query silently degrades to a literal search), while `\"` escaping leaks backslashes on pwsh 7+/bash. Prefer a single-layer-quoting shell (Git Bash), or write the query to a file and run `gxp --query-file query.gxp app.log` (v0.50, quoting-proof). Self-check with `--explain`: a string-equality predicate should appear under `L2 字段谓词`, not as an L1 literal. Full tested matrix: `skills/gxp/references/gxp-syntax.md` → "Windows PowerShell 引号".

## Install the skill into your agent

The agent skill (syntax cheat-sheet + 8 workflow templates + binary-install guidance) lives in [`skills/gxp/`](skills/gxp/):

| Agent | Install |
|---|---|
| Claude Code | `/plugin install gxp --marketplace cianlabs/gxp` |
| 20+ agents (Codex / Cursor / Copilot / Gemini / Windsurf / VS Code / …) | `npx skills add cianlabs/gxp` (`-g` for user-level; anonymous install telemetry, see skills.sh) |
| Any other agent (manual) | copy or symlink `skills/gxp/` into your agent's skills dir (`~/.claude/skills/`, `~/.agents/skills/`, `~/.copilot/skills/`, …) — or `install.sh --with-skill --skill-dir <DIR>` |

Pick **one** install path per machine for Claude Code (plugin *or* `--with-skill`, not both): either works alone, but installing both keeps two skill descriptions in context. The skill teaches your agent to check for the `gxp` binary first and guide installation on user approval — the binary always installs to your user PATH, no agent-specific wiring.

## Capabilities

- **BARE mode**: `gxp 'ERROR' app.log` ≡ `grep -E 'ERROR' app.log`
- **Boolean + field predicates**: `"timeout" && _tag in [checkout,billing] && cost_ms>800` (auto-detects logfmt / JSON-line / `[tag]` / ISO 8601)
- **14 pipeline verbs**: `where` / `context` / `count` / `group by` / `rank` / `sort` / `distinct` / `stats` / `percentiles` / `sample` / `head` / `fields` / `files` / `trace`, freely chainable
- **AST structural queries**: `ast(NAME, "PATTERN")` across 9 languages (ts/tsx/js/py/rs/go/java/c/cpp), extension-filtered, fail-loud
- **Capture extraction**: `capture(NAME, /REGEX/)` — captured values feed later predicates and stages
- **Stack-trace aggregation**: `trace [LANG]`, 6 languages + auto-detection (incl. both old and new Rust panic formats)
- **`--verify`**: brute-force cross-check of the optimized path against the naive scan (L1 ⊇ L2 pushdown invariant) — 100% match or it fails loud
- **Directory scanning**: rayon-parallel, `--glob` filtering, .gitignore-aware

## Performance

Raw scan throughput is not gxp's selling point — on a 900MB count workload (same machine, interleaved runs) ripgrep finished in 575ms, GNU grep 1,024ms, gxp 1,454ms; gxp's win is agent round-trips and self-contained output (see the A/B experiment above). On the 44MB full pipeline: ~107ms / L1 two-pass ~33.5ms / L2 field extraction ~8.2ms (i7-10700, median). Full numbers and methodology: [docs/performance-baseline.md](docs/performance-baseline.md) (Chinese).

## Documentation (Chinese)

| Doc | Contents |
|---|---|
| [docs/gxp-design-full.md](docs/gxp-design-full.md) | Design baseline: semantics / EBNF / execution model / decision log |
| [docs/rust-implementation-plan.md](docs/rust-implementation-plan.md) | Implementation plan: crate choices / module layout / roadmap |
| [docs/capability-matrix.md](docs/capability-matrix.md) | Capability matrix: full status of the 5-layer pipeline |
| [docs/changelog.md](docs/changelog.md) | Release history (v0.1 → v0.49) |
| [AGENTS.md](AGENTS.md) | Collaboration constraints: hard rules / test policy / dogfooding |

## License

MIT OR Apache-2.0 — see [LICENSE-MIT](LICENSE-MIT) / [LICENSE-APACHE](LICENSE-APACHE).
