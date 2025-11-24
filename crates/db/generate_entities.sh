#!/bin/bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
(cd "${SCRIPT_DIR}/entities/src" && cargo run -- generate entity -l)