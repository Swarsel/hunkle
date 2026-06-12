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

## Building

This is where it gets interesting:

```sh
cargo build --release
```
