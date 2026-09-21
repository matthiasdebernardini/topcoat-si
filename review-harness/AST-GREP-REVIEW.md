# Ast-grep source review

Checked 2026-09-21 against implementation commit `d12a7e4`. The installed canonical Rust rules are `/Users/md/.agents/skills/rust-new/templates/sgconfig.yml`; no project-local ruleset was present. The configured 40 rules contain 38 enabled rules and two disabled rules. The three hook-level blocking rules were promoted to errors for this run.

Run from the crate directory: scanning from the project root would incorrectly exclude this source through the rules’ `vendor/**` exclusions. All 97 source files were scanned; zero were skipped.

```sh
cd vendor/claudius
ast-grep scan --config /Users/md/.agents/skills/rust-new/templates/sgconfig.yml \
  --error=complex-conditional --error=deeply-nested-if --error=factory-factory-naming \
  --inspect=summary src
```

Exit status: **0; zero blocking matches**. The initial 196 matches fell to **153 advisory matches** after repairs. All remaining matches are retained with dispositions below; this is not a claim that ast-grep prints no warnings or hints. No rules were disabled and no blanket suppressions were added.

Fixed production unwraps in PTY process setup and shell quoting; removed panic paths in model pricing, client reconfiguration and stream decoding; removed the unimplemented missing-tool adapter. Simplified optional vector initialization, stream collection, beta deduplication and tool result handling. Numeric validation retains the original parse diagnostic. The complex conditional is simplified. Library and executable crate roots deny `clippy::unwrap_used` and `clippy::expect_used` outside tests. Clippy with `-D warnings` passes, including the binaries.

## Retained matches

Each row identifies every matched line for that rule/file combination. Line numbers refer to `d12a7e4`. Boolean test cases remain literal to make their fixtures explicit. Protocol parsing retains string matching at its boundary. Intentional cancellation and optional values are not reported as failures.

