# lode — usability demo (VHS: https://github.com/charmbracelet/vhs)
# Rendered by .github/workflows/demo.yml (or `vhs demo.tape` on Linux with lode on PATH).

Output assets/demo.gif

Require lode

Set Shell bash
Set FontSize 16
Set FontFamily "DejaVu Sans Mono"
Set Width 1000
Set Height 600
Set Padding 18
Set Margin 16
Set MarginFill "#0A0A0A"
Set BorderRadius 10
Set WindowBar Colorful
Set TypingSpeed 60ms

# lode palette: VS Code Dark+ base + the icon's amber accent (#FBBE3C -> #D97706).
Set Theme { "name": "lode", "background": "#1E1E1E", "foreground": "#D4D4D4", "cursor": "#FBBE3C", "selection": "#264F78", "black": "#1E1E1E", "red": "#F14C4C", "green": "#23D18B", "yellow": "#FBBE3C", "blue": "#3B8EEA", "magenta": "#D670D6", "cyan": "#29B8DB", "white": "#D4D4D4", "brightBlack": "#808080", "brightRed": "#F14C4C", "brightGreen": "#23D18B", "brightYellow": "#D97706", "brightBlue": "#3B8EEA", "brightMagenta": "#D670D6", "brightCyan": "#29B8DB", "brightWhite": "#FFFFFF" }

# Fresh pack dir, a git identity (so `init` can pre-fill the author), and an amber prompt
# (the brand color) — all configured off-camera.
Hide
Type "mkdir -p /tmp/demo && cd /tmp/demo"
Enter
Type "git config --global user.name 'Giovani Freitag'"
Enter
Type `export PS1=$'\[\e[38;2;251;190;60m\]» \[\e[0m\]'`
Enter
Type "clear"
Enter
Show

# Scaffold interactively — the author pre-fills from `git config user.name` (just Enter to accept),
# then pick a NON-default loader and filter the version list by typing.
Type "lode init"
Enter
Sleep 1.5s
Enter
Sleep 1.2s
Enter
Sleep 1.8s
Down@650ms 2
Sleep 1.4s
Enter
Sleep 2.6s
Type "1.20.1"
Sleep 1.7s
Enter
Sleep 2.6s
Enter
Sleep 2.4s

# Add a mod — resolved, its dependencies pulled in, and downloaded.
Type "lode add rei"
Enter
Sleep 4s

# The resolved pack: direct mod vs. its dependencies.
Type "lode list"
Enter
Sleep 3s
