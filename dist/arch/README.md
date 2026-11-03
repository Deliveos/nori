# Arch Linux packaging

`nori-git/` holds the PKGBUILD for `nori-git`, a package that builds the latest commit of `main`.
It is meant for the AUR but is not published there yet; until then, build it locally as below.
It installs the same files as the `niri` package in Arch's extra repository, under nori names, so both can be installed side by side.

## Build it locally

From the repository root:

```sh
build=$(mktemp -d -p /var/tmp nori-git.XXXXXX)
cp dist/arch/nori-git/PKGBUILD "$build/"
cd "$build"
makepkg -si
```

makepkg clones `main` from GitHub; it doesn't build your local checkout.
Building in a copy keeps the build files out of the checkout, and keeps makepkg from rewriting `pkgver=` in the tracked PKGBUILD.
The build needs more than 2 GiB of disk space, so it runs in `/var/tmp`, which is on disk: on Arch, `/tmp` is a tmpfs limited to half of the RAM by default.
Delete the build directory when you're done.

## Change the PKGBUILD

1. Edit `dist/arch/nori-git/PKGBUILD`.
2. Regenerate the metadata: `cd dist/arch/nori-git && makepkg --printsrcinfo > .SRCINFO`.
3. Open a pull request.
   The `AUR package` workflow builds the package from the pull request (GitHub's merge of it into `main`) in an Arch Linux container, checks that `.SRCINFO` is current, checks that the package has no build paths in it, fails on namcap errors, installs the package, and checks that `nori panic` prints file and line numbers.
   It also runs when a pull request changes `resources/`, `README.md`, `src/cli.rs` or the Cargo files, and once a month.

## Publish to the AUR

This needs an AUR account with an SSH key, and the package must be pushed from that account.
Cloning a package name that doesn't exist yet gives an empty repository; the first push creates the package.

Run this from the repository root.
The AUR repository lives next to the checkout, in `../aur-nori-git`; the first time, clone it, and later, update it:

```sh
if [ -d ../aur-nori-git ]; then
  git -C ../aur-nori-git pull --ff-only
else
  git -c init.defaultBranch=master clone ssh://aur@aur.archlinux.org/nori-git.git ../aur-nori-git
fi
cp dist/arch/nori-git/PKGBUILD dist/arch/nori-git/.SRCINFO ../aur-nori-git/
cd ../aur-nori-git
git add PKGBUILD .SRCINFO
git commit -m "Update PKGBUILD"
git push origin HEAD:master
```

When a package has no commits yet, git may not learn its default branch from the AUR, and then the clone uses your `init.defaultBranch`, for example `main`.
That branch tracks a branch the AUR never has, so later pulls fail; `-c init.defaultBranch=master` makes the clone use `master`, the only branch the AUR accepts.

The AUR only accepts pushes to `master`, needs `.SRCINFO` in the pushed commit and `PKGBUILD` in every commit, and rejects subdirectories other than `keys/` and `LICENSES/` (see `aurweb/git/update.py` in [aurweb](https://gitlab.archlinux.org/archlinux/aurweb)).
Push to the AUR only what is merged into `main` here, so the two stay the same.

After the first push, update the places that say the package isn't published: the top of this file, the "PKGBUILD for the AUR" entry in `CHANGES-FROM-NIRI.md`, and the installation note in `README.md`.
