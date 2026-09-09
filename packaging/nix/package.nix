# nixpkgs-style package definition for fussy-git.
#
# This is written to drop into nixpkgs at
#   pkgs/by-name/fu/fussy-git/package.nix
# with only the `src.hash` and `cargoHash` filled in (see packaging/nix/README.md).
#
# Locally:  nix-build packaging/nix
{
  lib,
  rustPlatform,
  fetchFromGitHub,
  git,
  makeWrapper,
  versionCheckHook,
  nix-update-script,
}:

rustPlatform.buildRustPackage rec {
  pname = "fussy-git";
  version = "0.1.1";

  src = fetchFromGitHub {
    owner = "jmsnll";
    repo = "fussy-git";
    tag = "v${version}";
    # nix-build will print the correct value on first run; paste it here.
    hash = lib.fakeHash;
  };

  # nix-build will print the correct value on first run; paste it here.
  cargoHash = lib.fakeHash;

  nativeBuildInputs = [ makeWrapper ];

  # fussy-git shells out to `git` for every network and auth operation.
  postInstall = ''
    wrapProgram $out/bin/fussy-git \
      --prefix PATH : ${lib.makeBinPath [ git ]}
  '';

  # The test suite creates real git repositories on disk.
  nativeCheckInputs = [ git ];
  preCheck = ''
    export HOME=$(mktemp -d)
    git config --global user.email nixbld@localhost
    git config --global user.name  nixbld
    git config --global init.defaultBranch main
  '';

  nativeInstallCheckInputs = [ versionCheckHook ];
  versionCheckProgramArg = "--version";
  doInstallCheck = true;

  passthru.updateScript = nix-update-script { };

  meta = {
    description = "Keeps cloned git repositories organised under a configurable root, by remote URL";
    homepage = "https://github.com/jmsnll/fussy-git";
    changelog = "https://github.com/jmsnll/fussy-git/blob/v${version}/CHANGELOG.md";
    license = lib.licenses.mit;
    maintainers = with lib.maintainers; [ ]; # add your nixpkgs handle here
    mainProgram = "fussy-git";
    platforms = lib.platforms.unix;
  };
}
