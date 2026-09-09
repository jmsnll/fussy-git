# Installation

`fussy-git` requires a `git` binary on `PATH`. It runs on macOS and Linux
(x86-64 and arm64).

## Homebrew

```sh
brew install jmsnll/tap/fussy-git
```

## crates.io

```sh
cargo install fussy-git
```

## Prebuilt binary

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/jmsnll/fussy-git/releases/latest/download/fussy-git-installer.sh | sh
```

The installer places the binary in `$CARGO_HOME/bin` (or `~/.cargo/bin`). Each
release on the [releases page](https://github.com/jmsnll/fussy-git/releases)
also has plain `.tar.xz` archives and `.sha256` checksums if you would rather
install by hand.

## Nix

A package definition lives in
[`packaging/nix/`](https://github.com/jmsnll/fussy-git/tree/main/packaging/nix).
Until it lands in nixpkgs you can vendor it into an overlay.

## Arch Linux (AUR)

`PKGBUILD`s for a source build (`fussy-git`) and a binary build
(`fussy-git-bin`) live in
[`packaging/aur/`](https://github.com/jmsnll/fussy-git/tree/main/packaging/aur).

## From a checkout

```sh
git clone https://github.com/jmsnll/fussy-git
cargo install --path fussy-git
```
