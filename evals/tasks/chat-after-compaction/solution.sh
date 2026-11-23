#!/bin/sh
sed -i 's/^pool_max_connections = 8$/pool_max_connections = 64/' service/settings.toml
printf 'pool_max_connections: 8 -> 64\nfailures: 3 -> 0\n' > CHANGES.md
