clean-deps:
	docker compose down

start-deps:
	@command -v docker >/dev/null 2>&1 && docker compose up -d || echo "Docker not found, skipping start-deps"

setup-db:
	cargo sqlx migrate run

reset-deps: clean-deps start-deps setup-db

test-in-ci: start-deps setup-db
	rm -rf $${CARGO_TARGET_DIR:-./target}/mdbook-test
	$(MAKE) test-book
	cargo nextest run --workspace --verbose
	cargo test --doc --workspace
	cargo doc --no-deps --workspace

test-book:
	cargo build --profile mdbook-test --features mdbook-test,sim-time --lib
	CARGO_MANIFEST_DIR=$(shell pwd) mdbook test book -L $${CARGO_TARGET_DIR:-./target}/mdbook-test,$${CARGO_TARGET_DIR:-./target}/mdbook-test/deps

serve-book:
	mdbook serve book --open

test-chapter:
	cargo build --profile mdbook-test --features mdbook-test,sim-time --lib
	CARGO_MANIFEST_DIR=$(shell pwd) mdbook test book -L ./target/mdbook-test,./target/mdbook-test/deps --chapter "$(CHAPTER)"

check-code:
	SQLX_OFFLINE=true cargo fmt --check --all
	SQLX_OFFLINE=true cargo check --workspace
	SQLX_OFFLINE=true cargo clippy --workspace --all-features
	# temporarily dissabled due to toml version issue
	# SQLX_OFFLINE=true cargo audit 
	# SQLX_OFFLINE=true cargo deny check

sqlx-prepare:
	cargo sqlx prepare --workspace
