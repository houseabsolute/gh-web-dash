# Everything but the host-only recipes runs inside the devcontainer, so a checkout needs nothing
# but just, mise, and a container runtime. The one exception is `upgrade-deps`, which also needs
# cargo, curl, and jq on the host.

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
# For mise, the cutoff comes from `minimum_release_age` in mise.toml. Keep it and `age_days` below
# in sync.
#
# Cargo's version of this is still unstable. RUSTC_BOOTSTRAP is what lets a stable cargo accept
# the -Z flag, so this does not need a nightly toolchain, but it does need cargo 1.99 or newer.
#
# `cargo upgrade` knows nothing about release ages and always picks the newest version, and
# `cargo update --breaking` ignores the age limit too. So we pick each version ourselves, from the
# publish times in the crates.io index, and give `cargo upgrade` that exact version. cargo-edit
# writes an explicit version as given, even across a semver-breaking boundary. A short
# requirement, like "0.8", keeps its shape, so the new version is cut down to the same number of
# parts, like "0.9", and used if it is greater than the requirement. So "1" only moves for a new
# major version, while "1.2" also moves to "1.3". Nothing here checks `rust-version`, so a bump
# that needs a newer Rust shows up when building.
#
# This runs on the host, because it needs jq, which is not in the dev container.
[doc("Upgrade Rust deps and mise tools, skipping releases under three days old")]
upgrade-deps: (_host_only "upgrade-deps")
    #!/usr/bin/env bash
    set -euo pipefail
    export RUSTC_BOOTSTRAP=1
    age_days=3
    crates_io="registry+https://github.com/rust-lang/crates.io-index"
    # See https://doc.rust-lang.org/cargo/reference/registry-index.html for the path layout.
    index_path() {
        local name
        name=$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')
        case ${#name} in
        1) echo "1/$name" ;;
        2) echo "2/$name" ;;
        3) echo "3/${name:0:1}/$name" ;;
        *) echo "${name:0:2}/${name:2:2}/$name" ;;
        esac
    }
    # This is captured before the loop, rather than fed to it with `< <(...)`, because a failure in
    # a process substitution doesn't stop the script.
    deps=$(mise exec -- cargo metadata --no-deps --format-version 1 |
        mise exec -- jq -r --arg crates_io "$crates_io" '
            # cargo-edit applies an explicit version to every table that has the crate, so this
            # compares against the highest requirement, so that none of them is lowered.
            [.packages[].dependencies[]
                | select(.source == $crates_io)
                | select(.req | test("^\\^?[0-9]+(\\.[0-9]+){0,2}$"))
                | {name, req: (.req | ltrimstr("^"))}]
            | group_by(.name)
            # Padding makes "1.2" sort below "1.10" rather than by string. On a tie, like "1" and
            # "1.0.0", the one with more parts wins, so the more precise table still gets bumped.
            | map(max_by(.req | split(".") | map(tonumber) | (. + [0, 0])[:3] + [length]))
            | .[]
            | "\(.name) \(.req)"
        ')
    packages=()
    while read -r name req; do
        [ -n "$name" ] || continue
        # Prereleases and yanked versions are never picked. A version that isn't newer than what
        # Cargo.toml already asks for is skipped too, so a requirement someone raised by hand to a
        # version that is still too new isn't lowered. Build metadata is dropped from the result,
        # since cargo warns about it in a requirement.
        index=$(curl -fsS --retry 3 "https://index.crates.io/$(index_path "$name")") || {
            echo "Could not fetch the crates.io index entry for $name" >&2
            exit 1
        }
        version=$(printf '%s\n' "$index" |
            mise exec -- jq -rs --arg cur "$req" --argjson age "$age_days" '
                def v: sub("\\+.*$"; "") | split(".") | map(tonumber);
                ($cur | v) as $cur
                | [.[]
                    | select(.yanked | not)
                    | select(.vers | test("^[0-9]+\\.[0-9]+\\.[0-9]+(\\+.*)?$"))
                    | select((.pubtime // "1970-01-01T00:00:00Z" | fromdateiso8601)
                        <= now - $age * 86400)]
                | max_by(.vers | v) // empty
                | (.vers | v)[:($cur | length)]
                | select(. > $cur)
                | map(tostring)
                | join(".")
            ')
        if [ -n "$version" ]; then
            packages+=("--package=$name@$version")
        fi
    done <<<"$deps"
    # With no `--package` arguments, `cargo upgrade` upgrades every dependency to its latest
    # version, which ignores the age limit. So skip it when there is nothing to upgrade.
    if [ "${#packages[@]}" -gt 0 ]; then
        mise exec -- cargo upgrade --incompatible --recursive false "${packages[@]}"
    else
        echo "No Rust dependencies to upgrade in Cargo.toml" >&2
    fi
    # `cargo upgrade` re-resolves the lockfile without the age limit, even with `--recursive
    # false`, so this comes last. It also brings the transitive and loosely required dependencies
    # up to date, and moves anything too new back down.
    mise exec -- cargo update -Z min-publish-age \
        --config "registry.global-min-publish-age=\"$age_days days\""
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
