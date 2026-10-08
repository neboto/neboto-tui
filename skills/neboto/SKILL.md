---
name: neboto
description: Read-only AWS lookups with the neboto CLI — list resources in any of 70+ services and fetch a resource's full detail (config, permissions, events, templates, drift) as JSON or Markdown. Use when answering questions about what exists in an AWS account or how something is configured. It cannot change anything, so prefer it over the aws CLI for looking.
---

# neboto: read-only AWS lookups

`neboto` is a terminal AWS browser whose subcommands print instead of opening
the UI. Every AWS call it can make is read-only (checked in its CI), so
nothing you run through it can create, change or delete a resource.

## The loop

1. **Find the service prefix** (once): `neboto services`
2. **Find the resource**: `neboto ls @<svc>` with filters, to get its id
3. **Read its detail**: `neboto get @<svc> <id>`, ideally with `--section`

```sh
neboto services -o json
neboto ls @ec2 -t instance --state running -o json
neboto ls @iam -t role -f 'deploy' -o json
neboto ls @ecs -f 'tag:team=payments' -o json
neboto get @iam deploy-role --section permissions --section 'trust policy' -o json
neboto get @lambda orders-api -o md
neboto get @ec2 i-0abc123 sg-0def456 -o json
```

## `ls` — list and filter

| Flag | Meaning |
|---|---|
| `-t TYPE` | Resource type, or its last word(s), any case: `instance`, `"security group"`, `role\|policy` |
| `-f QUERY` | Fuzzy text plus exact `tag:key=value` terms: `-f 'web tag:env=prod'` |
| `--state STATE` | Only this state (`running`, `available`, `stopped`, …) |
| `--hide-noise` | Drop AWS-managed defaults and other noise |
| `--limit N` | At most N rows |

A service lists several types (EC2: instances, security groups, volumes, …;
ECR: repositories and their images); use `-t` to narrow, e.g.
`neboto ls @ecr -t repository`. An empty `-t` result prints the types present
to stderr.

## `get` — full detail

`get @<svc> <ID|NAME>...` returns every section the resource's detail pane
has, including sections that need extra API calls (IAM policies, CloudFormation
events / template / drift, ECS deployments, …). Up to 50 ids per call.

- Matches the exact id (or ARN, when that is the id), then the exact name.
  No partial matches: run `ls` first if unsure.
- `--section NAME` (repeatable, any case) returns only those sections **and
  skips the other sections' API calls**. Prefer it: smaller output, faster.
  An unknown name fails and lists the real section names, so a first
  `get` without `--section` (or a wrong guess) tells you what exists.
- For **what an IAM role / user / group can do**, ask for `--section policies`:
  every attached and inline policy document (up to 20), where
  `permissions` only lists their names.
- `-t TYPE` picks one when a name matches several resources (an ECS service
  and its task definition share a name).
- `--wait SECS` (default 60): sections still loading after that come back
  as `null`, with a note on stderr.

## Output

Always pass `-o json` (or `-o md` for text to reason over); the default
switches between a table and JSON depending on whether stdout is a
terminal.

- `ls`: `{"schema": "neboto/v1", "service", "region", "count", "resources": [{"type", "id", "name", "state", …, "tags"}]}`
  — each row also carries its type's list-level fields (`"Runtime"`,
  `"Memory"`, …); `get` has the rest.
- `get`, one id: `{"schema", "service", "region", "resource": {"type", "id", "name", "state"}, "sections": {…}, "tags": {…}}`
- `get`, several ids: `{"schema", "service", "region", "count", "resources": [ <the one-id shape without schema/service/region> ]}`

Section values mirror the UI: human-readable labels and formatted values
(`"Memory": "512 MB"`), not raw API fields. Sub-groups nest as objects,
repeated keys get ` (2)`, ` (3)` suffixes, and verbatim text (templates,
policies, rule tables) is a `"content"` array of lines. A section that is
`null` didn't load.

**What's stable.** The envelopes above, the `type`/`id`/`name`/`state`/`tags`
keys, section names, type names, service prefixes, flags and exit codes only
change with a new schema version. What's *inside* a section follows the TUI
and can change in any release. Reading it to answer a question is exactly
what it's for; but if you're writing a script someone will keep, don't
hard-code paths into section bodies — select on the stable keys, or use the
`aws` CLI for the one field the script needs.

stdout carries only the result; warnings (e.g. a partial load) go to stderr.

## Exit codes

- `0` — success, including an empty list.
- `1` — AWS error (permissions, throttling, network). The message is AWS's.
- `2` — the request is wrong: unknown service / region / section, or a
  resource that isn't there or is ambiguous. **The message says what to
  pass instead** (the matching ids and types, or the real section names):
  read it and retry.

## Account, region, credentials

- `-p PROFILE` and `-r REGION` pick the account and region; pass them
  explicitly rather than relying on the environment. A wrong profile or
  region **fails** rather than falling back, so an answer is never from a
  different account than you asked for.
- Global services (IAM, Organizations, CloudFront, Route 53 hosted zones, …)
  report `"region": "global"`.
- `--demo` uses a built-in fake account (`acme-prod`) with no credentials.
  Use it to try the commands out; its data is not real.

## Rules

- **Secret values are never available.** Secrets Manager and SSM Parameter
  Store return metadata only. Don't try to fetch values another way. If
  the user needs one, tell them to use the console or their own tooling.
- **Cost of a call:** `ls` and `get` list the whole service first, then pick
  the resource. On a large account, list a service once and batch ids into
  one `get`, rather than one `get` per resource.
- The output is a snapshot of the read calls; nothing is cached between
  runs.
