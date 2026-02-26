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
  };
  outputs = {
    self,
    nixpkgs,
    flake-utils,
    rust-overlay,
    advisory-db,
    crane,
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
        docker-compose
        ytt
        podman
        podman-compose
        curl
      ];
      devEnvVars = rec {
        PGDATABASE = "pg";
        PGUSER = "user";
        PGPASSWORD = "password";
        PGHOST = "127.0.0.1";
        DATABASE_URL = "postgres://${PGUSER}:${PGPASSWORD}@${PGHOST}:5432/pg?sslmode=disable";
        PG_CON = "${DATABASE_URL}";
      };

      podman-runner = pkgs.callPackage ./nix/podman-runner.nix {};

      nextest-runner = pkgs.writeShellScriptBin "nextest-runner" ''
        set -e

        export PATH="${pkgs.lib.makeBinPath [
          podman-runner.podman-compose-runner
          pkgs.wait4x
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

        cleanup() {
          echo "Stopping deps..."
          ${podman-runner.podman-compose-runner}/bin/podman-compose-runner down || true
        }

        trap cleanup EXIT

        echo "Starting PostgreSQL..."
        ${podman-runner.podman-compose-runner}/bin/podman-compose-runner up -d

        echo "Waiting for PostgreSQL to be ready..."
        ${pkgs.wait4x}/bin/wait4x postgresql "$DATABASE_URL" --timeout 120s

        echo "Running database migrations..."
        ${pkgs.sqlx-cli}/bin/sqlx migrate run

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
