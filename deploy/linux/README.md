# ebbtop agent on Linux (x86-64, prebuilt)

`dist/ebbtop-agent-linux-x86_64.tar.gz` installs the ebbtop agent on a
monitored Linux host **without Git, Cargo or Rust**: extract the archive and run
its own installer. The source-build path in `deploy/install-agent.sh` is
unchanged and still works from a checkout.

## Install with one command

`deploy/linux/ebbtop-install.sh` wraps this whole page:

```sh
curl -fsSL https://github.com/Ebbwater/ebbtop/releases/latest/download/ebbtop-install.sh | bash
```

It resolves the release (`--version <tag>` pins one, `--archive <file>` uses a
local tarball with no network), refuses the download unless its sha256 equals
the digest the release reports for that asset, extracts it under `TMPDIR` and
re-checks the inner manifest with `sha256sum --strict -c SHA256SUMS`, preflights
x86-64, glibc 2.39 and a systemd user manager, chooses the bind address (an
existing `agent.env` wins, then `EBBTOP_ADDR`, then a prompt defaulting to this
host's tailnet/LAN address) and then runs the packaged
`install-agent.sh --prebuilt` from the extracted directory. It installs nothing
itself, so this one-liner and the archive path below cannot drift apart. It ends
with the pairing facts: `ws://<bind>:<port>/ws` and the token file path, never
the token value.

Flags: `--version <tag>`, `--archive <path>`, `--yes`/`-y`, `--dry-run`/`-n`,
`--help`/`-h`. Presentation follows `deploy/wizard/common.sh` (state, plan, one
confirmation, act, verify, report) but is inline: this file has to run
standalone from a pipe, where there is no checkout to source it from. Every
prompt reads `/dev/tty`, because a piped install has the script itself on stdin.

The script becomes a release asset at the **next** release cut. This slice lands
the script and the documentation only, so on the current release that URL is a
404 and the archive steps below are the way in.

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

When the installed binary supports pairing, one more line follows:

```
pair: mint a pairing code on this host with: /home/<user>/services/ebbtop-agent/bin/ebbtop-agent --pair
```

The capability is decided from the bytes that were just installed — the `--pair`
argument the binary carries — never by running the agent: a binary from before
pairing has no `--help` handler either, so probing it would start a listener. A
binary without `--pair` prints nothing extra, so the output of an older package
is unchanged. The installer never mints and never prints a pairing code or a
token; `--pair` prints the code on this host's own terminal.

## Upgrade, token retention and rollback

Re-running the installer from a newer archive is the upgrade path. An existing
`agent.env` is never rewritten: the bind address, the `EBBTOP_TOKEN` value and
any setting appended after first install stay byte-identical, and
`ebbtop.token` keeps its exact bytes and mode `600`. The previous binary is
kept as `~/services/ebbtop-agent/bin/ebbtop-agent.bak`.

### The `agent.env` syntax the installer accepts

An existing `agent.env` is read to learn the address the unit will bind, and
only a subset of systemd's `EnvironmentFile` grammar is supported. The check
runs **after** the package is verified and **before** any write or any
`systemctl`/`ss` call, so a file the installer cannot read exactly as systemd
reads it is refused while nothing has changed. The accepted file is:

- free of NUL bytes — systemd stops reading the file at the first NUL, so an
  `EBBTOP_BIND` after it would be ignored and the agent would fall back to
  `0.0.0.0`;
- free of every byte outside printable ASCII, TAB, CR and LF; any other byte is
  refused naming it (`the byte 0x0b is not supported`), because systemd treats
  VT, FF, other control bytes and non-ASCII bytes such as NBSP as part of a key
  or value rather than as whitespace;
- line-ended by LF, CRLF or a bare CR, since CR ends a line exactly as systemd
  treats it;
- indented only with space and TAB, and only at the start or end of a line;
- carrying exactly one plain, unquoted bind assignment — `EBBTOP_BIND=<IP
  literal>`, or the legacy `SOCKTOP_BIND=<IP literal>` on a host installed
  before the rename — with no quotes, no backslash escapes, no `export`, and no
  second assignment of either key in any spelling (duplicates are refused
  instead of resolved, because systemd applies the last one and the address
  would then depend on line order);
- plain `KEY=VALUE` on every other line, with keys matching
  `[A-Za-z_][A-Za-z0-9_]*`, or `#`/`;` comment lines.

Every unsupported or ambiguous form exits non-zero with exactly one error line.

#### Hosts installed before the `EBBTOP_*` rename (`SOCKTOP_BIND`)

An `agent.env` written by an older install carries `SOCKTOP_BIND` and
`SOCKTOP_TOKEN` and no `EBBTOP_BIND` at all. The agent reads `EBBTOP_BIND` first
and falls back to `SOCKTOP_BIND`, so the legacy line is the address it actually
binds. The prebuilt upgrade therefore accepts the legacy key as the bind source,
read under exactly the same byte grammar and the same IP-literal rules — no
wildcard, no loopback-only address — as `EBBTOP_BIND`.

Nothing is migrated. The file is not rewritten, no key is renamed, `ebbtop.token`
keeps its bytes and mode 600, and the token line keeps its own spelling; the
installer only reads the address and prints a `note:` line saying which key
supplied it.

An `agent.env` that carries **both** keys is refused as ambiguous, in either
order and in every spelling: the agent would take `EBBTOP_BIND` and ignore the
legacy line, so the two could disagree without anyone noticing. Remove one line
and re-run. Two assignments of the same bind key are refused for the same reason
they are for `EBBTOP_BIND`: systemd applies the last one, so the address would
depend on line order.

#### Why the old "leading whitespace is fine" promise was false

An earlier revision of this file promised that leading whitespace before the key
was ignored. That was true only for the space and TAB that systemd's parser
skips; it was false for VT (`0x0b`), FF (`0x0c`) and NBSP, which the installer
silently stripped but systemd keeps as part of the key. A file such as
`<VT>EBBTOP_BIND=100.64.0.5` therefore validated as the private address, was
installed, and reported success while the unit loaded no `EBBTOP_BIND` at all —
the agent then listened on `0.0.0.0`. The installer now decides from the raw
bytes, refuses those bytes by name, and cannot validate an address the unit will
not load.

If the new binary does not come up (`systemctl --user is-active` is not
`active` after the restart), the installer restores `ebbtop-agent.bak`,
restarts the unit and verifies that the restored binary has the previous
sha256 and is the binary the unit starts. It exits non-zero.

`bash ./install-agent.sh --rollback` performs the manual **rollback**: it moves
`.bak` back over the binary and restarts the unit. With no `.bak` present it
exits non-zero with `no .bak to roll back to` instead of pretending to succeed.

## Uninstall and `--purge`

```sh
bash ./install-agent.sh --uninstall            # stop, disable, remove unit and binary
bash ./install-agent.sh --uninstall --purge    # also delete the token and the pairing state
```

`--uninstall` stops and disables the unit, removes the unit file, the installed
binary and its `.bak`, and leaves `~/services/ebbtop-agent` in place:
`agent.env`, `ebbtop.token` and the pairing state are kept byte-identical, so the
next install reuses them and clients keep working.

`--purge` deletes that directory as well — token, environment file and pairing
state — after which enrolled clients lose access. When `EBBTOP_STATE_DIR` points
somewhere else (the agent's `--pair` and `/pair` honour it), `--purge` deletes
only `pairing.json` and `clients.json` there and touches no other file.

Both modes count what they act on and never print a token, a pairing code or any
file content:

```
tokens: 1 file(s) kept in /home/<user>/services/ebbtop-agent (--purge deletes them)
pairing: 2 state file(s) kept (--purge deletes them)
```

Neither mode prompts. `--uninstall` is the explicit request, and the `plan:` line
printed before anything is removed says exactly what will be deleted.

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
5. The bind address must be a configurable non-wildcard, non-loopback IP. An
   existing `agent.env`, when there is one, must also be a file the installer
   can read exactly as systemd reads it (see *The `agent.env` syntax the
   installer accepts*), carrying exactly one bind source — `EBBTOP_BIND`, or the
   legacy `SOCKTOP_BIND` on a host installed before the rename, never both.

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
touched. Cases: no-toolchain install, bare `agent.env`, invalid archive,
tampered member, member set, wrong architecture, token retention, settings
retention, rollback (with and without `.bak`), source-install parity, the
`agent.env` bind-assignment grammar, the packaging refusals, the legacy
`SOCKTOP_BIND` upgrade and its refusals, the pairing hint, and the
uninstall/`--purge` state handling.
`--no-toolchain-only` runs just the first. Each negative case diffs the whole
fixture home and the service-manager and `ss` call logs before and after, which
is how "refused before any mutation" is proved; each bind-assignment refusal
also asserts exactly one installer error line.

The bind-assignment case pins the byte grammar above: an ordinary upgrade keeps
`agent.env` and the token byte-identical, the plain/space-indented/tab-indented/
`KEY = value`/trailing-space/CRLF/comment forms (and a plain file with no final
newline) are accepted and read as systemd reads them, and every refused form —
duplicates in any spelling, quoted or escaped values, `export`, a non-comment
line without `=`, a file with no bind, VT/FF-indented or -suffixed keys, NUL
bytes and non-ASCII bytes — exits non-zero with one error line, an unchanged
fixture and zero `systemctl`/`ss` calls.

`test-install-latest.sh` is the harness for the one-command installer. It builds
its fixture archives from this checkout's own `deploy/install-agent.sh` and
`deploy/ebbtop-agent.service` (and checks that the packaged members are byte-
identical to them), serves a GitHub-shaped release document from a stub `curl`,
and runs every case in a fixture `HOME`, fixture `XDG_RUNTIME_DIR` and stub
`systemctl`/`ss`/`loginctl`/`curl`/`tailscale`/`ip`/`getconf`/`uname` path:

```sh
bash deploy/linux/test-install-latest.sh
```

Cases: latest release end to end; digest mismatch with zero writes and zero
service/listener calls; wrong architecture, old glibc and no systemd user
manager refused before any network request; `--dry-run` with no download and no
write; `--version` pinning the tag endpoint (never `latest`) and a pinned tag
answered by another release; offline `--archive` with zero `curl` calls; a
tampered member and a traversing member; a release with no Linux asset, an
asset with no digest, an unreadable release API and a missing `--archive` file
(all fail closed); a `BUILD` glibc floor above the host;
`EBBTOP_PORT` proven not to reach the packaged installer; `127.0.0.1` proven to
reach the packaged installer's own validation; an upgrade that keeps `agent.env`
and the token byte-identical; the tailnet-to-LAN bind fallback; no terminal
without `--yes`; the real piped `bash -s` shape; and argument handling. The
release document deliberately puts the Linux asset second, keeps a
`.tar.gz.sha256` look-alike first and does not order keys alphabetically, so a
parser that takes the first digest or prefix-matches the asset name fails the
harness instead of passing it.

The legacy-bind case does the same for a pre-rename host: an `agent.env` whose
only bind assignment is `SOCKTOP_BIND` upgrades with `agent.env` and the token
byte-identical and no `EBBTOP_BIND` line added, the accepted forms and the
refused ones (including a file carrying both keys in either order) mirror the
`EBBTOP_BIND` twins. The `ss` stub reads the bind with the same grammar and with
the agent's `EBBTOP_BIND`-before-`SOCKTOP_BIND` precedence, so the reported
listener cannot disagree with the validated address; its fallback is a
deliberately wrong address, so a fixture the stub cannot parse fails instead of
passing by accident.

The pairing case installs a package whose binary carries the `--pair` argument
and asserts exactly one hint line naming the installed binary, no
code-shaped value and no token on the output; the unmarked package must produce
no hint line at all. The uninstall case asserts that `--uninstall` removes the
unit file and the binary while keeping `ebbtop.token`, `pairing.json` and
`clients.json` byte-identical, that `--uninstall --purge` deletes them (counting
1 token and 2 pairing files) and removes the service directory, and that with an
overridden `EBBTOP_STATE_DIR` only those two pairing files are deleted from that
directory. Every mode is checked for printed content: a sentinel code and a
sentinel client token must not appear anywhere in the output.

What these harnesses cannot prove, and what still needs a native host or VM:

- install on a clean supported Linux host with no Git and no Cargo installed;
- `systemctl --user is-active` on a real unit (the harness uses a stub);
- a listener on the intended private address reached from a separate client;
- loopback/wildcard refusal against a real service manager;
- uninstall and `--uninstall --purge` against a real service manager, a real
  state directory and a real pairing-capable agent binary (the fixture marks
  the packaged binary instead);
- reboot with linger enabled and the unit coming back by itself;
- an upgrade on a VM with the token retained;
- rollback after a real failed service start;
- download and verify of the archive as a published release asset;
- the one-liner's live API and download paths against the real release (the stub
  serves them), and the systemd user manager probe against a live manager socket
  (the fixture creates a file at the path a live manager publishes a socket at);
- the one-liner against a host whose `agent.env` predates `EBBTOP_BIND` (the
  legacy `SOCKTOP_BIND` layout), which the packaged installer refuses by design.
