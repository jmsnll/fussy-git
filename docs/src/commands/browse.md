# browse

```text
fussy-git browse
```

A full-screen fuzzy repository picker. Type to filter, arrow keys to move,
Enter to select, Esc to quit.

On selection it prints the chosen path on stdout and exits `0`. If you quit
without choosing it prints nothing, so the [shell `fg` function](./shell.md)
stays where it is.

```sh
cd "$(fussy-git browse)"
```

The `fg` shell function calls this when you give it no argument.
