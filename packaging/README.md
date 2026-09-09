# Packaging

Downstream package definitions for `fussy-git`. Nothing here is built by CI; it
is kept in the repo so it is versioned alongside the code and easy to hand to a
distro.

## Nix

| File                | Purpose                                                              |
| ------------------- | ----------------------------------------------------------------- |
| `nix/package.nix`   | nixpkgs-style derivation, ready for `pkgs/by-name/fu/fussy-git/`.     |
| `nix/default.nix`   | `nix-build packaging/nix` against your ambient nixpkgs.              |
| `../flake.nix`      | `nix run github:jmsnll/fussy-git` — builds from source via `Cargo.lock`. |

### Trying it now

```sh
nix run github:jmsnll/fussy-git -- --version
nix build github:jmsnll/fussy-git
```

Generate and commit the lock file once:

```sh
nix flake lock
git add flake.lock && git commit -m "build: add flake.lock"
```

### Submitting to nixpkgs

1. Copy `nix/package.nix` to `pkgs/by-name/fu/fussy-git/package.nix` in a
   nixpkgs checkout.
2. Replace both `lib.fakeHash` placeholders: run `nix-build -A fussy-git`, and
   paste the `got:` hash it prints for the source, then again for `cargoHash`.
3. Add your nixpkgs handle to `meta.maintainers`.
4. `nix-build -A fussy-git && ./result/bin/fussy-git --version`, then open the PR.

The derivation wraps the binary so `git` is always on its `PATH`, and the check
phase runs the test suite with an isolated `HOME` and git identity.

## Arch Linux (AUR)

Two packages, so users can choose a source build or the prebuilt binary:

| Directory            | AUR package      | Builds from                                   |
| -------------------- | ---------------- | -------------------------------------------- |
| `aur/fussy-git/`     | `fussy-git`      | the tagged source tarball, with `cargo`.       |
| `aur/fussy-git-bin/` | `fussy-git-bin`  | the `cargo-dist` release archives.             |

### Publishing / updating

```sh
cd aur/fussy-git            # or aur/fussy-git-bin
# bump pkgver, reset pkgrel=1
updpkgsums                  # refresh sha256sums from the real artifacts
makepkg -f                  # build locally
namcap PKGBUILD             # lint
makepkg --printsrcinfo > .SRCINFO

# push to the AUR (first time: git clone ssh://aur@aur.archlinux.org/fussy-git.git)
git add PKGBUILD .SRCINFO
git commit -m "upgpkg: fussy-git 0.1.1-1"
git push
```

`.SRCINFO` is generated, not hand-edited — regenerate it whenever `PKGBUILD`
changes. `fussy-git-bin` declares `provides=('fussy-git')` and
`conflicts=('fussy-git')` so the two never install together.

## Checksums for the current release (v0.1.1)

| Artifact                                    | sha256                                                             |
| ------------------------------------------- | --------------------------------------------------------------- |
| GitHub source archive `v0.1.1.tar.gz`       | `baf611b34b58cb7e0c8539ccdfdc0cc0ba06f60995bc33cf4f6d072388bcd3c9` |
| `fussy-git-x86_64-unknown-linux-gnu.tar.xz` | `8665dc4af0c5730d1793d2dbf8c332acb0c87f05050ca312e87f65ef4c98fb7d` |
| `fussy-git-aarch64-unknown-linux-gnu.tar.xz`| `65bdc50b5aab8f1f516097aaef802c73d839eefac93eada131e6f596ee6974c8` |
