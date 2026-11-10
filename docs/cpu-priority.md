# CPU priority (#643, #645)

Bosn's builds should never make the desktop stutter. Two layers keep them below it.

## 1. Per-container weight (always on)

Every container Bosn creates gets `--cpu-shares=256`
(`bosn_engine::LOW_CPU_PRIORITY_ARG`; cgroup v2 `cpu.weight` about 10 instead of
100). CPU caps (`--cpus`) still bound how much a container may use; the weight
decides who wins when CPUs are contended. An idle machine still gives Bosn every
CPU its caps allow.

Weight only ranks siblings. Docker places containers under `system.slice`, which
competes with the desktop's `user.slice` at equal weight, so on its own this ranks
Bosn below other containers and services but not below the desktop.

At daemon start Bosn re-weights the running containers it owns (this registry's,
plus setup containers) with `docker update`, so containers created before #643
need no recreation. The daemon log reports the count and the placement in use:

```
bosn priority: 3 running container(s) at --cpu-shares=256; placement bosn.slice
```

## 2. The `bosn.slice` (opt-in, needs root once)

A top-level slice beside `user.slice` with a low `CPUWeight` moves Bosn's whole
share below the desktop. Bosn uses it automatically when all of these hold,
probed once per process:

- Docker's cgroup driver is `systemd` (`docker info --format '{{.CgroupDriver}}'`),
- `DOCKER_HOST` is unset or the local socket (a remote or rootless engine never
  joins a host slice),
- `systemctl show bosn.slice -p CPUWeight --value` is a number below 100.

Otherwise placement is `weight-only` and layer 1 applies alone. Containers join
the slice when they are created (`--cgroup-parent=bosn.slice`, added where the
command runs, so it never changes a setup container's name); a container that
already exists moves on its next recreation.

### Provision it

Plain systemd, `/etc/systemd/system/bosn.slice`:

```ini
[Unit]
Description=Bosn containers (low priority)
Before=slices.target

[Slice]
CPUWeight=10
IOWeight=10
```

then `sudo systemctl daemon-reload`.

NixOS:

```nix
systemd.slices.bosn = {
  description = "Bosn containers (low priority)";
  sliceConfig = { CPUWeight = 10; IOWeight = 10; };
};
```

Restart the Bosn daemon afterwards; the probe runs once per process.

### Check it

```bash
systemctl show bosn.slice -p CPUWeight          # CPUWeight=10
docker inspect -f '{{.HostConfig.CgroupParent}} {{.HostConfig.CpuShares}}' <container>
# bosn.slice 256
```

### Read the reported limits

`bosn ci doctor` and `bosn jobs` show placement and a fresh read of the
operator-provisioned slice's cgroup v2 `cpu.max` and `cpu.weight`, beside the
lane and engine CPU information. Their JSON reports include optional
`slice_cpu` evidence (under `ci` for jobs).

For example, `cpu.max: 200000 100000 (2 CPUs)` means the slice may consume
200000 microseconds of CPU time per 100000-microsecond period, or two CPUs
across its children. `max 100000 (unlimited)` means no quota at this slice;
ancestor and container limits can still apply. `cpu.weight: 10` is a relative
share under contention, not a CPU cap. JSON represents unlimited as a null
`cpu_max.quota`, with the period preserved.

`weight-only` means the slice is not selected; both limits are unavailable.
Missing, unreadable, oversized or malformed kernel values are also unavailable
(JSON null), independently for each value. These are observations only: Bosn
does not provision the slice or change its limits. The engine retains its
8-CPU default ceiling, overridden by `[engine] cpus`.
