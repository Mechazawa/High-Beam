default: check

run:
    cargo run

test:
    cargo test

lint:
    cargo clippy --all-targets --all-features -- -D warnings -A clippy::pedantic

lint-pedantic:
    cargo clippy --all-targets --all-features

fmt:
    cargo fmt -- --check

fmt-fix:
    cargo fmt

check: fmt lint test
