default:
    @just --list

fmt:
	cargo +nightly fmt --all

clippy:
	cargo clippy --all-features --all-targets -- -D warnings
