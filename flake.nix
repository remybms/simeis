{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = inputs: with inputs; flake-utils.lib.eachDefaultSystem (system: let
    pkgs = import nixpkgs {
      inherit system;
      overlays = [ rust-overlay.overlays.default ];
    };

    rustpkg = pkgs.rust-bin.stable."1.85.0".default.override {
      extensions = [ "rust-src" ];
    };

    deps = with pkgs; [
      gcc
      rustpkg
      cargo-audit
      cargo-vet
      mold
      (python312.withPackages (p: with p; [
        requests
      ]))
    ];

    mkScript = script: { type = "app"; program = builtins.toString script; };
    scripts = import ./.forgejo/scripts.nix { inherit pkgs deps; };
  in {
    packages.default = pkgs.stdenv.mkDerivation {
      pname = "simeis-manual";
      version = "0.1.0";
      src = ./doc;
      buildInputs = [ pkgs.typst pkgs.bash ];
      phases = ["unpackPhase" "buildPhase"];
      buildPhase = ''
        export HOME=$(realpath ./.home)
        ls -lhaR
        mkdir -p $out
        typst compile --root "$PWD" "./manual.typ" "$out/manual.pdf"
      '';
    };

    apps = builtins.mapAttrs (name: val: mkScript val) scripts;
    devShells.default = pkgs.mkShell {
      buildInputs = deps ++ [ pkgs.typst ];
      shellHook = ''
      '';
    };
  });
}
