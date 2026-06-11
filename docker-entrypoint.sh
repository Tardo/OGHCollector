#!/bin/sh
set -e

DB_PATH="/app/data/data.db"

if [ ! -f "$DB_PATH" ] || [ ! -s "$DB_PATH" ]; then
    echo "WARNING: Database '$DB_PATH' not found or empty." >&2
    echo "         Run oghcollector first to initialize the database." >&2
    echo "         The server will start but will not serve any data." >&2
fi

exec "$@"
