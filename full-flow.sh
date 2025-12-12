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
sed -i'.bak' -e "s/^DO_GENESIS_SLOT=.*/DO_GENESIS_SLOT=\"$LAST_SLOT\"/" .env
rm .env.bak

mkdir -p tmp

# set new variable to use tmux in a new env
tmux="tmux -L ad-demo -f /dev/null"

echo -e "opening tmux with 3 panels"
$tmux new-session -d -s fullflow
$tmux split-window -v

# run the Synchronizer server
$tmux send-keys -t fullflow:0.0 'RUST_LOG=synchronizer=debug,info cargo run --release -p synchronizer' C-m


# app command line:
# craft new wood item
$tmux send-keys -t fullflow:0.1 './wait-sync-epoch.sh 0' C-m
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- craft  --recipe wood --output ./tmp/item-wood' C-m
# commit the crafted item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- commit --input ./tmp/item-wood' C-m
# verify the crafted item
$tmux send-keys -t fullflow:0.1 './wait-sync-epoch.sh 1' C-m
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- verify --input ./tmp/item-wood' C-m

# craft new stone item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- craft --recipe stone --output ./tmp/item-stone' C-m
# commit the crafted item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- commit --input ./tmp/item-stone' C-m
# verify the crafted item
$tmux send-keys -t fullflow:0.1 './wait-sync-epoch.sh 2' C-m
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- verify --input ./tmp/item-stone' C-m

# craft new axe item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- craft --recipe axe --input ./tmp/item-wood --input ./tmp/item-stone --output ./tmp/item-axe' C-m
# commit the crafted item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- commit --input ./tmp/item-axe' C-m
# verify the crafted item
$tmux send-keys -t fullflow:0.1 './wait-sync-epoch.sh 3' C-m
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app_cli=debug cargo run --release -p app_cli -- verify --input ./tmp/item-axe' C-m

$tmux select-pane -t fullflow:0.1

$tmux attach -t fullflow
