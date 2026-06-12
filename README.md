# hunkle

hunkle is a simple TUI tool for managing staged changes in a repository.

I you are anything like me, when coding *anything*, you often end up fixing various related and unrelated issues before starting to commit anything. Afterwarde, you then go through your staged hunks again and again to create the final commits. `hunkle` is here to make this experience more nice.

It simply shows you your staged hunks one after another, and you can create the commits to which those should be added to on the fly :)

## Usage

It is mostly self explaining, but here are the keybinds:

| Key | Action |
| --- | --- |
| `n` | define a new commit and assign the hunk to it |
| `1`–`9` | assign the hunk to that commit |
| `0<id>` | assign by commit number: `01` is the same as `1`, commit 10 is `010`, and so on |
| `m` | change the commit order (also reassigns the `1-9` "quick buttons") |
| `v` | pick individual lines in a hunk (`space` toggle, `a` all, then `1`–`9`/`n` to assign) |
| `s` / `h` | skip to next / previous pending hunk |
| `j` / `k` | scroll down/up |
| `u` | unassign all lines of the current hunk |
| `d` / `enter` | go to the review screen |
| `q` | quit |
| `<?>` | probably others I will forget in the future :p |

Binary files are skipped and remain staged.

## Emacs / Magit

`hunkle` is also available as an extension to `emacs`' `magit` (it needs `hunkle` installed). To install:

```elisp
(require 'hunkle)
(hunkle-magit-setup)

; or, if you are using use-package
(use-package hunkle
  :after magit
  :config (hunkle-magit-setup))
```

`hunkle-magit-setup` binds `#` in the magit-status buffer and adds an entry to the magit help-menu.

The keybinds are essentially the same, with a few new ones:

| Key | Action |
| --- | --- |
| `e` | edit a commit message |
| `g` | reload the staged changes |
| `C-c C-c` | create the commits |
| `C-c C-k` | quit without committing (changes stay staged) |

## Nix

This flake provides the rust package both directly and in an overlay, and the emacs package in the overlay only. To use, first add the flake as an input:

```nix
# in your flake.nix
inputs.hunkle.url = "github:Swarsel/hunkle";
```

Then (optionally) you might want to add the overlay:

```nix
# in a module
nixpkgs.overlays = [ inputs.hunkle.overlays.default ];
```

You can now install `hunkle` using `inputs.hunkle.packages.${pkgs.system}.hunkle`, or, if you used the overlay, `pkgs.hunkle`.

## Building

This is where it gets interesting:

```sh
cargo build --release

# or, using nix
nix build
```
