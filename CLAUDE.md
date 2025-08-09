# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Samply is a command line CPU profiler that uses the Firefox Profiler as its UI. It's written in Rust and works across macOS, Linux, and Windows platforms. The profiler uses platform-specific sampling techniques to collect stack traces and performance data.

## Workspace Structure

This is a Cargo workspace with multiple crates organized by functionality:

### Main Crates
- **samply/** - Main CLI application and profiler implementation
- **fxprof-processed-profile/** - Firefox Profiler format library for creating profiles
- **wholesym/** - Symbol resolution library for debugging information
- **samply-symbols/** - Symbol handling utilities
- **samply-api/** - JSON API server for symbolication services

### Platform-Specific Code
- **samply/src/mac/** - macOS profiling using mach ports and system APIs
- **samply/src/linux/** - Linux profiling using perf events
- **samply/src/windows/** - Windows profiling using ETW (Event Tracing for Windows)
- **samply/src/linux_shared/** - Shared utilities between Linux and Android

### Supporting Libraries
- **samply-quota-manager/** - File cache quota management
- **geckoProfile/** - Gecko-specific profile format utilities
- **etw-reader/** - Windows ETW event parsing (Windows-only)
- **samply-mac-preload/** - macOS dynamic library for process injection

## Key Build Commands

```sh
# Build the entire workspace
cargo build --workspace

# Build in release mode with debug info (recommended for profiling)
cargo build --profile profiling --workspace

# Build only the main samply binary
cargo build -p samply

# Run tests across all crates
cargo test --workspace

# Check code without building
cargo check --workspace
```

## Core Architecture

### Main Application Flow (`samply/src/main.rs`)
1. **Record** - Profile a running process or launch new process with profiling
2. **Load** - Load an existing profile file and serve it via web interface
3. **Import** - Convert external formats (perf.data, ETL files) to Firefox Profiler format

### Platform-Specific Profilers
Each platform implements a `profiler` module with a `run()` function that:
- Sets up platform-specific sampling mechanisms
- Collects stack traces at regular intervals (default 1000Hz)
- Converts raw profiling data to Firefox Profiler format
- Returns a `Profile` object and exit status

### Symbol Resolution Architecture
- **wholesym** provides the main symbol resolution API
- **samply-symbols** handles different debug formats (DWARF, PDB, Breakpad)
- **samply-api** exposes symbolication as JSON web API compatible with Tecken
- Supports multiple symbol sources: local files, symbol servers, debuginfod

### Web Server (`samply/src/server.rs`)
- Serves the Firefox Profiler UI at localhost
- Provides symbolication API endpoints (`/symbolicate/v5`, `/source/v1`, `/asm/v1`)
- Handles profile loading and symbol resolution on-demand
- Auto-opens browser to profiler.firefox.com with local data

## API Compatibility

The symbolication API implements the same JSON interface as Mozilla's Tecken service, enabling compatibility with the Firefox Profiler frontend. Key endpoints:

- `/symbolicate/v5` - Convert addresses to symbols/filenames/line numbers
- `/source/v1` - Retrieve source code files referenced in debug info
- `/asm/v1` - Disassemble machine code for given address ranges

## Development Workflow

### Testing Profiles
Use the `fixtures/` directory which contains test binaries and profile data for different platforms and scenarios.

### Platform Development
- macOS: Run `samply setup` to codesign binary for process attachment
- Linux: May need `sudo sysctl kernel.perf_event_paranoid=1` for perf access
- Windows: Use `samply record -a` for system-wide profiling

### Profile Creation
The `ProfileCreationProps` struct controls profile generation settings including:
- Sampling interval
- Thread filtering
- Symbol resolution options
- Profile compression

## Special Considerations

### Platform Limitations
- macOS: Cannot profile signed system binaries due to SIP restrictions
- Linux: Requires perf_event access or CAP_PERFMON capability
- Windows: Elevated privileges may be required for some system profiling

### Symbol Management
- Symbols are cached in platform-specific directories
- Quota management prevents unbounded cache growth
- External symbol servers (Microsoft, Mozilla, Chrome) supported
- Local debug info takes precedence over remote sources

### Profile Format
Profiles use the Firefox Profiler "Processed Profile Format" which includes:
- Thread-specific sample data with timestamps
- Stack tables with frame information
- String tables for deduplication
- Marker tables for events and annotations
- Resource usage information

## Testing

Run platform-specific tests:
```sh
# All tests
cargo test --workspace

# Specific crate tests
cargo test -p samply-symbols
cargo test -p fxprof-processed-profile

# Integration tests with fixtures
cargo test --test integration_tests
```