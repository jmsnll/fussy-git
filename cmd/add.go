package cmd

import (
	"fmt"
	"github.com/jmsnll/fussy-git/internal/gitutil"
	"github.com/jmsnll/fussy-git/internal/state"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"
)

// addCmd represents the add command
var addCmd = &cobra.Command{
	Use:   "add <path_to_repo>",
	Short: "Adds an existing local Git repository to fussy-git's management.",
	Long: `Adds a Git repository that already exists on your local filesystem
to be tracked by fussy-git.

The command will:
1. Verify the given path is a Git repository.
2. Fetch its remote 'origin' URL.
3. Parse the URL to determine its components.
4. Add the repository information to fussy-git's state file.

If the repository is not located in the path fussy-git would conventionally use
(i.e., $FUSSY_GIT_HOME/<domain>/<user_or_org>/<project_name>), a warning will be displayed.
The 'reorganize' command (not yet implemented) could later move such repositories.`,
	Args: cobra.ExactArgs(1), // Requires exactly one argument: the path to the repository
	RunE: func(cmd *cobra.Command, args []string) error {
		repoPathArg := args[0]

		if verbose {
			fmt.Printf("Attempting to add repository at path: %s\n", repoPathArg)
		}

		// 1. Clean and absolutize the path
		absRepoPath, err := filepath.Abs(repoPathArg)
		if err != nil {
			return fmt.Errorf("failed to get absolute path for '%s': %w", repoPathArg, err)
		}
		if verbose {
			fmt.Printf("Absolute path to repository: %s\n", absRepoPath)
		}

		// 2. Verify it's a Git repository
		if !gitutil.IsGitRepository(absRepoPath) {
			return fmt.Errorf("path '%s' is not a valid Git repository", absRepoPath)
		}
		if verbose {
			fmt.Printf("Path '%s' confirmed as a Git repository.\n", absRepoPath)
		}

		// Check if already tracked
		if existingEntry, found := repoState.FindRepositoryByPath(absRepoPath); found {
			fmt.Printf("Repository at '%s' is already managed by fussy-git (Name: %s, URL: %s).\n", absRepoPath, existingEntry.Name, existingEntry.CurrentURL)
			return nil // Already tracked, nothing to do.
		}

		// 3. Fetch its remote origin URL
		originURL, err := gitutil.GetRemoteOriginURL(absRepoPath, verbose)
		if err != nil {
			return fmt.Errorf("failed to get remote origin URL for repository at '%s': %w. Ensure 'origin' remote is set", absRepoPath, err)
		}
		if originURL == "" {
			return fmt.Errorf("remote 'origin' URL is empty for repository at '%s'", absRepoPath)
		}
		if verbose {
			fmt.Printf("Found remote origin URL: %s\n", originURL)
		}

		// 4. Parse this URL
		parsedURL, err := gitutil.ParseGitURL(originURL)
		if err != nil {
			return fmt.Errorf("failed to parse remote origin URL '%s': %w", originURL, err)
		}
		if verbose {
			fmt.Printf("Parsed URL -> Domain: %s, Path: %s, User: %s, RepoName: %s\n",
				parsedURL.Domain, parsedURL.Path, parsedURL.User, parsedURL.RepoName)
		}

		// 5. Determine the conventional path fussy-git would use
		conventionalPath := parsedURL.GetLocalPath(appConfig.FussyGitHome)
		if verbose {
			fmt.Printf("Conventional fussy-git path for this repo: %s\n", conventionalPath)
		}

		// Warn if the current path is not the conventional one
		// Normalize paths for comparison
		normalizedAbsRepoPath := strings.TrimRight(filepath.Clean(absRepoPath), string(filepath.Separator))
		normalizedConventionalPath := strings.TrimRight(filepath.Clean(conventionalPath), string(filepath.Separator))

		if normalizedAbsRepoPath != normalizedConventionalPath {
			fmt.Printf("Warning: Repository at '%s' is not in the conventional fussy-git location.\n", absRepoPath)
			fmt.Printf("         Conventional location for URL '%s' would be: '%s'\n", originURL, conventionalPath)
			fmt.Println("         You can use the 'fussy-git reorganize' command (when implemented) to move it.")
		}

		// 6. Add the repository information to the state file
		newEntry := state.RepositoryEntry{
			Name:          parsedURL.RepoName,
			Path:          absRepoPath, // Use the actual current path
			OriginalURL:   originURL,   // The fetched origin URL is the "original" in this context
			CurrentURL:    originURL,   // Assume current is same as origin for a newly added repo
			Domain:        parsedURL.Domain,
			NormalizedFS:  parsedURL.GetNormalizedFSPath(),
			ManuallyAdded: true, // Mark as manually added
		}

		if err := repoState.AddRepository(newEntry); err != nil {
			return fmt.Errorf("failed to add repository to state: %w", err)
		}

		if err := repoState.Save(appConfig.StateFilePath); err != nil {
			return fmt.Errorf("repository information for '%s' processed, but failed to save state: %w", absRepoPath, err)
		}

		fmt.Printf("Successfully added repository '%s' (from %s) to fussy-git management.\n", parsedURL.RepoName, absRepoPath)
		if verbose {
			fmt.Printf("State file updated: %s\n", appConfig.StateFilePath)
		}

		return nil
	},
}

func init() {
	rootCmd.AddCommand(addCmd)
	// No specific flags for 'add' command yet.
}
