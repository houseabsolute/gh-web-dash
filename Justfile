# Everything runs inside the devcontainer, so a checkout needs nothing but
# just and a container runtime.

_dce := "devcontainer exec --workspace-folder ."
# The port `run` binds and forwards. The app defaults to an OS-assigned port,
# but a forwarded one has to be predictable.
port := "8420"

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
    {{ _dce }} cargo run -- --no-open --port {{ port }} {{ args }}

# Everything CI checks, in one command
ci: lint
    {{ _dce }} cargo test --locked
