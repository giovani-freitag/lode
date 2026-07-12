# lode — usability demo (VHS: https://github.com/charmbracelet/vhs)
# Rendered by .github/workflows/demo.yml (or `vhs demo.tape` on Linux with lode on PATH).

Output assets/demo.gif

Require lode

Set Shell bash
Set FontSize 18
Set FontFamily "DejaVu Sans Mono"
Set Width 1280
Set Height 860
Set Padding 20
Set Margin 20
Set MarginFill "#0A0A0A"
Set BorderRadius 12
Set WindowBar Colorful
Set TypingSpeed 55ms

# lode palette: VS Code Dark+ base + the icon's amber accent (#FBBE3C -> #D97706).
Set Theme { "name": "lode", "background": "#1E1E1E", "foreground": "#D4D4D4", "cursor": "#FBBE3C", "selection": "#264F78", "black": "#1E1E1E", "red": "#F14C4C", "green": "#23D18B", "yellow": "#FBBE3C", "blue": "#3B8EEA", "magenta": "#D670D6", "cyan": "#29B8DB", "white": "#D4D4D4", "brightBlack": "#808080", "brightRed": "#F14C4C", "brightGreen": "#23D18B", "brightYellow": "#D97706", "brightBlue": "#3B8EEA", "brightMagenta": "#D670D6", "brightCyan": "#29B8DB", "brightWhite": "#FFFFFF" }

# Fresh pack dir + an amber prompt (the brand color), configured off-camera.
Hide
Type "mkdir -p /tmp/demo && cd /tmp/demo"
Enter
Type `export PS1=$'\[\e[38;2;251;190;60m\]» \[\e[0m\]'`
Enter
Type "clear"
Enter
Show

# Scaffold interactively — the loader and version come from live pickers.
Type "lode init --minecraft 1.20.1"
Enter
Sleep 1.5s
Enter
Sleep 1s
Type "me"
Enter
Sleep 2.5s
Enter
Sleep 3s
Enter
Sleep 3s

# Add a mod — resolved and downloaded.
Type "lode add sodium -y"
Enter
Sleep 3.5s

# Add one with dependencies — they come along automatically.
Type "lode add rei -y"
Enter
Sleep 4.5s

# The resolved pack: direct mods vs. dependencies.
Type "lode list"
Enter
Sleep 3s

# Trace why a dependency is in the pack.
Type "lode why cloth-config"
Enter
Sleep 3.5s
