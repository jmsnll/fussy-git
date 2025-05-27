package gitutil

import (
	"fmt"
	"net/url"
	"path/filepath"
	"regexp"
	"strings"
)

// ParsedGitURL holds the components of a parsed Git URL.
type ParsedGitURL struct {
	OriginalURL string // The original URL as provided
	Scheme      string // e.g., "https", "ssh", "git"
	User        string // Username part of the URL (often "git" for SSH, or from https basic auth)
	Host        string // e.g., "github.com"
	Domain      string // Same as Host, but used for directory structure.
	Path        string // Path part of the URL, e.g., "owner/project.git" or "owner/project"
	RepoName    string // The name of the repository, e.g., "project"
	IsSSH       bool   // True if the URL is an SSH URL
}

// scpLikeURLRegex matches SCP-like SSH URLs, e.g., git@github.com:user/repo.git
// It captures:
// 1. User (e.g., "git")
// 2. Host (e.g., "github.com")
// 3. Path (e.g., "user/repo.git")
var scpLikeURLRegex = regexp.MustCompile(`^([a-zA-Z0-9_.-]+)@([a-zA-Z0-9.-]+):(.*)$`)

// ParseGitURL parses a Git repository URL (HTTPS or SSH) into its components.
func ParseGitURL(repoURL string) (*ParsedGitURL, error) {
	parsed := &ParsedGitURL{OriginalURL: repoURL}

	// Attempt to parse as SCP-like SSH URL first (e.g., git@github.com:user/repo.git)
	// This form is not a standard URI and net/url.Parse will misinterpret it.
	if matches := scpLikeURLRegex.FindStringSubmatch(repoURL); len(matches) == 4 {
		parsed.Scheme = "ssh"
		parsed.User = matches[1]
		parsed.Host = matches[2]
		parsed.Domain = parsed.Host // For SSH, host is the domain
		rawPath := matches[3]

		// Normalize path: remove leading slash if present (common in some SCP forms)
		// and remove .git suffix
		parsed.Path = strings.TrimPrefix(rawPath, "/")
		parsed.RepoName = strings.TrimSuffix(filepath.Base(parsed.Path), ".git")
		parsed.IsSSH = true
		return parsed, nil
	}

	// If not SCP-like, try standard URL parsing
	u, err := url.Parse(repoURL)
	if err != nil {
		return nil, fmt.Errorf("could not parse URL '%s': %w", repoURL, err)
	}

	parsed.Scheme = u.Scheme
	parsed.Host = u.Host // For https://user@host/path, Host includes user. We want u.Hostname()
	parsed.Domain = u.Hostname()

	if u.User != nil {
		parsed.User = u.User.Username()
		// Password, if present, is ignored: u.User.Password()
	}

	// Path for HTTP/S includes leading slash, remove it for consistency
	// and remove .git suffix
	parsed.Path = strings.TrimPrefix(u.Path, "/")
	parsed.RepoName = strings.TrimSuffix(filepath.Base(parsed.Path), ".git")

	if parsed.Scheme == "ssh" {
		parsed.IsSSH = true
		// For ssh://user@host/path/to/repo.git, Hostname() is correct.
		// User is from u.User.Username().
	} else if parsed.Scheme == "http" || parsed.Scheme == "https" {
		parsed.IsSSH = false
	} else if parsed.Scheme == "git" { // git://host/path
		parsed.IsSSH = false // Technically different but often handled similarly to https for pathing
	} else if parsed.Scheme == "" && strings.Contains(repoURL, ":") {
		// This could be an implicit SCP-like URL that the regex missed, or a local path.
		// For now, we assume if it got here and has a ':', it's likely an unhandled SCP or invalid.
		// A more robust solution might try to re-evaluate or specifically handle local paths.
		return nil, fmt.Errorf("ambiguous URL format (potentially SCP-like or local path not fully parsed): %s", repoURL)
	} else if parsed.Scheme == "" && !strings.Contains(repoURL, ":") {
		// This is likely a local path, e.g., /path/to/repo or ./repo
		// fussy-git primarily targets remote URLs for its structured organization.
		// For now, we'll treat local paths as needing special handling or being out of scope
		// for the domain/user/project structure.
		// However, to make it somewhat work, we can try to extract a "repo name".
		// The "domain" and "user" would be undefined or set to a placeholder.
		parsed.Scheme = "file" // Treat as local file
		parsed.Path = strings.TrimSuffix(repoURL, ".git")
		parsed.RepoName = strings.TrimSuffix(filepath.Base(parsed.Path), ".git")
		parsed.Domain = "local" // Placeholder domain for local paths
		parsed.User = ""        // No user for local paths in this context
		parsed.IsSSH = false
		// Note: This handling of local paths is basic.
		// A full implementation might require different logic for GetLocalPath.
		// return nil, fmt.Errorf("local file paths are not fully supported for structured cloning: %s", repoURL)
	}

	if parsed.Domain == "" || parsed.RepoName == "" {
		return nil, fmt.Errorf("could not determine domain or repository name from URL: %s (Domain: '%s', RepoName: '%s')", repoURL, parsed.Domain, parsed.RepoName)
	}

	return parsed, nil
}

