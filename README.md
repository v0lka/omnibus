# Omnibus

**One auditable, least-privilege API from your desktop OS to AI agents.**

Omnibus ships a single static binary — the daemon `omnibusd` — that exposes your
desktop OS to AI agents over the Model Context Protocol (MCP): services, packages,
logs, the desktop UI, files, clipboard, screenshots, notifications, knowledge bases
and network status; since v1.5 also app launch and live process / system / power
state.

All reasoning stays in your agent (Claude Code, goose, Cursor, or any other MCP
client): `omnibusd` performs zero model calls and collects zero telemetry. What it
adds is a uniform, honest, auditable contract between the agent and your machine.

## Why you would want it

- **One contract for the whole OS.** 35 tools instead of a pile of single-purpose
  MCP servers. The tool list already reflects your machine: `capabilities` is the
  single machine-readable truth, so the agent never has to guess the distro.
- **Least privilege by design.** A fresh install exposes exactly the read-only
  tool set. Anything that changes state — installing packages, starting services,
  writing files, launching apps, signaling processes — requires a grant that only
  you, the human, issue out of band.
- **No fake consent.** There is no runtime consent API and never will be: an
  in-band "the user confirmed" claim from an agent is ignored by construction.
- **Everything is audited.** Every call — reads included — produces exactly one
  structured audit event in your local journal. No telemetry, ever: the only
  network traffic a session can produce is the wiki fetches your agent explicitly
  requests.
- **Honest degradation.** A tool your machine cannot support is hidden from the
  tool list, not broken at runtime; a denied action returns a typed error with a
  copy-paste remedy naming the exact grant to issue.

## What your agent can do

| Area | Tools | Grant needed |
|---|---|---|
| Desktop UI inspection (accessibility tree) | `desktop.tree`, `desktop.element` | — |
| Typed UI writes (set value, invoke action) | `desktop.set_value`, `desktop.invoke` | `desktop.write.typed` |
| Clipboard | `clip_read`, `clip_write` | `clip_write` |
| Screenshots | `screenshot` | — |
| Notifications | `notify` | `portal.notify` |
| Services (systemd units) | `svc_list`, `svc_status` | — |
| Service control | `svc_start`, `svc_stop` | `service.write` |
| Network status (read-only) | `net_status` | — |
| Journal / logs (incl. past boots) | `journal_query` | `journal.persistent` opt-in for past boots |
| Package inventory (native + flatpak/snap) | `pkg_list`, `pkg_info` | — |
| Package changes | `pkg_install`, `pkg_remove`, `pkg_undo` | `pkg.write` / `pkg.undo` |
| Files inside your home | `fs_list`, `fs_read` | — |
| File writes | `fs_write` | `files.scope.write` (with paths) |
| Knowledge bases (Arch Wiki, distro docs) | `wiki_search`, `wiki_page` | — |
| App inventory (v1.5) | `app.list` | — |
| App launch / open URI or file (v1.5) | `app.launch`, `app.open` | `app.launch` |
| Live process list / per-PID info (v1.5) | `ps.list`, `ps.info` | — |
| System stats: CPU, memory, load (v1.5) | `sys.stats` | — |
| Battery / power facts (v1.5) | `power.status` | — |
| Terminate a process (v1.5) | `ps.signal` | `ps.signal` |
| Core: machine truth, session, diagnostics | `capabilities`, `session_info`, `doctor` | — |

A closed loop ties the v1.5 tools together: the agent launches an application,
watches its process and journal output, and terminates it — every write behind a
grant you issued, every step audited. Two more tools (`intent_list` /
`intent_run`, macOS AppIntents) arrive with milestone MS3.

Notes on scoping: on Linux `fs_list` / `fs_read` cover all of `$HOME` by default
(dotfiles included; the daemon's own config and state directories are always
excluded), everything beyond `$HOME` needs an explicit scope entry, and system
locations like `/etc` or `/usr` are never writable. `ps.list` / `ps.info` never
see PID 1 or other systemd-unit processes — services are addressed only through
the service tools.

## Platform support

Linux desktops today (Wayland or X11; server / headless profiles are out of scope):

