package state

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// RepositoryEntry represents a single repository tracked by fussy-git.
type RepositoryEntry struct {
	Name          string    `json:"name"`           // Short name of the repository (e.g., "cobra")
	Path          string    `json:"path"`           // Full local path to the repository
	OriginalURL   string    `json:"original_url"`   // The URL used when initially cloned
	CurrentURL    string    `json:"current_url"`    // The current origin URL (might change if remote changes)
	Domain        string    `json:"domain"`         // Domain of the repository (e.g., "github.com")
	NormalizedFS  string    `json:"normalized_fs"`  // Normalized path used for filesystem structure (e.g., github.com/user/repo)
	LastChecked   time.Time `json:"last_checked"`   // Timestamp of when the repo origin was last checked
	LastModified  time.Time `json:"last_modified"`  // Timestamp of when this entry was last modified
	ClonedAt      time.Time `json:"cloned_at"`      // Timestamp of when the repo was cloned
	ManuallyAdded bool      `json:"manually_added"` // True if this entry was added via a command other than clone (e.g. 'fussy-git add')
	Notes         string    `json:"notes"`          // Any user-added notes for this repository
}

// RepoState holds the collection of all tracked repositories.
type RepoState struct {
	Repositories []RepositoryEntry `json:"repositories"`
	filePath     string
	mu           sync.RWMutex // For thread-safe access to Repositories
}

// NewRepoState creates an empty RepoState, primarily for initialization.
func NewRepoState(filePath string) *RepoState {
	return &RepoState{
		Repositories: []RepositoryEntry{},
		filePath:     filePath,
	}
}

// LoadState loads the repository state from the given JSON file.
// If the file doesn't exist, it returns an empty state without error.
func LoadState(filePath string) (*RepoState, error) {
	rs := NewRepoState(filePath)

	rs.mu.Lock()
	defer rs.mu.Unlock()

	// Check if the state file exists
	if _, err := os.Stat(filePath); os.IsNotExist(err) {
		// File doesn't exist, return empty state. This is not an error.
		// Ensure the directory exists for future saves.
		dir := filepath.Dir(filePath)
		if dirErr := os.MkdirAll(dir, 0755); dirErr != nil {
			return nil, fmt.Errorf("failed to create directory for state file %s: %w", dir, dirErr)
		}
		// Attempt to save an empty state file to ensure writability
		if saveErr := rs.saveLocked(); saveErr != nil {
			return nil, fmt.Errorf("failed to create initial empty state file at %s: %w", filePath, saveErr)
		}
		return rs, nil
	} else if err != nil {
		// Some other error occurred when stating the file
		return nil, fmt.Errorf("error checking state file %s: %w", filePath, err)
	}

	// File exists, try to read and unmarshal it
	file, err := os.Open(filePath)
	if err != nil {
		return nil, fmt.Errorf("failed to open state file %s: %w", filePath, err)
	}
	defer file.Close()

	data, err := io.ReadAll(file)
	if err != nil {
		return nil, fmt.Errorf("failed to read state file %s: %w", filePath, err)
	}

	// If the file is empty, don't try to unmarshal
	if len(data) == 0 {
		return rs, nil // Return empty state
	}

	if err := json.Unmarshal(data, &rs); err != nil {
		// Check for specific unmarshal errors, e.g. if the file is not JSON
		// but contains some other data.
		if _, ok := err.(*json.SyntaxError); ok {
			return nil, fmt.Errorf("state file %s contains invalid JSON: %w. Consider backing it up and deleting it to start fresh", filePath, err)
		}
		return nil, fmt.Errorf("failed to unmarshal state file %s: %w", filePath, err)
	}

	return rs, nil
}

// Save writes the current repository state to the JSON file.
func (rs *RepoState) Save(customFilePath ...string) error {
	rs.mu.Lock()
	defer rs.mu.Unlock()
	return rs.saveLocked(customFilePath...)
}

// saveLocked is the internal implementation of Save, assuming the lock is held.
func (rs *RepoState) saveLocked(customFilePath ...string) error {
	filePathToUse := rs.filePath
	if len(customFilePath) > 0 && customFilePath[0] != "" {
		filePathToUse = customFilePath[0]
	}

	if filePathToUse == "" {
		return fmt.Errorf("cannot save state: file path is not set")
	}

	// Ensure the directory for the state file exists
	dir := filepath.Dir(filePathToUse)
	if err := os.MkdirAll(dir, 0755); err != nil { // 0755 for directory
		return fmt.Errorf("failed to create directory for state file %s: %w", dir, err)
	}

	data, err := json.MarshalIndent(rs, "", "  ") // Pretty print JSON
	if err != nil {
		return fmt.Errorf("failed to marshal state to JSON: %w", err)
	}

	// Write to a temporary file first, then rename. This makes the save atomic.
	tempFilePath := filePathToUse + ".tmp"
	err = os.WriteFile(tempFilePath, data, 0644) // 0644 for file permissions
	if err != nil {
		return fmt.Errorf("failed to write state to temporary file %s: %w", tempFilePath, err)
	}

	// Rename temporary file to actual state file
	err = os.Rename(tempFilePath, filePathToUse)
	if err != nil {
		// Attempt to clean up temp file if rename fails
		_ = os.Remove(tempFilePath)
		return fmt.Errorf("failed to rename temporary state file %s to %s: %w", tempFilePath, filePathToUse, err)
	}

	return nil
}

