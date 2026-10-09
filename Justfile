# Everything but the host-only recipes runs inside the devcontainer, so a checkout needs nothing
# but just, mise, and a container runtime. The one exception is `upgrade-deps`, which also needs
# cargo and jq on the host.

# Set by devcontainer.json, so recipes can tell whether they are already running inside the dev
# container. Outside it, everything is wrapped in `devcontainer exec`. Inside, there is nothing to
# wrap - the tools are right there, and wrapping would try to start a nested container. A container
# created before this variable existed needs a `just rebuild` once.
_in_container := env("GH_WEB_DASH_DEVCONTAINER", "")
# The devcontainer CLI is pinned in mise.toml, so go through mise rather than assuming the caller
# has mise activated in their shell. Git hooks in particular run with a bare PATH.
_dce := if _in_container != "" { "" } else { "mise exec -- devcontainer exec --workspace-folder ." }
# `devcontainer exec` takes env vars as flags. A plain shell needs `env` instead.
_env := if _in_container != "" { "env" } else { "--remote-env" }
# The port `run` binds and forwards. The app defaults to an OS-assigned port,
# but a forwarded one has to be predictable.
port := "8420"
# The port the installed service binds. Deliberately not `port`: the service
# and a `just run` are both dashboards, and sharing a port means whichever
# starts second fails to bind.
service_port := "8421"

# When we're in a git worktree, the workspace's .git is a file pointing at a
# gitdir outside the workspace, so git doesn't work in the container unless we
# also mount the main repo's git dir at the same path. Note that `devcontainer
# up` reuses an existing container without comparing mounts, so a container
# created before this mount existed needs a `just rebuild` once.
_git_common_dir := `test -f .git && realpath "$(git rev-parse --git-common-dir)" || true`
_git_mount := if _git_common_dir != "" { "--mount 'type=bind,source=" + _git_common_dir + ",target=" + _git_common_dir + "'" } else { "" }

_host_only recipe:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -n "{{ _in_container }}" ]; then
        echo "just {{ recipe }} has to run on the host, not inside the dev container" >&2
        exit 1
    fi

_up:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -n "{{ _in_container }}" ]; then
        exit 0
    fi
    .devcontainer/up.sh --workspace-folder . {{ _git_mount }}

rebuild: (_host_only "rebuild")
    .devcontainer/up.sh --workspace-folder . {{ _git_mount }} --remove-existing-container

# Log in to GitHub inside the container (persists across `just rebuild`)
auth: _up
    {{ _dce }} gh auth login

shell: _up
    {{ _dce }} bash -i

test rust-log="" *args: _up
    {{ _dce }} \
      {{ if rust-log != "" { _env + " RUST_LOG=" + rust-log } else { "" } }} \
      cargo test {{ args }}

lint *args: _up
    {{ _dce }} mise exec -- precious lint {{ args }}

tidy *args: _up
    {{ _dce }} mise exec -- precious tidy {{ args }}

# Run the dashboard on a fixed, forwarded port (no browser in a container)
run *args: _up
    {{ _dce }} cargo run -- --no-open --host 0.0.0.0 --port {{ port }} {{ args }}

# Everything CI checks, in one command
ci: lint
    {{ _dce }} cargo test --locked

# Upgrade the Rust dependencies and the tools in mise.toml, skipping any release that is less than
# three days old. Brand new releases are where a compromised package is most likely to show up, and
# a short wait gives the ecosystem time to notice and yank it.
#
# For mise, the cutoff comes from `minimum_release_age` in mise.toml. Keep the two ages in sync.
#
# Cargo's version of this is still unstable. RUSTC_BOOTSTRAP is what lets a stable cargo accept
# the -Z flag, so this does not need a nightly toolchain, but it does need cargo 1.99 or newer.
#
# `cargo upgrade` knows nothing about release ages and always picks the newest version. So rather
# than letting it choose, we update Cargo.lock with the age limit first, and then tell `cargo
# upgrade` to set each requirement in Cargo.toml to the version that ended up in the lockfile.
# Requirements that are deliberately loose, like "0.4", are left alone.
#
# This runs on the host, because it needs jq, which is not in the dev container.
[doc("Upgrade Rust deps and mise tools, skipping releases under three days old")]
upgrade-deps: (_host_only "upgrade-deps")
    #!/usr/bin/env bash
    set -euo pipefail
    export RUSTC_BOOTSTRAP=1
    mise exec -- cargo update -Z min-publish-age \
        --config 'registry.global-min-publish-age="3 days"'
    packages=$(mise exec -- cargo metadata --format-version 1 | mise exec -- jq -r '
        . as $md
        | ([$md.resolve.nodes[] | select(.id | IN($md.workspace_members[])) | .deps[].pkg]
            | unique) as $ids
        | ([$md.packages[] | select(.id | IN($ids[])) | {key: .name, value: .version}]
            | from_entries) as $locked
        | [$md.packages[]
            | select(.id | IN($md.workspace_members[]))
            | .dependencies[]
            | select(.source != null and (.req | test("^\\^?[0-9]+\\.[0-9]+\\.[0-9]+$")))
            | "--package=\(.name)@\($locked[.name])"]
        | unique
        | .[]
    ')
    # With no --package arguments, `cargo upgrade` would upgrade everything to the newest release,
    # which is exactly what the age limit is there to prevent.
    if [ -n "$packages" ]; then
        # `cargo upgrade` re-resolves the lockfile without the age limit, even with `--recursive
        # false`. The lockfile we already have still satisfies the new requirements, so put it
        # back.
        # The restore is in the trap so that it also happens when `cargo upgrade` fails.
        lock=$(mktemp)
        cp Cargo.lock "$lock"
        trap 'cp "$lock" Cargo.lock; rm -f "$lock"' EXIT
        # shellcheck disable=SC2086 # one argument per line of $packages
        mise exec -- cargo upgrade --incompatible --recursive false $packages
        cp "$lock" Cargo.lock
    fi
    mise exec -- cargo metadata --locked --format-version 1 >/dev/null
    mise upgrade --bump

# --- The installed service. These run on the host, not in the container: a
# --- systemd user unit has to live in your real session to see your login.

_unit_dir := env("XDG_CONFIG_HOME", env("HOME") + "/.config") + "/systemd/user"

# Install and start the login service (needs `cargo install --path .` first)
install-service: (_host_only "install-service")
    @test -x "$HOME/.cargo/bin/gh-web-dash" \
      || (echo "gh-web-dash is not installed — run: cargo install --path ." && exit 1)
    mkdir -p {{ _unit_dir }}
    sed 's/--port 8421/--port {{ service_port }}/' \
      dist/gh-web-dash.service > {{ _unit_dir }}/gh-web-dash.service
    systemctl --user daemon-reload
    systemctl --user enable --now gh-web-dash.service
    @echo "Dashboard at http://127.0.0.1:{{ service_port }}"

# Stop, disable, and remove the login service
uninstall-service: (_host_only "uninstall-service")
    -systemctl --user disable --now gh-web-dash.service
    rm -f {{ _unit_dir }}/gh-web-dash.service
    systemctl --user daemon-reload

# Restart the service after reinstalling the binary
restart-service: (_host_only "restart-service")
    systemctl --user restart gh-web-dash.service

# Follow the service's logs
service-logs *args: (_host_only "service-logs")
    journalctl --user -u gh-web-dash -f {{ args }}
