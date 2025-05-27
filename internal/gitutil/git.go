package gitutil

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// CloneRepository executes 'git clone' command.
// It returns the combined stdout/stderr output and an error if any.
func CloneRepository(repoURL, targetPath string, verbose bool) (string, error) {
	if verbose {
		fmt.Printf("Executing: git clone %s %s\n", repoURL, targetPath)
	}

	cmd := exec.Command("git", "clone", repoURL, targetPath)

	// Capture stdout and stderr for more detailed error reporting or verbose output
	var outb, errb bytes.Buffer
	cmd.Stdout = &outb
	cmd.Stderr = &errb

	// Set NO_PROMPT=1 or GIT_TERMINAL_PROMPT=0 to prevent git from asking for credentials interactively
	// This is important for a CLI tool that should be scriptable.
	// Users should configure credential helpers or use SSH keys.
	cmd.Env = append(os.Environ(), "GIT_TERMINAL_PROMPT=0")

	err := cmd.Run()

	stdOutput := outb.String()
	stdError := errb.String()
	combinedOutput := stdOutput + stdError

	if err != nil {
		// Provide more context in the error message
		errMsg := fmt.Sprintf("git clone failed for %s into %s", repoURL, targetPath)
		if exitErr, ok := err.(*exec.ExitError); ok {
			errMsg = fmt.Sprintf("%s (exit code %d)", errMsg, exitErr.ExitCode())
		}
		// It's useful to return the combined output even on error.
		return combinedOutput, fmt.Errorf("%s: %w. Output:\n%s", errMsg, err, combinedOutput)
	}

	if verbose && len(combinedOutput) > 0 {
		fmt.Printf("Git clone output:\n%s\n", combinedOutput)
	}
	return combinedOutput, nil
}

// GetRemoteOriginURL fetches the URL of the "origin" remote for a repository at a given path.
func GetRemoteOriginURL(repoPath string, verbose bool) (string, error) {
	if verbose {
		fmt.Printf("Executing: git -C %s remote get-url origin\n", repoPath)
	}
	cmd := exec.Command("git", "-C", repoPath, "remote", "get-url", "origin")

	var outb, errb bytes.Buffer
	cmd.Stdout = &outb
	cmd.Stderr = &errb
	cmd.Env = append(os.Environ(), "GIT_TERMINAL_PROMPT=0")

	err := cmd.Run()
	stdError := errb.String()

	if err != nil {
		errMsg := fmt.Sprintf("failed to get remote origin URL for %s", repoPath)
		if exitErr, ok := err.(*exec.ExitError); ok {
			errMsg = fmt.Sprintf("%s (exit code %d)", errMsg, exitErr.ExitCode())
		}
		// It's useful to return the combined output even on error.
		return "", fmt.Errorf("%s: %w. Stderr:\n%s", errMsg, err, stdError)
	}

	// `git remote get-url origin` output includes a newline.
	originURL := strings.TrimSpace(outb.String())

	if originURL == "" {
		return "", fmt.Errorf("origin URL is empty for repository at %s. Stderr: %s", repoPath, stdError)
	}

	if verbose {
		fmt.Printf("Found remote origin URL: %s for repo: %s\n", originURL, repoPath)
	}

	return originURL, nil
}

// SetRemoteOriginURL sets the URL of the "origin" remote for a repository.
func SetRemoteOriginURL(repoPath, newURL string, verbose bool) (string, error) {
	if verbose {
		fmt.Printf("Executing: git -C %s remote set-url origin %s\n", repoPath, newURL)
	}
	cmd := exec.Command("git", "-C", repoPath, "remote", "set-url", "origin", newURL)

	var outb, errb bytes.Buffer
	cmd.Stdout = &outb
	cmd.Stderr = &errb
	cmd.Env = append(os.Environ(), "GIT_TERMINAL_PROMPT=0")

	err := cmd.Run()
	stdOutput := outb.String()
	stdError := errb.String()
	combinedOutput := stdOutput + stdError

	if err != nil {
		errMsg := fmt.Sprintf("failed to set remote origin URL for %s to %s", repoPath, newURL)
		if exitErr, ok := err.(*exec.ExitError); ok {
			errMsg = fmt.Sprintf("%s (exit code %d)", errMsg, exitErr.ExitCode())
		}
		return combinedOutput, fmt.Errorf("%s: %w. Output:\n%s", errMsg, err, combinedOutput)
	}
	if verbose {
		fmt.Printf("Successfully set remote origin for %s to %s\n", repoPath, newURL)
	}
	return combinedOutput, nil
}

// IsGitRepository checks if the given path is a Git repository
// by looking for a .git directory or running `git rev-parse --is-inside-work-tree`.
func IsGitRepository(path string) bool {
	// Option 1: Check for .git directory (faster for simple cases)
	gitDir := filepath.Join(path, ".git")
	if stat, err := os.Stat(gitDir); err == nil && stat.IsDir() {
		return true
	}

	// Option 2: Use git command (more robust, handles worktrees, etc.)
	cmd := exec.Command("git", "-C", path, "rev-parse", "--is-inside-work-tree")
	err := cmd.Run()  // We only care about the exit status
	return err == nil // Exit code 0 means it's a git repo
}
