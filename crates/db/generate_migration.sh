#!/bin/bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
(cd "${SCRIPT_DIR}/migration" && cargo run -- generate $1)