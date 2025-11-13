# digital-objects-e2e-poc

## Testing

### Unit testing
- Run all tests: `cargo test --release -- --nocapture`
- Test specific package: `cargo test --release -p NAME_OF_PACKAGE -- --nocapture`



### Full flow testing
To run the integration flow, while also preparing the configuration and running the Synchronizer and the App, the script `full-flow.sh` contains all the necessary logic to run everything automatically.

#### Requirements
Required software: [curl](https://curl.se), [git](https://git-scm.com), [rust](https://rust-lang.org), [go](https://go.dev), [tmux](https://github.com/tmux/tmux), [jq](https://github.com/jqlang/jq).

Copy the `.env.default` file into `.env`, and set the `PRIV_KEY` (corresponding to an address which holds some Sepolia ETH) and `RPC_URL` values.

#### Run
Once having the `.env` file ready with the `PRIV_KEY` and `RPC_URL` properly filled, to run the Synchronizer and the cli app that crafts & commits the objects, together with a bash script that handles their interaction, run the following command:
- `./full-flow.sh`

This will generate all the needed files, and it will open a new tmux session with 2 panels; one for the Synchronizer and one to run the cli app which will be crafting and committing the materials.

### Testing the gui

Requires the same "Requirements" as "Full flow testing".  Make sure you fulfill them.

Run the synchronizer in the background:
```
./flow-synchronizer.sh
```

Start the gui app and begin crafting, committing, vieweing and verifying items
```
RUST_LOG=app_gui=debug,info cargo run --release -p app_gui
```

### Running a local testnet:

#### Install 

Requires docker.

Linux specific:
```
wget 'https://github.com/kurtosis-tech/kurtosis-cli-release-artifacts/releases/download/1.12.1/kurtosis-cli_1.12.1_linux_amd64.tar.gz'
tar xf kurtosis-cli_1.12.1_linux_amd64.tar.gz
```

MacOS specific:
```
brew install kurtosis-tech/tap/kurtosis-cli
```

#### Start
```
./kurtosis analytics disable
./kurtosis --enclave local-testnet run github.com/ethpandaops/ethereum-package@b0f4fabf9d2958d7b67e56a2e0dc91ef26c2dd9a --args-file network.yml
```

Find the CL and EL rpc ports with:
```
./kurtosis enclave inspect local-testnet | grep " http: 4000" # BEACON_URL (XXX)
./kurtosis enclave inspect local-testnet | grep " rpc: 8545" # RPC_URL (YYY)
```

Use this template for the `.env` and replace the `BEACON_URL` and `RPC_URL` pots for the correct ones:
```
# Local testnet
BEACON_URL="http://127.0.0.1:XXX"
RPC_URL="http://127.0.0.1:YYY"
REQUEST_RATE="0"
# Address = 0x8943545177806ED17B9F23F0a21ee5948eCaa776 (first pre_funded_accounts)
PRIV_KEY="bcdf20249abf0ed6d944c0288fad489e33f66b3960d9e6229c1cd214ed3bbe31"
DO_GENESIS_SLOT="0"
```

#### Stop
```
./kurtosis engine stop
./kurtosis enclave rm -f local-testnet
```

## License

### Icons license

The files in `app_gui/assets` are distributed under the Flaticon License:
- `axe.png`: <a href="https://www.flaticon.com/free-icons/axe" title="axe icons">Axe icons created by Freepik - Flaticon</a>
- `bronze.png`: <a href="https://www.flaticon.com/free-icons/bronze" title="bronze icons">Bronze icons created by Freepik - Flaticon</a>
- `copper.png`: <a href="https://www.flaticon.com/free-icons/copper" title="copper icons">Copper icons created by Freepik - Flaticon</a>
- `empty.png`: <a href="https://www.flaticon.com/free-icons/3d-cube" title="3d cube icons">3d cube icons created by Freepik - Flaticon</a>
- `tin.png`: <a href="https://www.flaticon.com/free-icons/tin" title="tin icons">Tin icons created by Freepik - Flaticon</a>
- `wood.png`: <a href="https://www.flaticon.com/free-icons/wood" title="wood icons">Wood icons created by Freepik - Flaticon</a>
- `water.png`: <a href="https://www.flaticon.com/free-icons/shark" title="shark icons">Shark icons created by Freepik - Flaticon</a>
- `stone.png`: <a href="https://www.flaticon.com/free-icons/coal" title="coal icons">Coal icons created by Freepik - Flaticon</a>
- `wooden-axe.png`: <a href="https://www.flaticon.com/free-icons/wood" title="wood icons">Wood icons created by Nsit - Flaticon</a>
