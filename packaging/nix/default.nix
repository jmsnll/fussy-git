# Lets you build the package against your ambient nixpkgs without a flake:
#
#   nix-build packaging/nix
#   ./result/bin/fussy-git --version
#
{ pkgs ? import <nixpkgs> { } }:

pkgs.callPackage ./package.nix { }
