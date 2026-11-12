# jlog: every task is a recipe here. `just` lists them.
#
# This Justfile does not run through jlog, because jlog builds itself here.
set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# The home folder. The recipes that need it stop when HOME is unset.
home := env_var_or_default("HOME", "")
need_home := 'if [ -z "' + home + '" ]; then echo "this recipe needs HOME, or JLOG_PLUGINS and NOSSAIFD" >&2; exit 1; fi'


# Where jlog copies the plugins. JLOG_PLUGINS can name another folder.
plugins_dir := env_var_or_default("JLOG_PLUGINS", home / ".local/share/jlog/plugins")

# Each plugin has its own justfile, loaded as a module, for example `just claude::test`.

# The plugin for Claude Code
mod claude 'plugins/claude'

# The plugin for Codex
mod codex 'plugins/codex'

[private]
default:
    @just --list --unsorted

# Build the debug and the release binaries
build:
    cargo build --all-targets
    cargo build --release

# Format the code
fmt:
    cargo fmt --all

# Check the format of the code
fmt-check:
    cargo fmt --all --check

# Run clippy, with warnings as errors
lint:
    cargo clippy --all-targets -- -D warnings

# Run the unit tests and the labs; give a name to run only the tests that match it
test target="":
    cargo test --all-targets {{target}}

# Check the whitespace of the changes, and of the files that git does not track yet
diff-check:
    git diff --check HEAD
    found="$(git ls-files -z --others --exclude-standard | xargs -0 grep -InE '[[:blank:]]+$' /dev/null || true)"; if [ -n "$found" ]; then printf '%s\n' "$found" "trailing whitespace in an untracked file" >&2; exit 1; fi

# The clone of nossaifd, whose check.py checks the prose. NOSSAIFD can name another folder.
nossaifd := env_var_or_default("NOSSAIFD", home / "Projects/nossaifd")

# Check the prose of the README with the nossaifd checker
prose-check:
    {{ if env_var_or_default("NOSSAIFD", "") == "" { need_home } else { "true" } }}
    test -f "{{nossaifd}}/check.py" || { echo "prose-check needs nossaifd: git clone https://github.com/rahuldave/nossaifd {{nossaifd}}" >&2; exit 1; }
    uv run --no-project --quiet python "{{nossaifd}}/check.py" README.md

# Measure the time that jlog adds to each line
bench: build
    bash scripts/bench.sh target/release/jlog

# The plugins go before the tests, so their environments exist when the labs run them.

# Everything that must pass before a commit
check: fmt-check lint claude::check codex::check test diff-check prose-check

# Install jlog with cargo, and copy the plugins to the plugins folder
install: build install-plugins
    cargo install --path . --locked

# Copy the plugins to the plugins folder, and make the environment of each one
install-plugins:
    {{ if env_var_or_default("JLOG_PLUGINS", "") == "" { need_home } else { "true" } }}
    for d in plugins/*/; do n="$(basename "$d")"; mkdir -p "{{plugins_dir}}/$n"; cp "$d/pyproject.toml" "$d/uv.lock" "$d/session.py" "{{plugins_dir}}/$n/"; done
    # jlog removes these variables when it runs a plugin, so the sync does too, and both find the same environment.
    for v in $(compgen -e | grep -E '^(UV_|PYTHON)|^VIRTUAL_ENV$' || true); do unset "$v"; done; for d in "{{plugins_dir}}"/*/; do uv sync --quiet --frozen --project "$d"; done
