package cmd

import (
	"fmt"
	"github.com/jmsnll/fussy-git/internal/config"
	"github.com/jmsnll/fussy-git/internal/state"
	"os"
	"os/exec"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"
	"github.com/spf13/viper"
)

var (
	cfgFile    string
	verbose    bool
	appConfig  *config.Config
	repoState  *state.RepoState
	AppVersion string // Populated by main.go from ldflags
	AppCommit  string // Populated by main.go from ldflags
	AppDate    string // Populated by main.go from ldflags
	AppBuiltBy string // Populated by main.go from ldflags
)

// rootCmd represents the base command when called without any subcommands
// It's also responsible for handling passthrough git commands.
var rootCmd = &cobra.Command{
	Use:   "fussy-git",
	Short: "fussy-git helps you keep your cloned git repositories organized.",
	Long: `fussy-git is a CLI tool to manage your local git repositories
by cloning them into a structured directory based on their origin URL.
It can also act as a proxy to the real 'git' command for unsupported operations.

Default FUSSY_GIT_HOME is ~/git.`,
	PersistentPreRunE: func(cmd *cobra.Command, args []string) error {
		// Initialize config
		var err error
		appConfig, err = config.LoadConfig(cfgFile)
		if err != nil {
			return fmt.Errorf("failed to load configuration: %w", err)
		}
		if verbose {
			fmt.Printf("Using FUSSY_GIT_HOME: %s\n", appConfig.FussyGitHome)
			fmt.Printf("Using state file: %s\n", appConfig.StateFilePath)
		}

		// Initialize state
		repoState, err = state.LoadState(appConfig.StateFilePath)
		if err != nil {
			return fmt.Errorf("failed to load repository state: %w", err)
		}
		if verbose {
			fmt.Printf("Loaded %d repositories from state file: %s\n", len(repoState.Repositories), appConfig.StateFilePath)
		}
		return nil
	},
	// This is the core of the passthrough logic.
	RunE: func(cmd *cobra.Command, args []string) error {
		// If no arguments are provided to fussy-git itself, and it's not a version request, show help.
		// Cobra handles --version automatically if rootCmd.Version is set.
		if len(args) == 0 && cmd.Flags().Lookup("version") != nil && !cmd.Flags().Lookup("version").Changed {
			return cmd.Help()
		}

		// If args are present, they were not parsed by a known subcommand.
		// Assume it's a passthrough git command.
		if len(args) > 0 {
			gitCmd := args[0]
			gitArgs := args[1:]

			if verbose {
				fmt.Printf("Passthrough: attempting to execute 'git %s' with args %v\n", gitCmd, gitArgs)
			}
			return executeGitPassthrough(gitCmd, gitArgs...)
		}
		// If no args and not asking for version, show help (already handled by Cobra's default if no Run/RunE)
		// but since we have RunE, we explicitly call Help.
		return cmd.Help()

	},
	SilenceErrors: true,
	SilenceUsage:  true, // Don't show usage on error for passthrough commands or if help is shown.
}

// Execute adds all child commands to the root command and sets flags appropriately.
// This is called by main.main(). It only needs to happen once to the rootCmd.
func Execute(appVersion, appCommit, appDate, appBuiltBy string) error {
	AppVersion = appVersion
	AppCommit = appCommit
	AppDate = appDate
	AppBuiltBy = appBuiltBy

	if AppVersion == "dev" && AppCommit == "none" { // If not set by ldflags
		rootCmd.Version = "dev-snapshot (manual build)"
	} else {
		rootCmd.Version = fmt.Sprintf("%s (commit: %s, built: %s, by: %s)", AppVersion, AppCommit, AppDate, AppBuiltBy)
	}
	return rootCmd.Execute()
}

func init() {
	cobra.OnInitialize(initConfig)

	rootCmd.PersistentFlags().StringVar(&cfgFile, "config", "", fmt.Sprintf("config file (default is $HOME/%s/%s.yaml)", config.ConfigDirNameForHelp, config.DefaultConfigNameForHelp))
	rootCmd.PersistentFlags().BoolVarP(&verbose, "verbose", "v", false, "verbose output")

	// Add known fussy-git commands here
	rootCmd.AddCommand(cloneCmd)
	rootCmd.AddCommand(listCmd)
	rootCmd.AddCommand(addCmd)
	rootCmd.AddCommand(doctorCmd)
	rootCmd.AddCommand(reorganizeCmd)
	// Add other fussy-git specific commands here

	// TraversalChildren enables passthrough for commands not explicitly defined.
	rootCmd.TraverseChildren = true
}

