default: check

run:
    cargo run

test:
    cargo test

lint:
    cargo clippy --all-targets --all-features -- -D warnings

fmt:
    cargo fmt -- --check

fmt-fix:
    cargo fmt

check: fmt lint test
