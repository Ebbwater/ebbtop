# ebbtop agent on Linux (x86-64, prebuilt)

`dist/ebbtop-agent-linux-x86_64.tar.gz` installs the ebbtop agent on a
monitored Linux host **without Git, Cargo or Rust**: extract the archive and run
its own installer. The source-build path in `deploy/install-agent.sh` is
unchanged and still works from a checkout.

## Install from the archive (no toolchain)

```sh
mkdir ebbtop-agent && tar -xzf ebbtop-agent-linux-x86_64.tar.gz -C ebbtop-agent
cd ebbtop-agent
sha256sum -c SHA256SUMS            # optional: the installer does this again
EBBTOP_ADDR=<this host's tailnet/LAN IP> bash ./install-agent.sh --prebuilt
```

The archive is extracted into a directory of its own: the installer reads
`ebbtop-agent`, `ebbtop-agent.service`, `BUILD` and `SHA256SUMS` from *its own
directory* and refuses to run if that directory holds anything else (the
member set is exactly those five files). It needs only `bash`, `install`,
`mkdir`, `sha256sum`, `uname`, `systemctl --user`, `ss` and the rest of
standard userland — no `git`, no `cargo`, no `rustc`, no `.git`, no
`Cargo.toml`, no checkout above it.

`EBBTOP_ADDR` is required on a fresh install only: it is the address the agent
binds and is written once into `~/services/ebbtop-agent/agent.env`. It must be
an IP literal on this host's tailnet or LAN address. `0.0.0.0`, `::` (listen
beyond the configured address) and loopback-only addresses such as `127.0.0.1`
or `::1` (the client cannot reach them) are refused. The token is generated
locally as 32 bytes of hex into `~/services/ebbtop-agent/ebbtop.token` with
mode `600` and is never printed.

What the installer prints at the end: the packaged version, `systemctl --user
is-active`, the address actually listening on port `45976`, the sha256 prefix of
the installed binary and the linger state. If linger is off it prints the exact
command needed: `sudo loginctl enable-linger <user>`.

## Upgrade, token retention and rollback

Re-running the installer from a newer archive is the upgrade path. An existing
`agent.env` is never rewritten: the bind address, the `EBBTOP_TOKEN` value and
any setting appended after first install stay byte-identical, and
`ebbtop.token` keeps its exact bytes and mode `600`. The previous binary is
kept as `~/services/ebbtop-agent/bin/ebbtop-agent.bak`.

If the new binary does not come up (`systemctl --user is-active` is not
`active` after the restart), the installer restores `ebbtop-agent.bak`,
restarts the unit and verifies that the restored binary has the previous
sha256 and is the binary the unit starts. It exits non-zero.

`bash ./install-agent.sh --rollback` performs the manual **rollback**: it moves
`.bak` back over the binary and restarts the unit. With no `.bak` present it
exits non-zero with `no .bak to roll back to` instead of pretending to succeed.

## What is verified before anything changes

The installer verifies the package first and mutates nothing if that fails:

1. `uname -m` must be `x86_64`.
2. The extracted member set must be exactly `BUILD`, `SHA256SUMS`,
   `ebbtop-agent`, `ebbtop-agent.service`, `install-agent.sh`, all regular
   files.
3. `SHA256SUMS` must name exactly `BUILD`, `ebbtop-agent` and
   `ebbtop-agent.service` by bare name and every **sha256** must match.
4. The unit must keep the documented contract (`EnvironmentFile` at
   `%h/services/ebbtop-agent/agent.env`, `ExecStart` at the installed binary).
5. The bind address must be a configurable non-wildcard, non-loopback IP.

Only then does it move the current binary aside, write the unit and call
`systemctl --user`. A truncated archive, a non-gzip file, a modified member, an
extra or missing member and a non-x86_64 host are each refused with a stated
reason, before any service-manager call or any write under
`~/services/ebbtop-agent`.

## ABI floor

The packaged binary is a dynamically linked x86-64 Linux ELF. Its highest
`GLIBC_*` symbol version, measured with

```sh
objdump -T dist-unpacked/ebbtop-agent | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1
```

is recorded in the archive's `BUILD` as `glibc_floor=`. For the current build
that is **GLIBC_2.39**, i.e. the host needs glibc 2.39 or newer (Ubuntu 24.04+,
Debian trixie+, Fedora 40+). Nothing lower is advertised: the binary was built
on a glibc 2.43 host, and a smoke run in `debian:bullseye-slim` (glibc 2.31)
fails at load time with a `GLIBC_2.39 not found` error. Lowering the floor needs
a build on an older glibc, which is a build-pipeline decision, not an installer
one.

## Integrity is not authenticity — known gap

`SHA256SUMS` travels **inside** the archive it describes. It proves that the
bytes were not corrupted or edited after packaging; it does **not** prove who
produced them. A tampered archive can ship a matching, self-consistent
`SHA256SUMS`. The checks above make a modified member or a repacked archive fail
closed, but only an independently trusted publisher identity (a signature or a
manifest fetched over an authenticated channel) turns integrity into
**authenticity**. How users obtain and verify the manifest is a publication gate
that is **not** solved here; until it is, treat the archive as unauthenticated.

## Building and testing the package

```sh
cargo build --release --locked -p ebbtop-agent --no-default-features --features gpu
bash deploy/linux/package-agent.sh
bash deploy/linux/test-prebuilt-install.sh
```

`package-agent.sh` refuses a dirty tree, a non-ELF binary, a wrong ELF machine
or a non-x86-64 target, writes the five members with a fixed order and mtime
(the tarball is reproducible), verifies the member set it just wrote and prints
the member list, the size and the tarball **sha256**.

`test-prebuilt-install.sh` runs every case inside a disposable fixture `HOME`
with stub `systemctl`, `ss`, `loginctl` and failing `git`/`cargo`/`rustc` stubs
on `PATH`, so the real `~/services/ebbtop-agent` and the live unit are never
touched. Cases: no-toolchain install, invalid archive, tampered member, member
set, wrong architecture, token retention, settings retention, rollback (with and
without `.bak`), and the packaging refusals. `--no-toolchain-only` runs just the
first. Each negative case diffs the whole fixture home and the service-manager
call log before and after, which is how "refused before any mutation" is proved.

What this harness cannot prove, and what still needs a native host or VM:

- install on a clean supported Linux host with no Git and no Cargo installed;
- `systemctl --user is-active` on a real unit (the harness uses a stub);
- a listener on the intended private address reached from a separate client;
- loopback/wildcard refusal against a real service manager;
- reboot with linger enabled and the unit coming back by itself;
- an upgrade on a VM with the token retained;
- rollback after a real failed service start;
- download and verify of the archive as a published release asset.
