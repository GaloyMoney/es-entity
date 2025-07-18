clean-deps:
	docker compose down

start-deps:
	docker compose up -d

setup-db:
	cargo sqlx migrate run

reset-deps: clean-deps start-deps setup-db

test-in-ci: start-deps setup-db
	cargo nextest run --verbose --locked
	cargo test --doc
	cargo doc --no-deps

check-code:
	SQLX_OFFLINE=true cargo fmt --check --all
	SQLX_OFFLINE=true cargo check
	SQLX_OFFLINE=true cargo clippy --workspace 
	SQLX_OFFLINE=true cargo audit
	SQLX_OFFLINE=true cargo deny check

sqlx-prepare:
	cargo sqlx prepare
