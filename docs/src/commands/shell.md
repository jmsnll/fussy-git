# Shell integration

```text
fussy-git shell-init <bash|zsh|fish>
```

Prints a shell script that defines an `fg` function. Add it to your shell
startup:

```sh
# ~/.bashrc
eval "$(fussy-git shell-init bash)"

# ~/.zshrc
eval "$(fussy-git shell-init zsh)"

# ~/.config/fish/config.fish
fussy-git shell-init fish | source
```

## The `fg` function

```sh
fg fussy         # cd to the single repo matching "fussy"
fg acme/api      # substring of host/owner/repo, or owner/repo
fg               # no argument: fuzzy-pick from all repos
```

With an argument, `fg` calls `fussy-git cd` and `cd`s to the unique match; an
ambiguous or missing match leaves you where you are.

With no argument it fuzzy-picks: through `fzf` if it is installed
(`fussy-git list --path | fzf`), otherwise through the built-in
[`browse`](./browse.md) picker.
