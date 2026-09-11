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

      systems = import inputs.systems;

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
              runtimeDeps = self'.packages.neoism.runtimeDependencies;
              tools =
                self'.packages.neoism.nativeBuildInputs ++ self'.packages.neoism.buildInputs ++ [ rust-toolchain ];
            in
            pkgs.mkShell {
              packages = [ self'.formatter ] ++ tools;
              LD_LIBRARY_PATH = "${lib.makeLibraryPath runtimeDeps}";
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
          overlayAttrs = { inherit (self'.packages) neoism; };
          packages = lib.mapAttrs' (k: v: {
            name =
              if
                builtins.elem k [
                  "neoism"
                  "default"
                ]
              then
                k
              else
                "neoism-${k}";
            value = pkgs.callPackage ./pkgNeoism.nix { rust-toolchain = v; };
          }) toolchains;
          # Different devshells for different rust versions
          devShells = lib.mapAttrs (_: v: mkDevShell v) toolchains;
        };
    };
}