| Rule | Source lines | Count | Disposition |
| --- | --- | ---: | --- |
| `continue-filter-loop` | [`src/agent.rs`](../../vendor/claudius/src/agent.rs): 2696 | 1 | Retain ordered mount shadowing: insert into the seen set before permission filtering, so a later inaccessible mount still hides an earlier mount. |
| `filter-in-loop` | [`src/prompt.rs`](../../vendor/claudius/src/prompt.rs): 842, 857, 894 | 3 | Retain assertion-building loops: each failed expectation produces a separate diagnostic in order. |
| `if-let-some-else` | [`src/agent.rs`](../../vendor/claudius/src/agent.rs): 2269, 2285, 2302, 2319, 2363 | 5 | Retain explicit branches: they contain side effects, early exits, asynchronous work, or variant destructuring; a map/unwrap chain would obscure control flow. |
| `if-let-some-else` | [`src/bin/claudius-chat.rs`](../../vendor/claudius/src/bin/claudius-chat.rs): 413, 440, 471 | 3 | Retain explicit branches: they contain side effects, early exits, asynchronous work, or variant destructuring; a map/unwrap chain would obscure control flow. |
| `if-let-some-else` | [`src/bin/claudius-prompt.rs`](../../vendor/claudius/src/bin/claudius-prompt.rs): 158 | 1 | Retain explicit branches: they contain side effects, early exits, asynchronous work, or variant destructuring; a map/unwrap chain would obscure control flow. |
| `if-let-some-else` | [`src/bin/claudius-render-transcript.rs`](../../vendor/claudius/src/bin/claudius-render-transcript.rs): 198 | 1 | Retain explicit branches: they contain side effects, early exits, asynchronous work, or variant destructuring; a map/unwrap chain would obscure control flow. |
| `if-let-some-else` | [`src/cache_control.rs`](../../vendor/claudius/src/cache_control.rs): 78 | 1 | Test assertion destructures a nested SystemPrompt variant; the else branch deliberately fails the test. |
| `if-let-some-else` | [`src/chat/session.rs`](../../vendor/claudius/src/chat/session.rs): 450 | 1 | Retain explicit branches: they contain side effects, early exits, asynchronous work, or variant destructuring; a map/unwrap chain would obscure control flow. |
| `if-let-some-else` | [`src/lib.rs`](../../vendor/claudius/src/lib.rs): 50 | 1 | Retain explicit branches: they contain side effects, early exits, asynchronous work, or variant destructuring; a map/unwrap chain would obscure control flow. |
| `if-let-some-else` | [`src/prompt.rs`](../../vendor/claudius/src/prompt.rs): 510, 820 | 2 | Retain explicit parent-directory fallback and expected-error assertion branching; neither silently discards an operational error. |
| `if-let-some-else` | [`src/pty.rs`](../../vendor/claudius/src/pty.rs): 1015, 1079 | 2 | Retain explicit branches: they contain side effects, early exits, asynchronous work, or variant destructuring; a map/unwrap chain would obscure control flow. |
| `if-let-some-else` | [`src/render.rs`](../../vendor/claudius/src/render.rs): 338 | 1 | Retain explicit branches: they contain side effects, early exits, asynchronous work, or variant destructuring; a map/unwrap chain would obscure control flow. |
| `let-underscore-call` | [`src/accumulating_stream.rs`](../../vendor/claudius/src/accumulating_stream.rs): 127, 188 | 2 | Receiver absence is an expected cancellation/opt-out condition, not a failed request. The result remains available to the active stream consumer; comments explain the send discard. |
| `let-underscore-call` | [`src/pty.rs`](../../vendor/claudius/src/pty.rs): 399 | 1 | Retain: broadcast send fails only when there are no live-output subscribers. Final command output is still returned normally; the inline comment documents this opt-out. |
| `match-option-verbose` | [`src/agent.rs`](../../vendor/claudius/src/agent.rs): 2028, 2074 | 2 | Retain explicit variant handling; these arms select behavior or destructure nested enum variants, rather than hiding fallible work in combinators. |
| `match-option-verbose` | [`src/bin/claudius-chat.rs`](../../vendor/claudius/src/bin/claudius-chat.rs): 448, 477 | 2 | Retain explicit variant handling; these arms select behavior or destructure nested enum variants, rather than hiding fallible work in combinators. |
| `match-option-verbose` | [`src/bin/claudius-render-transcript.rs`](../../vendor/claudius/src/bin/claudius-render-transcript.rs): 50 | 1 | Retain explicit variant handling; these arms select behavior or destructure nested enum variants, rather than hiding fallible work in combinators. |
| `match-option-verbose` | [`src/cache_control.rs`](../../vendor/claudius/src/cache_control.rs): 51 | 1 | Variant-aware predicate distinguishes block prompts and checks the last block; other variants intentionally return false. |
| `match-option-verbose` | [`src/chat/commands.rs`](../../vendor/claudius/src/chat/commands.rs): 130, 134, 189, 193, 233 | 5 | Retain explicit variant handling; these arms select behavior or destructure nested enum variants, rather than hiding fallible work in combinators. |
| `match-option-verbose` | [`src/chat/config.rs`](../../vendor/claudius/src/chat/config.rs): 164, 363, 504, 509 | 4 | Retain explicit configuration branches, including fallible effort parsing with ?, and extraction from distinct thinking variants. |
| `match-option-verbose` | [`src/types/thinking_config.rs`](../../vendor/claudius/src/types/thinking_config.rs): 214, 222 | 2 | Retain explicit variant handling; these arms select behavior or destructure nested enum variants, rather than hiding fallible work in combinators. |
| `match-result-verbose` | [`src/chat/commands.rs`](../../vendor/claudius/src/chat/commands.rs): 145, 153, 161, 234, 268 | 5 | Retain parser success/error arms so the accepted value and operator-facing rejection stay adjacent. |
| `match-result-verbose` | [`src/types/model.rs`](../../vendor/claudius/src/types/model.rs): 398 | 1 | Known-model parse success selects Known; unknown identifiers intentionally select Custom. Unknown model names are valid input. |
| `nested-match-pyramid` | [`src/agent.rs`](../../vendor/claudius/src/agent.rs): 3161 | 1 | Retain protocol dispatch followed by variant-specific handling; branches are named by the wire/command variants. |
| `nested-match-pyramid` | [`src/chat/commands.rs`](../../vendor/claudius/src/chat/commands.rs): 128 | 1 | Retain protocol dispatch followed by variant-specific handling; branches are named by the wire/command variants. |
| `opaque-bool-call` | [`src/agent.rs`](../../vendor/claudius/src/agent.rs): 186, 2419, 2882, 2933, 2998, 3177, 3361, 3376 | 8 | Boolean is a protocol field, named enable/disable setter, explicit test input, or state assignment caught through an enclosing call; it is not an ambiguous operation mode. |
| `opaque-bool-call` | [`src/bin/claudius-chat.rs`](../../vendor/claudius/src/bin/claudius-chat.rs): 176, 184 | 2 | Boolean is a protocol field, named enable/disable setter, explicit test input, or state assignment caught through an enclosing call; it is not an ambiguous operation mode. |
| `opaque-bool-call` | [`src/chat/commands.rs`](../../vendor/claudius/src/chat/commands.rs): 306, 307 | 2 | Boolean is a protocol field, named enable/disable setter, explicit test input, or state assignment caught through an enclosing call; it is not an ambiguous operation mode. |
| `opaque-bool-call` | [`src/chat/config.rs`](../../vendor/claudius/src/chat/config.rs): 711 | 1 | Boolean is a protocol field, named enable/disable setter, explicit test input, or state assignment caught through an enclosing call; it is not an ambiguous operation mode. |
| `opaque-bool-call` | [`src/client.rs`](../../vendor/claudius/src/client.rs): 179, 200, 204, 509, 1433 | 5 | Includes state assignments detected through enclosing closures. Other calls explicitly enable header sensitivity or construct a test status; booleans remain unambiguous. |
| `opaque-bool-call` | [`src/pty.rs`](../../vendor/claudius/src/pty.rs): 903, 913, 939, 945, 1814, 1818, 1820, 1836, 1838, 1852, 1855, 1867, 1870, 1884, 1887, 1891, 1918, 1934, 1961, 1977, 2000, 2005, 2020, 2034, 2050, 2064, 2073, 2089, 2092, 2125, 2142, 2143, 2145, 2159, 2161, 2175, 2191, 2209, 2227, 2251, 2268, 2274, 2313, 2336, 2355, 2375, 2402, 2422, 2444, 2578, 2583 | 51 | Live-output delivery explicitly distinguishes final versus partial output; the remaining calls are paired final/partial test fixtures. No error is ignored by these flags. |
| `opaque-bool-call` | [`src/render.rs`](../../vendor/claudius/src/render.rs): 498 | 1 | Boolean is a protocol field, named enable/disable setter, explicit test input, or state assignment caught through an enclosing call; it is not an ambiguous operation mode. |
| `opaque-bool-call` | [`src/sse.rs`](../../vendor/claudius/src/sse.rs): 187, 240, 251 | 3 | Includes state assignments detected through enclosing closures. Other calls explicitly enable header sensitivity or construct a test status; booleans remain unambiguous. |
| `opaque-bool-call` | [`src/types/message_create_params.rs`](../../vendor/claudius/src/types/message_create_params.rs): 831, 913, 934, 937, 992 | 5 | Boolean is a protocol field, named enable/disable setter, explicit test input, or state assignment caught through an enclosing call; it is not an ambiguous operation mode. |
| `opaque-bool-call` | [`src/types/message_create_template.rs`](../../vendor/claudius/src/types/message_create_template.rs): 375, 400, 427, 430, 458 | 5 | Boolean is a protocol field, named enable/disable setter, explicit test input, or state assignment caught through an enclosing call; it is not an ambiguous operation mode. |
| `opaque-bool-call` | [`src/types/output_config.rs`](../../vendor/claudius/src/types/output_config.rs): 133, 165 | 2 | Boolean is a protocol field, named enable/disable setter, explicit test input, or state assignment caught through an enclosing call; it is not an ambiguous operation mode. |
| `opaque-bool-call` | [`src/types/output_format.rs`](../../vendor/claudius/src/types/output_format.rs): 86 | 1 | Boolean is a protocol field, named enable/disable setter, explicit test input, or state assignment caught through an enclosing call; it is not an ambiguous operation mode. |
| `opaque-bool-call` | [`src/types/tool_choice.rs`](../../vendor/claudius/src/types/tool_choice.rs): 165 | 1 | Boolean is a protocol field, named enable/disable setter, explicit test input, or state assignment caught through an enclosing call; it is not an ambiguous operation mode. |
| `opaque-bool-call` | [`src/types/tool_param.rs`](../../vendor/claudius/src/types/tool_param.rs): 170, 211 | 2 | Boolean is a protocol field, named enable/disable setter, explicit test input, or state assignment caught through an enclosing call; it is not an ambiguous operation mode. |
| `opaque-bool-call` | [`src/types/tool_result_block.rs`](../../vendor/claudius/src/types/tool_result_block.rs): 167 | 1 | Boolean is a protocol field, named enable/disable setter, explicit test input, or state assignment caught through an enclosing call; it is not an ambiguous operation mode. |
| `silent-unwrap-or-else` | [`src/bin/claudius-chat.rs`](../../vendor/claudius/src/bin/claudius-chat.rs): 212 | 1 | Intentional known/custom model parser fallback. Unknown names are valid custom model identifiers, not operational failures. |
| `stringly-typed-dispatch` | [`src/agent.rs`](../../vendor/claudius/src/agent.rs): 3203 | 1 | False positive: contains() tests a HashSet of numeric content-block indices, not string-based type dispatch. |
| `stringly-typed-match` | [`src/agent.rs`](../../vendor/claudius/src/agent.rs): 2183 | 1 | Retain at the string-to-enum protocol/parser boundary. Matching wire discriminators or command names is the conversion itself. |
| `stringly-typed-match` | [`src/types/content_block.rs`](../../vendor/claudius/src/types/content_block.rs): 69 | 1 | Retain at the string-to-enum protocol/parser boundary. Matching wire discriminators or command names is the conversion itself. |
| `stringly-typed-match` | [`src/types/content_block_delta.rs`](../../vendor/claudius/src/types/content_block_delta.rs): 49 | 1 | Retain at the string-to-enum protocol/parser boundary. Matching wire discriminators or command names is the conversion itself. |
| `stringly-typed-match` | [`src/types/model.rs`](../../vendor/claudius/src/types/model.rs): 315 | 1 | Retain at the string-to-enum protocol/parser boundary. Matching wire discriminators or command names is the conversion itself. |
| `stringly-typed-match` | [`src/types/thinking_config.rs`](../../vendor/claudius/src/types/thinking_config.rs): 212 | 1 | Retain at the string-to-enum protocol/parser boundary. Matching wire discriminators or command names is the conversion itself. |
| `unwrap-or-default` | [`src/bin/claudius-chat.rs`](../../vendor/claudius/src/bin/claudius-chat.rs): 536 | 1 | Absent optional protocol/display data has a defined empty value; no Result or failure is being discarded. |
| `unwrap-or-default` | [`src/chat/session.rs`](../../vendor/claudius/src/chat/session.rs): 398 | 1 | Absent optional protocol/display data has a defined empty value; no Result or failure is being discarded. |
| `unwrap-or-default` | [`src/client.rs`](../../vendor/claudius/src/client.rs): 602 | 1 | Absent Retry-After contributes zero additional delay; the exponential backoff floor still applies. |
| `unwrap-or-default` | [`src/types/thinking_block.rs`](../../vendor/claudius/src/types/thinking_block.rs): 22, 27 | 2 | Absent optional protocol/display data has a defined empty value; no Result or failure is being discarded. |
| `unwrap-or-default` | [`src/types/thinking_delta.rs`](../../vendor/claudius/src/types/thinking_delta.rs): 18 | 1 | Absent optional protocol/display data has a defined empty value; no Result or failure is being discarded. |

## Ruleset identity

SHA-256 hashes allow a later scan to detect changes in the installed rules.

| File | SHA-256 |
| --- | --- |
| `sgconfig.yml` | `329f05cc3da607f4a42fc716fe10c70a97a0f45836356b7c56943f9071175085` |
