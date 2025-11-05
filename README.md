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
RUST_LOG=gui_app=debug,info cargo run --release -p gui_app
```
