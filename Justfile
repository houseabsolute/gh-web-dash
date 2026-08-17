# Everything runs inside the devcontainer, so a checkout needs nothing but
# just and a container runtime.

_dce := "devcontainer exec --workspace-folder ."
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

_up:
    devcontainer up --workspace-folder . {{ _git_mount }}

rebuild:
    devcontainer up --workspace-folder . {{ _git_mount }} --remove-existing-container

# Log in to GitHub inside the container (persists across `just rebuild`)
auth: _up
    {{ _dce }} gh auth login

shell: _up
    {{ _dce }} bash -i

test rust-log="" *args: _up
    {{ _dce }} \
      {{ if rust-log != "" { "--remote-env RUST_LOG=" + rust-log } else { "" } }} \
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

# --- The installed service. These run on the host, not in the container: a
# --- systemd user unit has to live in your real session to see your login.

_unit_dir := env("XDG_CONFIG_HOME", env("HOME") + "/.config") + "/systemd/user"

# Install and start the login service (needs `cargo install --path .` first)
install-service:
    @test -x "$HOME/.cargo/bin/gh-web-dash" \
      || (echo "gh-web-dash is not installed — run: cargo install --path ." && exit 1)
    mkdir -p {{ _unit_dir }}
    sed 's/--port 8421/--port {{ service_port }}/' \
      dist/gh-web-dash.service > {{ _unit_dir }}/gh-web-dash.service
    systemctl --user daemon-reload
    systemctl --user enable --now gh-web-dash.service
    @echo "Dashboard at http://127.0.0.1:{{ service_port }}"

# Stop, disable, and remove the login service
uninstall-service:
    -systemctl --user disable --now gh-web-dash.service
    rm -f {{ _unit_dir }}/gh-web-dash.service
    systemctl --user daemon-reload

# Restart the service after reinstalling the binary
restart-service:
    systemctl --user restart gh-web-dash.service

# Follow the service's logs
service-logs *args:
    journalctl --user -u gh-web-dash -f {{ args }}
