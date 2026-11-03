# Arch Linux packaging

`nori-git/` holds the PKGBUILD for `nori-git`, a package that builds the latest commit of `main`.
It is meant for the AUR but is not published there yet; until then, build it locally as below.
It installs the same files as the `niri` package in Arch's extra repository, under nori names, so both can be installed side by side.

## Build it locally

From the repository root:

```sh
build=$(mktemp -d)
cp dist/arch/nori-git/PKGBUILD "$build/"
cd "$build"
makepkg -si
```

makepkg clones `main` from GitHub; it doesn't build your local checkout.
Building in a copy keeps the build files out of the checkout, and keeps makepkg from rewriting `pkgver=` in the tracked PKGBUILD.

## Change the PKGBUILD

1. Edit `dist/arch/nori-git/PKGBUILD`.
2. Regenerate the metadata: `cd dist/arch/nori-git && makepkg --printsrcinfo > .SRCINFO`.
3. Open a pull request.
   The `AUR package` workflow builds the package from the pull request (GitHub's merge of it into `main`) in an Arch Linux container, checks that `.SRCINFO` is current, runs namcap, installs the package, checks that the binary has no build paths in it, and checks that `nori panic` prints file and line numbers.

## Publish to the AUR

This needs an AUR account with an SSH key, and the package must be pushed from that account.
Cloning a package name that doesn't exist yet gives an empty repository; the first push creates the package.

From the repository root, with the AUR repository cloned next to the checkout:

```sh
git clone ssh://aur@aur.archlinux.org/nori-git.git ../aur-nori-git
cp dist/arch/nori-git/PKGBUILD dist/arch/nori-git/.SRCINFO ../aur-nori-git/
cd ../aur-nori-git
git add PKGBUILD .SRCINFO
git commit -m "Update PKGBUILD"
git push origin HEAD:master
```

The AUR only accepts pushes to `master`, needs `.SRCINFO` in the pushed commit and `PKGBUILD` in every commit, and rejects subdirectories other than `keys/` and `LICENSES/` (see `aurweb/git/update.py` in [aurweb](https://gitlab.archlinux.org/archlinux/aurweb)).
Push to the AUR only what is merged into `main` here, so the two stay the same.

After the first push, update the places that say the package isn't published: the top of this file, the "PKGBUILD for the AUR" entry in `CHANGES-FROM-NIRI.md`, and the installation note in `README.md`.
