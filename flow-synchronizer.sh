#!/usr/bin/env bash

# if the .env file does not exist, create it from the .env.default
if [ ! -f ".env" ]; then
	cp .env.default .env
fi

echo -e "getting Beacon chain last slot number"
LAST_SLOT=$(curl -X GET https://ethereum-sepolia-beacon-api.publicnode.com/eth/v2/beacon/blocks/head | jq -r '.data.message.slot')

echo -e "updating DO_GENESIS_SLOT value in the .env file"
sed -i "s/^DO_GENESIS_SLOT=.*/DO_GENESIS_SLOT=\"$LAST_SLOT\"/" .env

RUST_LOG=synchronizer=debug,info cargo run --release -p synchronizer
