#!/usr/bin/env bash

# if the .env file does not exist, create it from the .env.default
if [ ! -f ".env" ]; then
	cp .env.default .env
fi

echo -e "getting Beacon chain last slot number"
LAST_SLOT=$(curl -X GET https://ethereum-sepolia-beacon-api.publicnode.com/eth/v2/beacon/blocks/head | jq -r '.data.message.slot')

echo -e "updating DO_GENESIS_SLOT value in the .env file"
sed -i "s/^DO_GENESIS_SLOT=.*/DO_GENESIS_SLOT=\"$LAST_SLOT\"/" .env

mkdir -p tmp

# set new variable to use tmux in a new env
tmux="tmux -L ad-demo -f /dev/null"

echo -e "opening tmux with 3 panels"
$tmux new-session -d -s fullflow
$tmux split-window -v

# run the Synchronizer server
$tmux send-keys -t fullflow:0.0 'RUST_LOG=synchronizer=debug cargo run --release -p synchronizer' C-m

# app command line:
# craft new copper item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app=debug cargo run --release -p app -- craft --output ./tmp/item-copper --key key0 --recipe copper' C-m
# commit the crafted item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app=debug cargo run --release -p app -- commit --input ./tmp/item-copper' C-m
# verify the crafted item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app=debug cargo run --release -p app -- verify --input ./tmp/item-copper' C-m

$tmux select-pane -t fullflow:0.1

$tmux attach -t fullflow
