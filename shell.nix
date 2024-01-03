let
  pkgs = import ./nix;
in
pkgs.stdenv.mkDerivation {
  name = "reord";
  buildInputs = (
    (with pkgs; [
      cargo-nextest
      niv

      (fenix.combine (with fenix; [
        minimal.cargo
        minimal.rustc
        rust-analyzer
      ]))
    ])
  );
}
