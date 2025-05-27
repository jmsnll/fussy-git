package cmd

import (
	"fmt"
	"os"
	"text/tabwriter" // For aligned output

	"github.com/spf13/cobra"
)

// listCmd represents the list command
var listCmd = &cobra.Command{
	Use:   "list",
	Short: "Lists all repositories managed by fussy-git.",
	Long: `Lists all repositories that have been cloned or added to fussy-git's tracking.
The information is read from the state file (e.g., ~/.fussy-git/repos.json).

Output includes the repository name, its local path, and the current remote URL.`,
	RunE: func(cmd *cobra.Command, args []string) error {
		if verbose {
			fmt.Printf("Listing repositories from state file: %s\n", appConfig.StateFilePath)
		}

		if len(repoState.Repositories) == 0 {
			fmt.Println("No repositories are currently managed by fussy-git.")
			fmt.Printf("Try cloning a repository using: fussy-git clone <repo_url>\n")
			return nil
		}

		// Initialize tabwriter
		// Parameters: output, minwidth, tabwidth, padding, padchar, flags
		w := tabwriter.NewWriter(os.Stdout, 0, 0, 2, ' ', 0)
		defer w.Flush()

		// Print header
		fmt.Fprintln(w, "NAME\tPATH\tCURRENT URL\tORIGINAL URL\tDOMAIN")
		fmt.Fprintln(w, "----\t----\t-----------\t------------\t------")

		for _, repo := range repoState.Repositories {
			fmt.Fprintf(w, "%s\t%s\t%s\t%s\t%s\n",
				repo.Name,
				repo.Path,
				repo.CurrentURL,
				repo.OriginalURL,
				repo.Domain,
			)
		}

		return nil
	},
}

func init() {
	rootCmd.AddCommand(listCmd)
	// Potentially add flags to listCmd in the future, e.g.:
	// listCmd.Flags().BoolP("full-path", "f", false, "Display full paths instead of truncated")
	// listCmd.Flags().StringP("sort-by", "s", "name", "Sort repositories by (name, path, url, domain)")
}
