#!/bin/sh

epoch="$1"
while ! (curl --silent --show-error http://0.0.0.0:8001/created_items_root | grep "\[$1,"); do sleep 1; done
