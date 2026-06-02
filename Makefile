NIX_DEPS_DIR := .nix-deps

.PHONY: start-deps clean-deps setup-db reset-deps sqlx-prepare check-code test-in-ci test-book test-chapter serve-book

start-deps:
	@mkdir -p $(NIX_DEPS_DIR)
	@eval "$$(nix run .#dev-env)"; \
	  nix run .#nix-deps-base -- up -D; \
	  for i in $$(seq 1 60); do \
	    if nix run .#nix-deps-base -- project is-ready 2>/dev/null; then break; fi; \
	    if [ "$$i" = "60" ]; then \
	      echo "ERROR: deps not ready after 5 minutes" >&2; \
	      nix run .#nix-deps-base -- process list || true; \
	      exit 1; \
	    fi; \
	    sleep 5; \
	  done; \
	  nix run .#setup-db-dev

clean-deps:
	-@eval "$$(nix run .#dev-env)"; nix run .#nix-deps-base -- down
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
