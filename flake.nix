{
  description = "Neoism | A hardware-accelerated terminal-first editor workspace";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    systems.url = "github:nix-systems/default";
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [ flake-parts.flakeModules.easyOverlay ];

      systems = builtins.filter (system: builtins.elem system [ "x86_64-linux" "aarch64-linux" ]) (import inputs.systems);

      perSystem =
        {
          self',
          inputs',
          pkgs,
          system,
          lib,
          ...
        }:
        let
          # Defines a devshell using the `rust-toolchain`, allowing for
          # different versions of rust to be used.
          mkDevShell =
            rust-toolchain:
            let
              runtimeDeps = self'.packages.neoism-services.runtimeDependencies;
              tools =
                self'.packages.neoism-services.nativeBuildInputs ++ self'.packages.neoism-services.buildInputs ++ [ rust-toolchain ];
            in
            pkgs.mkShell {
              packages = [ self'.formatter pkgs.nodejs_22 pkgs.rustup pkgs.git pkgs.clang
                blazeCompiler ] ++ tools;
              NEOISM_BLAZE_BIN = "${blazeCompiler}/bin/blaze";
              shellHook = ''
                export RUSTUP_HOME="$PWD/target/blaze-rustup"
                mkdir -p "$RUSTUP_HOME/toolchains"
                ln -sfn ${blazeToolchain} "$RUSTUP_HOME/toolchains/${blazePin.toolchain}-${pkgs.stdenv.hostPlatform.rust.rustcTarget}"
              '';
              LD_LIBRARY_PATH = "${lib.makeLibraryPath runtimeDeps}";
            };
          blazePin = builtins.fromJSON (builtins.readFile ./neoism-frontend/gpuix/native/blaze-pin.json);
          blazeCompiler = pkgs.callPackage ./pkgBlaze.nix { };
          blazeToolchain = pkgs.rust-bin.nightly.${lib.removePrefix "nightly-" blazePin.toolchain}.default.override {
            # Blaze links most proc macros as WASI modules, including during
            # offline Nix builds where rustup cannot download missing targets.
            targets = [ "wasm32-wasip1" ];
          };
          toolchains = rec {
            msrv = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
            stable = pkgs.rust-bin.stable.latest.minimal;
            nightly = pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.minimal);
            neoism = msrv;
            default = neoism;
          };
        in
        {
          formatter = pkgs.alejandra;
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ (import inputs.rust-overlay) ];
          };

          # Create overlay to override `neoism` with this flake's default
          overlayAttrs = { inherit (self'.packages) neoism-services; };
          packages = lib.mapAttrs' (k: v: {
            name = if k == "neoism" || k == "default" then "neoism-services${if k == "default" then "-default" else ""}" else "neoism-services-${k}";
            value = pkgs.callPackage ./pkgNeoism.nix {
              rust-toolchain = v;
              blaze-toolchain = blazeToolchain;
              blaze-compiler = blazeCompiler;
            };
          }) toolchains // lib.optionalAttrs pkgs.stdenv.isLinux {
            default = self'.packages.neoism-runtime;
            # Run an existing Linux release without compiling the Rust workspace.
            neoism-runtime = pkgs.writeShellScriptBin "neoism-runtime" ''
              if [ "$#" -eq 0 ]; then
                echo "Usage: neoism-runtime /path/to/neoism [arguments...]" >&2
                exit 64
              fi
              export NIX_LD="${pkgs.stdenv.cc.bintools.dynamicLinker}"
              export NIX_LD_LIBRARY_PATH="${lib.makeLibraryPath self'.packages.neoism-services.runtimeDependencies}''${NIX_LD_LIBRARY_PATH:+:$NIX_LD_LIBRARY_PATH}"
              export LD_LIBRARY_PATH="${lib.makeLibraryPath self'.packages.neoism-services.runtimeDependencies}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
              exec "$@"
            '';
          };
          # Different devshells for different rust versions
          devShells = lib.mapAttrs (_: v: mkDevShell v) toolchains;

          checks = lib.optionalAttrs pkgs.stdenv.isLinux {
            runtime-libraries = pkgs.runCommand "neoism-runtime-libraries" { } ''
              ${self'.packages.neoism-runtime}/bin/neoism-runtime \
                ${pkgs.python3}/bin/python3 - <<'PY'
              import ctypes
              for library in [
                  "libwayland-client.so.0", "libwayland-cursor.so.0", "libwayland-egl.so.1",
                  "libvulkan.so.1", "libxkbcommon.so.0", "libgbm.so.1", "libdbus-1.so.3",
                  "libfontconfig.so.1", "libstdc++.so.6", "libz.so.1", "libEGL.so.1", "libGL.so.1",
              ]:
                  ctypes.CDLL(library)
                  print("Loaded " + library)
              PY
              touch "$out"
            '';
          };
        };
    };
}
