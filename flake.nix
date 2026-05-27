{
  description = "EsEntity";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
    crane.url = "github:ipetkov/crane";
    process-compose-flake.url = "github:Platonic-Systems/process-compose-flake";
  };
  outputs = {
    self,
    nixpkgs,
    flake-utils,
    rust-overlay,
    advisory-db,
    crane,
    process-compose-flake,
  }:
    flake-utils.lib.eachDefaultSystem
    (system: let
      overlays = [
        (import rust-overlay)
      ];
      pkgs = import nixpkgs {
        inherit system overlays;
      };
      rustVersion = pkgs.pkgsBuildHost.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      rustToolchain = rustVersion.override {
        extensions = [
          "rust-analyzer"
          "rust-src"
          "rustfmt"
          "clippy"
        ];
      };

      craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
      rustSource = pkgs.lib.cleanSourceWith {
        src = craneLib.path ./.;
        filter = path: type:
          (builtins.match ".*\.sqlx/.*" path != null)
          || (builtins.match ".*deny\.toml$" path != null)
          || (craneLib.filterCargoSources path type);
      };
      commonArgs = {
        src = rustSource;
        SQLX_OFFLINE = "true";
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;

      nativeBuildInputs = with pkgs; [
        rustToolchain
        alejandra
        sqlx-cli
        cargo-nextest
        cargo-audit
        cargo-deny
        mdbook
        bacon
        postgresql
        process-compose
        curl
      ];

      pgPort = 5432;
      pgUser = "user";
      pgPassword = "password";
      pgDatabase = "pg";

      devEnvVars = rec {
        PGDATABASE = pgDatabase;
        PGUSER = pgUser;
        PGPASSWORD = pgPassword;
        PGHOST = "127.0.0.1";
        PGPORT = toString pgPort;
        DATABASE_URL = "postgres://${pgUser}:${pgPassword}@127.0.0.1:${toString pgPort}/${pgDatabase}?sslmode=disable";
        PG_CON = "${DATABASE_URL}";
      };

      # ── Postgres start helper ──────────────────────────────────────────
      pg-start = pkgs.writeShellApplication {
        name = "pg-start";
        runtimeInputs = [pkgs.postgresql pkgs.coreutils];
        text = ''
          NAME="$1" PORT="$2" PGUSER="$3" DB="$4"
          PGDATA="$PWD/.nix-deps/$NAME"

          mkdir -p "$PWD/.nix-deps"

          if [ ! -f "$PGDATA/PG_VERSION" ]; then
            echo "[$NAME] Initializing data directory at $PGDATA..."
            mkdir -p "$PGDATA"
            initdb -D "$PGDATA" --username="$PGUSER" --auth=trust --no-locale -E UTF8
            {
              echo "port = $PORT"
              echo "unix_socket_directories = '/tmp'"
              echo "listen_addresses = '127.0.0.1'"
            } >> "$PGDATA/postgresql.conf"
          fi

          if [ -f "$PGDATA/postmaster.pid" ]; then
            pg_ctl -D "$PGDATA" stop -m immediate 2>/dev/null || rm -f "$PGDATA/postmaster.pid"
          fi

          postgres -D "$PGDATA" -p "$PORT" -k /tmp &
          PG_PID=$!
          trap 'kill $PG_PID 2>/dev/null; wait $PG_PID 2>/dev/null' EXIT

          while ! pg_isready -p "$PORT" -U "$PGUSER" -h 127.0.0.1 -q 2>/dev/null; do
            sleep 0.1
          done

          if [ "$DB" != "$PGUSER" ]; then
            createdb -p "$PORT" -U "$PGUSER" -h 127.0.0.1 "$DB" 2>/dev/null || {
              if psql -p "$PORT" -U "$PGUSER" -h 127.0.0.1 -lqt | cut -d \| -f 1 | grep -qw "$DB"; then
                echo "[$NAME] Database '$DB' already exists"
              else
                echo "[$NAME] ERROR: Failed to create database '$DB'" >&2
                exit 1
              fi
            }
          fi

          echo "[$NAME] Ready on port $PORT (database: $DB)"
          wait $PG_PID
        '';
      };

      setupDbDev = pkgs.writeShellApplication {
        name = "setup-db-dev";
        runtimeInputs = [pkgs.sqlx-cli];
        text = ''
          export DATABASE_URL="${devEnvVars.DATABASE_URL}"
          exec sqlx migrate run
        '';
      };

      # ── process-compose: core-pg + setup-db ────────────────────────────
      pcLib = import process-compose-flake.lib {inherit pkgs;};

      mkPg = {
        name,
        port,
        user,
        db,
      }: {
        command = "${
          pkgs.writeShellApplication {
            name = "start-${name}";
            runtimeInputs = [pg-start];
            text = ''
              exec pg-start ${name} ${toString port} ${user} ${db}
            '';
          }
        }/bin/start-${name}";
        readiness_probe = {
          exec.command = "${
            pkgs.writeShellApplication {
              name = "ready-${name}";
              runtimeInputs = [pkgs.postgresql];
              text = ''
                exec psql -p ${toString port} -U ${user} -h 127.0.0.1 -d ${db} -c 'SELECT 1' -t -q
              '';
            }
          }/bin/ready-${name}";
          initial_delay_seconds = 1;
          period_seconds = 1;
          failure_threshold = 60;
        };
        shutdown = {
          signal = 2;
          timeout_seconds = 10;
        };
      };

      baseProcesses = {
        core-pg = mkPg {
          name = "core-pg";
          port = pgPort;
          user = pgUser;
          db = pgDatabase;
        };
        setup-db = {
          command = "${setupDbDev}/bin/setup-db-dev";
          depends_on.core-pg.condition = "process_healthy";
          availability.exit_on_end = false;
          shutdown = {
            signal = 2;
            timeout_seconds = 10;
          };
        };
      };

      nix-deps-base = pcLib.makeProcessCompose {
        name = "nix-deps-base";
        modules = [
          {
            settings = {
              log_level = "info";
              log_location = ".nix-deps/process-compose.log";
              processes = baseProcesses;
            };
          }
        ];
      };

      # ── CI test runner ────────────────────────────────────────────────
      nextest-runner = pkgs.writeShellScriptBin "nextest-runner" ''
        set -e

        export PATH="${pkgs.lib.makeBinPath [
          pkgs.sqlx-cli
          pkgs.cargo-nextest
          pkgs.coreutils
          pkgs.gnumake
          rustToolchain
          pkgs.mdbook
          pkgs.stdenv.cc
        ]}:$PATH"

        export DATABASE_URL="${devEnvVars.DATABASE_URL}"
        export PG_CON="${devEnvVars.PG_CON}"
        export PGDATABASE="${devEnvVars.PGDATABASE}"
        export PGUSER="${devEnvVars.PGUSER}"
        export PGPASSWORD="${devEnvVars.PGPASSWORD}"
        export PGHOST="${devEnvVars.PGHOST}"
        export PGPORT="${devEnvVars.PGPORT}"

        cleanup() {
          echo "Stopping deps..."
          ${nix-deps-base}/bin/nix-deps-base down 2>/dev/null || true
        }
        trap cleanup EXIT

        mkdir -p .nix-deps

        echo "Starting PostgreSQL via process-compose..."
        ${nix-deps-base}/bin/nix-deps-base up -D
        ${nix-deps-base}/bin/nix-deps-base project is-ready --wait

        echo "Running mdbook tests..."
        rm -rf ''${CARGO_TARGET_DIR:-./target}/mdbook-test
        cargo build --profile mdbook-test --features mdbook-test --lib
        CARGO_MANIFEST_DIR=$(pwd) mdbook test book -L ''${CARGO_TARGET_DIR:-./target}/mdbook-test,''${CARGO_TARGET_DIR:-./target}/mdbook-test/deps

        echo "Running nextest..."
        cargo nextest run --workspace --verbose

        echo "Running doc tests..."
        cargo test --doc --workspace

        echo "Building docs..."
        cargo doc --no-deps --workspace

        echo "Tests completed successfully!"
      '';
    in
      with pkgs; {
        packages = {
          nextest = nextest-runner;
          setup-db-dev = setupDbDev;
          inherit nix-deps-base;
        };

        apps.setup-db-dev = flake-utils.lib.mkApp {
          drv = setupDbDev;
          name = "setup-db-dev";
        };

        checks = {
          workspace-fmt = craneLib.cargoFmt commonArgs;
          workspace-clippy = craneLib.cargoClippy (commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-features -- --deny warnings";
            });
          workspace-audit = craneLib.cargoAudit {
            inherit advisory-db;
            src = rustSource;
          };
          workspace-deny = craneLib.cargoDeny {
            src = rustSource;
          };
          check-fmt = stdenv.mkDerivation {
            name = "check-fmt";
            src = ./.;
            nativeBuildInputs = [alejandra];
            dontBuild = true;
            doCheck = true;
            checkPhase = ''
              alejandra -qc .
            '';
            installPhase = ''
              mkdir -p $out
            '';
          };
        };

        devShells.default = mkShell (devEnvVars
          // {
            inherit nativeBuildInputs;
          });

        formatter = alejandra;
      });
}
