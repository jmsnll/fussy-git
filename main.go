package main

import (
	"github.com/jmsnll/fussy-git/cmd" // Assuming cmd is your package for cobra commands
	"os"
)

// These variables are set via ldflags by GoReleaser or your Makefile
var (
	version = "dev" // Default value if not built with ldflags
	commit  = "none"
	date    = "unknown"
	builtBy = "unknown"
)

// main is the entry point of the fussy-git application.
func main() {
	// Pass the version information to the command execution logic.
	// The cmd.Execute function in cmd/root.go will use these to set rootCmd.Version.
	if err := cmd.Execute(version, commit, date, builtBy); err != nil {
		// Cobra's Execute() often prints errors to stderr itself.
		// Exiting with 1 indicates failure.
		os.Exit(1)
	}
}
