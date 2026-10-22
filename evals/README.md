# topcoat-si evals

**Warning: running an eval costs real money.** Every run calls the Anthropic API, Firecrawl and Jev with your keys. `--budget-cents` caps only the Anthropic spend; see [Budget](#budget).

## What `code-mode` measures

The `code-mode` eval asks the same questions two ways, with the same model, and compares them:

- **`tool_loop`**: `Anthropic::run_tools` with three client tools: `web_search` and `fetch_page` (Firecrawl) and `jev`. Claude calls one tool, reads the result, calls the next.
- **`code_mode`**: `run_tools` with the one `python` tool and all five packs (`firecrawl` with no credit cap, as in the tool loop; `jev`, `research`, `triage`, `reports`) and `Limits::default()`. Claude writes Python that calls the same services.

Both arms use `max_tokens` 4096, at most 15 turns, prompt caching and the same base system prompt: "Answer the user's question using the tools. Do not answer from memory. End with a plain-text answer." The tool loop adds "You may call several tools in one turn." Code mode adds the system prompt from the code-mode example in the main README, as "Advice for apps" recommends.

It answers: does code mode answer as often as the tool loop, and what does it cost in money, model calls and time?

Runs are sequential. For each case and repeat, each model runs both arms back to back. The arm that goes first alternates between repeats (repeat 0: tool loop first, repeat 1: code mode first), so neither arm always gets a warm cache or a fresher web.

## Grading

A run passes only if both checks pass:

1. **Deterministic.** The run ended with non-empty text, the text contains at least one string from every `must_include` group, and none of the `must_not_include` strings. Matching ignores case.
2. **Judge.** Jev, TypeSafe's decision model, is asked the case's `judge` question with the final answer as its state. The run passes on a yes with certainty `|2p - 1|` of at least 0.8. Every run with non-empty text is judged, and the probability is recorded either way. If the judge call fails, that is recorded and the run does not pass.

The judge model comes from `TYPESAFE_DEFAULT_MODEL`. It must be a pinned version such as `jev-1.13.0`: the eval refuses to start, before any paid call, if it contains `latest` or `preview`.

## The case file

`evals/suites/code_mode.jsonl` holds one JSON (JavaScript Object Notation) object per line:

```json
{"id": "rust-editions", "split": "dev", "kind": "fanout",
 "prompt": "List every Rust edition and the Rust version that stabilised it.",
 "must_include": [["2015"], ["2018", "1.31"], ["2021", "1.56"], ["2024", "1.85"]],
 "must_not_include": [],
 "judge": "Does the answer list the Rust editions 2015, 2018, 2021 and 2024, with 2018 stabilised in 1.31, 2021 in 1.56 and 2024 in 1.85?",
 "sources": ["https://doc.rust-lang.org/edition-guide/"]}
```

(Shown wrapped; in the file each case is one line.)

| Field | Meaning |
|---|---|
| `id` | unique, non-empty |
| `split` | `dev` for tuning, `holdout` to check the tuning did not overfit |
| `kind` | `fanout`, `classify`, `aggregate`, `control` or `multihop` |
| `prompt` | the user message |
| `must_include` | groups of alternatives; each group passes if the answer contains any one of its strings. Not empty, and no group empty |
| `must_not_include` | strings that fail the run if present |
| `judge` | one yes/no question for Jev about the final answer |
| `sources` | for humans only, never sent to a model |

The loader rejects the whole file on an unknown field, `kind` or `split`, an empty `must_include` or group, or a duplicate `id`, naming the line.

## Running it

```bash
cargo run -p topcoat-evals -- code-mode --budget-cents 300 [options]
```

| Option | Default | Meaning |
|---|---|---|
| `--budget-cents N` | required | stop before a run once the Anthropic spend reaches N cents |
| `--models A,B` | `sonnet-5-5` | models, with or without the `claude-` prefix: `opus-5-5`, `sonnet-5-5`, `haiku-5-5`, `fable-5-1` |
| `--repeat N` | 3 | runs per case, arm and model |
| `--split S` | `dev` | `dev`, `holdout` or `all` |
| `--cases ID,ID` | all in the split | only these case ids; an id that is unknown or outside `--split` is an error |
| `--plan` | | print the runs in execution order and their count, then exit; calls no API and needs no keys |
| `--db PATH` | | also store every run in the SurrealDB directory `PATH`, under the same timestamp as the file names, as it ends; ignored with `--plan`, which opens nothing |
| `--save-baseline` | | also write this run's summary to `evals/baselines/code-mode.json`; refused when the budget cut the run short |

Always run with `--plan` first to see how many runs a command makes.

Environment variables, all required except with `--plan`:

| Variable | For |
|---|---|
| `ANTHROPIC_API_KEY` | Claude |
| `FIRECRAWL_API_KEY` | web search and page fetch |
| `TYPESAFE_API_KEY` | Jev, in both arms and as the judge |
| `TYPESAFE_DEFAULT_MODEL` | the Jev model, pinned |
| `MONTY_BIN` | the `monty` worker binary from `monty-runtime` 1.2 (see the main README's code-mode setup) |

### Budget

Before each run, the eval adds up the Anthropic cost of every response so far (`KnownModel::cost_micro_cents`) and stops if it has reached `--budget-cents`. It prints the run it stopped before and how many runs it skipped, then still writes the summary for the runs done. The run in progress always finishes, so the spend can pass the budget by up to one run.

Jev calls (in both arms and the judge) and Firecrawl credits are billed by TypeSafe and Firecrawl and are **not** counted in the budget.

## Output

Each invocation writes to `evals/results/` (ignored by git), named by its UTC (Coordinated Universal Time) start time. It never overwrites: a second invocation in the same second fails rather than replace the first one's files.

- `<timestamp>-code-mode.jsonl`: one line per run, written and flushed as each run ends, so a crash keeps the runs before it. Fields: `case_id`, `kind`, `split`, `arm`, `model`, `repeat`; `pass`, `deterministic_pass`, `judge_probability` (of yes), `judge_error`, `answer`, `error` and `error_kind` when the run failed; `model_calls`, `tool_calls` (name to count), `python_calls`, `empty_final_answer`; `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`; `cost_micro_cents` (100,000,000 to the dollar) and `wall_seconds`.
- `<timestamp>-code-mode.md`: the summary, also printed.
- `<timestamp>-spotcheck.md`: up to 10 judged runs spread evenly over the run order (index i × judged / 10), each with the question, the answer, Jev's probability and the reference facts (judge question, `must_include`, `must_not_include`). Read it to check that the judge agrees with you.

### Reading the summary

Every cost in the summary is the **Anthropic cost** only: what Claude's responses cost by `KnownModel::cost_micro_cents`. Firecrawl and Jev costs are excluded, in both arms. In code mode, the Firecrawl and Jev calls a snippet makes happen inside the sandbox, so `tool_calls` shows them only as `python` calls.

**Per model and arm:**

- **pass rate** with its 95% Wilson score interval. Overlapping intervals mean the difference may be noise; more repeats or cases narrow them.
- **model calls**, mean and median per run: round trips to Claude.
- **median Anthropic cost per case**: each case's mean run cost, then the median across cases.
- **Anthropic cost per pass**: total cost divided by passing runs; what one good answer costs Anthropic.
- **p50 / p90 time**: wall-clock seconds per run, judge excluded.
- **empty answers**: runs that ended without error but with no text. **errors**: runs that failed.

**Code mode against the tool loop, case by case**, per model, for all cases and then per `kind`. For each case, each arm's pass rate over its repeats is compared:

- **wins / losses / ties**: cases where code mode's pass rate is higher, lower or the same.
- **median Anthropic cost ratio**: code mode's mean Anthropic cost divided by the tool loop's, median across cases. Below 1 means code mode's Anthropic bill is lower; Firecrawl and Jev costs are not in it.

**Change from the baseline** appears when `evals/baselines/code-mode.json` exists: each model and arm's pass rate and median Anthropic cost per case, before and after. Write a new baseline with `--save-baseline`. The baseline records its selection (split, case ids, models, repeat, and whether the budget cut it short); when this run's selection differs, the section opens with a warning that it compares unlike runs. The committed baseline is the October 2026 dev run (16 cases, Fable 5.1, Opus 5.5, Sonnet 5.5 and Haiku 5.5, 2 repeats, judge `jev-1.14.0`); `code-mode.jsonl` and `code-mode.md` next to it are that run's results and summary. It ran with code mode capped at 40 Firecrawl credits, which no longer applies.

## Summarizing a results file

`summarize` rebuilds the summary of a finished run from its `<timestamp>-code-mode.jsonl` without calling anything or needing keys. It prints the summary (with the change from the baseline, if there is one) and writes no results files; `--save-baseline` writes it to `evals/baselines/code-mode.json`. The selection comes from the file, as `code-mode` would have recorded it: the case ids in suite file order (an id not in the suite is an error), the models in first-seen order and the highest repeat plus one. The split is `dev` or `holdout` when every run is in that split, else `all`; a file of only dev cases cannot tell `--split all --cases` from `--split dev`, so `summarize` picks the narrower split. If any case, model, arm and repeat is missing or duplicated, `summarize` prints how many and the first few, marks the run partial, and `--save-baseline` refuses it. It cannot catch a run that stopped exactly between cases, or after the first model finished both arms of every repeat: the file then looks like a smaller complete run. Only save a baseline from a run whose `code-mode` output did not report a budget stop.

```bash
cargo run -p topcoat-evals -- summarize evals/results/<timestamp>-code-mode.jsonl [--save-baseline]
```

## The results database

With `--db PATH`, `code-mode` also stores each run's record in a SurrealDB (embedded database) directory, in the `eval_run` table: the same fields as a line of the `.jsonl` file, plus `run` (the timestamp in the file names) and `at` (when it was stored). The database holds many runs, so it can be queried across them. SurrealKV locks the directory, so one process at a time can open it.

```bash
cargo run -p topcoat-evals -- summarize --db PATH               # list the stored run stamps
cargo run -p topcoat-evals -- summarize --db PATH --run <timestamp> [--save-baseline]
```

`summarize` takes either a results file or `--db`, not both; `--run` needs `--db`.