| Tier | Platforms | Promise |
|---|---|---|
| T1 | Arch, Manjaro | full tool set, read + write, CI-tested |
| T2 | Debian 13 & 12, Ubuntu LTS, Mint, Fedora | all reads guaranteed; writes where declared; documented degradations (e.g. apt has no transactional undo) |
| T3 | Debian 11 | community tier; `capabilities` reports honestly, some tools below version floors are absent |

macOS is a specified target but ships with milestone MS3 (v2.0); the adapters in
this repository are Linux-only today. Individual tools activate only where their
backends exist — e.g. `power.status` appears only when UPower and a battery are
present, `app.launch` needs `systemd-run` — and `capabilities` names the reason
on every machine.

## Installation

Download the build for your platform from the
[**Releases**](../../releases) page, unpack it and put the `omnibusd` binary on
your `PATH`:

```bash
tar -xJf omnibusd-*.tar.xz
install -m 0755 omnibusd ~/.local/bin/     # or /usr/local/bin
omnibusd --version
```

> Release artifacts are not published yet. Until the first release lands, build
> from source — see [CONTRIBUTING.md](CONTRIBUTING.md). Distribution packages
> (AUR, Debian repo, Fedora COPR, Ubuntu PPA) are planned; the packaging recipes
> already live in [`packaging/`](packaging/).

Optional backends are picked up automatically when present — UPower for
`power.status`, `systemd-run` for `app.launch`, `xdg-open` for `app.open`, the
`flatpak` / `snap` CLI for the store inventory. Nothing else needs installing.

## Connecting your agent

`omnibusd` speaks MCP over stdio and is started by your MCP client as a child
process (one instance per connection; it never runs as a system daemon and dies
with the client). Register it in your client's MCP configuration:

```json
{
  "mcpServers": {
    "omnibus": {
      "command": "omnibusd"
    }
  }
}
```

Use the full path if `omnibusd` is not on the client's `PATH`; no arguments are
needed. In a fresh session, a good first move for the agent is to call
`capabilities` — the truth about what this machine supports — and `doctor` for a
health report with copy-paste remedies.

## Enabling writes (grants)

Reads work out of the box; the default is read-only. Writes require a grant
issued by you in a terminal — never through the agent:

```bash
omnibusd grant pkg.undo                              # unlock pkg_undo
omnibusd grant service.write                         # svc_start / svc_stop
omnibusd grant files.scope.write --path ~/Projects   # fs_write inside ~/Projects
omnibusd grant app.launch                            # app.launch / app.open
omnibusd grant ps.signal                             # ps.signal
omnibusd grant journal.persistent                    # opt-in: past boots in journal_query
```

Grants are TOML entries in your user store
(`$XDG_CONFIG_HOME/omnibus/scopes.d/90-grants.toml`, fallback
`~/.config/omnibus/scopes.d/`); `omnibusd grant` edits it for you, and you can
revoke by deleting the entry. The new grant set is picked up by the next session
(restart the connection). System-wide templates ship to
`/etc/omnibus/scopes.d/` with the packages. If the agent hits a denied action,
the typed error carries the exact `omnibusd grant …` command to run.

## Troubleshooting

- Ask the agent to run `doctor`, or run `omnibusd doctor` yourself in a
  terminal: it diagnoses the machine, the audit sink and the write gate, and
  returns ready-to-run remedies.
- The MCP `doctor` tool can only diagnose; opt-in mutations (like enabling
  journal persistence) run through the CLI doctor as `omnibusd doctor --apply
  <name>` — and only with your interactive confirmation, one step at a time.
- A denied write returns `ScopeDenied` (never a raw OS error) together with the
  grant command that would unlock it; the attempt itself is audited.

## Status and roadmap

Current release train: **v1.5.0** — v1.0 (reads + typed writes across the target
distros) and v1.5 (app launch, live state, full package inventory on Linux) are
implemented; milestone MS3 (v2.0) adds macOS and AppIntents. The work plan lives
in `docs/development/omnibus-roadmap.md`, formal specs in
[`specs/`](specs/INDEX.md) — see [CONTRIBUTING.md](CONTRIBUTING.md) if you want
to dig deeper or contribute.