// AddRepository adds a new repository to the state or updates an existing one.
// It checks for duplicates based on the repository path.
func (rs *RepoState) AddRepository(entry RepositoryEntry) error {
	rs.mu.Lock()
	defer rs.mu.Unlock()

	if entry.Path == "" {
		return fmt.Errorf("cannot add repository: path is empty")
	}
	if entry.OriginalURL == "" {
		return fmt.Errorf("cannot add repository '%s': original URL is empty", entry.Name)
	}

	now := time.Now()
	entry.LastModified = now
	if entry.ClonedAt.IsZero() { // Set ClonedAt if not already set (e.g. during add command)
		entry.ClonedAt = now
	}
	if entry.LastChecked.IsZero() {
		entry.LastChecked = now
	}

	for i, r := range rs.Repositories {
		if r.Path == entry.Path {
			// Repository with this path already exists, update it.
			// Preserve some fields like ClonedAt and OriginalURL unless explicitly changed.
			if entry.OriginalURL == "" { // If new entry doesn't specify original URL, keep old one
				entry.OriginalURL = r.OriginalURL
			}
			if entry.ClonedAt.IsZero() {
				entry.ClonedAt = r.ClonedAt
			}
			rs.Repositories[i] = entry
			return nil
		}
		// Also check for duplicate by original URL to prevent adding the same repo twice
		// if it was somehow cloned to a different path (should be rare with fussy-git logic)
		if r.OriginalURL == entry.OriginalURL && r.Path != entry.Path {
			// This case is a bit tricky. It implies the same repo exists in two places.
			// For now, we'll allow it but a more robust system might flag this.
		}
	}

	// If not found, add as a new entry
	rs.Repositories = append(rs.Repositories, entry)
	return nil
}

// FindRepositoryByPath searches for a repository by its full local path.
func (rs *RepoState) FindRepositoryByPath(path string) (*RepositoryEntry, bool) {
	rs.mu.RLock()
	defer rs.mu.RUnlock()

	for _, r := range rs.Repositories {
		if r.Path == path {
			return &r, true
		}
	}
	return nil, false
}

// FindRepositoryByOriginalURL searches for a repository by its original clone URL.
func (rs *RepoState) FindRepositoryByOriginalURL(originalURL string) (*RepositoryEntry, bool) {
	rs.mu.RLock()
	defer rs.mu.RUnlock()

	for _, r := range rs.Repositories {
		if r.OriginalURL == originalURL {
			return &r, true
		}
	}
	return nil, false
}

// RemoveRepositoryByPath removes a repository from the state by its path.
func (rs *RepoState) RemoveRepositoryByPath(path string) bool {
	rs.mu.Lock()
	defer rs.mu.Unlock()

	for i, r := range rs.Repositories {
		if r.Path == path {
			rs.Repositories = append(rs.Repositories[:i], rs.Repositories[i+1:]...)
			return true
		}
	}
	return false
}

// UpdateRepository updates an existing repository entry.
// It finds the repository by its current path and updates its fields.
func (rs *RepoState) UpdateRepository(updatedEntry RepositoryEntry) error {
	rs.mu.Lock()
	defer rs.mu.Unlock()

	if updatedEntry.Path == "" {
		return fmt.Errorf("cannot update repository: path is empty in updated entry")
	}

	found := false
	for i, r := range rs.Repositories {
		if r.Path == updatedEntry.Path {
			// Preserve ClonedAt and OriginalURL if not explicitly set in updatedEntry
			if updatedEntry.ClonedAt.IsZero() {
				updatedEntry.ClonedAt = r.ClonedAt
			}
			if updatedEntry.OriginalURL == "" {
				updatedEntry.OriginalURL = r.OriginalURL
			}
			updatedEntry.LastModified = time.Now()
			rs.Repositories[i] = updatedEntry
			found = true
			break
		}
	}

	if !found {
		return fmt.Errorf("repository with path %s not found in state, cannot update", updatedEntry.Path)
	}
	return nil
}
