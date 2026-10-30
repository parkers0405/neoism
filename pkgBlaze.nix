# Published compiler and its own standard-library source, available offline in Nix.
{ stdenv, fetchurl, autoPatchelfHook }:
let
  pin = builtins.fromJSON (builtins.readFile ./neoism-frontend/gpuix/native/blaze-pin.json);
  archive = pin.archives.${stdenv.hostPlatform.system};
  library = fetchurl { inherit (pin.library) url sha256; };
in stdenv.mkDerivation {
  pname = "neoism-blaze";
  version = builtins.substring 1 ((builtins.stringLength pin.release) - 1) pin.release;
  src = fetchurl {
    url = "https://github.com/indexable-inc/blaze/releases/download/${pin.release}/blaze-${pin.release}-${archive.target}.tar.gz";
    inherit (archive) sha256;
  };
  nativeBuildInputs = [ autoPatchelfHook ];
  buildInputs = [ stdenv.cc.cc.lib ];
  dontConfigure = true;
  dontBuild = true;
  installPhase = ''
    runHook preInstall
    mkdir -p $out/bin $out/share/blaze $out/share/licenses/blaze
    cp blaze blaze-engine blaze-rustc blaze-proc-macro-srv $out/bin/
    cp LICENSE-* $out/share/licenses/blaze/
    tar -xJf ${library} -C $out/share/blaze
    runHook postInstall
  '';
}
