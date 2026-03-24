{
  description = "qssh - QUIC-based SSH proxy with connection migration support";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      overlay = final: prev: {
        qssh = final.callPackage ./nix/package.nix { };
      };
    in
    {
      overlays.default = overlay;

      nixosModules.default = import ./nix/module.nix self;
    }
    // flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ overlay ];
        };
      in
      {
        packages = {
          qssh = pkgs.qssh;
          default = pkgs.qssh;
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ pkgs.qssh ];
        };
      }
    );
}
