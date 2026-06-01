NIX_DEPS_DIR := .nix-deps

.PHONY: start-deps clean-deps setup-db reset-deps sqlx-prepare check-code test-in-ci test-book test-chapter serve-book

start-deps:
	@mkdir -p $(NIX_DEPS_DIR)
	nix run .#nix-deps-base -- up -D
	nix run .#nix-deps-base -- project is-ready --wait
	nix run .#setup-db-dev

clean-deps:
	-nix run .#nix-deps-base -- down
	chmod -R u+w $(NIX_DEPS_DIR) 2>/dev/null || true
	rm -rf $(NIX_DEPS_DIR)

setup-db:
	nix run .#setup-db-dev

reset-deps: clean-deps start-deps

test-in-ci: start-deps
	rm -rf $${CARGO_TARGET_DIR:-./target}/mdbook-test
	$(MAKE) test-book
	cargo nextest run --workspace --verbose
	cargo test --doc --workspace
	cargo doc --no-deps --workspace

test-book:
	cargo build --profile mdbook-test --features mdbook-test --lib
	CARGO_MANIFEST_DIR=$(shell pwd) mdbook test book -L $${CARGO_TARGET_DIR:-./target}/mdbook-test,$${CARGO_TARGET_DIR:-./target}/mdbook-test/deps

serve-book:
	mdbook serve book --open

test-chapter:
	cargo build --profile mdbook-test --features mdbook-test --lib
	CARGO_MANIFEST_DIR=$(shell pwd) mdbook test book -L ./target/mdbook-test,./target/mdbook-test/deps --chapter "$(CHAPTER)"

check-code:
	nix flake check

sqlx-prepare:
	cargo sqlx prepare --workspace
