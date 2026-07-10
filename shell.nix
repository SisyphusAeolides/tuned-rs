{ pkgs ? import <nixpkgs> {} }:
pkgs.mkShell {
  buildInputs = with pkgs; [
    pkg-config
    systemd.dev
    rustc
    cargo
  ];
}
