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
