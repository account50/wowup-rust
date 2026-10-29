-- WowUp.rs launcher (GPL-3.0-or-later)
-- Opens the terminal UI in Terminal.app. Also receives wowup:// and curseforge:// links,
-- which macOS delivers as Apple Events rather than command line arguments.

on run
	launchTUI("")
end run

on open location theURL
	launchTUI(theURL)
end open location

on launchTUI(theArg)
	set bin to POSIX path of (path to me) & "Contents/Resources/bin/wowup-tui"
	set cmd to "clear; exec " & quoted form of bin
	if theArg is not "" then set cmd to cmd & " " & quoted form of theArg
	tell application "Terminal"
		activate
		do script cmd
	end tell
end launchTUI
