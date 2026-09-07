# Upstream provenance

- Repository: `https://github.com/wezterm/wezterm.git`
- Revision: `d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b`
- Upstream package: `wezterm-ssh` `0.4.0`
- License: MIT; see `LICENSE.md`.

The `src/` directory was copied verbatim from that revision before applying
the Android patch below. The standalone manifest replaces upstream
`workspace = true` dependency declarations with equivalent explicit
dependencies; WezTerm-owned dependencies remain pinned to the same revision.

## Android patch

Only `src/sessioninner.rs` is behaviorally changed:

1. `wezterm_ssh_dir` maps to libssh's `SshOption::SshDir`.
2. `wezterm_ssh_process_config=false` skips libssh's implicit per-user and
   system OpenSSH configuration parsing and sets `SshOption::ProcessConfig(false)`.
3. `globalknownhostsfile` maps to libssh's `SshOption::GlobalKnownHosts`.

These options keep Android credentials and trust state inside the app-private
SSH directory. The default remains upstream-compatible: absent
`wezterm_ssh_process_config`, libssh parses its default configuration files.