// initConfig reads in config file and ENV variables if set.
func initConfig() {
	// This function is called by cobra.OnInitialize()
	// It uses viper to load configuration.
	// config.LoadConfig in PersistentPreRunE handles the actual loading logic using viper.
	// This initConfig here is more for viper's global setup if needed,
	// but our current design centralizes loading in LoadConfig.
	// We can keep it minimal or rely on LoadConfig entirely.

	// For FUSSY_GIT_HOME environment variable binding, it's handled in config.LoadConfig.
	// Viper's automatic env var binding can be set up here if we want viper to manage more.
	viper.SetEnvPrefix("FUSSY_GIT") // e.g. FUSSY_GIT_HOME, FUSSY_GIT_STATE_FILE_PATH
	viper.AutomaticEnv()

	// If a config file is specified via flag, viper will use it.
	// Otherwise, config.LoadConfig searches default locations.
	if cfgFile != "" {
		viper.SetConfigFile(cfgFile)
	} else {
		home, err := os.UserHomeDir()
		if err == nil { // Only proceed if home dir is found
			defaultConfigPath := filepath.Join(home, config.ConfigDirNameForHelp) // Using constants from config package
			viper.AddConfigPath(defaultConfigPath)
			viper.SetConfigName(config.DefaultConfigNameForHelp)
			viper.SetConfigType(config.DefaultConfigFileTypeForHelp)
		}
	}

	// It's fine if the config file doesn't exist, defaults will be used.
	if err := viper.ReadInConfig(); err != nil {
		if _, ok := err.(viper.ConfigFileNotFoundError); ok {
			if verbose && cfgFile != "" { // Only print if a specific config file was expected but not found
				fmt.Fprintf(os.Stderr, "Info: Config file '%s' not found. Using defaults/env vars.\n", cfgFile)
			} else if verbose && cfgFile == "" {
				// fmt.Fprintf(os.Stderr, "Info: No default config file found. Using defaults/env vars.\n")
			}
		} else if cfgFile != "" { // An actual error occurred with the specified config file
			fmt.Fprintf(os.Stderr, "Error reading specified config file '%s': %v\n", cfgFile, err)
		}
	} else {
		if verbose {
			fmt.Fprintln(os.Stderr, "Using config file:", viper.ConfigFileUsed())
		}
	}
}

// executeGitPassthrough attempts to run a git command.
func executeGitPassthrough(command string, args ...string) error {
	cwd, err := os.Getwd()
	if err != nil {
		return fmt.Errorf("failed to get current working directory: %w", err)
	}

	var repoDir string
	// Check if CWD is within a known fussy-git managed repository
	if repoState != nil { // repoState might not be initialized if PersistentPreRunE failed
		for _, repo := range repoState.Repositories {
			// Check if cwd is repo.Path or a subdirectory of repo.Path
			rel, err := filepath.Rel(repo.Path, cwd)
			if err == nil && !strings.HasPrefix(rel, "..") {
				repoDir = repo.Path
				if verbose {
					fmt.Printf("Executing git command in context of known fussy-git repo: %s (CWD: %s)\n", repoDir, cwd)
				}
				break
			}
		}
	}

	// If not in a known fussy-git repo context, try to find .git upwards from CWD
	if repoDir == "" {
		gitTopLevel, err := findGitRepoRoot(cwd)
		if err == nil && gitTopLevel != "" {
			repoDir = gitTopLevel
			if verbose {
				fmt.Printf("Executing git command in context of discovered git repo: %s (CWD: %s)\n", repoDir, cwd)
			}
		} else {
			// Fallback: execute in current working directory
			repoDir = cwd
			if verbose {
				fmt.Printf("Executing git command in current working directory: %s (not a known fussy-git repo or .git dir not found upwards)\n", repoDir)
			}
		}
	}

	gitCommand := exec.Command("git", append([]string{command}, args...)...)
	gitCommand.Dir = repoDir
	gitCommand.Stdout = os.Stdout
	gitCommand.Stderr = os.Stderr
	gitCommand.Stdin = os.Stdin

	if verbose {
		fmt.Printf("Executing: git %s %s (in %s)\n", command, strings.Join(args, " "), gitCommand.Dir)
	}

	err = gitCommand.Run()
	if err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			// Propagate the exit code from the git command
			// This requires main to os.Exit with this code.
			// For now, we just return an error that includes the exit code.
			// os.Exit(exitErr.ExitCode()) // This would terminate immediately.
			return fmt.Errorf("git command '%s' exited with code %d", command, exitErr.ExitCode())
		}
		return fmt.Errorf("failed to execute git command '%s': %w", command, err)
	}
	return nil
}

// findGitRepoRoot tries to find the root of a git repository by looking for a .git directory
// starting from 'startPath' and going upwards.
func findGitRepoRoot(startPath string) (string, error) {
	currentPath, err := filepath.Abs(startPath)
	if err != nil {
		return "", fmt.Errorf("failed to get absolute path for %s: %w", startPath, err)
	}

	for {
		gitDir := filepath.Join(currentPath, ".git")
		stat, err := os.Stat(gitDir)
		if err == nil && stat.IsDir() {
			return currentPath, nil // Found .git directory
		}
		// Stop if we encounter an error other than "not exist" or if we reach root.
		if err != nil && !os.IsNotExist(err) {
			return "", fmt.Errorf("error stating .git directory at %s: %w", gitDir, err)
		}

		parentPath := filepath.Dir(currentPath)
		if parentPath == currentPath { // Reached the root of the filesystem
			break
		}
		currentPath = parentPath
	}
	return "", fmt.Errorf(".git directory not found upwards from %s", startPath)
}