// GetLocalPath constructs the full local filesystem path for the repository
// based on FUSSY_GIT_HOME, domain, user (if present), and repository path.
// Example:
// FUSSY_GIT_HOME: /home/user/git
// URL: https://github.com/owner/project.git -> /home/user/git/github.com/owner/project
// URL: git@gitlab.com:group/subgroup/project.git -> /home/user/git/gitlab.com/group/subgroup/project
func (pu *ParsedGitURL) GetLocalPath(fussyGitHome string) string {
	// The pu.Path already has .git stripped and leading slashes removed.
	// For github.com/user/repo, pu.Path is "user/repo".
	// For git@custom.com:project/component.git, pu.Path is "project/component".
	// The structure is FUSSY_GIT_HOME / domain / path_segments...
	// We don't explicitly use pu.User here because for many HTTPS URLs, it's not present,
	// and for SSH, it's often 'git'. The hierarchical path comes from pu.Path.
	return filepath.Join(fussyGitHome, pu.Domain, pu.Path)
}

// GetNormalizedFSPath returns a string representation suitable for filesystem paths,
// combining domain and the rest of the path.
// e.g., github.com/user/project
func (pu *ParsedGitURL) GetNormalizedFSPath() string {
	// pu.Path already has .git suffix removed.
	return filepath.Join(pu.Domain, pu.Path)
}

// ToSSH converts a parsed URL to its SSH equivalent if possible.
// Only really makes sense for HTTP/S URLs from common providers.
// Example: https://github.com/user/repo -> git@github.com:user/repo.git
func (pu *ParsedGitURL) ToSSH() (string, error) {
	if pu.IsSSH {
		return pu.OriginalURL, nil // Or reconstruct for normalization
	}
	if pu.Scheme == "https" || pu.Scheme == "http" {
		if pu.Domain == "" || pu.Path == "" {
			return "", fmt.Errorf("cannot convert to SSH: domain or path is empty (Original: %s)", pu.OriginalURL)
		}
		// Ensure .git suffix for SSH representation
		sshPath := pu.Path
		if !strings.HasSuffix(sshPath, ".git") {
			sshPath += ".git"
		}
		// Default SSH user is often 'git'
		sshUser := "git"
		if pu.User != "" && pu.Scheme == "ssh" { // if original was ssh://user@host...
			sshUser = pu.User
		}
		return fmt.Sprintf("%s@%s:%s", sshUser, pu.Domain, sshPath), nil
	}
	return "", fmt.Errorf("cannot convert URL scheme '%s' to SSH (Original: %s)", pu.Scheme, pu.OriginalURL)
}

// ToHTTPS converts a parsed URL to its HTTPS equivalent if possible.
// Example: git@github.com:user/repo.git -> https://github.com/user/repo.git
func (pu *ParsedGitURL) ToHTTPS() (string, error) {
	if !pu.IsSSH && (pu.Scheme == "https" || pu.Scheme == "http") {
		return pu.OriginalURL, nil // Or reconstruct for normalization
	}
	if pu.Scheme == "ssh" || pu.Scheme == "git" { // Treat 'git://' scheme as convertible too
		if pu.Domain == "" || pu.Path == "" {
			return "", fmt.Errorf("cannot convert to HTTPS: domain or path is empty (Original: %s)", pu.OriginalURL)
		}
		// Path for HTTPS should not have .git suffix for cleaner URLs, though git handles both
		httpsPath := strings.TrimSuffix(pu.Path, ".git")
		return fmt.Sprintf("https://%s/%s", pu.Domain, httpsPath), nil
	}
	return "", fmt.Errorf("cannot convert URL scheme '%s' to HTTPS (Original: %s)", pu.Scheme, pu.OriginalURL)
}
