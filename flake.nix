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
    with pkgs; {
      formatter = nixpkgs-fmt;
      packages.default = emumet;
      devShells.default = mkShell rec {
        nativeBuildInputs = [ pkg-config ];
        buildInputs = [ openssl ];
        packages = [
          nodePackages.pnpm
          sqlx-cli
        ];
        env = {
            LD_LIBRARY_PATH = lib.makeLibraryPath buildInputs;
        };
        shellHook = ''
          #export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="${pkgs.clang}/bin/clang"
          #export CARGO_TARGET_X86_64-UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=${pkgs.mold-wrapped.override(old: { extraPackages = nativeBuildInputs ++ buildInputs; })}/bin/mold"
        '';
      };
    });
}
