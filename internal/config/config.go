package config

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/spf13/viper"
	// It's generally better to use os.MkdirAll which respects umask by default.
	// If specific umask manipulation is absolutely needed, ensure it's cross-platform or conditional.
	// For now, direct umask manipulation is removed for simplicity as os.MkdirAll is usually sufficient.
	// "golang.org/x/sys/unix"
)

const (
	defaultFussyGitDirName = "git"             // Default directory name under home for repositories
	configDirName          = ".fussy-git"      // Directory name for config and state files under home
	stateFileName          = "repos.json"      // Name of the state file
	defaultConfigFileType  = "yaml"            // Default config file type
	defaultConfigFileName  = "config"          // Default config file name (e.g. config.yaml)
	envFussyGitHome        = "FUSSY_GIT_HOME"  // Environment variable for FUSSY_GIT_HOME
	configKeyFussyGitHome  = "fussy_git_home"  // Key in config file for FUSSY_GIT_HOME
	configKeyStateFilePath = "state_file_path" // Key in config file for state file path (can be overridden)

	// Constants for help messages in Cobra (exported)
	// These need to be Exported (start with uppercase) to be accessible by other packages.
	ConfigDirNameForHelp         = configDirName
	DefaultConfigNameForHelp     = defaultConfigFileName
	DefaultConfigFileTypeForHelp = defaultConfigFileType
)

// Config stores the application's configuration.
type Config struct {
	FussyGitHome  string // Base directory where git repositories will be cloned.
	StateFilePath string // Path to the JSON file storing repository state.
	ConfigFile    string // Path to the config file used.
}

// LoadConfig loads the application configuration.
// It prioritizes:
// 1. Explicitly passed configFile path (from --config flag).
// 2. Environment variable FUSSY_GIT_HOME.
// 3. Configuration file (~/.fussy-git/config.yaml).
// 4. Default values.
func LoadConfig(configFileFromFlag string) (*Config, error) {
	cfg := &Config{}

	// Determine user's home directory
	homeDir, err := os.UserHomeDir()
	if err != nil {
		return nil, fmt.Errorf("failed to get user home directory: %w", err)
	}

	// Set up Viper
	v := viper.New()

	// --- Configure FUSSY_GIT_HOME ---
	// Default FUSSY_GIT_HOME
	defaultGitHomePath := filepath.Join(homeDir, defaultFussyGitDirName)
	v.SetDefault(configKeyFussyGitHome, defaultGitHomePath)

	// --- Configure State File Path ---
	// Default state file path
	defaultConfigDirPath := filepath.Join(homeDir, configDirName)
	defaultStateFilePath := filepath.Join(defaultConfigDirPath, stateFileName)
	v.SetDefault(configKeyStateFilePath, defaultStateFilePath)

	// --- Configure Config File ---
	// This logic is primarily for viper to find and read a config file.
	// The actual `cfg.ConfigFile` field should reflect what was loaded or attempted.
	if configFileFromFlag != "" {
		v.SetConfigFile(configFileFromFlag) // Tell Viper to use this specific file
		cfg.ConfigFile = configFileFromFlag
	} else {
		// Search config in default config directory with name "config" (e.g., config.yaml).
		v.AddConfigPath(defaultConfigDirPath)
		v.SetConfigName(defaultConfigFileName)
		v.SetConfigType(defaultConfigFileType) // e.g. "yaml"
		// Store the path viper will attempt to use for user feedback
		cfg.ConfigFile = filepath.Join(defaultConfigDirPath, defaultConfigFileName+"."+defaultConfigFileType)
	}

	// Bind environment variables AFTER setting defaults and config paths
	// This allows env vars to override file/defaults.
	// Viper's BindEnv needs to be told about the keys it should look for.
	v.SetEnvPrefix("FUSSY_GIT") // Optional: FUSSY_GIT_FUSSY_GIT_HOME, FUSSY_GIT_STATE_FILE_PATH
	v.AutomaticEnv()            // Reads all env vars that match the prefix or keys

	// Explicitly bind specific env vars to viper keys for clarity and control.
	// This ensures that FUSSY_GIT_HOME overrides fussy_git_home from a config file.
	if err := v.BindEnv(configKeyFussyGitHome, envFussyGitHome); err != nil {
		return nil, fmt.Errorf("failed to bind env var %s: %w", envFussyGitHome, err)
	}
	if err := v.BindEnv(configKeyStateFilePath, "FUSSY_GIT_STATE_FILE_PATH"); err != nil { // Example for state path env var
		return nil, fmt.Errorf("failed to bind env var FUSSY_GIT_STATE_FILE_PATH: %w", err)
	}

	// Attempt to read the config file.
	// It's not an error if the config file doesn't exist and no specific file was passed,
	// defaults will be used.
	if err := v.ReadInConfig(); err != nil {
		if _, ok := err.(viper.ConfigFileNotFoundError); ok {
			// Config file not found. This is okay if no specific file was required by flag.
			// If configFileFromFlag was set, this means that specific file wasn't found.
			// The verbose message in root.go's initConfig handles this feedback.
		} else {
			// A different error occurred while reading the config file (e.g., permissions, malformed)
			return nil, fmt.Errorf("error reading config file %s: %w", v.ConfigFileUsed(), err)
		}
	}

	// Populate Config struct from Viper (which now has values from defaults, file, or env)
	cfg.FussyGitHome = v.GetString(configKeyFussyGitHome)
	cfg.StateFilePath = v.GetString(configKeyStateFilePath)

	// Ensure FUSSY_GIT_HOME directory exists
	if err := ensureDirExists(cfg.FussyGitHome, 0755); err != nil {
		return nil, fmt.Errorf("failed to ensure FUSSY_GIT_HOME directory %s exists: %w", cfg.FussyGitHome, err)
	}

	// Ensure the directory for the state file exists
	stateFileDir := filepath.Dir(cfg.StateFilePath)
	if err := ensureDirExists(stateFileDir, 0700); err != nil { // More restrictive for config dir
		return nil, fmt.Errorf("failed to ensure state file directory %s exists: %w", stateFileDir, err)
	}

	return cfg, nil
}

// ensureDirExists checks if a directory exists, and if not, creates it with the given permissions.
// os.MkdirAll respects the system's umask by default.
func ensureDirExists(path string, perm os.FileMode) error {
	if _, err := os.Stat(path); os.IsNotExist(err) {
		if mkdirErr := os.MkdirAll(path, perm); mkdirErr != nil {
			return fmt.Errorf("failed to create directory %s: %w", path, mkdirErr)
		}
	} else if err != nil {
		// Path exists but there was another error stating it (e.g. permission denied to stat)
		return fmt.Errorf("failed to stat directory %s: %w", path, err)
	}
	// If path exists and is a directory, os.Stat returns nil error, all good.
	// If path exists and is NOT a directory, os.MkdirAll will return an error.
	return nil
}

// GetDefaultFussyGitHome returns the default FUSSY_GIT_HOME path.
func GetDefaultFussyGitHome() (string, error) {
	homeDir, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("could not get user home directory: %w", err)
	}
	return filepath.Join(homeDir, defaultFussyGitDirName), nil
}

// GetDefaultStateFilePath returns the default path for the repos.json state file.
func GetDefaultStateFilePath() (string, error) {
	homeDir, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("could not get user home directory: %w", err)
	}
	return filepath.Join(homeDir, configDirName, stateFileName), nil
}
