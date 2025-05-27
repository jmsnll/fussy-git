# Makefile for the fussy-git project

# Go parameters
GOCMD=go
GOBUILD=$(GOCMD) build
GOTEST=$(GOCMD) test
GOFMT=$(GOCMD) fmt
GORUN=$(GOCMD) run
GOCLEAN=$(GOCMD) clean
GOGET=$(GOCMD) get

# Target binary name
BINARY_NAME=fussy-git
# Output directory for the binary
OUTPUT_DIR_LINUX=./bin/linux
OUTPUT_DIR_DARWIN=./bin/darwin
OUTPUT_DIR_WINDOWS=./bin/windows

# Main Go package (location of main.go)
MAIN_PACKAGE=./main.go

# All Go packages in the project for formatting and testing
GO_PACKAGES=$(shell $(GOCMD) list ./... | grep -v /vendor/)

# Default target executed when no arguments are given to make.
.PHONY: all
all: build

# Build the application for the current OS/ARCH
.PHONY: build
build:
	@echo "Building $(BINARY_NAME) for $(shell go env GOOS)/$(shell go env GOARCH)..."
	$(GOBUILD) -o $(BINARY_NAME) $(MAIN_PACKAGE)
	@echo "$(BINARY_NAME) built successfully."

# Build for specific platforms
.PHONY: build-linux
build-linux:
	@echo "Building $(BINARY_NAME) for linux/amd64..."
	@mkdir -p $(OUTPUT_DIR_LINUX)
	GOOS=linux GOARCH=amd64 $(GOBUILD) -o $(OUTPUT_DIR_LINUX)/$(BINARY_NAME) $(MAIN_PACKAGE)
	@echo "$(BINARY_NAME) for linux/amd64 built in $(OUTPUT_DIR_LINUX)/"

.PHONY: build-darwin
build-darwin:
	@echo "Building $(BINARY_NAME) for darwin/amd64..."
	@mkdir -p $(OUTPUT_DIR_DARWIN)
	GOOS=darwin GOARCH=amd64 $(GOBUILD) -o $(OUTPUT_DIR_DARWIN)/$(BINARY_NAME) $(MAIN_PACKAGE)
	@echo "$(BINARY_NAME) for darwin/amd64 built in $(OUTPUT_DIR_DARWIN)/"
	@echo "Building $(BINARY_NAME) for darwin/arm64..."
	GOOS=darwin GOARCH=arm64 $(GOBUILD) -o $(OUTPUT_DIR_DARWIN)/$(BINARY_NAME)-arm64 $(MAIN_PACKAGE)
	@echo "$(BINARY_NAME) for darwin/arm64 built in $(OUTPUT_DIR_DARWIN)/"


.PHONY: build-windows
build-windows:
	@echo "Building $(BINARY_NAME) for windows/amd64..."
	@mkdir -p $(OUTPUT_DIR_WINDOWS)
	GOOS=windows GOARCH=amd64 $(GOBUILD) -o $(OUTPUT_DIR_WINDOWS)/$(BINARY_NAME).exe $(MAIN_PACKAGE)
	@echo "$(BINARY_NAME) for windows/amd64 built in $(OUTPUT_DIR_WINDOWS)/"

# Run tests
.PHONY: test
test:
	@echo "Running tests..."
	$(GOTEST) -v $(GO_PACKAGES)

# Run go fmt on all Go files
.PHONY: fmt
fmt:
	@echo "Formatting Go files..."
	$(GOFMT) $(GO_PACKAGES)

# Run the application
# Pass arguments to the application using `make run ARGS="--verbose clone <url>"`
.PHONY: run
run: build
	@echo "Running $(BINARY_NAME)..."
	./$(BINARY_NAME) $(ARGS)

# Clean build artifacts
.PHONY: clean
clean:
	@echo "Cleaning up..."
	$(GOCLEAN)
	rm -f $(BINARY_NAME)
	rm -rf ./bin
	@echo "Cleanup complete."

# Get dependencies
.PHONY: deps
deps:
	@echo "Getting dependencies..."
	$(GOGET) -v ./...

# List available targets
.PHONY: help
help:
	@echo "Available targets:"
	@echo "  all           - Build the application (default)"
	@echo "  build         - Build the application for the current OS/ARCH"
	@echo "  build-linux   - Build the application for linux/amd64"
	@echo "  build-darwin  - Build the application for darwin/amd64 and darwin/arm64"
	@echo "  build-windows - Build the application for windows/amd64"
	@echo "  test          - Run tests"
	@echo "  fmt           - Format Go source files"
	@echo "  run           - Build and run the application (pass ARGS, e.g., make run ARGS=\"clone --help\")"
	@echo "  clean         - Remove build artifacts"
	@echo "  deps          - Fetch dependencies"
	@echo "  help          - Show this help message"

