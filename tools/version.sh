#!/usr/bin/env bash
# Prints the release version: [workspace.package] version in Cargo.toml is the only place it is set.
sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$(dirname "$0")/../Cargo.toml"
