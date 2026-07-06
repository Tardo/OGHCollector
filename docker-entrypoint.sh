#!/bin/sh
set -e

DB_PATH="${OGHCOLLECTOR_DB_PATH:-/app/data/data.db}"

oghmigrate "$DB_PATH"

exec "$@"
