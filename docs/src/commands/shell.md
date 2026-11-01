# Shell integration

```text
fussy-git shell-init <bash|zsh|fish> [--cmd <NAME>]
```

Prints a shell script that defines a jump function, `fzg` by default. Add it to
your shell startup:

```sh
# ~/.bashrc
eval "$(fussy-git shell-init bash)"

# ~/.zshrc
eval "$(fussy-git shell-init zsh)"

# ~/.config/fish/config.fish
fussy-git shell-init fish | source
```

## The `fzg` function

```sh
fzg fussy            # cd to the single repo matching "fussy"
fzg acme/api         # substring of host/owner/repo, or owner/repo
fzg api@feat-auth    # cd to the checkout of a branch (main clone or worktree)
fzg                  # no argument: fuzzy-pick from all repos
```

With an argument, `fzg` calls [`fussy-git cd`](./list.md#cd) and changes to the
unique match. If the match is ambiguous or missing, you stay where you are.

With no argument it opens a fuzzy picker: `fzf` if it's installed
(`fussy-git list --path | fzf`), otherwise the built-in [`browse`](./browse.md)
picker.

## Choosing the name

`--cmd` picks a different name:

```sh
eval "$(fussy-git shell-init zsh --cmd j)"   # defines `j` instead
```

Names that would shadow a shell builtin or a common command (`cd`, `fg`, `ls`,
`git`, …) are rejected.

Before 0.4.0 the function was called `fg`, which hid the shell's own `fg`
(resume a stopped job). If you have `fg` muscle memory, use `--cmd` with a
name of your choice. `fg` itself isn't allowed.
