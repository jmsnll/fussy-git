package cmd

import (
	"fmt"
	"github.com/jmsnll/fussy-git/internal/gitutil"
	"github.com/jmsnll/fussy-git/internal/state"
	"os"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"
)

var dryRunReorg bool

// reorganizeCmd represents the reorganize command
var reorganizeCmd = &cobra.Command{
	Use:   "reorganize",
	Short: "Reorganizes repositories to their conventional paths and updates remote URLs.",
	Long: `Scans all managed repositories and performs the following actions:
1. Checks if the local repository path exists and is a valid Git repository.
2. Fetches the current remote 'origin' URL from the local repository.
3. If the live 'origin' URL differs from the 'CurrentURL' stored in fussy-git's state,
   the state will be updated (unless --dry-run is active).
4. Calculates the conventional filesystem path for the repository based on its
   (potentially updated) 'origin' URL and your FUSSY_GIT_HOME.
5. If the repository's actual local path differs from this conventional path,
   it will be moved to the conventional path, and fussy-git's state will be updated
   (unless --dry-run is active).

Use --dry-run to see what changes would be made without applying them.`,
	RunE: func(cmd *cobra.Command, args []string) error {
		if verbose {
			fmt.Println("Starting repository reorganization process...")
			if dryRunReorg {
				fmt.Println("DRY RUN active: No changes will be made to the filesystem or state file.")
			}
			fmt.Printf("State file: %s\n", appConfig.StateFilePath)
			fmt.Printf("FUSSY_GIT_HOME: %s\n", appConfig.FussyGitHome)
		}

		if len(repoState.Repositories) == 0 {
			fmt.Println("No repositories are currently managed by fussy-git. Nothing to reorganize.")
			return nil
		}

		fmt.Printf("Found %d repositories to check for reorganization.\n\n", len(repoState.Repositories))

		var modifiedEntries []state.RepositoryEntry
		stateModified := false
		actionsTaken := 0
		actionsProposed := 0

		originalRepositories := make([]state.RepositoryEntry, len(repoState.Repositories))
		copy(originalRepositories, repoState.Repositories)

		// Create a new slice for updated repositories to avoid modifying while iterating
		updatedRepositories := make([]state.RepositoryEntry, 0, len(repoState.Repositories))

		for _, repoEntry := range originalRepositories {
			currentRepo := repoEntry // Make a mutable copy for this iteration
			fmt.Printf("Processing: %s (Path: %s)\n", currentRepo.Name, currentRepo.Path)
			actionLog := []string{} // Log actions for this specific repo

			// --- Basic Health Checks ---
			if _, err := os.Stat(currentRepo.Path); os.IsNotExist(err) {
				actionLog = append(actionLog, fmt.Sprintf("  [SKIP] Path does not exist: %s. Consider removing from state.", currentRepo.Path))
				fmt.Println(strings.Join(actionLog, "\n"))
				updatedRepositories = append(updatedRepositories, currentRepo) // Keep original entry if skipped
				continue
			} else if err != nil {
				actionLog = append(actionLog, fmt.Sprintf("  [SKIP] Error accessing path %s: %v. Manual check required.", currentRepo.Path, err))
				fmt.Println(strings.Join(actionLog, "\n"))
				updatedRepositories = append(updatedRepositories, currentRepo)
				continue
			}

			if !gitutil.IsGitRepository(currentRepo.Path) {
				actionLog = append(actionLog, fmt.Sprintf("  [SKIP] Path is not a Git repository: %s. Manual check required.", currentRepo.Path))
				fmt.Println(strings.Join(actionLog, "\n"))
				updatedRepositories = append(updatedRepositories, currentRepo)
				continue
			}

			// --- URL Check and Update ---
			liveOriginURL, err := gitutil.GetRemoteOriginURL(currentRepo.Path, verbose)
			if err != nil {
				actionLog = append(actionLog, fmt.Sprintf("  [WARN] Failed to get live origin URL: %v. Skipping URL and path checks for this repo.", err))
				fmt.Println(strings.Join(actionLog, "\n"))
				updatedRepositories = append(updatedRepositories, currentRepo)
				continue
			}

			parsedLiveURL, errLiveParse := gitutil.ParseGitURL(liveOriginURL)
			if errLiveParse != nil {
				actionLog = append(actionLog, fmt.Sprintf("  [WARN] Failed to parse live origin URL '%s': %v. Skipping URL and path checks.", liveOriginURL, errLiveParse))
				fmt.Println(strings.Join(actionLog, "\n"))
				updatedRepositories = append(updatedRepositories, currentRepo)
				continue
			}

			parsedStoredURL, _ := gitutil.ParseGitURL(currentRepo.CurrentURL) // Error handled by checking if nil later

			// Compare normalized URLs (e.g. HTTPS vs SSH)
			liveHTTPS, _ := parsedLiveURL.ToHTTPS()
			storedHTTPS := ""
			if parsedStoredURL != nil {
				storedHTTPS, _ = parsedStoredURL.ToHTTPS()
			}

			if parsedStoredURL == nil || liveHTTPS != storedHTTPS {
				oldURL := currentRepo.CurrentURL
				actionLog = append(actionLog, fmt.Sprintf("  Remote URL changed: Was '%s', now '%s'", oldURL, liveOriginURL))
				actionsProposed++
				if !dryRunReorg {
					currentRepo.CurrentURL = liveOriginURL
					// If OriginalURL was the same as the old CurrentURL, update it too,
					// assuming the "original" intent was to track this remote.
					if currentRepo.OriginalURL == oldURL {
						currentRepo.OriginalURL = liveOriginURL
						actionLog = append(actionLog, fmt.Sprintf("    Also updated OriginalURL to '%s'", liveOriginURL))
					}
					stateModified = true
					actionsTaken++
				}
			}

			// --- Path Reorganization Check ---
			// Use the live (and potentially updated in `currentRepo.CurrentURL`) URL for conventional path
			finalParsedURLForPath, _ := gitutil.ParseGitURL(currentRepo.CurrentURL)
			if finalParsedURLForPath == nil {
				actionLog = append(actionLog, fmt.Sprintf("  [WARN] Cannot determine conventional path due to unparsable CurrentURL '%s'.", currentRepo.CurrentURL))
				fmt.Println(strings.Join(actionLog, "\n"))
				updatedRepositories = append(updatedRepositories, currentRepo)
				continue
			}

			conventionalPath := finalParsedURLForPath.GetLocalPath(appConfig.FussyGitHome)
			normalizedActualPath := strings.TrimRight(filepath.Clean(currentRepo.Path), string(filepath.Separator))
			normalizedConventionalPath := strings.TrimRight(filepath.Clean(conventionalPath), string(filepath.Separator))

			if normalizedActualPath != normalizedConventionalPath {
				actionLog = append(actionLog, fmt.Sprintf("  Path mismatch: Actual '%s', Conventional '%s'", currentRepo.Path, conventionalPath))
				actionsProposed++

				if !dryRunReorg {
					// Pre-move safety checks
					if _, err := os.Stat(conventionalPath); !os.IsNotExist(err) {
						// Target path exists. This is a conflict.
						actionLog = append(actionLog, fmt.Sprintf("  [FAIL] Conventional path '%s' already exists. Cannot move. Manual intervention required.", conventionalPath))
						// Do not proceed with move for this repo
					} else {
						// Ensure parent directory of conventionalPath exists
						parentConventionalDir := filepath.Dir(conventionalPath)
						if err := os.MkdirAll(parentConventionalDir, 0755); err != nil {
							actionLog = append(actionLog, fmt.Sprintf("  [FAIL] Failed to create parent directory '%s' for move: %v", parentConventionalDir, err))
						} else {
							actionLog = append(actionLog, fmt.Sprintf("  Moving repository from '%s' to '%s'...", currentRepo.Path, conventionalPath))
							if err := os.Rename(currentRepo.Path, conventionalPath); err != nil {
								actionLog = append(actionLog, fmt.Sprintf("  [FAIL] Failed to move repository: %v", err))
							} else {
								actionLog = append(actionLog, "    Move successful.")
								currentRepo.Path = conventionalPath
								stateModified = true
								actionsTaken++
							}
						}
					}
				}
			}

			// Update name if it was derived from the old path/URL and the URL changed significantly
			if currentRepo.Name != finalParsedURLForPath.RepoName {
				oldName := currentRepo.Name
				currentRepo.Name = finalParsedURLForPath.RepoName
				actionLog = append(actionLog, fmt.Sprintf("  Repository name updated from '%s' to '%s' based on new URL.", oldName, currentRepo.Name))
				if !dryRunReorg {
					stateModified = true
					// This doesn't count as a separate "action taken" if URL/path already changed.
				}
			}

			if len(actionLog) > 0 {
				fmt.Println(strings.Join(actionLog, "\n"))
			} else {
				fmt.Println("  No issues or changes needed.")
			}
			fmt.Println("---")
			updatedRepositories = append(updatedRepositories, currentRepo)
			if stateModified && !dryRunReorg { // If any modification happened to this repo's entry
				modifiedEntries = append(modifiedEntries, currentRepo)
			}
		}

		// Replace the old repoState.Repositories with the updated ones
		repoState.Repositories = updatedRepositories

		if stateModified && !dryRunReorg {
			fmt.Println("\nSaving updated state to file...")
			if err := repoState.Save(appConfig.StateFilePath); err != nil {
				fmt.Fprintf(os.Stderr, "Error: Failed to save updated state: %v\n", err)
				fmt.Println("Please check the state file manually:", appConfig.StateFilePath)
				return fmt.Errorf("failed to save state after reorganization: %w", err)
			}
			fmt.Println("State saved successfully.")
		} else if dryRunReorg && actionsProposed > 0 {
			fmt.Println("\nDRY RUN summary: The above changes would be made.")
		} else if !stateModified && actionsProposed == 0 {
			fmt.Println("\nNo changes were necessary. All repositories are organized.")
		}

		fmt.Printf("\nReorganization summary:\n")
		if dryRunReorg {
			fmt.Printf("  Actions proposed: %d\n", actionsProposed)
		} else {
			fmt.Printf("  Actions taken:    %d\n", actionsTaken)
		}
		return nil
	},
}

func init() {
	rootCmd.AddCommand(reorganizeCmd)
	reorganizeCmd.Flags().BoolVar(&dryRunReorg, "dry-run", false, "Show what changes would be made without actually applying them")
}
