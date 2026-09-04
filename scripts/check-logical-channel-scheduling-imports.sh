#!/usr/bin/env bash
# Fail if the portable scheduler module imports Apple-platform (or libc) APIs.
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

sources=client-macos/LogicalChannelScheduling/Sources
if [ ! -d "$sources" ]; then
    echo "error: $sources does not exist" >&2
    exit 1
fi

pattern='^[[:space:]]*import[[:space:]]+(Foundation|FoundationEssentials|Darwin|Glibc|AppKit|Cocoa|Security|Network|Combine|SwiftUI|UIKit|CoreFoundation|Dispatch|ObjectiveC)'

if grep -rnE "$pattern" "$sources"; then
    echo "error: LogicalChannelScheduling must stay free of Foundation, Darwin, AppKit, Security, Network, and other Apple-platform imports." >&2
    exit 1
fi

echo "LogicalChannelScheduling Sources have no Apple-platform imports."
