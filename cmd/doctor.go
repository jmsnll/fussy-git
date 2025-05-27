package cmd

import (
	"fmt"
	"github.com/jmsnll/fussy-git/internal/gitutil"
	"os"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"
)

// doctorCmd represents the doctor command
var doctorCmd = &cobra.Command{
	Use:   "doctor",
	Short: "Checks the health and consistency of fussy-git managed repositories.",
	Long: `The doctor command inspects all repositories tracked by fussy-git and reports any issues.
Checks performed include:
- Existence of the repository path on the filesystem.
- Whether the path is a valid Git repository.
- Consistency of the current remote 'origin' URL with the stored state.
- Whether the repository is in its conventional fussy-git location.

This command is read-only and does not make any changes.`,
	RunE: func(cmd *cobra.Command, args []string) error {
		if verbose {
			fmt.Printf("Running fussy-git doctor...\n")
			fmt.Printf("State file: %s\n", appConfig.StateFilePath)
			fmt.Printf("FUSSY_GIT_HOME: %s\n", appConfig.FussyGitHome)
		}

		if len(repoState.Repositories) == 0 {
			fmt.Println("No repositories are currently managed by fussy-git. Nothing to check.")
			return nil
		}

		fmt.Printf("Found %d repositories to check.\n\n", len(repoState.Repositories))

		issuesFound := 0
		reposOk := 0

		for i, repo := range repoState.Repositories {
			fmt.Printf("Checking repository #%d: %s (Path: %s)\n", i+1, repo.Name, repo.Path)
			var repoIssues []string

			// 1. Check if path exists
			if _, err := os.Stat(repo.Path); os.IsNotExist(err) {
				repoIssues = append(repoIssues, fmt.Sprintf("Path does not exist: %s", repo.Path))
			} else if err != nil {
				repoIssues = append(repoIssues, fmt.Sprintf("Error accessing path %s: %v", repo.Path, err))
			} else {
				// Path exists, proceed with more checks

				// 2. Check if it's a Git repository
				if !gitutil.IsGitRepository(repo.Path) {
					repoIssues = append(repoIssues, fmt.Sprintf("Path is not a Git repository: %s", repo.Path))
				} else {
					// It's a Git repository

					// 3. Check remote origin URL consistency
					currentLiveOriginURL, err := gitutil.GetRemoteOriginURL(repo.Path, verbose)
					if err != nil {
						repoIssues = append(repoIssues, fmt.Sprintf("Failed to get live origin URL: %v", err))
					} else {
						// Normalize both URLs for comparison (e.g. SSH vs HTTPS)
						parsedStoredURL, errStored := gitutil.ParseGitURL(repo.CurrentURL)
						parsedLiveURL, errLive := gitutil.ParseGitURL(currentLiveOriginURL)

						if errStored != nil {
							repoIssues = append(repoIssues, fmt.Sprintf("Could not parse stored CurrentURL '%s': %v", repo.CurrentURL, errStored))
						}
						if errLive != nil {
							repoIssues = append(repoIssues, fmt.Sprintf("Could not parse live origin URL '%s': %v", currentLiveOriginURL, errLive))
						}

						if errStored == nil && errLive == nil {
							// Compare based on normalized HTTPS versions for robustness
							storedHTTPS, _ := parsedStoredURL.ToHTTPS()
							liveHTTPS, _ := parsedLiveURL.ToHTTPS()

							if storedHTTPS != liveHTTPS {
								repoIssues = append(repoIssues,
									fmt.Sprintf("Remote URL mismatch: Stored: '%s', Live: '%s'", repo.CurrentURL, currentLiveOriginURL))
							}
						} else if repo.CurrentURL != currentLiveOriginURL { // Fallback to direct string comparison if parsing failed for one
							repoIssues = append(repoIssues,
								fmt.Sprintf("Remote URL mismatch (direct string): Stored: '%s', Live: '%s'", repo.CurrentURL, currentLiveOriginURL))
						}

						// 4. Check conventional path
						// Use the live URL for determining conventional path, as it's the most current.
						// If live URL parsing failed, this check might be less reliable or skipped.
						if parsedLiveURL != nil {
							conventionalPath := parsedLiveURL.GetLocalPath(appConfig.FussyGitHome)
							normalizedActualPath := strings.TrimRight(filepath.Clean(repo.Path), string(filepath.Separator))
							normalizedConventionalPath := strings.TrimRight(filepath.Clean(conventionalPath), string(filepath.Separator))

							if normalizedActualPath != normalizedConventionalPath {
								// Only flag as a major issue if not manually added to a custom path,
								// or if it's a significant deviation.
								// For now, just note it.
								msg := fmt.Sprintf("Not in conventional location. Actual: '%s', Expected: '%s'", repo.Path, conventionalPath)
								if repo.ManuallyAdded && verbose { // Less critical if manually added, more of an FYI
									msg += " (Note: Repository was manually added)"
								}
								repoIssues = append(repoIssues, msg)
							}
						}
					}
				}
			}

			if len(repoIssues) > 0 {
				issuesFound++
				fmt.Println("  Status: ISSUES FOUND")
				for _, issue := range repoIssues {
					fmt.Printf("    - %s\n", issue)
				}
			} else {
				reposOk++
				fmt.Println("  Status: OK")
			}
			fmt.Println("---") // Separator for readability
		}

		fmt.Printf("\nDoctor summary:\n")
		fmt.Printf("  Repositories checked: %d\n", len(repoState.Repositories))
		fmt.Printf("  Repositories OK:      %d\n", reposOk)
		fmt.Printf("  Repositories with issues: %d\n", issuesFound)

		if issuesFound > 0 {
			fmt.Println("\nPlease review the issues listed above.")
			// Suggest commands to fix, e.g., 'fussy-git reorganize' or manual intervention.
			return fmt.Errorf("%d repositories reported issues", issuesFound) // Return an error to indicate non-zero exit status
		}

		fmt.Println("All checks passed. Your fussy-git setup looks healthy!")
		return nil
	},
}

func init() {
	rootCmd.AddCommand(doctorCmd)
	// Potential flags for doctorCmd:
	// doctorCmd.Flags().BoolP("fix", "f", false, "Attempt to automatically fix some common issues (use with caution)")
}
