{
  description = "Keeps cloned git repositories organised under a configurable root, by remote URL";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        fussy-git = pkgs.rustPlatform.buildRustPackage {
          pname = "fussy-git";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;

          src = self;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.makeWrapper ];
          postInstall = ''
            wrapProgram $out/bin/fussy-git \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.git ]}
          '';

          nativeCheckInputs = [ pkgs.git ];
          preCheck = ''
            export HOME=$(mktemp -d)
            git config --global user.email nixbld@localhost
            git config --global user.name  nixbld
            git config --global init.defaultBranch main
          '';

          meta = {
            description = "Keeps cloned git repositories organised under a configurable root, by remote URL";
            homepage = "https://github.com/jmsnll/fussy-git";
            license = pkgs.lib.licenses.mit;
            mainProgram = "fussy-git";
            platforms = pkgs.lib.platforms.unix;
          };
        };
        default = fussy-git;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.clippy
            pkgs.rustfmt
            pkgs.git
          ];
        };
      });
    };
}
