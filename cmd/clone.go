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

// cloneCmd represents the clone command
var cloneCmd = &cobra.Command{
	Use:   "clone <repo_url>",
	Short: "Clones a repository into the fussy-git directory structure.",
	Long: `Clones a Git repository from the given URL.
The repository will be placed in a structured directory:
$FUSSY_GIT_HOME/<domain>/<user_or_org>/<project_name>.

Examples:
  fussy-git clone https://github.com/spf13/cobra.git
  fussy-git clone git@github.com:spf13/cobra.git

This command will:
1. Parse the repository URL.
2. Determine the target directory based on FUSSY_GIT_HOME.
3. Clone the repository into the target directory.
4. Update the local state file (e.g., repos.json) with the repository's information.`,
	Args: cobra.ExactArgs(1), // Requires exactly one argument: the repository URL
	RunE: func(cmd *cobra.Command, args []string) error {
		repoURL := args[0]

		if verbose {
			fmt.Printf("Attempting to clone: %s\n", repoURL)
			fmt.Printf("Using FUSSY_GIT_HOME: %s\n", appConfig.FussyGitHome)
		}

		// 1. Parse the repository URL
		parsedURL, err := gitutil.ParseGitURL(repoURL)
		if err != nil {
			return fmt.Errorf("invalid repository URL '%s': %w", repoURL, err)
		}
		if verbose {
			fmt.Printf("Parsed URL -> Domain: %s, Path: %s, User: %s, RepoName: %s\n",
				parsedURL.Domain, parsedURL.Path, parsedURL.User, parsedURL.RepoName)
		}

		// 2. Determine the target directory
		targetPath := parsedURL.GetLocalPath(appConfig.FussyGitHome)

		if verbose {
			fmt.Printf("Target clone directory: %s\n", targetPath)
		}

		// Check if the repository already exists at the target path or is already tracked
		if existingEntry, found := repoState.FindRepositoryByPath(targetPath); found {
			// Path exists and is tracked. Check if URL matches.
			if existingEntry.OriginalURL == repoURL || existingEntry.CurrentURL == repoURL {
				fmt.Printf("Repository %s already cloned at %s and tracked with a matching URL.\n", parsedURL.RepoName, targetPath)
				return nil // Already exists and matches, do nothing
			}
			// Path exists and is tracked, but with a different URL. This is a conflict.
			return fmt.Errorf("directory %s is already tracked by fussy-git with a different URL (%s). Please remove or reorganize.", targetPath, existingEntry.CurrentURL)
		}

		// Path is not tracked by fussy-git. Check if it exists on disk.
		if _, statErr := os.Stat(targetPath); !os.IsNotExist(statErr) {
			// Directory exists but is not in our state file.
			// It could be an untracked git repo or a non-git directory.
			// For now, we'll error out to prevent accidental overwrites or confusion.
			// A more advanced version could offer to adopt/overwrite if it's a git repo.
			return fmt.Errorf("directory %s already exists on disk but is not tracked by fussy-git. Please remove it or use 'fussy-git add %s' if it's a valid git repository you wish to track from its current location", targetPath, targetPath)
		}

		// 3. Create the parent directory if it doesn't exist
		parentDir := filepath.Dir(targetPath)
		if err := os.MkdirAll(parentDir, 0755); err != nil {
			return fmt.Errorf("failed to create parent directory %s: %w", parentDir, err)
		}
		if verbose {
			fmt.Printf("Ensured parent directory exists: %s\n", parentDir)
		}

		// 4. Clone the repository
		fmt.Printf("Cloning %s into %s...\n", repoURL, targetPath)
		output, err := gitutil.CloneRepository(repoURL, targetPath, verbose)
		if err != nil {
			// CloneRepository already formats the error well, including output.
			return err // No need to wrap further, CloneRepository provides good context.
		}
		fmt.Printf("Successfully cloned %s\n", parsedURL.RepoName)
		if verbose && len(output) > 0 && !strings.Contains(output, "Cloning into") { // Avoid redundant "Cloning into..."
			fmt.Printf("Git clone output:\n%s\n", output)
		}

		// 5. Update the local state file
		newRepoEntry := state.RepositoryEntry{
			Name:         parsedURL.RepoName,
			Path:         targetPath,
			OriginalURL:  repoURL,
			CurrentURL:   repoURL, // Initially, original and current are the same
			Domain:       parsedURL.Domain,
			NormalizedFS: parsedURL.GetNormalizedFSPath(),
			// Timestamps (ClonedAt, LastChecked, LastModified) are set by AddRepository
		}
		err = repoState.AddRepository(newRepoEntry)
		if err != nil {
			// Attempt to clean up the cloned directory if adding to state fails.
			// This is a best-effort cleanup.
			fmt.Fprintf(os.Stderr, "Error: Failed to add repository to state: %v\n", err)
			fmt.Fprintf(os.Stderr, "Attempting to clean up cloned directory: %s\n", targetPath)
			if removeErr := os.RemoveAll(targetPath); removeErr != nil {
				fmt.Fprintf(os.Stderr, "Warning: Failed to clean up directory %s: %v\n", targetPath, removeErr)
			}
			return fmt.Errorf("failed to add repository to state after cloning: %w", err)
		}

		err = repoState.Save(appConfig.StateFilePath)
		if err != nil {
			// At this point, the repo is cloned and state in memory is updated, but saving failed.
			// This is not ideal. The user might need to manually check the state file.
			return fmt.Errorf("repository cloned to %s and state updated in memory, but failed to save state to disk: %w. Please check %s", targetPath, err, appConfig.StateFilePath)
		}

		if verbose {
			fmt.Printf("Repository state updated and saved to %s\n", appConfig.StateFilePath)
		}

		fmt.Printf("Repository %s successfully cloned and tracked by fussy-git.\n", parsedURL.RepoName)
		return nil
	},
}

func init() {
	// rootCmd.AddCommand(cloneCmd) // This is done in cmd/root.go's init()
}
