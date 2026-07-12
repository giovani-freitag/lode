# lode — usability demo (VHS: https://github.com/charmbracelet/vhs)
# Rendered by .github/workflows/demo.yml (or `vhs demo.tape` on Linux with lode on PATH).

Output assets/demo.gif

Require lode

Set Shell bash
Set FontSize 18
Set Width 1200
Set Height 800
Set Padding 24
Set TypingSpeed 55ms

# lode palette: VS Code Dark+ base + the icon's amber accent (#FBBE3C -> #D97706).
Set Theme { "name": "lode", "background": "#1E1E1E", "foreground": "#D4D4D4", "cursor": "#FBBE3C", "selection": "#264F78", "black": "#1E1E1E", "red": "#F14C4C", "green": "#23D18B", "yellow": "#FBBE3C", "blue": "#3B8EEA", "magenta": "#D670D6", "cyan": "#29B8DB", "white": "#D4D4D4", "brightBlack": "#808080", "brightRed": "#F14C4C", "brightGreen": "#23D18B", "brightYellow": "#D97706", "brightBlue": "#3B8EEA", "brightMagenta": "#D670D6", "brightCyan": "#29B8DB", "brightWhite": "#FFFFFF" }

# An amber prompt (the brand color), configured off-camera.
Hide
Type `export PS1=$'\[\e[38;2;251;190;60m\]❯ \[\e[0m\]'`
Enter
Type "clear"
Enter
Show

Type "lode init -y --loader fabric --minecraft 1.20.1 --loader-version 0.15.11 --name demo --author me"
Enter
Sleep 2.5s

Type "lode add create -y"
Enter
Sleep 4s

Type "lode add rei -y"
Enter
Sleep 4s

Type "lode list"
Enter
Sleep 3s

Type "lode why cloth-config"
Enter
Sleep 3.5s
