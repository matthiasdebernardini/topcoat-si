## Per model and arm

| model | arm | runs | pass rate (95% Wilson) | model calls mean / median | median Anthropic cost per case | Anthropic cost per pass | p50 / p90 time | empty answers | errors |
|---|---|---|---|---|---|---|---|---|---|
| claude-fable-5-1 | tool_loop | 32 | 94% (80%-98%) | 2.2 / 2.0 | $0.6049 | $0.8237 | 14.6s / 34.7s | 0% | 0% |
| claude-fable-5-1 | code_mode | 32 | 97% (84%-99%) | 4.1 / 3.0 | $0.0676 | $0.0918 | 11.6s / 27.2s | 0% | 0% |
| claude-opus-5-5 | tool_loop | 32 | 94% (80%-98%) | 2.6 / 2.0 | $0.1081 | $0.2414 | 9.0s / 32.9s | 0% | 0% |
| claude-opus-5-5 | code_mode | 32 | 100% (89%-100%) | 4.1 / 3.0 | $0.0243 | $0.0359 | 9.1s / 27.0s | 0% | 0% |
| claude-sonnet-5-5 | tool_loop | 32 | 94% (80%-98%) | 2.6 / 2.0 | $0.1319 | $0.1252 | 10.4s / 25.9s | 0% | 0% |
| claude-sonnet-5-5 | code_mode | 32 | 97% (84%-99%) | 4.9 / 4.0 | $0.0150 | $0.0230 | 8.8s / 27.9s | 3% | 0% |
| claude-haiku-5-5 | tool_loop | 32 | 91% (76%-97%) | 2.2 / 2.0 | $0.0068 | $0.0256 | 11.8s / 40.5s | 0% | 0% |
| claude-haiku-5-5 | code_mode | 32 | 88% (72%-95%) | 4.1 / 4.0 | $0.0012 | $0.0015 | 10.6s / 23.7s | 0% | 0% |

## Code mode against the tool loop, case by case

| model | kind | cases | wins | losses | ties | median Anthropic cost ratio |
|---|---|---|---|---|---|---|
| claude-fable-5-1 | all | 16 | 2 | 1 | 13 | 0.14 |
| claude-fable-5-1 | fanout | 5 | 1 | 0 | 4 | 0.07 |
| claude-fable-5-1 | classify | 4 | 0 | 1 | 3 | 0.14 |
| claude-fable-5-1 | aggregate | 3 | 1 | 0 | 2 | 0.22 |
| claude-fable-5-1 | control | 3 | 0 | 0 | 3 | 0.17 |
| claude-fable-5-1 | multihop | 1 | 0 | 0 | 1 | 0.51 |
| claude-opus-5-5 | all | 16 | 2 | 0 | 14 | 0.24 |
| claude-opus-5-5 | fanout | 5 | 1 | 0 | 4 | 0.14 |
| claude-opus-5-5 | classify | 4 | 0 | 0 | 4 | 0.31 |
| claude-opus-5-5 | aggregate | 3 | 1 | 0 | 2 | 0.31 |
| claude-opus-5-5 | control | 3 | 0 | 0 | 3 | 0.22 |
| claude-opus-5-5 | multihop | 1 | 0 | 0 | 1 | 0.37 |
| claude-sonnet-5-5 | all | 16 | 1 | 1 | 14 | 0.20 |
| claude-sonnet-5-5 | fanout | 5 | 0 | 0 | 5 | 0.16 |
| claude-sonnet-5-5 | classify | 4 | 0 | 0 | 4 | 0.30 |
| claude-sonnet-5-5 | aggregate | 3 | 0 | 1 | 2 | 0.26 |
| claude-sonnet-5-5 | control | 3 | 0 | 0 | 3 | 0.14 |
| claude-sonnet-5-5 | multihop | 1 | 1 | 0 | 0 | 0.25 |
| claude-haiku-5-5 | all | 16 | 1 | 2 | 13 | 0.13 |
| claude-haiku-5-5 | fanout | 5 | 0 | 0 | 5 | 0.11 |
| claude-haiku-5-5 | classify | 4 | 0 | 1 | 3 | 0.19 |
| claude-haiku-5-5 | aggregate | 3 | 1 | 0 | 2 | 0.19 |
| claude-haiku-5-5 | control | 3 | 0 | 0 | 3 | 0.15 |
| claude-haiku-5-5 | multihop | 1 | 0 | 1 | 0 | 0.26 |
