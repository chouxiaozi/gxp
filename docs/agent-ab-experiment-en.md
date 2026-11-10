# Agent A/B experiment — gxp vs rg+jq+ast-grep (2026-11-09)

> English summary of the full Chinese write-up [agent-ab-experiment.md](agent-ab-experiment.md). Numbers are reproduced verbatim; the full doc adds ground-truth methodology and per-task detail.

## Question

Does one query language (gxp) cost an LLM agent less than shuttling between `rg` + `jq` + `ast-grep`?

## Method

- 16 independent zero-context sub-agents (8 scenarios × 2 arms), same model, same machine, same time window. No agent knew the expected answers.
- **gxp arm**: only `gxp` (v0.47.0, `l3-structural` on); rg/grep/jq/ast-grep and built-in search tools disabled.
- **rg arm**: only `rg` 15.2 + `jq` 1.8 + `ast-grep` 0.51; gxp disabled.
- Correctness was graded against ground-truth answers **frozen before the runs** and cross-validated by two independent methods (gxp and grep/construction models).
- Metrics: tool calls and token totals (agent-reported under honesty instructions); wall-clock excluded (16 parallel agents distort scheduling).
- Datasets: deterministic synthetic JSONL — 899.81 MiB, 6.7M lines — plus the gxp source tree for the AST/code tasks.

## Results

| Scenario | gxp: correct / calls / tokens | rg+jq: correct / calls / tokens | fewer calls |
|---|---|---|---|
| A error triage | ✓✓ / 3 / 28.6k | ✓✓ / 4 / 33.3k | gxp |
| B numeric analysis | ✓✓ / 2 / 27.1k | ✓✓ / 3 / 31.8k | gxp |
| C AST structural match | ✓✓ / 4 / 30.5k | ✓✓ / 1 / 31.1k | rg |
| D time-window tracing | ✓✓ / 4 / 27.9k | ✓✓ / 5 / 36.6k | gxp |
| E file inventory | ✓✓ / 4 / 30.2k | ✓✓ / 1 / 29.1k | rg |
| F random sampling | ✓✓ / 3 / 33.6k | ✓✓ / 5 / 47.8k | gxp |
| G dedup | ✓✓ / 9 / 42.4k | ✓✓ / 2 / 31.5k | rg |
| H full sort | ✓✓ / 3 / 28.8k | ✓✓ / 4 / 32.8k | gxp |
| **Total** | **16/16** / **32** / **249.1k** | **16/16** / **25** / **274.0k** | |

- **Correctness: tie** — 16/16 both arms; a strong model finishes either way.
- **Tokens: gxp −9%** (249.1k vs 274.0k).
- **Raw calls: gxp 32 vs rg 25.** Decomposition: **effective query rounds ≈13 vs ≈13 (tie)**. gxp's extra rounds were discovery cost of an unfamiliar DSL (`--help`, syntax trials, calls eaten by rate limits) plus voluntary `--verify` cross-checks; the rg arm's overhead was dual-engine (rg×jq) cross-validation.
- **Most lopsided task** — random sampling (F): gxp `| sample 20` in 3 calls; the rg arm hand-wrote a Park–Miller LCG + reservoir sampling inside jq — 5 calls, 47.8k tokens, the most expensive single task of the whole experiment.
- **`--verify` as a confidence asset**: rg-arm agents noted "rg has no JSON semantics, regex alone can't be trusted" and manually cross-checked with a second engine; gxp-arm agents reached the same confidence with one `--verify`.
- **Where rg+jq won**: pure text / structured-field tasks (C/E/G) — shell pipes fuse `rg | jq` into a single call, and both tools sit in the model's pretraining distribution with zero discovery cost.
- One runtime cost the call counts hide: jq parsing the full 900MB file ran near a minute in some rg-arm tasks, and piping large streams through PowerShell added overhead.

## Honest limitations

n=1 per cell (no variance estimate); calls are agent-reported, not independently instrumented; single model, single day, single machine; the gxp-arm prompt contained a usage example while the rg-arm prompt did not (asymmetry, disclosed); wall-clock excluded.

## Conclusions

- "One DSL beats multi-tool round-trips" **does not hold for raw call counts** with a strong model, **partially holds for tokens (−9%)**, and holds most clearly in semantic-predicate scenarios (sampling / numeric / time-window). The discovery cost of a new DSL is real but should amortize within a session.
- The experiment directly shaped v0.48: the single largest friction (3/8 gxp-arm agents spontaneously wrote `level="ERROR"` and were rejected) shipped as string-equality predicates.
- Queued follow-ups: weak-model rerun of F/G (is "implementation-burden transfer" survivable?), n=3 variance estimates, within-session amortization (same agent, 10 consecutive tasks).
