# RootCX CLI init demo
# Records: rootcx init -> self-host -> scaffold -> deploy

Output .github/demo.gif

Require rootcx
Require docker

Set Shell "zsh"
Set FontSize 16
Set Width 900
Set Height 600
Set Padding 20
Set Theme { "name": "RootCX", "black": "#3a3a4a", "red": "#ff6b7f", "green": "#4ade80", "yellow": "#fbbf24", "blue": "#009EFF", "magenta": "#c084fc", "cyan": "#22d3ee", "white": "#f0f0f5", "brightBlack": "#5a5a6e", "brightRed": "#ff8fa0", "brightGreen": "#6ee7a0", "brightYellow": "#fcd44d", "brightBlue": "#3db8ff", "brightMagenta": "#d4a0ff", "brightCyan": "#5ce8f0", "brightWhite": "#ffffff", "background": "#121218", "foreground": "#e0e0ea", "selection": "#2a2a3a", "cursor": "#009EFF" }
Set TypingSpeed 30ms
Set WindowBar Colorful
Set PlaybackSpeed 1.5

Env HOME "/tmp/rootcx-demo-home"

# Clean app + config, but keep bun cache
Hide
Type "rm -rf /tmp/rootcx-demo /tmp/rootcx-demo-home/.rootcx/config.json && mkdir -p /tmp/rootcx-demo-home /tmp/rootcx-demo"
Enter
Sleep 1s
Type "cd /tmp/rootcx-demo"
Enter
Sleep 500ms
Type "clear"
Enter
Sleep 500ms
Show

Sleep 500ms

Type "rootcx init"
Enter
Sleep 3s

# App name prompt
Type "team_crm"
Enter
Sleep 2s

# "Where do you want to run?" -> Self-host (Docker) is second option
Down
Sleep 200ms
Enter
Sleep 5s

# Email prompt
Type "demo@company.com"
Enter
Sleep 1s

# Password prompt
Type "DemoPass123!"
Enter

# Scaffold + build + deploy (bun cached, fast)
Sleep 12s

# Hold final output
Sleep 5s
