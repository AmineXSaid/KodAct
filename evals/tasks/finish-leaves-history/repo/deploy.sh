#!/bin/sh
# Start the service against its upstream.
exec ./inventory --upstream old-host.example.internal:8080
