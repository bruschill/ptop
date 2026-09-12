Output demo.gif

Set Shell "bash"
Set FontSize 24
Set Width 2200
Set Height 1200
Set Padding 10
Set Theme "Catppuccin Mocha"
Set Framerate 30

Hide
# --demo now follows the default Pi monitor mode and is collector-free.
Type "../target/debug/ptop --demo"
Enter
Sleep 1s
Show

# The Pi fixture starts with storefront selected. It has attached telemetry,
# a known token rate, a listening port, and a healthy fleet run with a child.
Sleep 3s

# Move through the other attached Pi sessions. Their inferred and estimated
# context precision makes the telemetry state visible without private content.
Type "j"
Sleep 2s
Type "j"
Sleep 2s

# Return to the fleet session before quitting.
Type "k"
Sleep 1s
Type "k"
Sleep 2s

Hide
Type "q"
