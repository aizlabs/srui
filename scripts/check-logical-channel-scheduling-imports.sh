#!/usr/bin/env bash
# Fail if the portable scheduler module imports Apple-platform (or libc) APIs.
set -euo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

sources=client-macos/LogicalChannelScheduling/Sources
tests=client-macos/LogicalChannelScheduling/Tests
for dir in "$sources" "$tests"; do
    if [ ! -d "$dir" ]; then
        echo "error: $dir does not exist" >&2
        exit 1
    fi
done

# Matches attributed (`@_exported import Foundation`, `@testable import Foundation`) and
# qualified (`import class Foundation.NSString`) forms as well as the plain one.
pattern='^[[:space:]]*(@[[:alnum:]_]+[[:space:]]+)*import[[:space:]]+((typealias|struct|class|enum|protocol|let|var|func)[[:space:]]+)?(Foundation|FoundationEssentials|Darwin|Glibc|AppKit|Cocoa|Security|Network|Combine|SwiftUI|UIKit|CoreFoundation|Dispatch|ObjectiveC)([.[:space:]]|$)'

if grep -rnE "$pattern" "$sources" "$tests"; then
    echo "error: LogicalChannelScheduling must stay free of Foundation, Darwin, AppKit, Security, Network, and other Apple-platform imports." >&2
    exit 1
fi

echo "LogicalChannelScheduling Sources and Tests have no Apple-platform imports."
