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

# rust::version prints the version of Cargo.toml, for the release tag. The rust profile of jnterfaces gives the same
# recipe. If jlog installs that profile, take out this line and scripts/rust.just, because just stops on a module
# defined twice.
mod rust 'scripts/rust.just'

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

# Check the prose of the README and of the pages of the site with the nossaifd checker
prose-check:
    {{ if env_var_or_default("NOSSAIFD", "") == "" { need_home } else { "true" } }}
    test -f "{{nossaifd}}/check.py" || { echo "prose-check needs nossaifd: git clone https://github.com/rahuldave/nossaifd {{nossaifd}}" >&2; exit 1; }
    for f in README.md docs/index.md docs/shell.md docs/environment.md docs/log.md docs/sessions.md docs/pass.md; do uv run --no-project --quiet python "{{nossaifd}}/check.py" "$f"; done

# The site in docs/ is built with smalldocs. Keep the version the same as in .github/workflows/docs-site.yml.
smalldocs := "uvx --quiet smalldocs@0.5.12"

# Check the site: ignored keys, syntax that only Pandoc knows, and broken links
docs-check:
    {{smalldocs}} check docs

# Build the site, and serve it on localhost with a reload on each change
docs:
    {{smalldocs}} preview docs

# Build the site into docs/_site in strict mode, as the workflow does
docs-build:
    {{smalldocs}} render docs --strict

# Measure the time that jlog adds to each line
bench: build
    bash scripts/bench.sh target/release/jlog
    bash scripts/bench.sh target/release/jlog 500 10 scope

# The plugins go before the tests, so their environments exist when the labs run them.

# The release tags. scripts/release_tag.py is a copy of .agents/skills/gpa/scripts/release_tag.py of
# agent_gest_git_skills at the commit 799a44b, as in cx. The job release of ci.yml runs just release on each push to
# main. The script runs just rust::version, because .agents/stack.json names the profile rust, so the tag is v plus
# the version of Cargo.toml.
release_tag := "uv run --no-project --quiet --no-python-downloads --python '>=3.9' python scripts/release_tag.py"

# Tag HEAD with v plus the version of Cargo.toml, push the tag and check it. Takes --dry-run first
[positional-arguments]
release *args:
    @{{release_tag}} "$@"

# Fail when the version of Cargo.toml is below the last tag or the bump that the commits ask for. It needs no network
release-check:
    {{release_tag}} --check

# Build the release binary for TARGET, a Rust target triple, and pack it as dist/jlog-TARGET.tar.gz, with jlog and
# the plugins at its top level, plus dist/jlog-TARGET.tar.gz.sha256. The job binaries of ci.yml runs it, and
# scripts/install.sh installs the archive.
dist target:
    rustup target add {{target}}
    cargo build --release --locked --target {{target}}
    rm -rf "target/dist-stage/{{target}}"
    mkdir -p "target/dist-stage/{{target}}" dist
    cp "target/{{target}}/release/jlog" "target/dist-stage/{{target}}/jlog"
    for d in plugins/*/; do n="$(basename "$d")"; mkdir -p "target/dist-stage/{{target}}/plugins/$n"; cp "$d/pyproject.toml" "$d/uv.lock" "$d/session.py" "target/dist-stage/{{target}}/plugins/$n/"; done
    COPYFILE_DISABLE=1 tar -czf "dist/jlog-{{target}}.tar.gz" -C "target/dist-stage/{{target}}" jlog plugins
    cd dist && shasum -a 256 "jlog-{{target}}.tar.gz" > "jlog-{{target}}.tar.gz.sha256"

# Everything that must pass before a commit
check: release-check fmt-check lint claude::check codex::check test diff-check prose-check docs-check

# Install jlog with cargo, and copy the plugins to the plugins folder
install: build install-plugins
    cargo install --path . --locked

# Copy the plugins to the plugins folder, and make the environment of each one
install-plugins:
    {{ if env_var_or_default("JLOG_PLUGINS", "") == "" { need_home } else { "true" } }}
    for d in plugins/*/; do n="$(basename "$d")"; mkdir -p "{{plugins_dir}}/$n"; cp "$d/pyproject.toml" "$d/uv.lock" "$d/session.py" "{{plugins_dir}}/$n/"; done
    # jlog removes these variables when it runs a plugin, so the sync does too, and both find the same environment.
    for v in $(compgen -e | grep -E '^(UV_|PYTHON)|^VIRTUAL_ENV$' || true); do unset "$v"; done; for d in "{{plugins_dir}}"/*/; do uv sync --quiet --frozen --project "$d"; done
