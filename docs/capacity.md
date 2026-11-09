# `bosn capacity`

`bosn capacity` observes Bosn queues, aggregate host pressure, and running
containers outside Bosn. It is advisory and can be stale before work starts.
An empty Bosn registry does not mean an idle host.

```sh
bosn capacity
bosn capacity --json
bosn capacity --no-docker --timeout-ms 1000
bosn capacity --state-dir /path/to/state --json
```

The JSON document has `schema: 1`, sampling time and age in milliseconds,
independent `bosn`, `host`, and `docker` coverage (`full`, `partial`, or `none`),
and an `unavailable` list with typed reasons. Unknown numeric observations are
`null`; failed Docker reads never become zero containers. Exit status is 0 for
an observation, including a partial one, 2 for invalid arguments, and 1 for an
internal failure. The default timeout is 5000 ms. Docker reads are capped at
5 seconds and 4 MiB, even with a larger requested timeout.

The command never starts a daemon. It contacts an existing daemon for at most
2 seconds and uses the raw jobs and CI list operations, without per-job Docker
enrichment or an HTTP cookie grant. An absent daemon leaves Bosn counts unknown;
an unresponsive daemon is reported as `unreachable`.

CPU utilization and IO delay come from two aggregate `/proc/stat` readings
250 ms apart. Linux memory, swap, load and PSI avg10 observations come from
`/proc`. Unsupported platforms and unreadable files are reported explicitly.
No existing cgroup CPU quota reader is available, so `host.cpu_quota` is `null`
with reason `unsupported`. This makes host coverage partial even on an idle host.
Disk space is measured at the state directory: discovering a Docker data root
would require `docker info`, which this command deliberately does not run.

Bosn's read-only CI list supplies admission counts and limits but no engine
CPU/memory limits. The existing runners-list operation saves settings and can
restart its dispatcher, so capacity does not call it. Engine fields stay `null`
with `bosn.engine: unsupported`; no IPC operation or protocol change is added.

The only Docker command is one bounded `docker ps -a`. Running containers with
no Bosn label count under `external`, even if their controller has exited.
Counts are aggregated by the `com.docker.compose.project` label, falling back to
`com.docker.compose.service`, then `other`. At most 20 entries are emitted;
overflow is summed into `other`. These labels are observations, not ownership
claims. Container names, image names, process names and command lines are never
included. The command never kills, cancels, prunes, repairs or adopts work.
