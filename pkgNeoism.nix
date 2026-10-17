{
  # rust-overlay deps
  rust-toolchain,
  makeRustPlatform,
  # Normal deps
  lib,
  stdenv,
  darwin,
  autoPatchelfHook,
  cmake,
  pkg-config,
  gcc-unwrapped,
  fontconfig,
  libGL,
  libgbm,
  dbus,
  zlib,
  glslang,
  vulkan-loader,
  libxkbcommon,
  withX11 ? !stdenv.isDarwin,
  libX11,
  libXcursor,
  libXi,
  libXrandr,
  libxcb,
  withWayland ? !stdenv.isDarwin,
  wayland,
  ...
}:
let
  readTOML = f: builtins.fromTOML (builtins.readFile f);
  cargoToml = readTOML ./Cargo.toml;
  neoismToml = readTOML ./neoism-frontend/desktop/Cargo.toml;
  rustPlatform = makeRustPlatform {
    cargo = rust-toolchain;
    rustc = rust-toolchain;
  };
  rlinkLibs =
    lib.optionals stdenv.isLinux [
      (lib.getLib gcc-unwrapped)
      fontconfig
      libGL
      libgbm
      dbus
      zlib
      libxkbcommon
      vulkan-loader
    ]
    ++ lib.optionals withX11 [
      libX11
      libXcursor
      libXi
      libXrandr
      libxcb
    ]
    ++ lib.optionals withWayland [
      wayland
    ];

  # Build and test the same shipped programs, not every workspace member.
  cargoPackageFlags = lib.concatMap (package: [ "-p" package ]) [
    "neoism"
    "neoism-workspace-daemon"
    "neoism-agent"
    "neoism-agent-lua-runner"
  ];

  inherit (lib.fileset) unions toSource;
in
rustPlatform.buildRustPackage {
  inherit (cargoToml.workspace.package) version;
  name = "neoism";
  src = toSource {
    root = ./.;
    fileset = unions (
      [
        ./Cargo.lock
        ./Cargo.toml
        ./README.md # Workspace package metadata
        ./assets/omarchy # Embedded desktop integration
        ./docs/lua-api.lua # neoism-lua's checked-in API contract test fixture
        # Optional path dependencies still have to exist during Cargo resolution.
        ./neoism-frontend/interactive-artifacts
        ./misc # Extra desktop files
      ]
      ++ (map (x: ./. + "/${x}") cargoToml.workspace.members)
    );
  };
  cargoLock.lockFile = ./Cargo.lock;

  cargoBuildFlags = cargoPackageFlags;
  cargoTestFlags = cargoPackageFlags;

  buildInputs = rlinkLibs ++ (lib.optionals stdenv.isDarwin [ darwin.libutil ]);
  runtimeDependencies = rlinkLibs;

  nativeBuildInputs = [
    rustPlatform.bindgenHook
  ]
  ++ lib.optionals stdenv.isLinux [
    cmake
    glslang # sugarloaf compiles its Vulkan shaders with glslangValidator
    pkg-config
    autoPatchelfHook
  ];

  postInstall = ''
    install -D -m 644 misc/neoism.desktop -t \
                      $out/share/applications
    install -D -m 644 neoism-frontend/desktop/assets/icons/neoism.png \
                      $out/share/icons/hicolor/512x512/apps/neoism.png
  ''
  + lib.optionalString stdenv.hostPlatform.isDarwin ''
    mkdir -p $out/Applications/Neoism.app/Contents/MacOS \
             $out/Applications/Neoism.app/Contents/Resources
    sed -e 's/{{VERSION}}/${lib.head (lib.splitString "-" (lib.head (lib.splitString "+" cargoToml.workspace.package.version)))}/g' \
      -e 's/{{RELEASE_VERSION}}/${cargoToml.workspace.package.version}/g' \
      misc/macos/Info.plist > $out/Applications/Neoism.app/Contents/Info.plist
    cp misc/macos/neoism.icns $out/Applications/Neoism.app/Contents/Resources/
    ln -s $out/bin/neoism $out/Applications/Neoism.app/Contents/MacOS/
  '';

  buildNoDefaultFeatures = true;
  buildFeatures = (lib.optionals withX11 [ "x11" ]) ++ (lib.optionals withWayland [ "wayland" ]);
  checkType = "debug";
  meta = {
    description = neoismToml.package.description;
    longDescription = neoismToml.package.metadata.deb.extended-description;
    homepage = cargoToml.workspace.package.homepage;
    license = lib.licenses.mit;
    platforms = lib.platforms.unix;
    changelog = "https://github.com/parkers0405/neoism/blob/main/CHANGELOG.md";
    mainProgram = "neoism";
  };
}
