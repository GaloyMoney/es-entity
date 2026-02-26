{pkgs}: let
  podman-compose-runner = pkgs.stdenv.mkDerivation {
    pname = "podman-compose-runner";
    version = "0.1.0";

    dontUnpack = true;

    buildInputs = with pkgs; [
      makeWrapper
    ];

    installPhase = ''
      mkdir -p $out/bin

      cat > $out/bin/podman-compose-runner << 'EOF'
      #!/usr/bin/env bash
      set -e

      if [[ "$OSTYPE" == "darwin"* ]]; then
        if podman machine list --format json | jq -e '.[] | select(.Name == "podman-machine-default")' >/dev/null 2>&1; then
          if ! podman machine list --format json | jq -e '.[] | select(.Name == "podman-machine-default" and .Running == true)' >/dev/null 2>&1; then
            echo "Starting podman machine..."
            podman machine start
          fi
        else
          echo "No podman machine found. Creating and starting podman-machine-default..."
          podman machine init
          podman machine start
        fi

        mkdir -p ~/.config/containers
        echo 'unqualified-search-registries = ["docker.io"]' > ~/.config/containers/registries.conf
        echo '{"default":[{"type":"insecureAcceptAnything"}]}' > ~/.config/containers/policy.json
      else
        echo "Using podman on Linux..."

        export XDG_RUNTIME_DIR="''${XDG_RUNTIME_DIR:-/tmp/podman-runtime-$(id -u)}"
        mkdir -p "$XDG_RUNTIME_DIR"

        mkdir -p /var/tmp
        mkdir -p /tmp
        export TMPDIR=/tmp

        mkdir -p ~/.config/containers
        echo 'unqualified-search-registries = ["docker.io"]' > ~/.config/containers/registries.conf
        echo '{"default":[{"type":"insecureAcceptAnything"}]}' > ~/.config/containers/policy.json

        echo "Checking podman installation..."
        if ! podman version >/dev/null 2>&1; then
          echo "ERROR: podman version failed. Output:"
          podman version 2>&1 || true
        fi

        echo "Getting podman info..."
        podman info 2>&1 || true

        if podman ps >/dev/null 2>&1; then
          echo "Podman is working correctly"
        else
          echo "WARNING: podman ps test failed, but continuing anyway..."
        fi
      fi

      exec podman-compose "$@"
      EOF

      chmod +x $out/bin/podman-compose-runner

      wrapProgram $out/bin/podman-compose-runner \
        --prefix PATH : ${pkgs.lib.makeBinPath (
        [
          pkgs.podman
          pkgs.podman-compose
          pkgs.coreutils
          pkgs.bash
          pkgs.jq
        ]
        ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
          pkgs.fuse-overlayfs
          pkgs.iptables
          pkgs.netavark
          pkgs.aardvark-dns
        ]
      )}
    '';

    meta = with pkgs.lib; {
      description = "Podman-compose runner that auto-manages podman machine on macOS";
      license = licenses.mit;
      platforms = platforms.all;
    };
  };
in {
  inherit podman-compose-runner;
}
