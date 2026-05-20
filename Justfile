default: check

run:
    cargo run

test:
    cargo test

# Run vitest in every example plugin that ships its own test suite.
# Assumes `npm install` has been run once per plugin dir.
test-plugins:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob
    failed=0
    for dir in examples/plugins/*/; do
        if [ -f "$dir/package.json" ] && [ -d "$dir/node_modules" ]; then
            echo "--- vitest: $dir ---"
            (cd "$dir" && npm test --silent) || failed=1
        elif [ -f "$dir/package.json" ]; then
            echo "skip $dir — run \`npm install\` in this directory first"
        fi
    done
    exit $failed

lint:
    cargo clippy --all-targets --all-features -- -D warnings -A clippy::pedantic

lint-pedantic:
    cargo clippy --all-targets --all-features

fmt:
    cargo fmt -- --check

fmt-fix:
    cargo fmt

check: fmt lint test
