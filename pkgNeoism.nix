# Source-built workspace services. The GPUIX desktop is distributed as a bundle
# and runs on NixOS through the flake's neoism-runtime launcher.
{ rust-toolchain, blaze-toolchain, blaze-compiler, makeRustPlatform, lib, stdenv, cmake, pkg-config, perl, cacert, util-linux, nodejs, gitMinimal,
  gcc-unwrapped, fontconfig, libGL, libgbm, dbus, zlib, vulkan-loader,
  libxkbcommon, libX11, libXcursor, libXi, libXrandr, libxcb, wayland, ... }:
let
  cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
  blazePin = builtins.fromJSON (builtins.readFile ./neoism-frontend/gpuix/native/blaze-pin.json);
  rustPlatform = makeRustPlatform { cargo = blaze-toolchain; rustc = blaze-toolchain; };
  runtimeLibraries = [ fontconfig zlib ] ++ lib.optionals stdenv.isLinux [
    (lib.getLib gcc-unwrapped) libGL libgbm dbus libxkbcommon vulkan-loader
    libX11 libXcursor libXi libXrandr libxcb wayland
  ];
in rustPlatform.buildRustPackage {
  pname = "neoism-services";
  inherit (cargoToml.workspace.package) version;
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions ([ ./Cargo.lock ./Cargo.toml ./README.md
      ./.cargo/config.toml
      ./vendor/blaze-compat
      ./neoism-frontend/interactive-artifacts
      ./scripts/blaze.sh
      ./scripts/test-workspace.mjs
      ./neoism-frontend/gpuix/native/blaze-pin.json
    ] ++ map (member: ./. + "/${member}") cargoToml.workspace.members);
  };
  cargoLock.lockFile = ./Cargo.lock;
  cargoBuildFlags = [ "-p" "neoism-workspace-daemon" "-p" "neoism-agent" ];
  cargoTestFlags = [ "-p" "neoism-workspace-daemon" "-p" "neoism-agent" ];
  nativeBuildInputs = [ cmake pkg-config perl util-linux nodejs rustPlatform.bindgenHook ];
  # Exercise real repository operations instead of letting integration tests
  # skip their Git-dependent cases in Nix's restricted build environment.
  nativeCheckInputs = [ gitMinimal ];
  buildInputs = runtimeLibraries;
  runtimeDependencies = runtimeLibraries;
  # Blaze rejects Nix's unmapped sandbox-root owner. Keep the compiler's
  # ownership checks intact and run this derivation in Nix's relaxed mode.
  __noChroot = true;
  preBuild = ''
    export NEOISM_BLAZE_BIN=${blaze-compiler}/bin/blaze
    export RUSTUP_HOME="$PWD/.blaze-rustup"
    export BLAZE_SANDBOX=off
    export BLAZE_HOME="$NIX_BUILD_TOP/blaze"
    mkdir -p "$RUSTUP_HOME/toolchains"
    ln -s ${blaze-toolchain} "$RUSTUP_HOME/toolchains/${blazePin.toolchain}-${stdenv.hostPlatform.rust.rustcTarget}"
  '';
  buildPhase = ''
    runHook preBuild
    bash scripts/blaze.sh build --locked --offline --release -p neoism-workspace-daemon -p neoism-agent
    bash scripts/blaze.sh stop
    runHook postBuild
  '';
  SSL_CERT_FILE = "${cacert}/etc/ssl/certs/ca-bundle.crt";
  preCheck = ''
    # Nix's default /homeless-shelter is deliberately absent. Service identity
    # tests need an existing home, isolated from the invoking user's state.
    export HOME="$NIX_BUILD_TOP/check-home"
    mkdir -p "$HOME"
    git --version
  '';
  checkPhase = ''
    runHook preCheck
    node scripts/test-workspace.mjs --locked --offline --package neoism-workspace-daemon --package neoism-agent
    bash scripts/blaze.sh stop
    runHook postCheck
  '';
  installPhase = ''
    runHook preInstall
    mkdir -p $out/bin
    for binary in neoism-workspace-daemon neoism-agent; do
      candidate="target/release/$binary"
      if [ ! -f "$candidate" ]; then candidate="target/${stdenv.hostPlatform.rust.rustcTarget}/release/$binary"; fi
      install -m755 "$candidate" "$out/bin/$binary"
    done
    runHook postInstall
  '';
  meta = {
    description = "Neoism workspace daemon and Agent CLI for the GPUIX frontend";
    homepage = cargoToml.workspace.package.homepage;
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
    mainProgram = "neoism-workspace-daemon";
  };
}
