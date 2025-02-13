{
  description = "Rust-Nix";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
  };

  outputs = { nixpkgs, flake-utils, ... }: flake-utils.lib.eachDefaultSystem (system:
    let
      pkgs = import nixpkgs {
        inherit system;
      };
      emumet = pkgs.rustPlatform.buildRustPackage {
        pname = "server";
        name = "emumet";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = [ pkgs.pkg-config ];
        buildInputs = [ pkgs.openssl ];
        PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
      };
    in
    with pkgs; rec {
      formatter = nixpkgs-fmt;
      packages.default = emumet;
      devShells.default = mkShell {
        nativeBuildInputs = [ pkg-config ];
        buildInputs = [ openssl ];
        packages = [
          nodePackages.pnpm
          sqlx-cli
        ];
      };
    });
}
