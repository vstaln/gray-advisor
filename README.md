<h1 align="center">gray-advisor</h1>
<p align="center">Second-opinion advisor for gray — port of pi-advisor-flow.</p>

`/advise <question>` ships the question plus a digest of the current
session's tail to a *second*, separately configured advisor model, then
returns `{prompt}` so the executor continues with both the question and
the advisor's take.

## Commands

- `/advise <question>` — ask the advisor; its take and the question are
  handed back to the agent as a prompt.
- `/advise model <provider/model-id>` — set the advisor model (writes
  `~/.gray/advisor/config.json`).
- `/advise status` — show current config and usage.

## Configuration

`~/.gray/advisor/config.json`:

```json
{"model": "openrouter/openai/gpt-5", "bin": "gray"}
```

Env overrides: `GRAY_ADVISOR_MODEL`, `GRAY_ADVISOR_BIN`. Honors
`$GRAY_HOME`.

## Advisor call choice

A `gray -p --bare --model <advisor>` **subprocess**, not the wire's
`host/run`: `host/run` has no model-selection parameter (it always runs
the session's own model, which defeats the "second model" point) and caps
at ~28s. The subprocess needs no `host.turn` capability grant and keeps
the plugin self-contained. `--bare` skips plugins/AGENTS.md so advisor
runs can't recurse or leak context; the child is killed at 24s to stay
under the host's `command/run` TTL.

## Install

```sh
cargo build --release
gray plugin install ~/grayplugins/gray-advisor/target/release/gray-advisor
```

## Limits

No advisor model configured → `/advise` says so and returns text (no
prompt injected). Session digest covers the last 8 transcript entries,
160 chars each — enough for situational awareness, not full context.
