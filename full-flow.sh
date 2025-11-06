#!/usr/bin/env bash

# if the .env file does not exist, create it from the .env.default
if [ ! -f ".env" ]; then
	cp .env.default .env
fi

# Use env-specific beacon URL
BEACON_URL=$(cat .env | sed -n 's/^BEACON_URL[ \t]*=[ \t]*"\(.*\)".*/\1/p')

if [[ -z "$BEACON_URL" ]]; then BEACON_URL="https://ethereum-sepolia-beacon-api.publicnode.com"; fi

echo -e "getting Beacon chain last slot number"
LAST_SLOT=$(curl -X GET "$BEACON_URL/eth/v2/beacon/blocks/head" | jq -r '.data.message.slot')

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
$tmux send-keys -t fullflow:0.1 './wait-sync-epoch.sh 0' C-m
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- craft  --recipe copper --output ./tmp/item-copper' C-m
# commit the crafted item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- commit --input ./tmp/item-copper' C-m
# verify the crafted item
$tmux send-keys -t fullflow:0.1 './wait-sync-epoch.sh 1' C-m
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- verify --input ./tmp/item-copper' C-m

# craft new tin item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- craft --recipe tin --output ./tmp/item-tin' C-m
# commit the crafted item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- commit --input ./tmp/item-tin' C-m
# verify the crafted item
$tmux send-keys -t fullflow:0.1 './wait-sync-epoch.sh 2' C-m
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- verify --input ./tmp/item-tin' C-m

# craft new bronze item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- craft --recipe bronze --input ./tmp/item-tin --input ./tmp/item-copper --output ./tmp/item-bronze' C-m
# commit the crafted item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- commit --input ./tmp/item-bronze' C-m
# verify the crafted item
$tmux send-keys -t fullflow:0.1 './wait-sync-epoch.sh 3' C-m
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- verify --input ./tmp/item-bronze' C-m

$tmux select-pane -t fullflow:0.1

$tmux attach -t fullflow
