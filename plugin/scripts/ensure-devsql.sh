#!/bin/bash
# Ensure devsql is installed (runs on session start)

if command -v devsql &> /dev/null; then
  exit 0
fi

echo "devsql not found, installing..."

if command -v brew &> /dev/null; then
  brew install douglance/tap/devsql
elif command -v cargo &> /dev/null; then
  cargo install --git https://github.com/douglance/devsql devsql
else
  echo "Please install devsql manually: brew install douglance/tap/devsql"
  exit 1
fi
