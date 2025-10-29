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

wait_sync() {
	pattern="$2"
	while ! (curl http://0.0.0.0:8001/created_items_root | grep "$pattern"); do sleep 1; done
}

# Wait for synchronizer to be online
wait_sync "[0,"

# app command line:
# craft new copper item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app=debug cargo run --release -p app -- craft  --recipe copper --output ./tmp/item-copper' C-m
# commit the crafted item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app=debug cargo run --release -p app -- commit --input ./tmp/item-copper' C-m
# verify the crafted item
wait_sync "[1,"
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app=debug cargo run --release -p app -- verify --input ./tmp/item-copper' C-m

# craft new tin item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app=debug cargo run --release -p app -- craft --recipe tin --output ./tmp/item-tin' C-m
# commit the crafted item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app=debug cargo run --release -p app -- commit --input ./tmp/item-tin' C-m
# verify the crafted item
wait_sync "[2,"
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app=debug cargo run --release -p app -- verify --input ./tmp/item-tin' C-m

# craft new bronze item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app=debug cargo run --release -p app -- craft --recipe bronze --input ./tmp/item-tin --input ./tmp/item-copper --output ./tmp/item-bronze' C-m
# commit the crafted item
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app=debug cargo run --release -p app -- commit --input ./tmp/item-bronze' C-m
# verify the crafted item
wait_sync "[3,"
$tmux send-keys -t fullflow:0.1 'RUST_LOG=app=debug cargo run --release -p app -- verify --input ./tmp/item-bronze' C-m

$tmux select-pane -t fullflow:0.1

$tmux attach -t fullflow
